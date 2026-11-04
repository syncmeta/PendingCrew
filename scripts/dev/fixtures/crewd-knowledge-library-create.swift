import Foundation

@MainActor private final class LibraryWire {
    let document = CrewdDataRef(namespace:"namespace",kind:"view",id:"20000000-0000-0000-0000-000000000001")
    let body = CrewdDataRef(namespace:"namespace",kind:"block",id:"20000000-0000-0000-0000-000000000002")
    let placement = "20000000-0000-0000-0000-000000000003"
    let title = "库原文"
    let text = "exact e\u{301}🌙\n"
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"initial",crewId:"crew",namespace:"namespace",generation:"0")
    var receipt: CrewdDataChangeResult?
    var requests: [String] = []
    var lookups: [String] = []
    var historicalReads: [String] = []
    var loseReply = false
    var staleRefusal = false
    var missingLookup = false
    var badTitle = false
    var badBody = false
    var beforeReply: (() -> Void)?
    var workspace: CrewdDataWorkspaceInfo { .init(context:context,state:context.generation == "0" ? "uninitialized":"ready",engineVersion:"SQLite",reason:nil) }

    func call(_ method: String, _ value: CrewdValue) throws -> CrewdValue {
        switch method {
        case "data.workspace.get": return try CrewdKnowledgeStore.encode(workspace)
        case "data.object.list": return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,items:[],next:nil))
        case "data.submit":
            let submit = try CrewdKnowledgeStore.decode(value,CrewdDataSubmitParams.self)
            precondition(submit.context == context)
            guard case .apply(_,let edits) = submit.change, edits.count == 2,
                  case .createBlock("body","text/plain",.string(let payload),_,_) = edits[0],
                  case .createView("document",let documentTitle,.document(let positions,_),_,_) = edits[1] else {
                preconditionFailure("library write must be one native Block+View batch")
            }
            precondition(payload.utf8.elementsEqual(text.utf8) && documentTitle == title)
            precondition(positions == [.set(id:nil,alias:"bodyPlacement",block:.created(alias:"body"),atRevision:nil,layout:.object([:]))])
            requests.append(submit.requestId)
            if staleRefusal { throw CrewdClientError(message:"stale",code:-32009,data:.object(["refusal":.string("data.context_conflict")])) }
            let before = context.generation
            context.generation = "1"
            let docVersion = CrewdDataObjectVersion(reference:document,revision:"1")
            let bodyVersion = CrewdDataObjectVersion(reference:body,revision:"1")
            let committed = CrewdDataChangeResult(requestId:submit.requestId,phase:.committed,context:context,beforeGeneration:before,afterGeneration:"1",snapshotId:nil,
                created:["document":.object(object:docVersion),"body":.object(object:bodyVersion),"bodyPlacement":.placement(id:placement,parentView:docVersion)],
                changed:[bodyVersion,docVersion],conflicts:[],reason:nil,resolvedByRequest:nil)
            receipt = committed
            beforeReply?()
            if loseReply { throw NSError(domain:"reply lost",code:1) }
            return try CrewdKnowledgeStore.encode(committed)
        case "data.request.get":
            let lookup = try CrewdKnowledgeStore.decode(value,CrewdDataRequestGetParams.self)
            precondition(lookup.context.generation == "0" && lookup.requestId == requests.first)
            lookups.append(lookup.requestId)
            return try CrewdKnowledgeStore.encode(CrewdDataRequestGetResult(workspace:workspace,lookup:missingLookup ? .notFound : .known(value:receipt!)))
        case "data.object.get":
            let get = try CrewdKnowledgeStore.decode(value,CrewdDataObjectGetParams.self)
            precondition(get.context == context && get.atRevision == "1")
            historicalReads.append(get.reference.id)
            let human: CrewdValue = .object(["kind":.string("human")])
            let provenance = CrewdDataProvenance(createdBy:human,updatedBy:human,createdAt:"1",updatedAt:"1",sources:[],attributes:.object([:]))
            let object: CrewdDataObject
            var references: [CrewdDataResolvedRef] = []
            if get.reference == document {
                let visible = CrewdDataRefState.visible(reference:body)
                let item = CrewdDataPlacement(id:placement,block:visible,atRevision:"1",layout:.object([:]))
                object = .view(id:document.id,revision:"1",title:badTitle ? "different":title,
                    definition:.document(placements:[item],layout:.object([:])),archived:false,provenance:provenance)
                references = [.init(anchor:"placement:"+placement,target:visible,commit:nil,revision:"1",live:false,complete:true)]
            } else {
                precondition(get.reference == body)
                object = .block(id:body.id,revision:"1",contentType:"text/plain",content:.string(badBody ? "exact é🌙\n":text),archived:false,provenance:provenance)
            }
            return try CrewdKnowledgeStore.encode(CrewdDataObjectDetail(context:context,resolvedContext:context,generation:context.generation,
                object:object,references:references,affectedLiveViews:[],affectedTruncated:false))
        default: preconditionFailure("unexpected (possibly channel) request: " + method)
        }
    }
}

@main struct LibraryCreationFixture {
    static func scope(root:URL, namespace:UUID, db:String = "db") -> CrewdPresentationJournal.Scope {
        .init(namespace:namespace,localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
    }

    @MainActor static func main() async throws {
        let arguments = CommandLine.arguments
        let mode = arguments[1]
        if mode == "claim" || mode == "finish" || mode == "init" {
            let root = URL(fileURLWithPath:arguments[2])
            let journal = try CrewdPresentationJournal(root:root,scope:scope(root:root,namespace:UUID(uuidString:arguments[3])!))
            if mode == "init" { return }
            let request = UUID(uuidString:arguments[4])!
            if mode == "finish" {
                guard let stage = try journal.currentDocumentStage(crew:"crew",channel:nil) else { preconditionFailure() }
                precondition(stage.id == request)
                try journal.recordDocumentStageRefusal(stage)
                try journal.finishDocumentStage(stage)
                print("finished")
                return
            }
            if arguments.count > 5 {
                let gate = arguments[5]
                while !FileManager.default.fileExists(atPath:gate) { try await Task.sleep(for:.milliseconds(5)) }
            }
            do {
                let claim = try journal.claimDocumentStage(crew:"crew",channel:nil,title:"same",body:"exact",context:.string("cut"),requestID:request)
                precondition(claim.stage.id == request)
                print(claim.maySubmit ? "winner":"observer")
            } catch { print("contended") }
            return
        }
        if mode == "public-wire" { try await publicWire(); return }
        let root = URL(fileURLWithPath:"/private/tmp/pcw-library-fixture-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = scope(root:root,namespace:UUID())
        let wire = LibraryWire()
        let id = UUID()
        var observation = UUID()
        func store() throws -> CrewdKnowledgeStore {
            let journal = try CrewdPresentationJournal(root:root,scope:scope)
            return CrewdKnowledgeStore(origin:{.init(scope:scope,observation:observation,connection:observation,crew:"crew")},
                                       rpc:{try wire.call($0,$1)},journal:{journal})
        }
        let first = try store()
        await first.refresh()
        let context = first.workspace!.context
        wire.loseReply = mode == "lost" || mode == "not-found" || mode == "origin" || mode == "restore"
        wire.staleRefusal = mode == "refused"
        wire.badTitle = mode == "bad-title"
        wire.badBody = mode == "bad-body"
        if mode == "event" { wire.beforeReply = { first.handleEvent("data.changed",.object(["crewId":.string("crew")])) } }
        if mode == "origin" { wire.beforeReply = { observation=UUID() } }
        let result = await first.submitNativeDocument(title:wire.title,body:wire.text,channel:nil,requestID:id,expectedContext:context)
        precondition(wire.requests == [id.uuidString.lowercased()])
        let reopened = try store()
        if mode == "restore" {
            wire.context.restoreEpoch = "restored"
            wire.context.generation = "2"
            await reopened.refresh()
            let current = try reopened.nativeDocumentStage(channel:nil)
            let journal = try CrewdPresentationJournal(root:root,scope:scope)
            let historical = try journal.currentDocumentStage(crew:"crew",channel:nil)
            precondition(current == nil && !reopened.hasBlockingPending && historical?.id == id)
            precondition(wire.lookups.isEmpty && wire.requests.count == 1)
            print("PASS old restore stage retained without blocking current library")
            return
        }
        await reopened.refresh()
        let stage = try reopened.nativeDocumentStage(channel:nil)!
        let group = try reopened.nativeDocumentStage(channel:"group")
        precondition(stage.id == id && stage.channel == nil && group == nil)
        if mode == "refused" {
            let refused = try reopened.nativeDocumentWasRefused(stage)
            precondition(result == nil && refused && !reopened.hasBlockingPending)
            try reopened.finishNativeDocument(stage)
            let repeatResult = await reopened.submitNativeDocument(title:wire.title,body:wire.text,channel:nil,requestID:id,expectedContext:wire.context)
            precondition(repeatResult == nil && wire.requests.count == 1)
        } else if mode == "event" || mode == "direct" {
            precondition(result?.phase == .committed)
            precondition(wire.historicalReads == [wire.document.id,wire.body.id] && wire.lookups.isEmpty)
            try reopened.finishNativeDocument(stage)
            let repeatResult = await reopened.submitNativeDocument(title:wire.title,body:wire.text,channel:nil,requestID:id,expectedContext:wire.context)
            precondition(repeatResult == result && wire.requests.count == 1)
        } else {
            precondition(result == nil && reopened.hasBlockingPending)
            if mode == "not-found" {
                wire.missingLookup = true
                let missing = await reopened.lookupNativeDocument(stage)
                precondition(missing == nil && reopened.hasBlockingPending)
            } else {
                wire.badTitle=false;wire.badBody=false
                wire.context.generation = "2" // A later real knowledge write does not change the original proof cut.
                let recovered = await reopened.lookupNativeDocument(stage)
                precondition(recovered == wire.receipt && !reopened.hasBlockingPending)
                let cached = try reopened.nativeDocumentResult(stage)
                precondition(cached == recovered)
                try reopened.finishNativeDocument(stage)
            }
            precondition(wire.lookups == [id.uuidString.lowercased()] && wire.requests.count == 1)
        }
        print("PASS native crew library create " + mode)
    }

    @MainActor private static func publicWire() async throws {
        let decoder=JSONDecoder(),encoder=JSONEncoder()
        func receive() throws -> CrewdValue {
            guard let line=readLine() else { throw CocoaError(.fileReadUnknown) }
            return try decoder.decode(CrewdValue.self,from:Data(line.utf8))
        }
        func send(_ value:CrewdValue) throws { FileHandle.standardOutput.write(try encoder.encode(value)+Data([10])) }
        let config=try receive()
        let root=URL(fileURLWithPath:config["journalRoot"].string!)
        let scope=scope(root:root,namespace:UUID(uuidString:config["journalNamespace"].string!)!,db:config["dbId"].string!)
        let crew=config["crewId"].string!
        let origin=CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:crew)
        let journal=try CrewdPresentationJournal(root:root,scope:scope)
        let store=CrewdKnowledgeStore(origin:{origin},rpc:{method,params in
            try send(.object(["method":.string(method),"params":params]))
            let envelope=try receive()
            if envelope["error"] != .null {throw CrewdClientError(message:envelope["error"]["message"].string ?? "lost reply",code:envelope["error"]["code"].int,data:envelope["error"]["data"])}
            return envelope["result"]
        },journal:{journal})
        await store.refresh()
        if config["recover"] == .bool(true) {
            let stage=try store.nativeDocumentStage(channel:nil)!
            guard let result=await store.lookupNativeDocument(stage),result.phase == .committed,
                  case .object(let view)? = result.created["document"] else { preconditionFailure(store.status) }
            await store.refresh()
            await store.open(view.reference)
            guard let detail = store.detail,
                  case .view(let id,_,let title,.document(let positions,_),false,_) = detail.object,
                  id == view.reference.id, title.utf8.elementsEqual(stage.title.utf8), positions.count == 1,
                  let position = positions.first,
                  let block = try await store.resolved(position.block,anchor:"placement:"+position.id,atRevision:position.atRevision,parent:detail),
                  case .block(_,_,_,.string(let text),false,_) = block.object, text.utf8.elementsEqual(stage.body.utf8)
            else { preconditionFailure("saved native document was not readable at the original identity") }
            try store.finishNativeDocument(stage)
            try send(.object(["finished":try CrewdKnowledgeStore.encode(result)]))
        } else {
            let id=UUID(uuidString:config["requestId"].string!)!
            let result=await store.submitNativeDocument(title:config["title"].string!,body:config["body"].string!,channel:nil,requestID:id,expectedContext:store.workspace?.context)
            if config["expectLostReply"] == .bool(true) {
                precondition(result == nil)
                try send(.object(["unknown":.string(id.uuidString.lowercased())]))
            } else {
                guard let result,result.phase == .committed else { preconditionFailure(store.status) }
                try send(.object(["finished":try CrewdKnowledgeStore.encode(result)]))
            }
        }
    }
}
