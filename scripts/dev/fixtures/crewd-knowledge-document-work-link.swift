import Foundation

@MainActor private final class LinkWire {
    let document = CrewdDataRef(namespace:"knowledge",kind:"view",id:"10000000-0000-0000-0000-000000000001")
    let relation = CrewdDataRef(namespace:"knowledge",kind:"relation",id:"20000000-0000-0000-0000-000000000001")
    let work = CrewdDataRef(namespace:"knowledge",kind:"plan",id:"W7")
    let requirement = CrewdDataRef(namespace:"knowledge",kind:"requirement",id:"#8")
    let predicate = "pendingcrew.document-supports-work.v1"
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:"crew",namespace:"knowledge",generation:"1")
    var archived: [Bool] = []
    var target: CrewdDataRef?
    var sends: [CrewdDataSubmitParams] = []
    var readbacks: [String] = []
    var lookups: [String] = []
    var receipts: [String:CrewdDataChangeResult] = [:]
    var loseReply = false
    var genericFailure = false
    var typedRefusal = false
    var wrongPinnedArchive = false
    var wrongLiveWork = false
    var webDocument = false
    var hideWork = false
    var paginate = false
    var onCommit: (() -> Void)?
    var state: CrewdDataWorkspaceInfo { .init(context:context,state:"ready",engineVersion:"SQLite",reason:nil) }

    private var provenance: CrewdDataProvenance {
        let actor: CrewdValue = .object(["kind":.string("human")])
        return .init(createdBy:actor,updatedBy:actor,createdAt:"1",updatedAt:"1",sources:[],attributes:.object([:]))
    }
    private func detail(_ object: CrewdDataObject) -> CrewdDataObjectDetail {
        .init(context:context,resolvedContext:context,generation:context.generation,object:object,
              references:[],affectedLiveViews:[],affectedTruncated:false)
    }
    private func relationObject(_ revision: Int, historical: Bool) -> CrewdDataObject {
        let object: CrewdDataRefState = historical || hideWork ? .unavailable : .visible(reference:wrongLiveWork ? requirement : target!)
        let archived = archived[revision-1]
        return .relation(id:relation.id,revision:String(revision),subject:.visible(reference:document),predicate:predicate,
                         object:object,attributes:.object([:]),archived:historical && wrongPinnedArchive ? !archived : archived,provenance:provenance)
    }
    func call(_ method: String, _ params: CrewdValue) throws -> CrewdValue {
        switch method {
        case "data.workspace.get":
            precondition(params["crewId"] == .string("crew"))
            return try CrewdKnowledgeStore.encode(state)
        case "data.object.get":
            let p = try CrewdKnowledgeStore.decode(params,CrewdDataObjectGetParams.self)
            precondition(p.context == context)
            if p.reference == document {
                let definition: CrewdDataViewDefinition = webDocument
                    ? .web(renderer:"web",rendererVersion:"1",html:"",css:"",javascript:"",bindings:[],requestedCapabilities:[],attributes:.object([:]))
                    : .document(placements:[],layout:.object([:]))
                return try CrewdKnowledgeStore.encode(detail(.view(id:document.id,revision:"1",title:"原文档",definition:definition,archived:false,provenance:provenance)))
            }
            precondition(p.reference == relation)
            readbacks.append(p.atRevision ?? "live")
            return try CrewdKnowledgeStore.encode(detail(relationObject(p.atRevision.flatMap(Int.init) ?? archived.count,historical:p.atRevision != nil)))
        case "data.object.list":
            let p = try CrewdKnowledgeStore.decode(params,CrewdDataObjectListParams.self)
            precondition(p.context == context && p.objectType == "relation" && p.limit == 12 && p.includeArchived)
            let legacy: [CrewdDataObject] = (1...12).map {
                let id = "10000000-0000-0000-0000-" + String(format:"%012d",$0)
                return .relation(id:id,revision:"1",subject:.visible(reference:document),predicate:predicate,
                                 object:.visible(reference:document),attributes:.object([:]),archived:false,provenance:provenance)
            }
            let items: [CrewdDataObject], next: String?
            if paginate && p.after == nil { items = legacy; next = legacy.last?.identity }
            else {
                precondition(!paginate || p.after == legacy.last?.identity)
                items = archived.isEmpty ? [] : [relationObject(archived.count,historical:false)]; next = nil
            }
            return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,items:items,next:next))
        case "data.submit":
            let p = try CrewdKnowledgeStore.decode(params,CrewdDataSubmitParams.self)
            precondition(p.context == context && p.requestId == p.requestId.lowercased())
            sends.append(p)
            if genericFailure { throw CrewdClientError(message:"generic store error",code:-32009,data:.null) }
            if typedRefusal {
                throw CrewdClientError(message:"reserved relation refused",code:-32009,data:.object(["refusal":.string("data.document_work_link_refused")]))
            }
            guard case .apply(_,let edits) = p.change, edits.count == 1, let edit = edits.first else { preconditionFailure() }
            var created: [String:CrewdDataCreated] = [:]
            switch edit {
            case let .createRelation(alias,subject,predicate,object,attributes):
                precondition(archived.isEmpty && alias == "documentWorkLink" && subject == .existing(reference:document)
                    && predicate == self.predicate && attributes == .object([:]))
                guard case .existing(let ref) = object, ref == work || ref == requirement else { preconditionFailure() }
                target = ref; archived.append(false)
                created[alias] = .object(object:.init(reference:relation,revision:"1"))
            case let .archiveRelation(id,revision,state):
                precondition(id == relation.id && revision == String(archived.count) && state != archived.last)
                archived.append(state)
            default: preconditionFailure("Only one typed relation can be changed")
            }
            let before = context.generation
            context.generation = String(Int(before)! + 1)
            let receipt = CrewdDataChangeResult(requestId:p.requestId,phase:.committed,context:context,beforeGeneration:before,
                afterGeneration:context.generation,snapshotId:nil,created:created,changed:[.init(reference:relation,revision:String(archived.count))],
                conflicts:[],reason:nil,resolvedByRequest:nil)
            receipts[p.requestId] = receipt
            onCommit?()
            if loseReply { throw CocoaError(.fileReadUnknown) }
            return try CrewdKnowledgeStore.encode(receipt)
        case "data.request.get":
            let p = try CrewdKnowledgeStore.decode(params,CrewdDataRequestGetParams.self)
            precondition(p.context.generation == "1")
            lookups.append(p.requestId)
            return try CrewdKnowledgeStore.encode(CrewdDataRequestGetResult(workspace:state,
                lookup:receipts[p.requestId].map { .known(value:$0) } ?? .notFound))
        default: preconditionFailure("unexpected relationship RPC \(method)")
        }
    }
}

@main struct DocumentWorkLinkContract {
    @MainActor static func main() async throws {
        let mode = CommandLine.arguments.dropFirst().first ?? "lifecycle"
        let root = URL(fileURLWithPath:"/private/tmp/pcw-knowledge-work-link-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        var origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
        let wire = LinkWire()
        func makeStore() throws -> CrewdKnowledgeStore {
            let journal = try CrewdPresentationJournal(root:root,scope:scope)
            return CrewdKnowledgeStore(origin:{origin},rpc:{try wire.call($0,$1)},journal:{journal})
        }
        func documentAnchor(_ store: CrewdKnowledgeStore) async throws -> CrewdKnowledgeStore.DocumentWorkLinkAnchor {
            await store.refresh(loadViews:false)
            await store.open(wire.document)
            return try store.documentWorkLinkAnchor(document:store.detail!)
        }
        let store = try makeStore()
        let preactivated = try makeStore()
        var anchor = try await documentAnchor(store)
        let otherAnchor = try await documentAnchor(preactivated)
        wire.loseReply = mode == "lost"
        wire.genericFailure = mode == "generic"
        wire.typedRefusal = mode == "refused"
        wire.wrongPinnedArchive = mode == "pinned"
        wire.wrongLiveWork = mode == "work"
        if mode == "own-event" {
            wire.onCommit = { [weak store] in store?.handleEvent("data.changed",.object(["crewId":.string("crew")])) }
        }
        let target = mode == "lifecycle" ? wire.requirement : wire.work
        let created: CrewdDataChangeResult?
        do { created = try await store.createDocumentWorkLink(anchor,work:target) }
        catch let error as CrewdClientError where mode == "refused" {
            precondition(error.refusal == "data.document_work_link_refused" && wire.sends.count == 1 && store.pending.isEmpty && wire.readbacks.isEmpty)
            print("PASS exact typed precommit refusal")
            return
        }
        if mode == "own-event" {
            precondition(created?.phase == .committed && store.workspace == nil && !store.documentWorkLinkScopeIsCurrent(anchor))
            anchor = try await store.reloadDocumentWorkLinkAnchor(anchor)
            let rows = try await store.loadDocumentWorkLinkPage(anchor)
            precondition(rows.items.first?.work == wire.work && store.pending.isEmpty && wire.sends.count == 1)
        } else if mode == "lifecycle" {
            precondition(created?.phase == .committed && wire.readbacks == ["1","live"])
            anchor = try await store.reloadDocumentWorkLinkAnchor(anchor)
            wire.paginate = true
            let partial = try await store.loadDocumentWorkLinkPage(anchor)
            precondition(partial.items.isEmpty && partial.next != nil)
            let page = try await store.loadDocumentWorkLinkPage(anchor,after:partial.next)
            precondition(page.next == nil && page.items.count == 1 && page.items[0].work == target)
            let reader = try makeStore()
            await reader.refresh(loadViews:false)
            let fromWork = try reader.documentWorkLinkAnchor(work:target)
            try await reader.openDocumentWorkLinkDocument(fromWork,link:page.items[0])
            precondition(reader.detail?.object.identity == wire.document.id)
            wire.webDocument = true
            do { try await reader.openDocumentWorkLinkDocument(fromWork,link:page.items[0]); preconditionFailure("web view was opened") }
            catch CrewdKnowledgeStore.Failure.unavailable {}
            guard case .view(_,_,_,.document,false,_)? = reader.detail?.object else { preconditionFailure("native presentation was replaced") }
            wire.webDocument = false; wire.paginate = false; wire.hideWork = true
            let unavailable = try await store.loadDocumentWorkLinkPage(anchor).items[0]
            precondition(unavailable.work == nil)
            let archived = try await store.setDocumentWorkLinkArchived(anchor,link:unavailable,archived:true)
            precondition(archived?.phase == .committed)
            anchor = try await store.reloadDocumentWorkLinkAnchor(anchor)
            wire.hideWork = false
            let archivedLink = try await store.loadDocumentWorkLinkPage(anchor).items[0]
            precondition(archivedLink.archived && archivedLink.revision == "2")
            await reader.refresh(loadViews:false)
            let archivedWork = try reader.documentWorkLinkAnchor(work:target)
            do { try await reader.openDocumentWorkLinkDocument(archivedWork,link:archivedLink); preconditionFailure("archived link was opened") }
            catch CrewdKnowledgeStore.Failure.stale {}
            let restored = try await store.setDocumentWorkLinkArchived(anchor,link:archivedLink,archived:false)
            precondition(restored?.phase == .committed && wire.sends.count == 3 && wire.archived == [false,true,false])
        } else {
            precondition(created == nil && store.hasBlockingPending)
            let id = store.pendingDocumentWorkLinks(anchor).first!.id
            do { _ = try await preactivated.createDocumentWorkLink(otherAnchor,work:wire.requirement); preconditionFailure("second store bypassed pending journal") }
            catch CrewdKnowledgeStore.Failure.unresolved {}
            precondition(wire.sends.count == 1)
            origin = .init(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
            do { _ = try await store.createDocumentWorkLink(anchor,work:target); preconditionFailure("stale connection accepted a write") }
            catch CrewdKnowledgeStore.Failure.stale {}
            wire.wrongPinnedArchive = false; wire.wrongLiveWork = false
            if mode != "generic" { wire.context.generation = "5" }
            let reopened = try makeStore()
            anchor = try await documentAnchor(reopened)
            precondition(reopened.pendingDocumentWorkLinks(anchor).map(\.id) == [id])
            await reopened.lookup(id)
            precondition(wire.sends.count == 1 && wire.lookups == [wire.sends[0].requestId])
            if mode == "generic" {
                precondition(reopened.hasBlockingPending && reopened.result == nil && wire.readbacks.isEmpty)
            } else {
                precondition(!reopened.hasBlockingPending && reopened.result?.phase == .committed)
                precondition(wire.readbacks.suffix(2) == ["1","live"])
            }
        }
        print("PASS document work link \(mode)")
    }
}
