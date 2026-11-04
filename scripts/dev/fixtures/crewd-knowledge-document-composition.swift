import Foundation

@MainActor private final class CompositionWire {
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:"crew",namespace:"knowledge",generation:"2")
    let view = CrewdDataRef(namespace:"knowledge",kind:"view",id:"10000000-0000-0000-0000-000000000001")
    let body = CrewdDataRef(namespace:"knowledge",kind:"block",id:"10000000-0000-0000-0000-000000000002")
    let shared = CrewdDataRef(namespace:"knowledge",kind:"block",id:"10000000-0000-0000-0000-000000000003")
    let bodyPosition = "10000000-0000-0000-0000-000000000004"
    let newPosition = "10000000-0000-0000-0000-000000000005"
    let reinsertedPosition = "10000000-0000-0000-0000-000000000006"
    var revision = "2"
    var positions: [CrewdDataPlacement] = []
    var versions: [String:[CrewdDataPlacement]] = [:]
    var submits: [CrewdDataSubmitParams] = []
    var lookupIDs: [String] = []
    var checkedVersions: [String] = []
    var receipt: CrewdDataChangeResult?
    var loseReply = false
    var reject = false
    var wrongHistoricalReply = false
    let attributes: CrewdValue = .object(["stable":.bool(true)])
    let layout: CrewdValue = .object(["width":.integer(3)])
    var provenance: CrewdDataProvenance {
        let human: CrewdValue = .object(["kind":.string("human")])
        return .init(createdBy:human,updatedBy:human,createdAt:"1",updatedAt:"2",sources:[],attributes:attributes)
    }
    var workspace: CrewdDataWorkspaceInfo { .init(context:context,state:"ready",engineVersion:"SQLite",reason:nil) }
    init() {
        positions = [.init(id:bodyPosition,block:.visible(reference:body),atRevision:nil,layout:layout)]
        versions = ["1":positions,"2":positions]
    }
    func document(_ revision: String, historical: Bool) -> CrewdDataObjectDetail {
        let projected = versions[revision]!.map { position -> CrewdDataPlacement in
            var result = position
            if historical && result.atRevision == nil { result.atRevision = "1" }
            return result
        }
        let refs: [CrewdDataResolvedRef] = projected.map {
            .init(anchor:"placement:" + $0.id,target:$0.block,commit:nil,
                  revision:$0.atRevision ?? "1",live:$0.atRevision == nil,complete:true)
        }
        return detail(.view(id:view.id,revision:revision,title:"原题",definition:.document(placements:projected,layout:layout),
                            archived:false,provenance:provenance),references:refs)
    }
    func detail(_ object: CrewdDataObject, references: [CrewdDataResolvedRef] = []) -> CrewdDataObjectDetail {
        .init(context:context,resolvedContext:context,generation:context.generation,object:object,
              references:references,affectedLiveViews:[],affectedTruncated:false)
    }
    var block: CrewdDataObject {
        .block(id:shared.id,revision:"7",contentType:"text/plain",content:.string("共享正文"),archived:false,provenance:provenance)
    }
    func call(_ method: String, _ input: CrewdValue) async throws -> CrewdValue {
        switch method {
        case "data.workspace.get": return try CrewdKnowledgeStore.encode(workspace)
        case "data.object.list":
            let params = try CrewdKnowledgeStore.decode(input,CrewdDataObjectListParams.self)
            precondition(params.context == context && !params.includeArchived)
            return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,
                items:params.objectType == "block" ? [block] : [document(revision,historical:false).object],next:nil))
        case "data.object.get":
            let params = try CrewdKnowledgeStore.decode(input,CrewdDataObjectGetParams.self)
            precondition(params.context == context)
            if params.reference == shared { return try CrewdKnowledgeStore.encode(detail(block)) }
            precondition(params.reference == view)
            if let requested = params.atRevision {
                checkedVersions.append(requested)
                return try CrewdKnowledgeStore.encode(document(wrongHistoricalReply ? revision : requested,historical:true))
            }
            return try CrewdKnowledgeStore.encode(document(revision,historical:false))
        case "data.history":
            let params = try CrewdKnowledgeStore.decode(input,CrewdDataHistoryParams.self)
            precondition(params.context == context && params.reference == view && params.after == nil)
            let entries = (1...Int(revision)!).map { document(String($0),historical:false).object }
            return try CrewdKnowledgeStore.encode(CrewdDataHistory(context:context,items:entries,next:nil))
        case "data.submit":
            let request = try CrewdKnowledgeStore.decode(input,CrewdDataSubmitParams.self)
            submits.append(request)
            if reject {
                throw CrewdClientError(message:"stale generation",code:-32009,data:.object(["refusal":.string("data.context_conflict")]))
            }
            precondition(request.context == context)
            guard case .apply(_,let edits) = request.change, edits.count == 1,
                  case .updateView(let id,let expected,let title,.document(let inputs,let layout),let sources,let attributes) = edits[0],
                  id == view.id, expected == revision, title == "原题", layout == self.layout,
                  sources == nil, attributes == self.attributes else { preconditionFailure("incorrect scoped mutation") }
            let next = String(Int(revision)! + 1)
            let version = CrewdDataObjectVersion(reference:view,revision:next)
            var created: [String:CrewdDataCreated] = [:]
            positions = inputs.map { position in
                switch position {
                case .keep(let id,let layout):
                    let old = positions.first { $0.id == id }!
                    precondition(old.layout == layout)
                    return old
                case .set(let id,let alias,.existing(let reference),let pin,let layout):
                    precondition(pin == (reference == shared ? "7" : "1"))
                    let positionID = id ?? (revision == "2" ? newPosition : reinsertedPosition)
                    if let alias { created[alias] = .placement(id:positionID,parentView:version) }
                    return .init(id:positionID,block:.visible(reference:reference),atRevision:pin,layout:layout)
                default: preconditionFailure("unexpected placement edit")
                }
            }
            let previous = context.generation
            context.generation = next
            revision = next
            versions[next] = positions
            let committed = CrewdDataChangeResult(requestId:request.requestId,phase:.committed,context:context,
                beforeGeneration:previous,afterGeneration:context.generation,snapshotId:nil,created:created,
                changed:[version],conflicts:[],reason:nil,resolvedByRequest:nil)
            receipt = committed
            if loseReply { throw CocoaError(.fileReadUnknown) }
            return try CrewdKnowledgeStore.encode(committed)
        case "data.request.get":
            lookupIDs.append(input["requestId"].string!)
            precondition(receipt?.requestId == lookupIDs.last)
            return try CrewdKnowledgeStore.encode(CrewdDataRequestGetResult(workspace:workspace,lookup:.known(value:receipt!)))
        default: preconditionFailure("unexpected RPC " + method)
        }
    }
}

@main struct TestDocumentComposition {
    @MainActor static func main() async throws {
        let mode = CommandLine.arguments.dropFirst().first ?? "save"
        let wire = CompositionWire()
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at:root,withIntermediateDirectories:false,attributes:[.posixPermissions:0o700])
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,
            authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        func makeStore() throws -> CrewdKnowledgeStore {
            let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
            let journal = try CrewdPresentationJournal(root:root,scope:scope)
            return .init(origin:{origin},rpc:{try await wire.call($0,$1)},journal:{journal})
        }
        func read(_ store: CrewdKnowledgeStore) async throws -> CrewdKnowledgeStore.DocumentComposition {
            await store.refresh(loadViews:false)
            await store.open(wire.view)
            return try store.nativeDocumentComposition(store.detail!)
        }
        let store = try makeStore()
        let original = try await read(store)
        let page = try await store.loadDocumentCompositionBlocks(original)
        let block = page.items[0]
        precondition(block.reference == wire.shared && block.revision == "7")
        let proposed: [CrewdKnowledgeStore.DocumentCompositionPosition] = [.pinned(block),.existing(wire.bodyPosition)]
        if mode == "scope" {
            let foreign = CrewdKnowledgeStore.DocumentCompositionBlock(
                reference:.init(namespace:"foreign",kind:"block",id:block.id),revision:block.revision,
                contentType:block.contentType,preview:block.preview)
            do {
                _ = try await store.saveDocumentComposition(original,positions:[.pinned(foreign)])
                preconditionFailure("foreign block must be rejected without a write")
            } catch CrewdKnowledgeStore.Failure.invalid {}
            precondition(wire.submits.isEmpty && !store.hasBlockingPending)
            print("PASS document composition " + mode)
            return
        }
        wire.reject = mode == "conflict"
        wire.loseReply = mode == "lost"
        if wire.reject {
            do {
                _ = try await store.saveDocumentComposition(original,positions:proposed)
                preconditionFailure("expected the typed CAS refusal")
            } catch let error as CrewdClientError { precondition(error.code == -32009) }
            precondition(wire.submits.count == 1 && !store.hasBlockingPending && wire.receipt == nil)
        } else {
            let result = try await store.saveDocumentComposition(original,positions:proposed)
            precondition(wire.positions.map(\.id) == [wire.newPosition,wire.bodyPosition])
            precondition(wire.positions.map(\.atRevision) == ["7",nil])
            if wire.loseReply {
                precondition(result == nil && wire.submits.count == 1)
                let request = UUID(uuidString:wire.submits[0].requestId)!
                wire.revision = "4"; wire.context.generation = "4"
                wire.versions["4"] = [wire.positions[1]]
                let reopened = try makeStore()
                _ = try await read(reopened)
                precondition(reopened.pendingDocumentCompositions(viewID:wire.view.id).map(\.id) == [request])
                wire.wrongHistoricalReply = true
                await reopened.lookup(request)
                precondition(reopened.hasBlockingPending && reopened.result == nil)
                wire.wrongHistoricalReply = false
                await reopened.lookup(request)
                precondition(reopened.result?.phase == .committed && !reopened.hasBlockingPending)
                precondition(wire.checkedVersions == ["3","3"])
                precondition(wire.lookupIDs == Array(repeating:wire.submits[0].requestId,count:2) && wire.submits.count == 1)
            } else {
                precondition(result?.phase == .committed && wire.checkedVersions == ["3"])
                let latest = try await read(store)
                let revisions = try await store.loadNativeDocumentRevisions(latest.target)
                let historical = try await store.inspectDocumentCompositionRevision(latest,revision:revisions.items[0])
                precondition(historical.canRestore && historical.placements.map(\.atRevision) == ["1"])
                let restored = try await store.restoreDocumentComposition(historical,replacing:latest)
                precondition(restored?.phase == .committed && wire.checkedVersions == ["3","1","4"])
                precondition(wire.positions.map(\.id) == [wire.bodyPosition] && wire.positions.map(\.atRevision) == ["1"])
                let current = try await read(store)
                let available = try await store.loadNativeDocumentRevisions(current.target)
                let previous = try await store.inspectDocumentCompositionRevision(current,revision:available.items[2])
                let reinserted = try await store.restoreDocumentComposition(previous,replacing:current)
                precondition(reinserted?.phase == .committed && wire.positions.map(\.id) == [wire.reinsertedPosition,wire.bodyPosition])
                precondition(wire.checkedVersions == ["3","1","4","3","5"])
                precondition(wire.submits.count == 3)
            }
        }
        print("PASS document composition " + mode)
    }
}
