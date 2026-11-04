import Foundation

@MainActor private final class ArchiveWire {
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:"crew",namespace:"namespace",generation:"1")
    let reference = CrewdDataRef(namespace:"namespace",kind:"view",id:"10000000-0000-0000-0000-000000000001")
    var revisions: [Bool] = [false]
    var sends: [CrewdDataSubmitParams] = []
    var readbacks: [String] = []
    var lookups: [String] = []
    var receipts: [String:CrewdDataChangeResult] = [:]
    var loseReply = false
    var wrongArchiveFlag = false
    var wrongChangedView = false
    var typedStale = false
    var state: CrewdDataWorkspaceInfo { .init(context:context,state:"ready",engineVersion:"SQLite",reason:nil) }

    func detail(_ revision: Int, archived: Bool) -> CrewdDataObjectDetail {
        let actor: CrewdValue = .object(["kind":.string("human")])
        let provenance = CrewdDataProvenance(createdBy:actor,updatedBy:actor,createdAt:"1",updatedAt:"1",sources:[],attributes:.object([:]))
        let view = CrewdDataObject.view(id:reference.id,revision:String(revision),title:"原文档",definition:.document(placements:[],layout:.object([:])),archived:archived,provenance:provenance)
        return .init(context:context,resolvedContext:context,generation:context.generation,object:view,references:[],affectedLiveViews:[],affectedTruncated:false)
    }
    func call(_ method: String, _ params: CrewdValue) throws -> CrewdValue {
        switch method {
        case "data.workspace.get": return try CrewdKnowledgeStore.encode(state)
        case "data.object.list":
            let include = params["includeArchived"] == .bool(true)
            let archived = revisions.last!
            let items = archived && !include ? [] : [detail(revisions.count,archived:archived).object]
            return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,items:items,next:nil))
        case "data.object.get":
            let rev = params["atRevision"].string
            if let rev { readbacks.append(rev) }
            let n = rev.flatMap(Int.init) ?? revisions.count
            return try CrewdKnowledgeStore.encode(detail(n,archived:rev != nil && wrongArchiveFlag ? !revisions[n-1]:revisions[n-1]))
        case "data.submit":
            let input = try CrewdKnowledgeStore.decode(params,CrewdDataSubmitParams.self)
            sends.append(input)
            if typedStale {
                throw CrewdClientError(message:"stale",code:-32009,data:.object(["refusal":.string("data.context_conflict")]))
            }
            guard case .apply(_,let edits) = input.change,
                  case let .archiveView(id,revision,archived)? = edits.first,
                  edits.count == 1 && id == reference.id && revision == String(revisions.count) && archived != revisions.last else {
                preconditionFailure("archive must bind the current view and exact opposite state")
            }
            let before = context.generation
            revisions.append(archived); context.generation = String(Int(before)!+1)
            var result = CrewdDataChangeResult(requestId:input.requestId,phase:.committed,context:context,beforeGeneration:before,afterGeneration:context.generation,snapshotId:nil,created:[:],changed:[.init(reference:reference,revision:String(revisions.count))],conflicts:[],reason:nil,resolvedByRequest:nil)
            receipts[input.requestId] = result
            if loseReply { throw NSError(domain:"original reply dropped",code:1) }
            if wrongChangedView { result.changed[0].reference.id = "10000000-0000-0000-0000-000000000002" }
            return try CrewdKnowledgeStore.encode(result)
        case "data.request.get":
            let request = try CrewdKnowledgeStore.decode(params,CrewdDataRequestGetParams.self)
            lookups.append(request.requestId)
            let original = receipts[request.requestId].map { CrewdDataRequestLookup.known(value:$0) } ?? .notFound
            return try CrewdKnowledgeStore.encode(CrewdDataRequestGetResult(workspace:state,lookup:original))
        default: preconditionFailure("unexpected archive method \(method)")
        }
    }
}

@main struct ArchiveContract {
    @MainActor static func main() async throws {
        if CommandLine.arguments[1] == "public-wire" { try await publicWire(); return }
        let root = URL(fileURLWithPath:"/private/tmp/pcw-knowledge-archive-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
        let wire = ArchiveWire()
        func makeStore() throws -> CrewdKnowledgeStore {
            let journal = try CrewdPresentationJournal(root:root,scope:scope)
            return CrewdKnowledgeStore(origin:{origin},rpc:{try wire.call($0,$1)},journal:{journal})
        }
        let store = try makeStore()
        await store.refreshViewCatalog(includeArchived:false)
        await store.openViewCatalogItem(wire.reference.id,kind:.document)
        let archive = try store.prepareNativeViewArchive(store.detail!)
        let mode = CommandLine.arguments[1]
        if mode == "stale" {
            store.invalidate()
            do { _ = try await store.submitNativeViewArchive(archive); preconditionFailure("stale sheet cannot submit") }
            catch CrewdKnowledgeStore.Failure.stale {}
            precondition(wire.sends.isEmpty)
        } else if mode == "typed-stale" {
            wire.typedStale = true
            do { _ = try await store.submitNativeViewArchive(archive); preconditionFailure("typed refusal must surface") }
            catch let error as CrewdClientError { precondition(error.refusal == "data.context_conflict") }
            precondition(wire.sends.count == 1 && !store.hasBlockingPending && wire.readbacks.isEmpty)
        } else {
            wire.loseReply = mode == "lost"
            wire.wrongArchiveFlag = mode == "wrong-flag"
            wire.wrongChangedView = mode == "wrong-view"
            let result = try await store.submitNativeViewArchive(archive)
            if mode == "lifecycle" {
                precondition(result?.phase == .committed && wire.readbacks == ["2"])
                await store.refreshViewCatalog(includeArchived:false)
                precondition(store.viewCatalog?.items.isEmpty == true)
                let reopened = try makeStore()
                await reopened.refreshViewCatalog(includeArchived:true)
                precondition(reopened.viewCatalog?.items.first?.archived == true)
                await reopened.openViewCatalogItem(wire.reference.id,kind:.document)
                let restore = try reopened.prepareNativeViewArchive(reopened.detail!)
                let restored = try await reopened.submitNativeViewArchive(restore)
                precondition(restored?.phase == .committed && wire.readbacks == ["2","3"])
                precondition(wire.revisions == [false,true,false] && wire.sends.count == 2)
            } else {
                precondition(result == nil && store.hasBlockingPending)
                precondition(wire.readbacks == (mode == "wrong-flag" ? ["2"]:[]))
                wire.wrongArchiveFlag = false
                // A later generation may exist when the original UUID is read.
                wire.context.generation = "5"
                let reopened = try makeStore()
                await reopened.refreshViewCatalog(includeArchived:true)
                let id = reopened.pending.first!.id
                await reopened.lookup(id)
                precondition(reopened.result?.phase == .committed && !reopened.hasBlockingPending && wire.sends.count == 1)
                precondition(wire.lookups == [id.uuidString.lowercased()] && wire.readbacks.last == "2")
            }
        }
        print("PASS document recycle bin \(mode)")
    }

    @MainActor private static func publicWire() async throws {
        let decoder = JSONDecoder()
        let encoder = JSONEncoder()
        func send(_ value: CrewdValue) throws {
            FileHandle.standardOutput.write(try encoder.encode(value) + Data([10]))
        }
        func receive() throws -> CrewdValue {
            guard let line = readLine() else { throw CocoaError(.fileReadUnknown) }
            return try decoder.decode(CrewdValue.self,from:Data(line.utf8))
        }
        let config = try receive()
        let root = URL(fileURLWithPath:config["journalRoot"].string!)
        let db = config["dbId"].string!, crew = config["crewId"].string!
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(uuidString:config["journalNamespace"].string!)!,localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:crew)
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        func rpc(_ method: String, _ params: CrewdValue) throws -> CrewdValue {
            try send(.object(["method":.string(method),"params":params]))
            let envelope = try receive()
            if envelope["transportLost"] == .bool(true) { throw CocoaError(.fileReadUnknown) }
            if envelope["error"] != .null {
                throw CrewdClientError(message:envelope["error"]["message"].string ?? "refused",code:envelope["error"]["code"].int,data:envelope["error"]["data"])
            }
            return envelope["result"]
        }
        let store = CrewdKnowledgeStore(origin:{origin},rpc:{try rpc($0,$1)},journal:{journal})
        let id = config["viewId"].string!
        let restoring = config["restore"].string.flatMap(UUID.init(uuidString:))
        await store.refreshViewCatalog(includeArchived:restoring != nil)
        if let restoring {
            precondition(store.pending.map(\.id) == [restoring])
            await store.lookup(restoring)
            precondition(store.result?.phase == .committed && !store.hasBlockingPending)
            let group = CrewdChannelKnowledgeStore(call:{try rpc($0,$1)})
            group.selectChannel(crew:crew,channel:config["channelId"].string!)
            try await group.refresh()
            precondition(group.items.count == 1 && group.items[0].isArchivedReference && !group.items[0].isReadableActiveView)
        }
        precondition(store.viewCatalog?.items.first(where:{$0.id == id})?.archived == (restoring != nil))
        await store.openViewCatalogItem(id,kind:.document)
        guard let detail = store.detail else { throw CrewdKnowledgeStore.Failure.unavailable }
        let target = try store.prepareNativeViewArchive(detail)
        let outcome = try await store.submitNativeViewArchive(target)
        if restoring == nil {
            precondition(outcome == nil && store.hasBlockingPending)
            try send(.object(["finished":.string(store.pending.first!.id.uuidString.lowercased())]))
        } else {
            precondition(outcome?.phase == .committed && !store.hasBlockingPending)
            try send(.object(["finished":.string(outcome!.requestId)]))
        }
    }
}
