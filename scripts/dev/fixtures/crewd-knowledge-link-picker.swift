import Foundation

@MainActor private final class PickerWire {
    let docID = "00000000-0000-0000-0000-000000000013"
    var requests: [(String,CrewdValue)] = []
    var onSecondPage: (() async -> Void)?

    func context(_ crew: String) -> CrewdDataContext {
        .init(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:crew,namespace:"namespace-"+crew,generation:"1")
    }
    func id(_ number: Int) -> String { String(format:"00000000-0000-0000-0000-%012d",number) }
    func view(_ number: Int, crew: String) -> CrewdDataObject {
        let actor: CrewdValue = .object(["kind":.string("human")])
        let provenance = CrewdDataProvenance(createdBy:actor,updatedBy:actor,createdAt:"1",updatedAt:"1",sources:[],attributes:.object([:]))
        return .view(id:id(number),revision:"1",title:"\(crew) native \(number)",definition:.document(placements:[],layout:.object([:])),archived:false,provenance:provenance)
    }
    func call(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        requests.append((method,params))
        switch method {
        case "data.workspace.get":
            return try CrewdKnowledgeStore.encode(CrewdDataWorkspaceInfo(context:context(params["crewId"].string!),state:"ready",engineVersion:"SQLite",reason:nil))
        case "data.object.list":
            let list = try CrewdKnowledgeStore.decode(params,CrewdDataObjectListParams.self)
            let crew = list.context.crewId
            if list.after != nil && crew == "first" { await onSecondPage?() }
            let numbers = list.after == nil ? Array(1...12):[13]
            return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:list.context,generation:list.context.generation,
                items:numbers.map{view($0,crew:crew)},next:list.after == nil ? id(12):nil))
        case "data.object.get":
            let request = try CrewdKnowledgeStore.decode(params,CrewdDataObjectGetParams.self)
            return try CrewdKnowledgeStore.encode(CrewdDataObjectDetail(context:request.context,resolvedContext:request.context,generation:request.context.generation,
                object:view(1,crew:request.context.crewId),references:[],affectedLiveViews:[],affectedTruncated:false))
        default: preconditionFailure("picker executed unexpected method \(method)")
        }
    }
}

@main struct KnowledgeLinkPickerContract {
    @MainActor static func main() async throws {
        if CommandLine.arguments[1] == "public-wire" { try await publicWire(); return }
        let root = URL(fileURLWithPath:"/private/tmp/pcw-knowledge-link-picker-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        let observation = UUID(), connection = UUID()
        var sourceCrew = "first"
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        let wire = PickerWire()
        let picker = CrewdKnowledgeStore(origin:{.init(scope:scope,observation:observation,connection:connection,crew:sourceCrew)},rpc:{try await wire.call($0,$1)},journal:{journal})
        let reader = CrewdKnowledgeStore(origin:{.init(scope:scope,observation:observation,connection:connection,crew:"destination")},rpc:{try await wire.call($0,$1)},journal:{journal})
        await reader.refreshViewCatalog(includeArchived:false)
        await reader.openViewCatalogItem(wire.id(1),kind:.document)
        await picker.refreshViewCatalog(includeArchived:false)
        precondition(picker.catalogLinkSource(wire.docID,kind:.document,crew:"first") == nil)
        if CommandLine.arguments[1] == "late-page" {
            wire.onSecondPage = {
                sourceCrew = "second"
                picker.synchronizeOrigin()
                await picker.refreshViewCatalog(includeArchived:false)
            }
            await picker.loadNextViewCatalogPage()
            precondition(picker.viewCatalog?.context.crewId == "second" && picker.viewCatalog?.items.count == 12)
            precondition(picker.catalogLinkSource(wire.id(1),kind:.document,crew:"first") == nil)
            precondition(picker.catalogLinkSource(wire.docID,kind:.document,crew:"second") == nil)
        }
        await picker.loadNextViewCatalogPage()
        let exact = picker.catalogLinkSource(wire.docID,kind:.document,crew:sourceCrew)
        precondition(exact?.context == wire.context(sourceCrew) && exact?.reference.id == wire.docID)
        precondition(picker.catalogLinkSource(wire.docID,kind:.web,crew:sourceCrew) == nil)
        precondition(reader.detail?.context.crewId == "destination" && reader.detail?.object.identity == wire.id(1))
        precondition(wire.requests.filter{$0.0 == "data.object.get"}.count == 1,"only the existing reader may load content")
        picker.handleEvent("data.changed",.object(["crewId":.string(sourceCrew)]))
        precondition(picker.catalogLinkSource(wire.docID,kind:.document,crew:sourceCrew) == nil)
        print("PASS document source picker \(CommandLine.arguments[1])")
    }

    @MainActor private static func publicWire() async throws {
        let decoder=JSONDecoder(), encoder=JSONEncoder()
        func send(_ value:CrewdValue)throws { FileHandle.standardOutput.write(try encoder.encode(value)+Data([10])) }
        func receive()throws->CrewdValue {
            guard let line=readLine() else {throw CocoaError(.fileReadUnknown)}
            return try decoder.decode(CrewdValue.self,from:Data(line.utf8))
        }
        let config=try receive(), root=URL(fileURLWithPath:config["journalRoot"].string!)
        let db=config["dbId"].string!, target=config["target"].string!, channel=config["channel"].string!
        let scope=CrewdPresentationJournal.Scope(namespace:UUID(uuidString:config["journalNamespace"].string!)!,localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        let journal=try CrewdPresentationJournal(root:root,scope:scope)
        let observation=UUID(), connection=UUID()
        func rpc(_ method:String,_ params:CrewdValue)throws->CrewdValue {
            try send(.object(["method":.string(method),"params":params]))
            let envelope=try receive()
            if envelope["transportLost"] == .bool(true) {throw CocoaError(.fileReadUnknown)}
            if envelope["error"] != .null {
                throw CrewdClientError(message:envelope["error"]["message"].string ?? "refused",code:envelope["error"]["code"].int,data:envelope["error"]["data"])
            }
            return envelope["result"]
        }
        let group=CrewdChannelKnowledgeStore { method,params in
            if method == "channel.knowledgeLink" {
                return try await journal.performResourceMutation(method,params:params,crew:target) {try rpc($0,$1)}
            }
            return try rpc(method,params)
        }
        group.selectChannel(crew:target,channel:channel)
        try await group.refresh()
        if let recover=config["recover"].string.flatMap(UUID.init(uuidString:)) {
            let saved=CrewdPendingKnowledgeLink.inChannel(try journal.unknownRecords(),scope:scope,crew:target,channel:channel)
            precondition(saved.map(\.requestID) == [recover])
            let original=saved[0]
            let reply=try await group.lookup(original.snapshot)
            let receipt=try CrewdKnowledgeStore.decode(reply!["result"],CrewdChannelKnowledgeReceipt.self)
            precondition(receipt.linked && receipt.source == original.source && receipt.channelId == channel)
            precondition(group.linkedViewIDs(in:original.source.context) == [original.source.reference.id])
            try journal.acknowledge(original.record,detail:"Exact original link recovered")
            let remaining=try journal.unknownRecords()
            precondition(CrewdPendingKnowledgeLink.inChannel(remaining,scope:scope,crew:target,channel:channel).isEmpty)
            try send(.object(["finished":.string(recover.uuidString.lowercased())])); return
        }
        let reader=CrewdKnowledgeStore(origin:{.init(scope:scope,observation:observation,connection:connection,crew:target)},rpc:{try rpc($0,$1)},journal:{journal})
        await reader.refresh(loadViews:false)
        let readerRef=try CrewdKnowledgeStore.decode(config["readerView"],CrewdDataRef.self)
        await reader.open(readerRef)
        precondition(reader.detail?.object.identity == readerRef.id)
        var source=config["decoy"].string!
        let picker=CrewdKnowledgeStore(origin:{.init(scope:scope,observation:observation,connection:connection,crew:source)},rpc:{try rpc($0,$1)},journal:{journal})
        await picker.refreshViewCatalog(includeArchived:false)
        precondition(picker.linkSourceCatalog(crew:source)?.items.count == 1)
        source=config["source"].string!
        picker.synchronizeOrigin()
        await picker.refreshViewCatalog(includeArchived:false)
        let id=config["viewId"].string!
        precondition(picker.catalogLinkSource(id,kind:.document,crew:source) == nil)
        await picker.loadNextViewCatalogPage()
        let matches=picker.linkSourceCatalog(crew:source)!.items.filter {CrewdKnowledgeTitleSearch.matches($0.title,query:"deep source")}
        precondition(matches.map(\.id) == [id])
        let exact=picker.catalogLinkSource(id,kind:.document,crew:source)!
        precondition(exact.context.crewId == source && reader.detail?.object.identity == readerRef.id && picker.detail == nil)
        precondition(!group.linkedViewIDs(in:exact.context).contains(id))
        var drafts=CrewdChannelKnowledgeDrafts()
        let snapshot=drafts.begin(crew:target,channel:channel,source:exact,revision:group.linkRevision!,linked:true)!
        do { _=try await group.submit(snapshot); preconditionFailure("public bridge intentionally drops original link reply") }
        catch { precondition(CrewdClientError.isUncertainSubmission(error)) }
        let records=try journal.unknownRecords()
        precondition(CrewdPendingKnowledgeLink.inChannel(records,scope:scope,crew:target,channel:channel).map(\.requestID) == [snapshot.id])
        try send(.object(["finished":.string(snapshot.id.uuidString.lowercased())]))
    }
}
