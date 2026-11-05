import Foundation
import Combine
import CryptoKit

enum CrewdKnowledgeTitleSearch {
    static func normalized(_ query: String) -> String { query.trimmingCharacters(in:.whitespacesAndNewlines) }
    static func matches(_ title: String?, query: String) -> Bool {
        let term = normalized(query)
        if term.isEmpty { return true }
        return title?.range(of:term,options:[.caseInsensitive,.diacriticInsensitive,.widthInsensitive],locale:.current) != nil
    }
}

/// SQLite authority client. This model never opens the knowledge database locally.
@MainActor final class CrewdKnowledgeStore: ObservableObject, CrewdKnowledgeViewAuthority {
    struct Origin: Equatable {
        let scope: CrewdPresentationJournal.Scope
        let observation: UUID
        let connection: UUID
        let crew: String
        // SQLite Core authority is the authenticated database identity. Remote
        // pairing authority/grant remain separate in scope and Origin equality.
        var knowledgeAuthorityID: String? { scope.databaseID.flatMap { $0.isEmpty ? nil : $0 } }
    }
    struct Intent: Codable, Equatable, Identifiable {
        let id: UUID
        let context: CrewdDataContext
        let method: String? // operation identity only; never a replay payload
        let changeDigest: String // no replay payload or view code in the observation journal
        var documentEdit: NativeDocumentEditReceipt? = nil
        var documentCreation: NativeDocumentCreationWitness? = nil
        var viewArchive: NativeViewArchiveReceipt? = nil
        var documentWorkLink: DocumentWorkLinkReceipt? = nil
        var composition: DocumentCompositionReceipt? = nil
        var nativeTable: NativeTableWitness? = nil
    }
    struct NativeDocumentCreationWitness: Codable, Equatable {
        let titleDigest: String
        let bodyDigest: String

        init(title:String,body:String) throws {
            titleDigest = try CrewdKnowledgeStore.compositionDigest(title)
            bodyDigest = try CrewdKnowledgeStore.compositionDigest(body)
        }
    }
    struct NativeDocumentEdit: Equatable {
        let title: String
        let body: String
        let selectedPlacement: EditableDocumentPlacement?
        fileprivate let origin: Origin
        fileprivate let parent: CrewdDataObjectDetail
        fileprivate let view: CrewdDataRef
        fileprivate let placement: CrewdDataPlacement
        fileprivate let layout: CrewdValue
        fileprivate let viewAttributes: CrewdValue
        fileprivate let bodyRef: CrewdDataRef
        fileprivate let bodyRevision: String
        fileprivate let bodyAttributes: CrewdValue

        func hasChanges(title: String, body: String) -> Bool {
            !title.utf8.elementsEqual(self.title.utf8) || !body.utf8.elementsEqual(self.body.utf8)
        }
        func rebasedDraft(from previous: Self, title: String, body: String) -> (title: String, body: String) {
            (title.utf8.elementsEqual(previous.title.utf8) ? self.title : title,
             body.utf8.elementsEqual(previous.body.utf8) ? self.body : body)
        }
    }
    struct EditableDocumentPlacement: Equatable, Identifiable {
        let id: String
        let position: Int
        let pinnedRevision: String?
    }
    fileprivate struct SelectedPlacementEditReceipt: Codable, Equatable {
        struct Position: Codable, Equatable {
            let id: String
            let layoutDigest: String
            let visibleBlock: CrewdDataRef?
            let pinnedRevision: String?
        }
        let selectedID: String
        let positions: [Position]
        let titleDigest: String
        let layoutDigest: String
        let viewAttributesDigest: String
        let bodyDigest: String?
        let bodyAttributesDigest: String?
        let sourceRevision: String?
    }
    struct NativeDocumentEditReceipt: Codable, Equatable {
        fileprivate let view: CrewdDataObjectVersion
        fileprivate let replacedBody: CrewdDataRef?
        fileprivate var selected: SelectedPlacementEditReceipt? = nil

        fileprivate func matches(_ result: CrewdDataChangeResult) -> Bool {
            if let replacedBody {
                guard result.created.count == 1, case .object(let body)? = result.created["replacementBody"],
                      body.revision == "1", body.reference.kind == "block",
                      body.reference.namespace == view.reference.namespace, body.reference != replacedBody,
                      UUID(uuidString:body.reference.id)?.uuidString.lowercased() == body.reference.id else { return false }
                return result.changed.count == 2 && result.changed.contains(view) && result.changed.contains(body)
            }
            return result.created.isEmpty && result.changed == [view]
        }
    }
    struct NativeViewArchiveTarget: Equatable {
        var id: String { reference.id }
        let title: String
        let archived: Bool
        fileprivate let origin: Origin
        fileprivate let parent: CrewdDataObjectDetail
        fileprivate let reference: CrewdDataRef
    }
    struct NativeViewArchiveReceipt: Codable, Equatable {
        fileprivate let view: CrewdDataObjectVersion
        let archived: Bool
    }
    static let documentWorkPredicate = "pendingcrew.document-supports-work.v1"
    struct DocumentWorkLinkAnchor: Equatable {
        let endpoint: CrewdDataRef
        var crew: String { context.crewId }
        fileprivate let context: CrewdDataContext
        fileprivate let origin: Origin
        fileprivate let document: CrewdDataObjectDetail?
    }
    struct DocumentWorkLink: Identifiable, Equatable {
        var id: String { reference.id }
        let reference: CrewdDataRef
        let revision: String
        let document: CrewdDataRef?
        let work: CrewdDataRef?
        let archived: Bool
    }
    struct DocumentWorkLinkPage {
        let items: [DocumentWorkLink]
        let next: String?
    }
    struct DocumentWorkLinkReceipt: Codable, Equatable {
        let document: CrewdDataRef
        let work: CrewdDataRef?
        let archived: Bool
        fileprivate let next: CrewdDataObjectVersion?

        fileprivate func version(in result: CrewdDataChangeResult) -> CrewdDataObjectVersion? {
            if let next { return result.created.isEmpty && result.changed == [next] ? next:nil }
            guard result.created.count == 1, case .object(let value)? = result.created["documentWorkLink"],
                  value.reference.namespace == document.namespace, value.reference.kind == "relation",
                  UUID(uuidString:value.reference.id)?.uuidString.lowercased() == value.reference.id,
                  value.revision == "1", result.changed == [value] else { return nil }
            return value
        }
    }
    private struct NativeSubmissionRefusal: Error { let original: CrewdClientError }
    struct NativeDocumentHistoryTarget: Equatable {
        var head: String { parent.object.revision }
        fileprivate let origin: Origin
        fileprivate let parent: CrewdDataObjectDetail
        fileprivate let reference: CrewdDataRef
    }
    struct NativeDocumentRevision: Equatable, Identifiable {
        var id: String { revision }
        let revision: String
        let title: String
        let updatedAt: String
        let author: String?
        let archived: Bool
    }
    struct NativeDocumentRevisionPage {
        let items: [NativeDocumentRevision]
        let next: String?
    }
    struct NativeDocumentRevisionSnapshot {
        let revision: NativeDocumentRevision
        let body: String
        fileprivate let target: NativeDocumentHistoryTarget
        fileprivate let source: CrewdDataRef
        fileprivate let sourceRevision: String
    }
    struct DocumentComposition {
        let target: NativeDocumentHistoryTarget
        let placements: [CrewdDataPlacement]
    }
    struct DocumentCompositionBlock: Equatable, Identifiable {
        var id: String { reference.id }
        let reference: CrewdDataRef
        let revision: String
        let contentType: String
        let preview: String
    }
    struct DocumentCompositionBlockPage {
        let items: [DocumentCompositionBlock]
        let next: String?
    }
    struct DocumentCompositionRevision {
        let summary: NativeDocumentRevision
        let placements: [CrewdDataPlacement]
        fileprivate let target: NativeDocumentHistoryTarget
        fileprivate let restorable: Bool
        var canRestore: Bool { restorable && summary.revision != target.head }
    }
    struct DocumentCompositionReceipt: Codable, Equatable {
        struct Placement: Codable, Equatable {
            let id: String?
            let alias: String?
            let block: CrewdDataRef
            let revision: String?
            let layoutDigest: String
        }
        let view: CrewdDataObjectVersion
        let titleDigest: String
        let layoutDigest: String
        let attributesDigest: String
        let placements: [Placement]

        func placementIDs(in result: CrewdDataChangeResult) -> [String]? {
            let aliases = Set(placements.compactMap(\.alias))
            guard result.changed == [view], Set(result.created.keys) == aliases else { return nil }
            var ids: [String] = []
            for placement in placements {
                switch (placement.id, placement.alias) {
                case (.some(let id), nil): ids.append(id)
                case (nil, .some(let alias)):
                    guard case .placement(let id, let parent)? = result.created[alias], parent == view,
                          UUID(uuidString:id)?.uuidString.lowercased() == id else { return nil }
                    ids.append(id)
                default: return nil
                }
            }
            return Set(ids).count == ids.count ? ids : nil
        }
    }
    enum ViewCatalogKind: Equatable { case document, web }
    enum ViewCatalogStatus: Equatable { case idle, uninitialized, unavailable, ready, failed }
    struct ViewCatalogItem: Identifiable, Equatable {
        let id: String
        let title: String
        let kind: ViewCatalogKind
        let archived: Bool
    }
    struct ViewCatalog: Equatable {
        let context: CrewdDataContext
        let includeArchived: Bool
        fileprivate(set) var items: [ViewCatalogItem]
        fileprivate(set) var next: String?
        fileprivate let origin: Origin
    }
    enum Failure: Error { case unavailable, invalid, stale, unresolved, unsupported }
    typealias RPC = (String, CrewdValue) async throws -> CrewdValue
    @Published private(set) var workspace: CrewdDataWorkspaceInfo?
    @Published private(set) var objects: [CrewdDataObject] = []
    @Published private(set) var detail: CrewdDataObjectDetail?
    @Published private(set) var schema: CrewdDataSchemaResult?
    @Published private(set) var rows: CrewdDataRows?
    @Published private(set) var history: CrewdDataHistory?
    @Published private(set) var snapshots: CrewdDataSnapshots?
    @Published private(set) var preview: CrewdDataPreview?
    @Published private(set) var pending: [Intent] = []
    @Published private(set) var result: CrewdDataChangeResult?
    @Published private(set) var busy = false
    @Published private(set) var status = "知识数据尚未读取。"
    @Published private(set) var viewCatalog: ViewCatalog?
    @Published private(set) var catalogStatus = ViewCatalogStatus.idle
    private(set) var detailIsSnapshot = false
    private(set) var detailReference: CrewdDataRef?
    var canEditDetail: Bool { !detailIsSnapshot && detailReference?.namespace == workspace?.context.namespace }
    var canSaveNativeDocument: Bool {
        guard let active, origin() == active, let info = workspace else { return false }
        return info.state == "ready" || info.state == "uninitialized"
    }
    var hasBlockingPending: Bool {
        guard let context = workspace?.context else { return !pending.isEmpty }
        return pending.contains { sameAuthority($0.context, context) }
    }
    private let origin: () -> Origin?
    private let nativeNavigation: (CrewdDataRef, CrewdDataContext) -> Bool
    private let rpc: RPC
    private let journal: () throws -> CrewdPresentationJournal
    private var active: Origin?
    private var records: [UUID: CrewdPresentationJournal.Prepared] = [:]
    private var operation = UUID()
    private var rendererTerminal: [UUID: CrewdKnowledgeViewWriteObservation] = [:]
    private var rendererPrepared: [UUID: (Intent, CrewdPresentationJournal.Prepared, CrewdDataChange)] = [:]
    lazy var renderer = CrewdKnowledgeViewRuntime(authority: self)
    init(origin: @escaping () -> Origin?, rpc: @escaping RPC, journal: @escaping () throws -> CrewdPresentationJournal, nativeNavigation: @escaping (CrewdDataRef, CrewdDataContext) -> Bool = { _,_ in false }) {
        self.origin = origin; self.rpc = rpc; self.journal = journal; self.nativeNavigation = nativeNavigation
    }
    static func encode<T: Encodable>(_ value: T) throws -> CrewdValue { try JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(value)) }
    static func decode<T: Decodable>(_ value: CrewdValue, _ type: T.Type) throws -> T { try JSONDecoder().decode(type, from: JSONEncoder().encode(value)) }
    func invalidate() {
        operation = UUID(); active = nil; workspace = nil; objects = []; detail = nil; detailReference = nil
        schema = nil; rows = nil; history = nil; snapshots = nil; preview = nil
        pending = []; result = nil; busy = false; records = [:]
        clearViewCatalog()
        rendererPrepared = [:]; rendererTerminal = [:]; renderer.revoke()
        status = "连接、机组或权限已变化；未重发旧请求。"
    }
    func handleEvent(_ name: String, _ value: CrewdValue) {
        guard let active else { return }
        if name == "events.lagged" { invalidate(); return }
        let crews = Set([active.crew, detail?.resolvedContext.crewId].compactMap { $0 })
        if ["data.changed", "crew.changed", "crew.purged", "document.changed"].contains(name) {
            if let crew = value["crewId"].string, !crews.contains(crew) { return }
            invalidate(); return
        }
        if name == "session.changed" {
            let relatedCrew = value["crewId"].string.map { crews.contains($0) } ?? false
            let id = value["sessionId"].string ?? value["id"].string
            let relatedLink = detail?.references.contains { item in
                if case .visible(let ref) = item.target { return ref.kind == "session" && ref.id == id }; return false
            } ?? false
            if relatedCrew || relatedLink { invalidate() }
        }
    }
    private func clearPresentation() {
        renderer.revoke(); detail = nil; detailReference = nil; objects = []; schema = nil; rows = nil
        history = nil; snapshots = nil; preview = nil
        clearViewCatalog()
    }
    private func clearViewCatalog() { viewCatalog = nil; catalogStatus = .idle }
    private func perform(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        let captured = active
        do { return try await rpc(method, params) }
        catch {
            if active == captured, origin() == captured { clearPresentation() }
            throw error
        }
    }
    func synchronizeOrigin() { if let active, origin() != active { invalidate() } }
    private func current(_ o: Origin, _ token: UUID) -> Bool { origin() == o && active == o && operation == token }
    private func reconcilePending(_ o: Origin) throws {
        let stored = try journal().unknownRecords().filter { $0.method == "data.submit.observation" && $0.scope == o.scope && $0.crew == o.crew }
        var refreshed: [Intent] = []
        var evidence: [UUID:CrewdPresentationJournal.Prepared] = [:]
        for record in stored {
            let intent = try Self.decode(record.params, Intent.self)
            guard intent.context.crewId == o.crew, intent.context.dbId == o.scope.databaseID,
                  intent.context.authorityId == o.knowledgeAuthorityID, evidence[intent.id] == nil else { throw Failure.invalid }
            evidence[intent.id] = record
            refreshed.append(intent)
        }
        records = evidence
        if pending != refreshed { pending = refreshed }
    }
    private func begin() throws -> (Origin, UUID) {
        guard !busy, let o = origin(), !o.crew.isEmpty else { throw Failure.unavailable }
        if active != o { invalidate() }
        try reconcilePending(o)
        active = o
        let token = UUID(); operation = token; busy = true; return (o, token)
    }
    private func finish(_ o: Origin, _ token: UUID) {
        guard active == o, operation == token else { return }
        if origin() == o { busy = false } else { invalidate() }
    }
    private func checked(_ context: CrewdDataContext, _ o: Origin) throws {
        guard context.dbId == o.scope.databaseID, context.authorityId == o.knowledgeAuthorityID,
              context.crewId == o.crew, !context.namespace.isEmpty, !context.restoreEpoch.isEmpty, !context.generation.isEmpty else { throw Failure.invalid }
    }
    private func sameAuthority(_ a: CrewdDataContext, _ b: CrewdDataContext) -> Bool {
        a.dbId == b.dbId && a.authorityId == b.authorityId && a.restoreEpoch == b.restoreEpoch && a.crewId == b.crewId && a.namespace == b.namespace
    }
    func refresh(loadViews: Bool = true) async {
        let o: Origin, token: UUID
        do { (o, token) = try begin() } catch { status = "知识数据或原提交历史当前不可用。"; return }
        defer { finish(o, token) }
        do {
            let info = try Self.decode(try await perform("data.workspace.get", Self.encode(CrewdDataWorkspaceGetParams(crewId: o.crew))), CrewdDataWorkspaceInfo.self)
            try checked(info.context, o)
            guard current(o, token) else { return }
            if let old = workspace, old.context != info.context { renderer.revoke(); detail = nil; objects = []; clearViewCatalog() }
            workspace = info
            guard info.state == "ready" else { status = info.state == "uninitialized" ? "知识工作区尚未初始化；读取不会创建数据库。" : "知识工作区不可用；未退回旧库。"; return }
            if loadViews {
                let page = try Self.decode(try await perform("data.object.list", Self.encode(CrewdDataObjectListParams(context: info.context, objectType: "view", after: nil, limit: 50, atRevision: nil, includeArchived: false))), CrewdDataObjectPage.self)
                guard current(o, token), page.context == info.context else { throw Failure.stale }
                objects = page.items
            }
            status = "已读取当前数据视图；未改动数据。"
        } catch { if current(o, token) { status = "读取未完成或权限拒绝；未初始化或切换数据来源。" } }
    }
    private func catalogItem(_ object: CrewdDataObject) -> ViewCatalogItem? {
        guard case .view(let id,_,let title,let definition,let archived,_) = object,
              UUID(uuidString:id)?.uuidString.lowercased() == id else { return nil }
        switch definition {
        case .document: return .init(id:id,title:title,kind:.document,archived:archived)
        case .web: return .init(id:id,title:title,kind:.web,archived:archived)
        }
    }
    private func catalogPage(context: CrewdDataContext, includeArchived: Bool, after: String?) async throws -> ([ViewCatalogItem], String?) {
        // The wire returns full View values. Keep each reply small and retain
        // only the navigation metadata, never the Agent page HTML or scripts.
        let limit: UInt32 = 12
        let params = CrewdDataObjectListParams(context:context,objectType:"view",after:after,limit:limit,atRevision:nil,includeArchived:includeArchived)
        let page: CrewdDataObjectPage = try await call("data.object.list",params,preservingPresentation:true)
        guard page.context == context, page.generation == context.generation, page.items.count <= Int(limit),
              after == nil || !page.items.isEmpty else { throw Failure.invalid }
        var previous = after ?? ""
        let items = try page.items.map { object in
            guard let item = catalogItem(object), item.id > previous, includeArchived || !item.archived else { throw Failure.invalid }
            previous = item.id
            return item
        }
        if let next = page.next {
            guard items.count == Int(limit), next == items.last?.id else { throw Failure.invalid }
        }
        return (items,page.next)
    }
    func refreshViewCatalog(includeArchived: Bool) async {
        guard let (o, token) = try? begin() else { return }
        defer { finish(o,token) }
        clearViewCatalog()
        do {
            let info: CrewdDataWorkspaceInfo = try await call("data.workspace.get",CrewdDataWorkspaceGetParams(crewId:o.crew),preservingPresentation:true)
            try checked(info.context,o)
            guard current(o,token), !Task.isCancelled else { return }
            if let old = workspace, old.context != info.context { clearPresentation() }
            workspace = info
            guard info.state == "ready" else {
                clearPresentation()
                catalogStatus = info.state == "uninitialized" ? .uninitialized : .unavailable
                return
            }
            let (items,next) = try await catalogPage(context:info.context,includeArchived:includeArchived,after:nil)
            guard current(o,token), workspace?.context == info.context, !Task.isCancelled else { return }
            viewCatalog = .init(context:info.context,includeArchived:includeArchived,items:items,next:next,origin:o)
            catalogStatus = .ready
            status = "已读取当前数据视图；未改动数据。"
        } catch {
            guard current(o,token), !Task.isCancelled else { return }
            clearPresentation()
            catalogStatus = .failed
        }
    }
    func loadNextViewCatalogPage() async {
        guard var catalog = viewCatalog, let cursor = catalog.next, let (o, token) = try? begin() else { return }
        defer { finish(o,token) }
        guard catalog.origin == o, workspace?.context == catalog.context, viewCatalog == catalog else { return }
        do {
            let (items,next) = try await catalogPage(context:catalog.context,includeArchived:catalog.includeArchived,after:cursor)
            guard current(o,token), workspace?.context == catalog.context, viewCatalog == catalog, !Task.isCancelled else { return }
            catalog.items.append(contentsOf:items)
            catalog.next = next
            viewCatalog = catalog
            catalogStatus = .ready
        } catch {
            guard current(o,token), !Task.isCancelled else { return }
            let code = (error as? CrewdClientError)?.code
            if code == -32003 || code == -32009 { clearPresentation() }
            catalogStatus = .failed
        }
    }
    func openViewCatalogItem(_ id: String, kind: ViewCatalogKind, previouslyOpenedDocument: Bool = false) async {
        guard let catalog = viewCatalog, UUID(uuidString:id)?.uuidString.lowercased() == id,
              catalog.items.contains(where: { $0.id == id && $0.kind == kind }) || (previouslyOpenedDocument && kind == .document) else { return }
        await read { context in
            guard active == catalog.origin, context == catalog.context, viewCatalog == catalog else { throw Failure.stale }
            let reference = CrewdDataRef(namespace:context.namespace,kind:"view",id:id)
            let value: CrewdDataObjectDetail = try await self.call("data.object.get",
                CrewdDataObjectGetParams(context:context,reference:reference,atRevision:nil),preservingPresentation:true)
            guard value.context == context, value.resolvedContext == context, value.generation == context.generation,
                  let item = catalogItem(value.object), item.id == id, item.kind == kind,
                  catalog.includeArchived || !item.archived else { throw Failure.invalid }
            return {
                self.renderer.revoke(); self.detail = value; self.detailReference = reference; self.detailIsSnapshot = false
                self.installRenderer(value.object,snapshot:false)
            }
        }
    }
    func linkSourceCatalog(crew: String) -> ViewCatalog? {
        guard let catalog = viewCatalog, active == catalog.origin, origin() == catalog.origin,
              !catalog.includeArchived, catalog.context.crewId == crew, workspace?.context == catalog.context else { return nil }
        return catalog
    }
    func catalogLinkSource(_ id: String, kind: ViewCatalogKind, crew: String) -> CrewdChannelKnowledgeReference? {
        guard !busy, catalogStatus == .ready, let catalog = linkSourceCatalog(crew:crew),
              catalog.items.contains(where:{$0.id == id && $0.kind == kind && !$0.archived}) else { return nil }
        return .init(context:catalog.context,reference:.init(namespace:catalog.context.namespace,kind:"view",id:id))
    }
    func open(_ reference: CrewdDataRef, atRevision: String? = nil) async {
        guard ["block", "view", "relation"].contains(reference.kind) else { status = "关联对象须通过原生权限导航；未猜测知识 RPC。"; return }
        await read { context in
            let value: CrewdDataObjectDetail = try await self.call("data.object.get", CrewdDataObjectGetParams(context: context, reference: reference, atRevision: atRevision))
            guard value.object.identity == reference.id, value.resolvedContext.namespace == reference.namespace,
                  value.resolvedContext.authorityId == context.authorityId, value.resolvedContext.dbId == context.dbId,
                  value.resolvedContext.restoreEpoch == context.restoreEpoch, atRevision == nil || value.object.revision == atRevision else { throw Failure.invalid }
            return { self.renderer.revoke(); self.detail = value; self.detailReference = reference; self.detailIsSnapshot = atRevision != nil; self.installRenderer(value.object, snapshot: atRevision != nil) }
        }
    }
    func resolved(_ reference: CrewdDataRefState, anchor: String, atRevision: String?, parent: CrewdDataObjectDetail) async throws -> CrewdDataObjectDetail? {
        guard let o = active, origin() == o, let c = workspace?.context, detail == parent else { throw Failure.stale }
        guard case .visible(let ref) = reference, ["block", "view", "relation"].contains(ref.kind) else { return nil }
        let captured = parent.references.first { $0.anchor == anchor && $0.target == reference && $0.complete }
        let cut: String?
        if detailIsSnapshot {
            guard let commit = captured?.revision else { return nil }
            cut = commit
        } else { cut = atRevision }
        let value: CrewdDataObjectDetail = try await call("data.object.get", CrewdDataObjectGetParams(context:c,reference:ref,atRevision:cut))
        guard origin() == o, active == o, workspace?.context == c, detail == parent else { throw Failure.stale }
        guard value.object.identity == ref.id, value.resolvedContext.namespace == ref.namespace,
              value.resolvedContext.authorityId == c.authorityId, value.resolvedContext.dbId == c.dbId,
              value.resolvedContext.restoreEpoch == c.restoreEpoch, cut == nil || value.object.revision == cut else { throw Failure.invalid }
        return value
    }
    private func call<P: Encodable, R: Decodable>(_ method: String, _ params: P, preservingPresentation: Bool = false) async throws -> R {
        let input = try Self.encode(params)
        let output = try await (preservingPresentation ? rpc(method,input) : perform(method,input))
        if case .object = input["context"], case .object = output["context"] {
            let expected = try Self.decode(input["context"], CrewdDataContext.self)
            let actual = try Self.decode(output["context"], CrewdDataContext.self)
            guard method == "data.submit" ? sameAuthority(expected, actual) : expected == actual else { throw Failure.stale }
        }
        return try Self.decode(output, R.self)
    }
    private func read(_ action: (CrewdDataContext) async throws -> (() -> Void)) async {
        let o: Origin, token: UUID
        do { (o, token) = try begin() } catch { return }
        defer { finish(o, token) }
        do {
            guard let info = workspace, info.state == "ready" else { throw Failure.unavailable }
            try checked(info.context, o)
            let apply = try await action(info.context)
            guard current(o, token), workspace?.context == info.context else { return }
            apply(); status = "只读结果已更新。"
        } catch { if current(o, token) { status = "读取未完成；未重新提交任何写入。" } }
    }
    // Native table confirmations share the existing knowledge observation journal.
    struct NativeTableWitness: Codable, Equatable {
        enum Operation: String, Codable { case create, add, edit }
        let operation: Operation
        let table: String
        let columns: [String]
        let keyName: String?
        let key: CrewdDataCell?
        let expectedDigest: String
        var schemaDigest: String? = nil
    }
    struct NativeTableChange {
        let context: CrewdDataContext
        let statement: CrewdDataSql
        let witness: NativeTableWitness
        let row: [CrewdDataCell]?
    }
    struct NativeTablePreview {
        let change: NativeTableChange
        let preview: CrewdDataPreview
        fileprivate let origin: Origin
    }
    enum NativeTableReadback: Equatable { case exact, currentDiffers }

    func nativeTableCatalog(after: String? = nil) async throws -> CrewdDataSchemaResult {
        let (o, token) = try begin()
        defer { finish(o, token) }
        guard let info = workspace else { throw Failure.unavailable }
        try checked(info.context,o)
        if info.state == "uninitialized", after == nil {
            return .init(context:info.context,generation:info.context.generation,objects:[],next:nil)
        }
        guard info.state == "ready" else { throw Failure.unavailable }
        let page: CrewdDataSchemaResult = try await call("data.schema.get",
            CrewdDataSchemaParams(context:info.context,atRevision:nil,after:after,limit:30),preservingPresentation:true)
        guard current(o,token), workspace?.context == info.context, page.context == info.context,
              page.generation == info.context.generation, page.objects.count <= 30 else { throw Failure.stale }
        if let next = page.next {
            // Core may reach its byte budget before the requested row limit.
            guard let last = page.objects.last, last.name.utf8.elementsEqual(next.utf8) else { throw Failure.invalid }
        }
        var cursor = after.map { Array($0.utf8) }
        for object in page.objects {
            let name = Array(object.name.utf8)
            guard !name.isEmpty, cursor?.lexicographicallyPrecedes(name) ?? true else { throw Failure.invalid }
            cursor = name
        }
        return page
    }

    func nativeTablePage(_ table: CrewdDataSchemaObject, context: CrewdDataContext,
                         after: [CrewdDataCell]? = nil) async throws -> CrewdDataTablePage {
        let (o, token) = try begin()
        defer { finish(o, token) }
        guard workspace?.context == context, workspace?.state == "ready" else { throw Failure.stale }
        try checked(context,o)
        let page: CrewdDataTablePage = try await call("data.table.get",
            CrewdDataTableParams(context:context,table:table.name,afterKey:after,atRevision:nil,limit:16),preservingPresentation:true)
        guard current(o,token), workspace?.context == context, page.context == context, page.generation == context.generation,
              page.rows.count <= 16, !page.columns.isEmpty, page.rows.allSatisfy({ $0.count == page.columns.count }),
              after == nil || page.stableOrder, page.nextKey == nil || (page.stableOrder && !page.rows.isEmpty),
              !page.stableOrder || page.truncated == (page.nextKey != nil) else { throw Failure.invalid }
        if page.stableOrder {
            let keys = table.columns.filter { $0.primaryKeyPosition > 0 }
            guard keys.count == 1, let key = keys.first,
                  let index = page.columns.firstIndex(where: { $0.utf8.elementsEqual(key.name.utf8) }),
                  page.rows.allSatisfy({ $0[index] != .null }) else { throw Failure.invalid }
            if let next = page.nextKey {
                guard next.count == 1, let last = page.rows.last, CrewdNativeTable.exact(next[0],last[index]) else { throw Failure.invalid }
            }
            guard after == nil || after?.count == 1 else { throw Failure.invalid }
            var previous = after?.first
            for row in page.rows {
                let key = row[index]
                guard previous.map({ CrewdNativeTable.keyPrecedes($0,key) }) ?? true else { throw Failure.invalid }
                previous = key
            }
        }
        return page
    }

    func newNativeTable(_ definition: CrewdNativeTable.Definition, context: CrewdDataContext) throws -> NativeTableChange {
        guard workspace?.context == context, definition.columns.first?.type != .real else { throw Failure.stale }
        let statement = try definition.createStatement()
        let witness = NativeTableWitness(operation:.create,table:definition.name,columns:definition.columns.map(\.name),
            keyName:nil,key:nil,expectedDigest:try Self.compositionDigest(statement.sql))
        return .init(context:context,statement:statement,witness:witness,row:nil)
    }

    func saveNativeTableRow(_ model: CrewdNativeTable.Model, row: [CrewdDataCell], original: [CrewdDataCell]?,
                            context: CrewdDataContext) throws -> NativeTableChange {
        guard workspace?.context == context else { throw Failure.stale }
        let statement = try model.statement(row:row,original:original)
        let witness = NativeTableWitness(operation:original == nil ? .add : .edit,table:model.schema.name,
            columns:model.columns.map(\.name),keyName:model.key.name,key:row[model.keyIndex],
            expectedDigest:try Self.compositionDigest(row),schemaDigest:try Self.compositionDigest(model.schema.createSql))
        return .init(context:context,statement:statement,witness:witness,row:row)
    }

    func previewNativeTable(_ change: NativeTableChange, stillCurrent: () -> Bool) async throws -> NativeTablePreview {
        let (o, token) = try begin()
        defer { finish(o, token) }
        guard stillCurrent(), !hasBlockingPending, workspace?.context == change.context else { throw Failure.stale }
        let p = CrewdDataPreviewParams(context:change.context,statements:[change.statement])
        let preview: CrewdDataPreview = try await call("data.preview",p,preservingPresentation:true)
        let creating = change.witness.operation == .create
        guard stillCurrent(), current(o,token), workspace?.context == change.context,
              preview.context == change.context, UUID(uuidString:preview.token)?.uuidString.lowercased() == preview.token,
              try Self.compositionDigest(preview.statements) == Self.compositionDigest(p.statements),
              preview.schemaChanged == creating, preview.affectedRows == (creating ? 0 : 1) else { throw Failure.invalid }
        return .init(change:change,preview:preview,origin:o)
    }

    func confirmNativeTable(_ staged: NativeTablePreview, stillCurrent: () -> Bool) async throws -> NativeTableReadback? {
        let (o, token) = try begin()
        defer { finish(o, token) }
        let context = staged.change.context
        guard o == staged.origin, workspace?.context == context, stillCurrent(), !hasBlockingPending else { throw Failure.stale }
        let ledger = try journal(), id = UUID()
        let intent = Intent(id:id,context:context,method:"data.confirm",changeDigest:try Self.compositionDigest(staged.preview),
                            nativeTable:staged.change.witness)
        let record = try ledger.prepare(method:"data.submit.observation",params:Self.encode(intent),crew:o.crew)
        records[id] = record; pending.append(intent)
        let params = CrewdDataConfirmParams(requestId:id.uuidString.lowercased(),context:context,token:staged.preview.token)
        let reply: CrewdDataChangeResult
        do {
            reply = try Self.decode(try await rpc("data.confirm",Self.encode(params)),CrewdDataChangeResult.self)
        } catch let error as CrewdClientError where Self.definiteNativeRefusal(error) {
            try ledger.finish(record,state:"refused")
            if current(o,token) { try reconcilePending(o) }
            throw error
        }
        guard origin() == o, stillCurrent(), !Task.isCancelled else { return nil }
        try checkObservation(reply,intent:intent)
        guard reply.phase == .committed else { return nil }
        // An ordinary own data.changed can invalidate presentation before the reply.
        // Read against the actual fresh Core context; durable evidence remains scoped to the original.
        let fresh = try Self.decode(try await rpc("data.workspace.get",Self.encode(CrewdDataWorkspaceGetParams(crewId:o.crew))),CrewdDataWorkspaceInfo.self)
        try checked(fresh.context,o)
        let readback = try await verifyNativeTableReadback(reply,intent:intent,context:fresh.context,origin:o)
        guard origin() == o, stillCurrent(), !Task.isCancelled else { return nil }
        try observe(reply,intent:intent,record:record,ledger:ledger)
        if current(o,token) { applyObservation(reply,intent:intent) }
        return readback
    }

    private func verifyNativeTableReadback(_ receipt: CrewdDataChangeResult, intent: Intent, context: CrewdDataContext,
                                           origin o: Origin) async throws -> NativeTableReadback {
        try checkObservation(receipt,intent:intent)
        guard let witness = intent.nativeTable, intent.method == "data.confirm", receipt.phase == .committed,
              receipt.created.isEmpty, receipt.changed.isEmpty, receipt.conflicts.isEmpty,
              receipt.snapshotId.flatMap(UUID.init(uuidString:)) != nil,
              let before = UInt64(receipt.beforeGeneration), before < UInt64.max,
              receipt.afterGeneration == String(before + 1), let actual = UInt64(context.generation), actual > before,
              origin() == o, sameAuthority(intent.context,context) else { throw Failure.invalid }
        let tableLookup = CrewdDataSql(sql:"SELECT sql FROM sqlite_schema WHERE type = 'table' AND name COLLATE BINARY = ? LIMIT 2",
                                       params:[.text(value:witness.table)])
        let statement: CrewdDataSql
        if witness.operation == .create { statement = tableLookup }
        else {
            if context.generation != receipt.afterGeneration {
                let currentTable: CrewdDataRows = try await call("data.query",
                    CrewdDataQueryParams(context:context,statement:tableLookup,atRevision:nil,cursor:nil,limit:2),preservingPresentation:true)
                guard origin() == o, currentTable.generation == context.generation, currentTable.columns == ["sql"],
                      !currentTable.truncated, currentTable.next == nil, currentTable.rows.count <= 1 else { throw Failure.invalid }
                if currentTable.rows.isEmpty { return .currentDiffers }
                guard let row = currentTable.rows.first, row.count == 1, case .text(let sql) = row[0],
                      let schemaDigest = witness.schemaDigest else { throw Failure.invalid }
                if try Self.compositionDigest(sql) != schemaDigest { return .currentDiffers }
            }
            guard let keyName = witness.keyName, let key = witness.key else { throw Failure.invalid }
            statement = try CrewdNativeTable.rowQuery(table:witness.table,columns:witness.columns,keyName:keyName,key:key)
        }
        let rows: CrewdDataRows = try await call("data.query",
            CrewdDataQueryParams(context:context,statement:statement,atRevision:nil,cursor:nil,limit:2),preservingPresentation:true)
        guard origin() == o, rows.generation == context.generation, !rows.truncated, rows.next == nil,
              rows.rows.count <= 1 else { throw Failure.invalid }
        let matches: Bool
        if witness.operation == .create {
            if rows.columns == ["sql"], let row = rows.rows.first, row.count == 1, case .text(let sql) = row[0] {
                matches = try Self.compositionDigest(sql) == witness.expectedDigest
            } else { matches = false }
        } else {
            guard rows.columns.map({Array($0.utf8)}) == witness.columns.map({Array($0.utf8)}) else { throw Failure.invalid }
            if let row = rows.rows.first, row.count == witness.columns.count {
                matches = try Self.compositionDigest(row) == witness.expectedDigest
            } else { matches = false }
        }
        if matches { return .exact }
        guard context.generation != receipt.afterGeneration else { throw Failure.invalid }
        return .currentDiffers
    }

    func loadSchema(after: String? = nil) async {
        await read { c in let value: CrewdDataSchemaResult = try await self.call("data.schema.get", CrewdDataSchemaParams(context:c, atRevision:nil, after:after, limit:50)); return { self.schema = value } }
    }
    func query(_ statement: CrewdDataSql, cursor: String? = nil) async {
        await read { c in let value: CrewdDataRows = try await self.call("data.query", CrewdDataQueryParams(context:c, statement:statement, atRevision:nil, cursor:cursor, limit:100)); return { self.rows = value } }
    }
    func loadHistory(after: String? = nil) async {
        guard let reference = detailReference else { return }
        await read { c in
            let value: CrewdDataHistory = try await self.call("data.history", CrewdDataHistoryParams(context:c, reference:reference, after:after, limit:50))
            return { self.history = value }
        }
    }
    func loadSnapshots() async {
        await read { c in
            let value: CrewdDataSnapshots = try await self.call("data.snapshots", CrewdDataSnapshotsParams(context:c))
            return { self.snapshots = value }
        }
    }
    func readPreview(_ token: String) async {
        preview = nil
        await read { c in
            let value: CrewdDataPreview = try await self.call("data.preview.get", CrewdDataPreviewGetParams(context:c, token:token))
            return { self.preview = value }
        }
    }
    func previewSQL(_ statements: [CrewdDataSql]) async {
        preview = nil
        await read { c in
            let value: CrewdDataPreview = try await self.call("data.preview", CrewdDataPreviewParams(context:c, statements:statements))
            return { self.preview = value }
        }
    }
    func confirmPreview() async {
        guard let preview, preview.context == workspace?.context else { return }
        await privilegedMutation(method: "data.confirm", fields: ["token": .string(preview.token)])
        self.preview = nil
    }
    func restoreSnapshot(_ id: String) async {
        await privilegedMutation(method: "data.restore", fields: ["snapshotId": .string(id)])
    }
    private func privilegedMutation(method: String, fields: [String: CrewdValue]) async {
        let o: Origin, token: UUID
        do { (o,token) = try begin() } catch { return }
        defer { finish(o,token) }
        do {
            guard let c = workspace?.context, !hasBlockingPending else { throw Failure.unresolved }
            let id = UUID()
            // Store only the digest and original UUID, never replay this privileged operation.
            let bytes = try JSONEncoder().encode(CrewdValue.object(fields))
            let intent = Intent(id:id,context:c,method:method,changeDigest:SHA256.hash(data:bytes).map { String(format:"%02x",$0) }.joined())
            let ledger = try journal()
            let record = try ledger.prepare(method:"data.submit.observation",params:Self.encode(intent),crew:o.crew)
            records[id] = record; pending.append(intent)
            var params = fields
            params["requestId"] = .string(id.uuidString.lowercased()); params["context"] = try Self.encode(c)
            let response = try Self.decode(try await perform(method,.object(params)),CrewdDataChangeResult.self)
            try observe(response,intent:intent,record:record,ledger:ledger)
            guard current(o,token) else { return }
            applyObservation(response,intent:intent)
        } catch { if current(o,token) { renderer.revoke(); status = "结果未确认；只可核对原请求，不自动重试。" } }
    }
    private func stage(_ change: CrewdDataChange, id: UUID, context: CrewdDataContext, documentEdit: NativeDocumentEditReceipt? = nil, documentCreation: NativeDocumentCreationWitness? = nil, viewArchive: NativeViewArchiveReceipt? = nil, documentWorkLink: DocumentWorkLinkReceipt? = nil, composition: DocumentCompositionReceipt? = nil) throws -> (Intent, CrewdPresentationJournal.Prepared) {
        guard let o = active, origin() == o else { throw Failure.stale }
        try checked(context, o)
        // The library, group detail and renderer can hold distinct live stores.
        // Consult their common durable scope before preparing any new write.
        try reconcilePending(o)
        guard !hasBlockingPending else { throw Failure.unresolved }
        let bytes = try JSONEncoder().encode(change)
        guard bytes.count <= 8_388_608 else { throw Failure.invalid }
        let intent = Intent(id:id,context:context,method:"data.submit",changeDigest:SHA256.hash(data:bytes).map{String(format:"%02x",$0)}.joined(),
                            documentEdit:documentEdit,documentCreation:documentCreation,viewArchive:viewArchive,documentWorkLink:documentWorkLink,composition:composition)
        let record = try journal().prepare(method: "data.submit.observation", params: Self.encode(intent), crew: o.crew)
        records[id] = record; pending.append(intent)
        return (intent, record)
    }
    private func checkObservation(_ response: CrewdDataChangeResult, intent: Intent) throws {
        let sameScope = response.context.dbId == intent.context.dbId && response.context.authorityId == intent.context.authorityId && response.context.crewId == intent.context.crewId && response.context.namespace == intent.context.namespace
        guard UUID(uuidString:response.requestId) == intent.id, sameScope,
              intent.method == "data.restore" || response.context.restoreEpoch == intent.context.restoreEpoch else { throw Failure.invalid }
        if response.phase == .committed {
            guard response.beforeGeneration == intent.context.generation, response.afterGeneration == response.context.generation else { throw Failure.invalid }
            if let edit = intent.documentEdit, !edit.matches(response) { throw Failure.invalid }
            if let archive = intent.viewArchive, !response.created.isEmpty || response.changed != [archive.view] { throw Failure.invalid }
            if let link = intent.documentWorkLink, link.version(in:response) == nil { throw Failure.invalid }
            if let composition = intent.composition, composition.placementIDs(in:response) == nil { throw Failure.invalid }
        }
    }
    private func verifyDocumentWorkLinkReadback(_ response: CrewdDataChangeResult, intent: Intent, context: CrewdDataContext, origin o: Origin) async throws {
        guard let witness = intent.documentWorkLink, response.phase == .committed else { return }
        try checkObservation(response,intent:intent)
        guard origin() == o, sameAuthority(intent.context,context),
              let expected = witness.version(in:response) else { throw Failure.stale }
        let pinned = try await readDocumentWorkLinkVersion(expected.reference,revision:expected.revision,context:context,origin:o)
        guard pinned.revision == expected.revision, pinned.document == witness.document,
              pinned.archived == witness.archived else { throw Failure.invalid }
        // A historical projection deliberately redacts Work. Endpoints of the
        // typed relation are immutable, so verify a required W/# on the live ref.
        if let work = witness.work, !witness.archived {
            let live = try await readDocumentWorkLinkVersion(expected.reference,revision:nil,context:context,origin:o)
            guard live.work == work else { throw Failure.invalid }
        }
    }
    private func readDocumentWorkLinkVersion(_ reference: CrewdDataRef, revision: String?, context: CrewdDataContext, origin o: Origin) async throws -> DocumentWorkLink {
        let params = CrewdDataObjectGetParams(context:context,reference:reference,atRevision:revision)
        let detail: CrewdDataObjectDetail = try await call("data.object.get",params,preservingPresentation:true)
        guard origin() == o, detail.context == context, detail.resolvedContext == context,
              detail.generation == context.generation,
              let link = Self.documentWorkLink(detail.object,namespace:context.namespace), link.reference == reference else { throw Failure.invalid }
        return link
    }
    private func verifySelectedPlacementReadback(_ response: CrewdDataChangeResult, intent: Intent, context: CrewdDataContext, origin o: Origin) async throws {
        guard let edit = intent.documentEdit, let witness = edit.selected, response.phase == .committed else { return }
        try checkObservation(response,intent:intent)
        guard origin() == o, sameAuthority(intent.context,context) else { throw Failure.stale }
        let view: CrewdDataObjectDetail = try await call("data.object.get",
            CrewdDataObjectGetParams(context:context,reference:edit.view.reference,atRevision:edit.view.revision),preservingPresentation:true)
        guard origin() == o, view.context == context, view.resolvedContext == context, view.generation == context.generation,
              case .view(let id,let revision,let title,.document(let positions,let layout),false,let viewProvenance) = view.object,
              id == edit.view.reference.id, revision == edit.view.revision, positions.map(\.id) == witness.positions.map(\.id),
              try Self.compositionDigest(title) == witness.titleDigest,
              try Self.compositionDigest(layout) == witness.layoutDigest,
              try Self.compositionDigest(viewProvenance.attributes) == witness.viewAttributesDigest else { throw Failure.invalid }

        let captures = Dictionary(grouping:view.references,by:\.anchor)
        for (position, original) in zip(positions,witness.positions) {
            guard try Self.compositionDigest(position.layout) == original.layoutDigest else { throw Failure.invalid }
            if position.id == witness.selectedID, let source = edit.replacedBody {
                guard let sourceRevision = witness.sourceRevision, let digest = witness.bodyDigest,
                      let attributesDigest = witness.bodyAttributesDigest,
                      case .object(let body)? = response.created["replacementBody"],
                      position.block == .visible(reference:body.reference), position.atRevision == body.revision,
                      let links = captures["placement:" + position.id], links.count == 1, let link = links.first,
                      link.complete, !link.live, link.revision == body.revision, link.target == position.block else { throw Failure.invalid }
                let detail: CrewdDataObjectDetail = try await call("data.object.get",
                    CrewdDataObjectGetParams(context:context,reference:body.reference,atRevision:body.revision),preservingPresentation:true)
                guard origin() == o, detail.context == context, detail.resolvedContext == context, detail.generation == context.generation,
                      case .block(let blockID,let revision,"text/plain",.string(let text),false,let provenance) = detail.object,
                      blockID == body.reference.id, revision == body.revision, try Self.compositionDigest(text) == digest,
                      try Self.compositionDigest(provenance.attributes) == attributesDigest,
                      provenance.sources.count == 1, let recorded = provenance.sources.first,
                      recorded.reference == .visible(reference:source), recorded.atRevision == sourceRevision else { throw Failure.invalid }
                let sourceLinks = detail.references.filter { $0.anchor == "source:0" }
                guard sourceLinks.count == 1, let link = sourceLinks.first,
                      link.complete, !link.live, link.target == .visible(reference:source), link.revision == sourceRevision else { throw Failure.invalid }
            } else if let reference = original.visibleBlock {
                guard position.block == .visible(reference:reference),
                      original.pinnedRevision == nil || position.atRevision == original.pinnedRevision else { throw Failure.invalid }
            }
        }
    }
    private func nativeDocumentCreationIDs(_ response: CrewdDataChangeResult, context: CrewdDataContext) throws -> (view:CrewdDataObjectVersion,body:CrewdDataObjectVersion,placement:String) {
        func isNew(_ version: CrewdDataObjectVersion, kind:String) -> Bool {
            version.reference.kind == kind && version.reference.namespace == context.namespace && version.revision == "1" &&
            UUID(uuidString:version.reference.id)?.uuidString.lowercased() == version.reference.id
        }
        guard sameAuthority(response.context,context),
              response.created.count == 3, case .object(let view)? = response.created["document"], isNew(view,kind:"view"),
              case .object(let body)? = response.created["body"], isNew(body,kind:"block"),
              case .placement(let placementID,let parent)? = response.created["bodyPlacement"], parent == view,
              UUID(uuidString:placementID)?.uuidString.lowercased() == placementID,
              Set([view.reference.id,body.reference.id,placementID]).count == 3,
              response.changed.count == 2, response.changed.contains(view), response.changed.contains(body) else { throw Failure.invalid }
        return (view,body,placementID)
    }

    private func verifyNativeDocumentCreation(_ response: CrewdDataChangeResult, witness: NativeDocumentCreationWitness, context: CrewdDataContext, origin o: Origin) async throws {
        guard response.phase == .committed else { return }
        guard origin() == o else { throw Failure.stale }
        let (view,body,placementID) = try nativeDocumentCreationIDs(response,context:context)
        let viewDetail: CrewdDataObjectDetail = try await call("data.object.get",
            CrewdDataObjectGetParams(context:context,reference:view.reference,atRevision:view.revision),preservingPresentation:true)
        guard origin() == o, viewDetail.context == context, viewDetail.resolvedContext == context, viewDetail.generation == context.generation,
              case .view(let viewID,let viewRevision,let title,.document(let positions,let layout),false,let provenance) = viewDetail.object,
              viewID == view.reference.id, viewRevision == view.revision, try Self.compositionDigest(title) == witness.titleDigest,
              provenance.createdBy["kind"] == .string("human"), layout == .object([:]), positions.count == 1,
              let position = positions.first, position.id == placementID, position.layout == .object([:]),
              position.block == .visible(reference:body.reference), position.atRevision == body.revision,
              viewDetail.references.count == 1, let capture = viewDetail.references.first,
              capture.anchor == "placement:" + placementID, capture.target == position.block,
              capture.revision == body.revision, capture.complete, !capture.live else { throw Failure.invalid }
        let bodyDetail: CrewdDataObjectDetail = try await call("data.object.get",
            CrewdDataObjectGetParams(context:context,reference:body.reference,atRevision:body.revision),preservingPresentation:true)
        guard origin() == o, bodyDetail.context == context, bodyDetail.resolvedContext == context, bodyDetail.generation == context.generation,
              case .block(let bodyID,let revision,"text/plain",.string(let text),false,let provenance) = bodyDetail.object,
              bodyID == body.reference.id, revision == body.revision, try Self.compositionDigest(text) == witness.bodyDigest,
              provenance.createdBy["kind"] == .string("human"), provenance.sources.isEmpty else { throw Failure.invalid }
    }
    private func verifyArchiveReadback(_ response: CrewdDataChangeResult, intent: Intent, context: CrewdDataContext, origin o: Origin) async throws {
        guard let archive = intent.viewArchive, response.phase == .committed else { return }
        try checkObservation(response,intent:intent)
        guard origin() == o, sameAuthority(intent.context,context) else { throw Failure.stale }
        let params = CrewdDataObjectGetParams(context:context,reference:archive.view.reference,atRevision:archive.view.revision)
        let detail: CrewdDataObjectDetail = try await call("data.object.get",params,preservingPresentation:true)
        guard origin() == o, detail.context == context, detail.resolvedContext == context, detail.generation == context.generation,
              case .view(let id,let revision,_,_,let archived,_) = detail.object,
              id == archive.view.reference.id, revision == archive.view.revision, archived == archive.archived else { throw Failure.invalid }
    }
    private func observe(_ response: CrewdDataChangeResult, intent: Intent, record: CrewdPresentationJournal.Prepared, ledger: CrewdPresentationJournal) throws {
        try checkObservation(response,intent:intent)
        // Unknown/historical stays durable. A lookup can never replay the mutation.
        switch response.phase {
        case .committed, .rejected, .conflicted:
            try ledger.finish(record, state: response.phase == .committed ? "replyObserved" : "refused")
        case .unknown:
            if let resolved = response.resolvedByRequest, UUID(uuidString: resolved) != nil {
                try ledger.finish(record, state: "replyObserved", detail: "Original Unknown acknowledged; no replay or execution claim")
            }
        case .historical: break
        }
    }
    private func applyObservation(_ response: CrewdDataChangeResult, intent: Intent) {
        result = response
        if [.committed, .rejected, .conflicted].contains(response.phase) || (response.phase == .unknown && response.resolvedByRequest.flatMap(UUID.init(uuidString:)) != nil) { pending.removeAll{$0.id == intent.id}; records.removeValue(forKey:intent.id) }
        renderer.revoke()
        status = response.phase == .unknown ? (response.resolvedByRequest == nil ? "写入结果未知；请仅查询原请求，或在原生版本控制面明确处理。" : "原执行结果仍未知，已有明确人工处置记录；未重放。") : "已观察到后台结果；请重新读取当前 数据代次。"
    }
    /// Only native controls call this with an explicitly confirmed change. The JS path below allows Apply only.
    @discardableResult func submitFromNativeHost(_ change: CrewdDataChange, requestID:UUID?=nil) async -> CrewdDataChangeResult? {
        try? await submitNativeChange(change, requestID: requestID)
    }
    private func submitNativeChange(_ change: CrewdDataChange, requestID: UUID? = nil, documentEdit: NativeDocumentEditReceipt? = nil, documentCreation: NativeDocumentCreationWitness? = nil, viewArchive: NativeViewArchiveReceipt? = nil, documentWorkLink: DocumentWorkLinkReceipt? = nil, composition: DocumentCompositionReceipt? = nil) async throws -> CrewdDataChangeResult? {
        let (o, token) = try begin()
        defer { finish(o, token) }
        guard !hasBlockingPending else { throw Failure.unresolved }
        do {
            guard let info = workspace else { throw Failure.unavailable }
            let ledger = try journal()
            let (intent, record) = try stage(change,id:requestID ?? UUID(),context:info.context,documentEdit:documentEdit,documentCreation:documentCreation,viewArchive:viewArchive,documentWorkLink:documentWorkLink,composition:composition)
            let response: CrewdDataChangeResult
            do {
                response = try await call("data.submit", CrewdDataSubmitParams(requestId:intent.id.uuidString.lowercased(), context:intent.context, change:change))
            } catch let error as CrewdClientError where (documentEdit != nil || documentCreation != nil || viewArchive != nil || documentWorkLink != nil || composition != nil) &&
                (Self.definiteNativeRefusal(error) || (documentWorkLink != nil && error.code == -32009 && error.refusal == "data.document_work_link_refused")) {
                try ledger.finish(record, state:"refused")
                if current(o, token) { pending.removeAll { $0.id == intent.id }; records.removeValue(forKey: intent.id) }
                throw NativeSubmissionRefusal(original:error)
            }
            if let creation = intent.documentCreation {
                try checkObservation(response,intent:intent)
                try await verifyNativeDocumentCreation(response,witness:creation,context:response.context,origin:o)
                guard origin() == o else { return nil }
            }
            if intent.documentEdit?.selected != nil {
                try await verifySelectedPlacementReadback(response,intent:intent,context:response.context,origin:o)
                // The server can broadcast our data.changed before replying.
                // It revokes presentation, while the exact origin and journal still own this receipt.
                guard origin() == o else { return nil }
            }
            if intent.viewArchive != nil { try await verifyArchiveReadback(response,intent:intent,context:response.context,origin:o) }
            if intent.documentWorkLink != nil { try await verifyDocumentWorkLinkReadback(response,intent:intent,context:response.context,origin:o) }
            if intent.composition != nil { try await verifyCompositionReadback(response,intent:intent,context:response.context,origin:o) }
            try observe(response, intent:intent, record:record, ledger:ledger)
            guard current(o, token) else { return origin() == o ? response:nil }
            applyObservation(response, intent:intent)
            return response
        } catch let refusal as NativeSubmissionRefusal {
            if current(o,token) { status = "后台已拒绝此次更改，请重新读取当前资料。" }
            throw refusal.original
        } catch { if current(o,token) { renderer.revoke(); status = "未获得完整提交结果；保留原 UUID，不自动重试。" } }
        return nil
    }
    private static func definiteNativeRefusal(_ error: CrewdClientError) -> Bool {
        if case .notSubmitted = error.kind { return true }
        // An untyped conflict can also report a storage fault at commit time.
        return error.code == -32602 || error.code == -32003 ||
            (error.code == -32009 && error.refusal == "data.context_conflict")
    }
    private static func isWorkLinkReference(_ reference: CrewdDataRef, namespace: String) -> Bool {
        guard reference.namespace == namespace else { return false }
        let prefix: Character
        switch reference.kind {
        case "plan": prefix = "W"
        case "requirement": prefix = "#"
        default: return false
        }
        return reference.id.first == prefix && positiveRevision(String(reference.id.dropFirst())) != nil
    }
    private static func documentWorkLink(_ object: CrewdDataObject, namespace: String) -> DocumentWorkLink? {
        guard case .relation(let id,let revision,let subject,let predicate,let target,let attributes,let archived,_) = object,
              predicate == documentWorkPredicate, attributes == .object([:]),
              UUID(uuidString:id)?.uuidString.lowercased() == id, positiveRevision(revision) != nil else { return nil }
        let document: CrewdDataRef?
        switch subject {
        case .unavailable: document = nil
        case .visible(let ref):
            guard ref.namespace == namespace, ref.kind == "view", UUID(uuidString:ref.id)?.uuidString.lowercased() == ref.id else { return nil }
            document = ref
        }
        let work: CrewdDataRef?
        switch target {
        case .unavailable: work = nil
        case .visible(let ref):
            guard isWorkLinkReference(ref,namespace:namespace) else { return nil }
            work = ref
        }
        return .init(reference:.init(namespace:namespace,kind:"relation",id:id),revision:revision,document:document,work:work,archived:archived)
    }
    func documentWorkLinkAnchor(document parent: CrewdDataObjectDetail) throws -> DocumentWorkLinkAnchor {
        guard let o = active, origin() == o, let context = workspace?.context, workspace?.state == "ready",
              detail == parent, !detailIsSnapshot, parent.context == context, parent.resolvedContext == context,
              parent.generation == context.generation, case .view(let id,_,_,.document,false,_) = parent.object,
              let reference = detailReference, reference == .init(namespace:context.namespace,kind:"view",id:id) else { throw Failure.stale }
        return .init(endpoint:reference,context:context,origin:o,document:parent)
    }
    func documentWorkLinkAnchor(work: CrewdDataRef) throws -> DocumentWorkLinkAnchor {
        guard let o = active, origin() == o, let context = workspace?.context, workspace?.state == "ready",
              Self.isWorkLinkReference(work,namespace:context.namespace) else { throw Failure.stale }
        return .init(endpoint:work,context:context,origin:o,document:nil)
    }
    func documentWorkLinkScopeIsCurrent(_ anchor: DocumentWorkLinkAnchor) -> Bool {
        guard active == anchor.origin, origin() == anchor.origin,
              workspace?.state == "ready", workspace?.context == anchor.context else { return false }
        if let document = anchor.document { return (try? documentWorkLinkAnchor(document:document)) == anchor }
        return true
    }
    func reloadDocumentWorkLinkAnchor(_ anchor: DocumentWorkLinkAnchor) async throws -> DocumentWorkLinkAnchor {
        guard origin() == anchor.origin else { throw Failure.stale }
        await refresh(loadViews:false)
        guard origin() == anchor.origin, let context = workspace?.context, sameAuthority(anchor.context,context) else { throw Failure.stale }
        if anchor.document == nil { return try documentWorkLinkAnchor(work:anchor.endpoint) }
        let (o,token) = try begin()
        defer { finish(o,token) }
        let fresh = try await readActiveNativeDocument(anchor.endpoint,context:context,origin:o)
        guard current(o,token), !Task.isCancelled else { throw Failure.stale }
        showActiveNativeDocument(fresh,reference:anchor.endpoint)
        return try documentWorkLinkAnchor(document:fresh)
    }
    func loadDocumentWorkLinkPage(_ anchor: DocumentWorkLinkAnchor, after: String? = nil) async throws -> DocumentWorkLinkPage {
        guard documentWorkLinkScopeIsCurrent(anchor),
              after == nil || after.flatMap({UUID(uuidString:$0)?.uuidString.lowercased()}) == after else { throw Failure.stale }
        let (o, token) = try begin()
        defer { finish(o,token) }
        let params = CrewdDataObjectListParams(context:anchor.context,objectType:"relation",after:after,limit:12,atRevision:nil,includeArchived:true)
        let page: CrewdDataObjectPage = try await call("data.object.list",params,preservingPresentation:true)
        guard current(o,token), documentWorkLinkScopeIsCurrent(anchor), !Task.isCancelled,
              page.context == anchor.context, page.generation == anchor.context.generation,
              page.items.count <= Int(params.limit), after == nil || !page.items.isEmpty else { throw Failure.stale }
        var previous = after ?? ""
        for object in page.items {
            guard case .relation(let id,_,_,_,_,_,_,_) = object, id > previous,
                  UUID(uuidString:id)?.uuidString.lowercased() == id else { throw Failure.invalid }
            previous = id
        }
        if let next = page.next {
            guard page.items.count == Int(params.limit), next == previous else { throw Failure.invalid }
        }
        let items = page.items.compactMap { Self.documentWorkLink($0,namespace:anchor.context.namespace) }.filter {
            anchor.document == nil ? $0.work == anchor.endpoint : $0.document == anchor.endpoint
        }
        return .init(items:items,next:page.next)
    }
    func createDocumentWorkLink(_ anchor: DocumentWorkLinkAnchor, work: CrewdDataRef) async throws -> CrewdDataChangeResult? {
        guard anchor.document != nil, documentWorkLinkScopeIsCurrent(anchor),
              Self.isWorkLinkReference(work,namespace:anchor.context.namespace) else { throw Failure.stale }
        let edit = CrewdDataEdit.createRelation(alias:"documentWorkLink",subject:.existing(reference:anchor.endpoint),
            predicate:Self.documentWorkPredicate,object:.existing(reference:work),attributes:.object([:]))
        return try await submitNativeChange(.apply(message:"关联原文档和待办",edits:[edit]),
            documentWorkLink:.init(document:anchor.endpoint,work:work,archived:false,next:nil))
    }
    func setDocumentWorkLinkArchived(_ anchor: DocumentWorkLinkAnchor, link: DocumentWorkLink, archived: Bool) async throws -> CrewdDataChangeResult? {
        guard anchor.document != nil, documentWorkLinkScopeIsCurrent(anchor), link.document == anchor.endpoint,
              link.reference.namespace == anchor.context.namespace, link.archived != archived,
              archived || link.work != nil, let revision = Self.positiveRevision(link.revision), revision < Int64.max else { throw Failure.stale }
        let next = CrewdDataObjectVersion(reference:link.reference,revision:String(revision + 1))
        let edit = CrewdDataEdit.archiveRelation(id:link.id,expectedRevision:link.revision,archived:archived)
        return try await submitNativeChange(.apply(message:archived ? "归档资料待办关联":"恢复资料待办关联",edits:[edit]),
            documentWorkLink:.init(document:anchor.endpoint,work:link.work,archived:archived,next:next))
    }
    func pendingDocumentWorkLinks(_ anchor: DocumentWorkLinkAnchor) -> [Intent] {
        guard origin() == anchor.origin else { return [] }
        return pending.filter { intent in
            guard sameAuthority(intent.context,anchor.context),let relation = intent.documentWorkLink else { return false }
            return anchor.document == nil ? relation.work == anchor.endpoint : relation.document == anchor.endpoint
        }
    }
    private func readActiveNativeDocument(_ reference: CrewdDataRef, context: CrewdDataContext, origin o: Origin) async throws -> CrewdDataObjectDetail {
        guard reference.kind == "view", reference.namespace == context.namespace else { throw Failure.stale }
        let params = CrewdDataObjectGetParams(context:context,reference:reference,atRevision:nil)
        let view: CrewdDataObjectDetail = try await call("data.object.get",params,preservingPresentation:true)
        guard origin() == o, view.context == context, view.resolvedContext == context, view.generation == context.generation,
              case .view(let id,_,_,.document,false,_) = view.object, id == reference.id else { throw Failure.unavailable }
        return view
    }
    private func showActiveNativeDocument(_ value: CrewdDataObjectDetail, reference: CrewdDataRef) {
        renderer.revoke(); detail = value; detailReference = reference; detailIsSnapshot = false
    }
    func openDocumentWorkLinkDocument(_ anchor: DocumentWorkLinkAnchor, link: DocumentWorkLink) async throws {
        guard anchor.document == nil, documentWorkLinkScopeIsCurrent(anchor), !link.archived,
              link.work == anchor.endpoint, let document = link.document else { throw Failure.stale }
        let (o,token) = try begin()
        defer { finish(o,token) }
        let fresh = try await readDocumentWorkLinkVersion(link.reference,revision:nil,context:anchor.context,origin:o)
        guard fresh == link, current(o,token), !Task.isCancelled else { throw Failure.stale }
        let value = try await readActiveNativeDocument(document,context:anchor.context,origin:o)
        guard current(o,token), documentWorkLinkScopeIsCurrent(anchor), !Task.isCancelled else { throw Failure.stale }
        showActiveNativeDocument(value,reference:document)
    }
    func prepareNativeViewArchive(_ parent: CrewdDataObjectDetail) throws -> NativeViewArchiveTarget {
        guard let o = active, origin() == o, let context = workspace?.context, workspace?.state == "ready",
              !detailIsSnapshot, detail == parent, parent.context == context, parent.resolvedContext == context,
              parent.generation == context.generation,
              case .view(let id,let revision,let title,_,let archived,_) = parent.object,
              let number = Self.positiveRevision(revision), number < Int64.max,
              let reference = detailReference, reference == .init(namespace:context.namespace,kind:"view",id:id) else { throw Failure.stale }
        return .init(title:title,archived:archived,origin:o,parent:parent,reference:reference)
    }
    func submitNativeViewArchive(_ target: NativeViewArchiveTarget) async throws -> CrewdDataChangeResult? {
        guard !busy, try prepareNativeViewArchive(target.parent) == target,
              let revision = Self.positiveRevision(target.parent.object.revision), revision < Int64.max else { throw Failure.stale }
        let next = CrewdDataObjectVersion(reference:target.reference,revision:String(revision + 1))
        let archived = !target.archived
        let change = CrewdDataChange.apply(message:archived ? "归档资料" : "从回收站恢复资料",
            edits:[.archiveView(id:target.reference.id,expectedRevision:target.parent.object.revision,archived:archived)])
        return try await submitNativeChange(change,viewArchive:.init(view:next,archived:archived))
    }
    private func nativeDocumentPlacements(_ parent: CrewdDataObjectDetail) -> [CrewdDataPlacement]? {
        guard let context = workspace?.context, workspace?.state == "ready", let active, origin() == active,
              !detailIsSnapshot, detail == parent, parent.context == context, parent.resolvedContext == context,
              parent.generation == context.generation,
              detailReference == .init(namespace:context.namespace,kind:"view",id:parent.object.identity),
              case .view(_,_,_,.document(let placements,_),false,let provenance) = parent.object,
              provenance.createdBy["kind"].string == "human" else { return nil }
        return placements
    }
    private func nativeDocumentCapture(_ placement: CrewdDataPlacement, namespace: String, links: [CrewdDataResolvedRef]) -> CrewdDataResolvedRef? {
        guard case .visible(let block) = placement.block, block.namespace == namespace, block.kind == "block" else { return nil }
        guard links.count == 1, let link = links.first, link.target == placement.block, link.complete,
              let revision = link.revision, Self.positiveRevision(revision) != nil,
              link.live == (placement.atRevision == nil),
              placement.atRevision == nil || placement.atRevision == revision else { return nil }
        return link
    }
    private func nativeDocumentCapture(_ placement: CrewdDataPlacement, parent: CrewdDataObjectDetail) -> CrewdDataResolvedRef? {
        nativeDocumentCapture(placement,namespace:parent.context.namespace,links:parent.references.filter { $0.anchor == "placement:" + placement.id })
    }
    func editableDocumentPlacements(_ parent: CrewdDataObjectDetail) -> [EditableDocumentPlacement] {
        guard let placements = nativeDocumentPlacements(parent), Set(placements.map(\.id)).count == placements.count else { return [] }
        let captures = Dictionary(grouping:parent.references,by:\.anchor)
        return placements.enumerated().compactMap { index, placement in
            let links = captures["placement:" + placement.id] ?? []
            guard nativeDocumentCapture(placement,namespace:parent.context.namespace,links:links) != nil else { return nil }
            return .init(id:placement.id,position:index + 1,pinnedRevision:placement.atRevision)
        }
    }
    // Whole-body history/restore remains limited to the original simple document.
    func canPrepareNativeDocumentEdit(_ parent: CrewdDataObjectDetail) -> Bool {
        guard let placements = nativeDocumentPlacements(parent), placements.count == 1,
              let placement = placements.first, placement.atRevision == nil else { return false }
        return nativeDocumentCapture(placement,parent:parent) != nil
    }
    func prepareNativeDocumentEdit(_ parent: CrewdDataObjectDetail, placementID: String? = nil) async throws -> NativeDocumentEdit {
        let selected = placementID.flatMap { id in editableDocumentPlacements(parent).first { $0.id == id } }
        guard placementID == nil ? canPrepareNativeDocumentEdit(parent) : selected != nil,
              let active, let view = detailReference,
              case .view(_,_,let title,.document(let placements,let layout),_,let provenance) = parent.object,
              let placement = (selected == nil ? placements.first : placements.first { $0.id == selected?.id }),
              case .visible(let bodyRef) = placement.block,
              let capture = nativeDocumentCapture(placement,parent:parent) else { throw Failure.unsupported }
        let anchor = "placement:" + placement.id
        guard let body = try await resolved(placement.block,anchor:anchor,atRevision:placement.atRevision,parent:parent),
              (placementID == nil ? canPrepareNativeDocumentEdit(parent) : editableDocumentPlacements(parent).contains { $0.id == placement.id }),
              body.context == parent.context, body.resolvedContext == parent.context,
              body.generation == parent.generation,
              case .block(let id,let revision,"text/plain",.string(let text),false,let bodyProvenance) = body.object,
              id == bodyRef.id, revision == capture.revision, bodyProvenance.createdBy["kind"].string == "human" else { throw Failure.unsupported }
        return .init(title:title,body:text,selectedPlacement:selected,origin:active,parent:parent,view:view,placement:placement,layout:layout,
                     viewAttributes:provenance.attributes,bodyRef:bodyRef,bodyRevision:revision,bodyAttributes:bodyProvenance.attributes)
    }
    func reloadNativeDocumentEdit(_ edit: NativeDocumentEdit) async throws -> NativeDocumentEdit {
        guard origin() == edit.origin else { throw Failure.stale }
        await refresh()
        guard origin() == edit.origin, let context = workspace?.context,
              sameAuthority(edit.parent.context,context) else { throw Failure.stale }
        await open(edit.view)
        guard let detail else { throw Failure.unavailable }
        let current = try await prepareNativeDocumentEdit(detail,placementID:edit.selectedPlacement?.id)
        if edit.selectedPlacement != nil {
            guard current.bodyRef == edit.bodyRef, current.placement.atRevision == edit.placement.atRevision else { throw Failure.stale }
        }
        return current
    }
    static let documentCompositionLimit = 128

    func canPrepareNativeDocumentComposition(_ parent: CrewdDataObjectDetail) -> Bool {
        guard let context = workspace?.context, workspace?.state == "ready", let active, origin() == active,
              !detailIsSnapshot, detail == parent, parent.context == context, parent.resolvedContext == context,
              parent.generation == context.generation,
              detailReference == .init(namespace:context.namespace,kind:"view",id:parent.object.identity),
              case .view(_,let revision,_,.document(let placements,_),false,let provenance) = parent.object,
              provenance.createdBy["kind"].string == "human", Self.positiveRevision(revision) != nil,
              placements.count <= Self.documentCompositionLimit else { return false }
        return true
    }

    func nativeDocumentComposition(_ parent: CrewdDataObjectDetail) throws -> DocumentComposition {
        guard canPrepareNativeDocumentComposition(parent), let active, let reference = detailReference,
              case .view(_,_,_,.document(let placements,_),_,_) = parent.object else { throw Failure.unsupported }
        return .init(target:.init(origin:active,parent:parent,reference:reference),placements:placements)
    }

    private func compositionBlock(_ object: CrewdDataObject, in context: CrewdDataContext) throws -> DocumentCompositionBlock {
        guard case .block(let id,let revision,let type,let content,false,_) = object,
              UUID(uuidString:id)?.uuidString.lowercased() == id, Self.positiveRevision(revision) != nil else { throw Failure.invalid }
        let summary = content.string.map { String(decoding:$0.utf8.prefix(480),as:UTF8.self) } ?? type
        return .init(reference:.init(namespace:context.namespace,kind:"block",id:id),
                     revision:revision,contentType:type,preview:summary)
    }

    func loadDocumentCompositionBlocks(_ composition: DocumentComposition, after: String? = nil) async throws -> DocumentCompositionBlockPage {
        try await readNativeHistory(composition.target) {
            let context = composition.target.parent.context
            let limit: UInt32 = 8
            let params = CrewdDataObjectListParams(context:context,objectType:"block",after:after,
                                                  limit:limit,atRevision:nil,includeArchived:false)
            let page: CrewdDataObjectPage = try await call("data.object.list",params,preservingPresentation:true)
            guard page.context == context, page.generation == context.generation,
                  page.items.count <= Int(limit), after == nil || !page.items.isEmpty else { throw Failure.invalid }
            var previous = after ?? ""
            let items = try page.items.map { object in
                let item = try compositionBlock(object,in:context)
                guard item.id > previous else { throw Failure.invalid }
                previous = item.id
                return item
            }
            if let next = page.next {
                guard items.count == Int(limit), next == items.last?.id else { throw Failure.invalid }
            }
            return .init(items:items,next:page.next)
        }
    }

    func inspectDocumentCompositionRevision(_ composition: DocumentComposition, revision: NativeDocumentRevision) async throws -> DocumentCompositionRevision {
        let target = composition.target
        return try await readNativeHistory(target) {
            let context = target.parent.context
            guard let requested = Self.positiveRevision(revision.revision),
                  let head = Self.positiveRevision(target.head), requested <= head else { throw Failure.invalid }
            let historical: CrewdDataObjectDetail = try await call("data.object.get",
                CrewdDataObjectGetParams(context:context,reference:target.reference,atRevision:revision.revision),preservingPresentation:true)
            let observed = try nativeRevision(historical.object,target:target)
            guard historical.context == context, historical.resolvedContext == context,
                  historical.generation == context.generation, observed == revision,
                  case .view(_,_,_,.document(let placements,_),_,_) = historical.object else { throw Failure.invalid }
            let captures = Dictionary(grouping:historical.references,by:\.anchor)
            let unique = Set(placements.map(\.id)).count == placements.count
            let restorable = unique && placements.count <= Self.documentCompositionLimit && placements.allSatisfy { position in
                guard case .visible(let block) = position.block, block.kind == "block", block.namespace == context.namespace,
                      let pinned = position.atRevision, Self.positiveRevision(pinned) != nil,
                      let links = captures["placement:" + position.id], links.count == 1, let link = links.first else { return false }
                return link.complete && !link.live && link.target == position.block && link.revision == pinned
            }
            return .init(summary:observed,placements:placements,target:target,restorable:restorable)
        }
    }

    enum DocumentCompositionPosition: Equatable {
        case existing(String)
        case pinned(DocumentCompositionBlock)
    }

    func saveDocumentComposition(_ composition: DocumentComposition, positions: [DocumentCompositionPosition]) async throws -> CrewdDataChangeResult? {
        let target = composition.target
        let existing = Dictionary(grouping:composition.placements,by:\.id)
        let selectedBlocks = positions.compactMap { position -> DocumentCompositionBlock? in
            if case .pinned(let block) = position { return block }
            return nil
        }
        guard positions.count <= Self.documentCompositionLimit,
              positions != composition.placements.map({ .existing($0.id) }) else { throw Failure.invalid }
        if !selectedBlocks.isEmpty {
            try await readNativeHistory(target) {
                let context = target.parent.context
                var checked: Set<String> = []
                for block in selectedBlocks where checked.insert(block.id).inserted {
                    guard block.reference.namespace == context.namespace, block.reference.kind == "block" else { throw Failure.invalid }
                    let value: CrewdDataObjectDetail = try await call("data.object.get",
                        CrewdDataObjectGetParams(context:context,reference:block.reference,atRevision:nil),preservingPresentation:true)
                    guard value.context == context, value.resolvedContext == context, value.generation == context.generation,
                          try compositionBlock(value.object,in:context) == block else { throw Failure.stale }
                }
            }
        }
        let desired = try positions.map { position -> (String?, CrewdDataPlacement, Bool) in
            switch position {
            case .existing(let id):
                guard let matches = existing[id], matches.count == 1, let item = matches.first else { throw Failure.invalid }
                return (id,item,true)
            case .pinned(let block):
                guard selectedBlocks.first(where:{$0.id == block.id}) == block else { throw Failure.invalid }
                return (nil,.init(id:"",block:.visible(reference:block.reference),atRevision:block.revision,layout:.object([:])),false)
            }
        }
        return try await submitDocumentComposition(target,desired:desired,restoring:false)
    }

    func restoreDocumentComposition(_ revision: DocumentCompositionRevision, replacing composition: DocumentComposition) async throws -> CrewdDataChangeResult? {
        guard revision.target == composition.target, revision.canRestore else { throw Failure.invalid }
        let current = Set(composition.placements.map(\.id))
        let desired = revision.placements.map { position in
            (current.contains(position.id) ? position.id : nil,position,false)
        }
        return try await submitDocumentComposition(composition.target,desired:desired,restoring:true)
    }

    nonisolated private static func compositionDigest<T: Encodable>(_ value: T) throws -> String {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        return SHA256.hash(data:try encoder.encode(value)).map { String(format:"%02x",$0) }.joined()
    }

    private func submitDocumentComposition(_ target: NativeDocumentHistoryTarget,
            desired: [(String?, CrewdDataPlacement, Bool)], restoring: Bool) async throws -> CrewdDataChangeResult? {
        try checkNativeHistoryTarget(target)
        guard !busy, let current = Self.positiveRevision(target.head), current < Int64.max,
              case .view(_,_,let title,.document(_,let layout),false,let provenance) = target.parent.object else { throw Failure.stale }
        var edits: [CrewdDataPlacementEdit] = []
        var expected: [DocumentCompositionReceipt.Placement] = []
        var seen: Set<String> = []
        for (index, entry) in desired.enumerated() {
            let (id, position, keep) = entry
            if let id, !seen.insert(id).inserted { throw Failure.invalid }
            guard case .visible(let block) = position.block, block.namespace == target.parent.context.namespace,
                  block.kind == "block" else { throw Failure.unsupported }
            let alias = id == nil ? "compositionPosition\(index)" : nil
            if keep, let id { edits.append(.keep(id:id,layout:position.layout)) }
            else {
                guard let pinned = position.atRevision, Self.positiveRevision(pinned) != nil else { throw Failure.invalid }
                edits.append(.set(id:id,alias:alias,block:.existing(reference:block),atRevision:pinned,layout:position.layout))
            }
            expected.append(.init(id:id,alias:alias,block:block,revision:position.atRevision,
                                  layoutDigest:try Self.compositionDigest(position.layout)))
        }
        let next = CrewdDataObjectVersion(reference:target.reference,revision:String(current + 1))
        let witness = DocumentCompositionReceipt(view:next,titleDigest:try Self.compositionDigest(title),
            layoutDigest:try Self.compositionDigest(layout),attributesDigest:try Self.compositionDigest(provenance.attributes),placements:expected)
        let change = CrewdDataChange.apply(message:restoring ? "恢复文档编排" : "编辑文档编排",
            edits:[.updateView(id:target.reference.id,expectedRevision:target.head,title:title,
                definition:.document(placements:edits,layout:layout),sources:nil,attributes:provenance.attributes)])
        return try await submitNativeChange(change,composition:witness)
    }

    func pendingDocumentCompositions(viewID: String? = nil, crew: String? = nil) -> [Intent] {
        guard let active, origin() == active, crew == nil || crew == active.crew else { return [] }
        return pending.filter { intent in
            guard let composition = intent.composition else { return false }
            if let context = workspace?.context, !sameAuthority(intent.context,context) { return false }
            return viewID == nil || composition.view.reference.id == viewID
        }
    }

    private func verifyCompositionReadback(_ response: CrewdDataChangeResult, intent: Intent, context: CrewdDataContext, origin o: Origin) async throws {
        guard let witness = intent.composition, response.phase == .committed else { return }
        try checkObservation(response,intent:intent)
        guard origin() == o, sameAuthority(intent.context,context),
              let ids = witness.placementIDs(in:response) else { throw Failure.stale }
        let value: CrewdDataObjectDetail = try await call("data.object.get",
            CrewdDataObjectGetParams(context:context,reference:witness.view.reference,atRevision:witness.view.revision),preservingPresentation:true)
        guard origin() == o, value.context == context, value.resolvedContext == context, value.generation == context.generation,
              case .view(let id,let revision,let title,.document(let positions,let layout),false,let provenance) = value.object,
              id == witness.view.reference.id, revision == witness.view.revision, positions.map(\.id) == ids,
              try Self.compositionDigest(title) == witness.titleDigest,
              try Self.compositionDigest(layout) == witness.layoutDigest,
              try Self.compositionDigest(provenance.attributes) == witness.attributesDigest else { throw Failure.invalid }
        let captures = Dictionary(grouping:value.references,by:\.anchor)
        for (position, expected) in zip(positions,witness.placements) {
            guard position.block == .visible(reference:expected.block),
                  let revision = position.atRevision, Self.positiveRevision(revision) != nil,
                  expected.revision == nil || revision == expected.revision,
                  try Self.compositionDigest(position.layout) == expected.layoutDigest,
                  let links = captures["placement:" + position.id], links.count == 1, let link = links.first,
                  link.complete, !link.live, link.revision == revision, link.target == position.block else { throw Failure.invalid }
        }
    }

    func nativeDocumentHistoryTarget(_ parent: CrewdDataObjectDetail) throws -> NativeDocumentHistoryTarget {
        guard canPrepareNativeDocumentEdit(parent), let active, let reference = detailReference,
              Self.positiveRevision(parent.object.revision) != nil else { throw Failure.unsupported }
        return .init(origin:active,parent:parent,reference:reference)
    }
    private static func positiveRevision(_ raw: String) -> Int64? {
        guard let revision = Int64(raw), revision > 0, String(revision) == raw else { return nil }
        return revision
    }
    private func checkNativeHistoryTarget(_ target: NativeDocumentHistoryTarget) throws {
        guard active == target.origin, detailReference == target.reference,
              canPrepareNativeDocumentComposition(target.parent) else { throw Failure.stale }
    }
    private func readNativeHistory<T>(_ target: NativeDocumentHistoryTarget, _ read: () async throws -> T) async throws -> T {
        try checkNativeHistoryTarget(target)
        let (o, token) = try begin()
        defer { finish(o,token) }
        let value = try await read()
        guard current(o,token) else { throw Failure.stale }
        try checkNativeHistoryTarget(target)
        return value
    }
    private func nativeRevision(_ object: CrewdDataObject, target: NativeDocumentHistoryTarget) throws -> NativeDocumentRevision {
        guard case .view(let id,let revision,let title,_,let archived,let provenance) = object,
              id == target.reference.id, Self.positiveRevision(revision) != nil else { throw Failure.invalid }
        return .init(revision:revision,title:title,updatedAt:provenance.updatedAt,author:provenance.updatedBy["kind"].string,archived:archived)
    }
    func loadNativeDocumentRevisions(_ target: NativeDocumentHistoryTarget, after: String? = nil) async throws -> NativeDocumentRevisionPage {
        try await readNativeHistory(target) {
            guard let head = Self.positiveRevision(target.head) else { throw Failure.invalid }
            let start: Int64
            if let after {
                guard let cursor = Self.positiveRevision(after), cursor < head else { throw Failure.invalid }
                start = cursor
            } else { start = 0 }
            let params = CrewdDataHistoryParams(context:target.parent.context,reference:target.reference,after:after,limit:25)
            let page: CrewdDataHistory = try await call("data.history",params,preservingPresentation:true)
            guard page.context == params.context, !page.items.isEmpty, page.items.count <= Int(params.limit) else { throw Failure.invalid }
            let items = try page.items.map { try nativeRevision($0,target:target) }
            var last = start
            for item in items {
                guard last < head, Self.positiveRevision(item.revision) == last + 1 else { throw Failure.invalid }
                last += 1
            }
            guard page.next == (last < head ? String(last) : nil) else { throw Failure.invalid }
            return .init(items:items,next:page.next)
        }
    }
    func inspectNativeDocumentRevision(_ target: NativeDocumentHistoryTarget, revision: NativeDocumentRevision) async throws -> NativeDocumentRevisionSnapshot {
        try await readNativeHistory(target) {
            let context = target.parent.context
            guard let requested = Self.positiveRevision(revision.revision), let head = Self.positiveRevision(target.head),
                  requested <= head else { throw Failure.invalid }
            let params = CrewdDataObjectGetParams(context:context,reference:target.reference,atRevision:revision.revision)
            let historical: CrewdDataObjectDetail = try await call("data.object.get",params,preservingPresentation:true)
            try checkNativeHistoryTarget(target)
            let observed = try nativeRevision(historical.object,target:target)
            guard historical.context == context, historical.resolvedContext == context, historical.generation == context.generation,
                  observed == revision, observed.title.utf8.elementsEqual(revision.title.utf8),
                  case .view(_,_,_,.document(let placements,_),_,_) = historical.object,
                  placements.count == 1, let placement = placements.first,
                  case .visible(let source) = placement.block, source.kind == "block", source.namespace == context.namespace,
                  let pinned = placement.atRevision, Self.positiveRevision(pinned) != nil else { throw Failure.unsupported }
            let captures = historical.references.filter { $0.anchor == "placement:" + placement.id }
            guard captures.count == 1, let captured = captures.first, captured.complete, !captured.live,
                  captured.target == placement.block, captured.revision == pinned else { throw Failure.unavailable }
            let block: CrewdDataObjectDetail = try await call("data.object.get",
                CrewdDataObjectGetParams(context:context,reference:source,atRevision:pinned),preservingPresentation:true)
            guard block.context == context, block.resolvedContext == context, block.generation == context.generation,
                  case .block(let id,let actual,"text/plain",.string(let body),_,_) = block.object,
                  id == source.id, actual == pinned else { throw Failure.unavailable }
            return .init(revision:observed,body:body,target:target,source:source,sourceRevision:pinned)
        }
    }
    func restoreNativeDocumentRevision(_ snapshot: NativeDocumentRevisionSnapshot, replacing edit: NativeDocumentEdit) async throws -> CrewdDataChangeResult? {
        try checkNativeHistoryTarget(snapshot.target)
        guard snapshot.target.origin == edit.origin, snapshot.target.parent == edit.parent,
              snapshot.target.reference == edit.view,
              let revision = Self.positiveRevision(snapshot.revision.revision),
              let head = Self.positiveRevision(snapshot.target.head), revision < head else { throw Failure.stale }
        return try await submitNativeDocumentChange(edit,title:snapshot.revision.title,body:snapshot.body,restoring:snapshot)
    }
    func pendingNativeDocumentEdits(viewID: String? = nil, crew: String? = nil) -> [Intent] {
        guard let active, origin() == active, crew == nil || crew == active.crew else { return [] }
        return pending.filter { intent in
            guard let edit = intent.documentEdit else { return false }
            if let context = workspace?.context, !sameAuthority(intent.context,context) { return false }
            return viewID == nil || edit.view.reference.id == viewID
        }
    }
    func synchronizePending() {
        guard let (o, token) = try? begin() else { return }
        finish(o,token)
    }
    func submitNativeDocumentEdit(_ edit: NativeDocumentEdit, title: String, body: String) async throws -> CrewdDataChangeResult? {
        try await submitNativeDocumentChange(edit,title:title,body:body)
    }
    private func submitNativeDocumentChange(_ edit: NativeDocumentEdit, title: String, body: String, restoring snapshot: NativeDocumentRevisionSnapshot? = nil) async throws -> CrewdDataChangeResult? {
        guard !busy, active == edit.origin, let positions = nativeDocumentPlacements(edit.parent) else { throw Failure.stale }
        if let selected = edit.selectedPlacement {
            guard snapshot == nil, editableDocumentPlacements(edit.parent).contains(selected) else { throw Failure.stale }
        } else {
            guard canPrepareNativeDocumentEdit(edit.parent) else { throw Failure.stale }
        }
        let revision = edit.parent.object.revision
        guard !title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty, title.utf8.count <= 512, body.utf8.count <= 1_048_576,
              edit.hasChanges(title:title,body:body), let currentRevision = Int64(revision), currentRevision > 0,
              currentRevision < Int64.max else { throw Failure.invalid }
        let bodyChanged = snapshot != nil || !body.utf8.elementsEqual(edit.body.utf8)
        let placement: CrewdDataPlacementEdit
        var edits: [CrewdDataEdit] = []
        if bodyChanged {
            // Keep the exact historical source on restore, or the edited body
            // on a regular edit. Other views keep their original live block.
            let source = CrewdDataSourceInput(reference:.existing(reference:snapshot?.source ?? edit.bodyRef),
                                             atRevision:snapshot?.sourceRevision ?? edit.bodyRevision,attributes:.object([:]))
            edits.append(.createBlock(alias:"replacementBody",contentType:"text/plain",content:.string(body),sources:[source],attributes:edit.bodyAttributes))
            placement = .set(id:edit.placement.id,alias:nil,block:.created(alias:"replacementBody"),atRevision:nil,layout:edit.placement.layout)
        } else {
            placement = .keep(id:edit.placement.id,layout:edit.placement.layout)
        }
        let revised = positions.map { original -> CrewdDataPlacementEdit in
            original.id == edit.placement.id ? placement : .keep(id:original.id,layout:original.layout)
        }
        edits.append(.updateView(id:edit.view.id,expectedRevision:revision,title:title,
                                 definition:.document(placements:revised,layout:edit.layout),sources:nil,attributes:edit.viewAttributes))
        var witness = NativeDocumentEditReceipt(view:.init(reference:edit.view,revision:String(currentRevision + 1)),replacedBody:bodyChanged ? edit.bodyRef:nil)
        if edit.selectedPlacement != nil {
            let originalPositions = try positions.map { position -> SelectedPlacementEditReceipt.Position in
                let reference: CrewdDataRef?
                if case .visible(let block) = position.block { reference = block } else { reference = nil }
                return .init(id:position.id,layoutDigest:try Self.compositionDigest(position.layout),
                             visibleBlock:reference,pinnedRevision:position.atRevision)
            }
            witness.selected = try .init(selectedID:edit.placement.id,positions:originalPositions,
                titleDigest:Self.compositionDigest(title),layoutDigest:Self.compositionDigest(edit.layout),
                viewAttributesDigest:Self.compositionDigest(edit.viewAttributes),
                bodyDigest:bodyChanged ? Self.compositionDigest(body) : nil,
                bodyAttributesDigest:bodyChanged ? Self.compositionDigest(edit.bodyAttributes) : nil,
                sourceRevision:bodyChanged ? edit.bodyRevision : nil)
        }
        return try await submitNativeChange(.apply(message:snapshot == nil ? "编辑文档" : "恢复文档版本",edits:edits),documentEdit:witness)
    }
    func nativeDocumentStage(channel:String?)throws->CrewdPresentationJournal.DocumentStage? {
        guard let o=active,origin() == o else{throw Failure.stale}
        let authority: CrewdValue?
        if channel == nil {
            guard let context = workspace?.context else { throw Failure.stale }
            authority = try Self.encode(context)
        } else { authority = nil }
        return try journal().currentDocumentStage(crew:o.crew,channel:channel,authorityContext:authority)
    }
    private func documentStageOrigin(_ stage:CrewdPresentationJournal.DocumentStage) throws -> Origin {
        guard let o=active,o.scope == stage.scope,o.crew == stage.crew,origin() == o else{throw Failure.stale}
        if stage.channel == nil {
            guard let current = workspace?.context,
                  sameAuthority(try Self.decode(stage.context,CrewdDataContext.self),current) else { throw Failure.stale }
        }
        return o
    }
    func nativeDocumentResult(_ stage:CrewdPresentationJournal.DocumentStage)throws->CrewdDataChangeResult? {
        _ = try documentStageOrigin(stage)
        guard let value=try journal().documentStageResult(stage) else{return nil}
        let result = try Self.decode(value,CrewdDataChangeResult.self)
        if stage.channel == nil, result.phase == .committed {
            guard let context = workspace?.context else { throw Failure.stale }
            _ = try nativeDocumentCreationIDs(result,context:context)
        }
        return result
    }
    func nativeDocumentWasAttempted(_ stage:CrewdPresentationJournal.DocumentStage) throws -> Bool {
        _ = try documentStageOrigin(stage)
        return try journal().documentStageAttempted(stage)
    }
    func nativeDocumentWasRefused(_ stage:CrewdPresentationJournal.DocumentStage) throws -> Bool {
        _ = try documentStageOrigin(stage)
        return try journal().documentStageWasRefused(stage)
    }
    func finishNativeDocument(_ stage:CrewdPresentationJournal.DocumentStage, discardUnsent:Bool = false)throws {
        _ = try documentStageOrigin(stage)
        try journal().finishDocumentStage(stage,discardUnsent:discardUnsent)
    }
    /// Only the explicit save button can claim the original attempt.
    func submitNativeDocument(title:String,body:String,channel:String?,requestID:UUID? = nil,expectedContext:CrewdDataContext? = nil)async->CrewdDataChangeResult? {
        do {
            guard canSaveNativeDocument, !busy else { throw Failure.unavailable }
            guard let o=active,origin() == o,let context=workspace?.context,
                  expectedContext == nil || expectedContext == context else{throw Failure.stale}
            if channel == nil {
                guard requestID != nil, expectedContext != nil, !title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,
                      title.utf8.count <= 512, body.utf8.count <= 1_048_576 else { throw Failure.invalid }
            }
            let witness = try channel == nil ? NativeDocumentCreationWitness(title:title,body:body) : nil
            try reconcilePending(o)
            guard !hasBlockingPending else { throw Failure.unresolved }
            let ledger=try journal()
            let claim=try ledger.claimDocumentStage(crew:o.crew,channel:channel,title:title,body:body,context:Self.encode(context),requestID:requestID)
            let stage=claim.stage
            if !claim.maySubmit { status="原文档保存仅可核对，不会重发。";return try nativeDocumentResult(stage) }
            let change=CrewdDataChange.apply(message:"创建文档",edits:[
                .createBlock(alias:"body",contentType:"text/plain",content:.string(stage.body),sources:[],attributes:.object([:])),
                .createView(alias:"document",title:stage.title,definition:.document(placements:[.set(id:nil,alias:"bodyPlacement",block:.created(alias:"body"),atRevision:nil,layout:.object([:]))],layout:.object([:])),sources:[],attributes:.object([:]))])
            do {
                let response = try await submitNativeChange(change,requestID:stage.id,documentCreation:witness)
                guard origin() == o else { return nil }
                if let response,[.committed,.rejected,.conflicted].contains(response.phase) {
                    try ledger.recordDocumentStageResult(Self.encode(response),stage:stage)
                }
                return response
            } catch let error as CrewdClientError where witness != nil && Self.definiteNativeRefusal(error) {
                try ledger.recordDocumentStageRefusal(stage)
                throw error
            }
        }catch{status="文档保存阶段尚未确认，原输入保留："+error.localizedDescription;return nil}
    }
    func lookupNativeDocument(_ stage:CrewdPresentationJournal.DocumentStage)async->CrewdDataChangeResult? {
        let o:Origin,token:UUID
        do{(o,token)=try begin()}catch{return nil}
        defer{finish(o,token)}
        do {
            _ = try documentStageOrigin(stage)
            if let saved=try nativeDocumentResult(stage){return saved}
            let context=try Self.decode(stage.context,CrewdDataContext.self)
            let response:CrewdDataRequestGetResult=try await call("data.request.get",CrewdDataRequestGetParams(requestId:stage.id.uuidString.lowercased(),context:context))
            try checked(response.workspace.context,o)
            guard origin() == o else{throw Failure.stale}
            guard case .known(let result)=response.lookup,[.committed,.rejected,.conflicted].contains(result.phase) else{status="原保存请求尚无已确认结果；保留原文与UUID，不会重发。";return nil}
            if stage.channel == nil {
                guard sameAuthority(context,response.workspace.context), UUID(uuidString:result.requestId) == stage.id,
                      sameAuthority(context,result.context), result.phase != .committed ||
                        (result.beforeGeneration == context.generation && result.afterGeneration == result.context.generation) else { throw Failure.stale }
                let witness = try NativeDocumentCreationWitness(title:stage.title,body:stage.body)
                try await verifyNativeDocumentCreation(result,witness:witness,context:response.workspace.context,origin:o)
            }
            let ledger=try journal();try ledger.recordDocumentStageResult(Self.encode(result),stage:stage)
            if let intent=pending.first(where:{$0.id == stage.id}),let record=records[stage.id] {
                try observe(result,intent:intent,record:record,ledger:ledger)
                if current(o,token) { applyObservation(result,intent:intent) }
            }
            return result
        }catch{status="原文档结果未确认；仅保留原UUID核对。";return nil}
    }
    func lookupNativeTable(_ id: UUID, stillCurrent: () -> Bool) async throws -> NativeTableReadback? {
        let (o, token) = try begin()
        defer { finish(o, token) }
        guard stillCurrent(), let intent = pending.first(where:{ $0.id == id && $0.nativeTable != nil }),
              let record = records[id] else { throw Failure.invalid }
        let ledger = try journal()
        let read: CrewdDataRequestGetResult = try await call("data.request.get",
            CrewdDataRequestGetParams(requestId:id.uuidString.lowercased(),context:intent.context),preservingPresentation:true)
        try checked(read.workspace.context,o)
        guard current(o,token), stillCurrent(), !Task.isCancelled else { return nil }
        guard case .known(let original) = read.lookup, original.phase == .committed else { return nil }
        let result = try await verifyNativeTableReadback(original,intent:intent,context:read.workspace.context,origin:o)
        guard current(o,token), stillCurrent(), !Task.isCancelled else { return nil }
        try observe(original,intent:intent,record:record,ledger:ledger)
        applyObservation(original,intent:intent)
        return result
    }

    func lookup(_ id: UUID) async {
        let o: Origin, token: UUID
        do { (o,token) = try begin() } catch { return }
        defer { finish(o,token) }
        do {
            guard let intent = pending.first(where:{$0.id == id && $0.nativeTable == nil}), let record = records[id], workspace != nil else { throw Failure.invalid }
            let ledger = try journal()
            let response: CrewdDataRequestGetResult = try await call("data.request.get", CrewdDataRequestGetParams(requestId:id.uuidString.lowercased(), context:intent.context))
            try checked(response.workspace.context,o)
            guard current(o,token) else { return }
            if workspace?.context != response.workspace.context { clearPresentation() }
            workspace = response.workspace
            switch response.lookup {
            case .known(let original):
                if let creation = intent.documentCreation {
                    try checkObservation(original,intent:intent)
                    try await verifyNativeDocumentCreation(original,witness:creation,context:response.workspace.context,origin:o)
                    guard current(o,token) else { return }
                    if [.committed,.rejected,.conflicted].contains(original.phase) {
                        guard let stage = try ledger.currentDocumentStage(crew:o.crew,channel:nil,authorityContext:Self.encode(response.workspace.context)),
                              stage.id == intent.id else { throw Failure.stale }
                        try ledger.recordDocumentStageResult(Self.encode(original),stage:stage)
                    }
                }
                if intent.documentEdit?.selected != nil { try await verifySelectedPlacementReadback(original,intent:intent,context:response.workspace.context,origin:o) }
                if intent.viewArchive != nil { try await verifyArchiveReadback(original,intent:intent,context:response.workspace.context,origin:o) }
                if intent.documentWorkLink != nil { try await verifyDocumentWorkLinkReadback(original,intent:intent,context:response.workspace.context,origin:o) }
                if intent.composition != nil { try await verifyCompositionReadback(original,intent:intent,context:response.workspace.context,origin:o) }
                guard current(o,token) else { return }
                try observe(original,intent:intent,record:record,ledger:ledger)
                applyObservation(original,intent:intent)
            case .notFound: status = "原 UUID 未找到；没有重发。"
            case .historical: status = "原请求属于历史状态；未恢复为当前成功。"
            }
        } catch { if current(o,token) { status = "原请求仍未知；没有重新提交。" } }
    }
    func currentOrigin() -> CrewdKnowledgeViewOrigin? {
        guard let o = active, origin() == o, let c = workspace?.context, workspace?.state == "ready" else { return nil }
        return .init(authority:c.authorityId,database:c.dbId,restoreEpoch:c.restoreEpoch,crew:c.crewId,namespace:c.namespace,generation:c.generation,permissionEpoch:o.scope.grantRevision ?? o.observation.uuidString)
    }
    func hasUnresolvedWrite(in origin: CrewdKnowledgeViewOrigin) -> Bool { hasBlockingPending || currentOrigin() != origin || busy }
    private func installRenderer(_ object: CrewdDataObject, snapshot: Bool) {
        guard case .view(let id,let revision,_,let definition,let archived,_) = object, !archived,
              case .web(let kind,let version,let html,let css,let js,_,let capabilities,_) = definition,
              kind == "web", version == "1", let origin = currentOrigin() else { return }
        guard let detail, detail.resolvedContext.namespace == detailReference?.namespace,
              detail.resolvedContext.authorityId == detail.context.authorityId,
              detail.resolvedContext.dbId == detail.context.dbId,
              detail.resolvedContext.restoreEpoch == detail.context.restoreEpoch else { return }
        let bytes = (try? JSONEncoder().encode(definition)) ?? Data()
        let definitionHash = SHA256.hash(data:bytes).map{String(format:"%02x",$0)}.joined()
        // A foreign read scope is not a standing write grant. Switch to its real
        // crew through the native workspace before granting writes there.
        try? renderer.activate(.init(id:id,revision:revision,html:html,css:css,javascript:js,definitionDigest:definitionHash,assets:[:],declaresKnowledgeWrite:capabilities.contains("knowledge-write") && detailReference?.namespace == workspace?.context.namespace,readOnlySnapshot:snapshot),origin:origin,readContext:detail.resolvedContext,readGeneration:detail.generation)
    }
    private func renderContext(_ context: CrewdKnowledgeViewContext) throws -> CrewdDataContext {
        guard !busy, currentOrigin() == context.origin, renderer.context == context, let c = workspace?.context,
              detail?.object.identity == context.view, detail?.object.revision == context.revision,
              detail?.resolvedContext == context.readContext, detail?.generation == context.readGeneration else { throw Failure.stale }
        return c
    }
    func read(context: CrewdKnowledgeViewContext, input: CrewdValue) async throws -> CrewdValue {
        let c = try renderContext(context)
        guard case .object(let fields) = input, Set(fields.keys) == ["binding"], let name = input["binding"].string,
              case .view(_,_,_,.web(_,_,_,_,_,let bindings,_,_),_,_)? = detail?.object,
              let binding = bindings.first(where:{$0.id == name}) else { throw Failure.invalid }
        let response: CrewdValue
        switch binding.source {
        case .query(let statement):
            guard !detailIsSnapshot else { throw Failure.unsupported }
            let value: CrewdDataRows = try await call("data.query", CrewdDataQueryParams(context:context.readContext,statement:statement,atRevision:nil,cursor:nil,limit:100))
            guard !self.detailIsSnapshot, value.generation == context.readGeneration else { throw Failure.invalid }
            response = try Self.encode(value)
        case .object(let state,let atRevision):
            guard case .visible(let reference) = state, ["block", "view", "relation"].contains(reference.kind) else { return .object(["state":.string("unavailable")]) }
            let cut: String?
            if detailIsSnapshot {
                guard let captured = detail?.references.first(where: {$0.anchor == "binding:" + name && $0.target == state && $0.complete}), let commit = captured.revision else { return .object(["state":.string("unavailable")]) }
                cut = commit
            } else { cut = atRevision }
            let value: CrewdDataObjectDetail = try await call("data.object.get",CrewdDataObjectGetParams(context:c,reference:reference,atRevision:cut))
            guard value.object.identity == reference.id, value.resolvedContext.namespace == reference.namespace, cut == nil || value.object.revision == cut else { throw Failure.invalid }
            response = try Self.encode(value)
        case .objects(let kind,let limit):
            guard !detailIsSnapshot else { throw Failure.unsupported }
            let value: CrewdDataObjectPage = try await call("data.object.list",CrewdDataObjectListParams(context:context.readContext,objectType:kind,after:nil,limit:min(limit,100),atRevision:nil,includeArchived:false))
            guard !self.detailIsSnapshot, value.generation == context.readGeneration else { throw Failure.invalid }
            response = try Self.encode(value)
        }
        _ = try renderContext(context); return response
    }
    func prepare(_ intent: CrewdKnowledgeViewWrite) throws {
        let c = try renderContext(intent.context)
        guard !hasBlockingPending, case .object(let input) = intent.input, Set(input.keys) == ["message","edits"],
              let message = input["message"]?.string, let edits = input["edits"] else { throw Failure.invalid }
        let typed = try Self.decode(edits,[CrewdDataEdit].self)
        guard !typed.isEmpty, typed.count <= 64 else { throw Failure.invalid }
        let change = CrewdDataChange.apply(message:message,edits:typed)
        let (recorded,record) = try stage(change,id:intent.requestID,context:c)
        rendererPrepared[intent.requestID] = (recorded,record,change)
    }
    func submit(_ intent: CrewdKnowledgeViewWrite) async throws -> CrewdKnowledgeViewWriteObservation {
        let c = try renderContext(intent.context)
        guard let (recorded,record,change) = rendererPrepared[intent.requestID], recorded.context == c else { throw Failure.invalid }
        let ledger = try journal()
        let response: CrewdDataChangeResult = try await call("data.submit",CrewdDataSubmitParams(requestId:intent.requestID.uuidString.lowercased(),context:c,change:change))
        try observe(response,intent:recorded,record:record,ledger:ledger)
        _ = try renderContext(intent.context)
        result = response
        switch response.phase {
        case .committed, .rejected, .conflicted:
            pending.removeAll{$0.id == recorded.id}; records.removeValue(forKey:recorded.id)
            let value = try Self.encode(response)
            let observation: CrewdKnowledgeViewWriteObservation = response.phase == .committed ? .committed(value) : (response.phase == .rejected ? .rejected(value) : .conflicted(value))
            rendererTerminal[intent.requestID] = observation
            return observation
        default: return .unknown
        }
    }
    func lookup(_ intent: CrewdKnowledgeViewWrite) async throws -> CrewdKnowledgeViewWriteObservation {
        _ = try renderContext(intent.context)
        if let terminal = rendererTerminal[intent.requestID] { return terminal }
        guard let (recorded,record,_) = rendererPrepared[intent.requestID] else { throw Failure.invalid }
        let ledger = try journal()
        let response: CrewdDataRequestGetResult = try await call("data.request.get",CrewdDataRequestGetParams(requestId:recorded.id.uuidString.lowercased(),context:recorded.context))
        _ = try renderContext(intent.context)
        switch response.lookup {
        case .known(let original):
            try observe(original,intent:recorded,record:record,ledger:ledger)
            guard [.committed,.rejected,.conflicted].contains(original.phase) else { return .unknown }
            pending.removeAll{$0.id == recorded.id}; records.removeValue(forKey:recorded.id)
            let value = try Self.encode(original)
            let observation: CrewdKnowledgeViewWriteObservation = original.phase == .committed ? .committed(value) : (original.phase == .rejected ? .rejected(value) : .conflicted(value))
            rendererTerminal[intent.requestID] = observation
            return observation
        case .notFound: return .notFound
        case .historical: return .historical
        }
    }
    /// Core links are current native navigation, never a historical knowledge object.
    /// The injected original workspace resolves only already-known current
    /// session members and retains its own actual Core ACL/read paths.
    func follow(_ link: CrewdDataResolvedRef) async {
        guard let current = detail, current.references.contains(link),
              case .visible(let ref) = link.target, let c = workspace?.context,
              current.context == c, currentOrigin() != nil else { status = "关联不可用。"; return }
        if ["block", "view", "relation"].contains(ref.kind) {
            if detailIsSnapshot {
                guard link.complete, let cut = link.revision else { status = "历史关联不可用；未读取 latest。"; return }
                await open(ref, atRevision: cut)
            } else {
                if !link.live { guard link.complete, link.revision != nil else { status = "固定关联不可用；未读取 latest。"; return } }
                await open(ref, atRevision: link.live ? nil : link.revision)
            }
        } else {
            guard nativeNavigation(ref,c) else { status = "当前原生解析器无法导航此关联；未猜测 RPC。"; return }
        }
    }
    func navigate(context: CrewdKnowledgeViewContext, reference: CrewdValue) async throws {
        _ = try renderContext(context)
        let ref = try Self.decode(reference,CrewdDataRef.self)
        guard detail?.references.contains(where:{if case .visible(let r) = $0.target{return r == ref};return false}) == true else { throw Failure.invalid }
        if ["block", "view", "relation"].contains(ref.kind) {
            guard let link = detail?.references.first(where: { if case .visible(let value) = $0.target { return value == ref }; return false }) else { throw Failure.invalid }
            await follow(link)
        } else {
            guard let c = workspace?.context, nativeNavigation(ref,c) else { throw Failure.unsupported }
        }
    }
}
extension CrewdDataObject {
    var identity: String { switch self {case .block(let id,_,_,_,_,_),.view(let id,_,_,_,_,_),.relation(let id,_,_,_,_,_,_,_):return id} }
    var revision: String { switch self {case .block(_,let r,_,_,_,_),.view(_,let r,_,_,_,_),.relation(_,let r,_,_,_,_,_,_):return r} }
    var displayTitle: String { switch self {case .view(_,_,let title,_,_,_):return title;case .block(let id,_,let type,_,_,_):return type+" · "+id;case .relation(_,_,_,let predicate,_,_,_,_):return predicate} }
    var objectKind: String {switch self {case .view:return "view";case .block:return "block";case .relation:return "relation"}}
}

/// A read projection of Core's real channel placements. The content store is
/// unchanged; linking a view never copies blocks or submits content again.
extension CrewdChannelKnowledgeItem {
    var isReadableActiveView: Bool { state == "ready" && archived == false }
    var isArchivedReference: Bool { state == "ready" && archived == true }
}

@MainActor final class CrewdChannelKnowledgeStore:ObservableObject {
    typealias Call=(String,CrewdValue)async throws->CrewdValue
    @Published private(set) var crew=""
    @Published private(set) var channel=""
    @Published private(set) var items:[CrewdChannelKnowledgeItem]=[]
    @Published private(set) var linkRevision:String?
    @Published private(set) var loaded=false
    @Published private(set) var loading=false
    @Published private(set) var writing=false
    @Published private(set) var failure:String?
    @Published private(set) var activationEpoch=UUID()
    private var generation=UUID(),query=UUID()
    var readScope: UUID { generation }
    private var owners:Set<UUID>=[]
    private var refreshTask:Task<Void,Never>?
    private let call:Call
    init(call:@escaping Call){self.call=call}
    func linkedViewIDs(in context: CrewdDataContext) -> Set<String> {
        guard loaded,!loading,failure == nil else { return [] }
        return Set(items.compactMap { item in
            let source = CrewdChannelKnowledgeReference(context:context,reference:item.source.reference)
            return source.identity == item.id ? item.source.reference.id : nil
        })
    }
    func selectChannel(crew:String,channel:String){
        guard self.crew != crew || self.channel != channel else{return}
        generation=UUID();query=UUID();refreshTask?.cancel();self.crew=crew;self.channel=channel;items=[];linkRevision=nil;loaded=false;loading=false;failure=nil
    }
    func invalidate(){generation=UUID();query=UUID();refreshTask?.cancel();items=[];loaded=false;loading=false;linkRevision=nil;activationEpoch=UUID()}
    func setActive(_ active:Bool,owner:UUID){if active{owners.insert(owner)}else{owners.remove(owner)};if owners.isEmpty{refreshTask?.cancel()}}
    private func check(_ token:UUID,_ request:UUID?=nil)throws{guard token == generation,!Task.isCancelled,request == nil || query == request else{throw CancellationError()}}
    func handleEvent(_ name:String,_ value:CrewdValue){
        guard !owners.isEmpty,!channel.isEmpty else{return}
        if name == "channel.workChanged" {guard value["channelId"] == .string(channel) else{return}}
        else if name == "data.changed" {guard value["crewId"] == .string(crew) || items.contains(where:{.string($0.source.context.crewId) == value["crewId"]}) else{return}}
        else if name != "events.lagged"{return}
        refreshTask?.cancel();let token=generation
        refreshTask=Task{[weak self] in try? await Task.sleep(for:.milliseconds(150));guard let self,!Task.isCancelled,self.generation == token else{return};try? await self.refresh()}
    }
    func refresh()async throws {
        guard !channel.isEmpty else{throw CrewdClientError(message:"请先选择群。",kind:.notSubmitted)}
        let token=generation,request=UUID(),scope=channel;query=request;loading=true
        defer{if query == request{loading=false}}
        do {
            var result:[CrewdChannelKnowledgeItem]=[],seen:Set<String>=[],cursors:Set<String>=[],cursor:String?,links:String?,source:String?
            repeat {
                var p:[String:CrewdValue]=["channelId":.string(scope),"limit":.integer(100)];if let cursor{p["after"] = .string(cursor)}
                let raw=try await call("channel.knowledgeList",.object(p));try check(token,request)
                let page=try CrewdKnowledgeStore.decode(raw,CrewdChannelKnowledgePage.self)
                guard page.channelId == scope,!page.linkRevision.isEmpty,!page.sourceRevision.isEmpty,page.items.count <= 100,
                      links == nil || links == page.linkRevision,source == nil || source == page.sourceRevision else{throw CrewdClientError(message:"群知识引用范围或分页版本不符。")}
                links=page.linkRevision;source=page.sourceRevision
                for item in page.items {
                    guard item.source.valid,["ready","contextChanged","unavailable"].contains(item.state),seen.insert(item.id).inserted,
                          item.state != "ready" || (item.title != nil && item.revision != nil && item.archived != nil) else{throw CrewdClientError(message:"群知识引用身份、状态或元数据无效。")}
                    result.append(item)
                }
                cursor=page.next
                if let cursor{guard !cursor.isEmpty,cursor.utf8.count <= 4096,cursors.insert(cursor).inserted else{throw CrewdClientError(message:"群知识引用分页无效。")}}
            }while cursor != nil
            try check(token,request);items=result;linkRevision=links;loaded=true;failure=nil
        }catch{if token == generation,query == request,!(error is CancellationError){failure=error.localizedDescription};throw error}
    }
    @discardableResult func submit(_ snapshot:CrewdChannelKnowledgeDrafts.Snapshot)async throws->CrewdChannelKnowledgeReceipt {
        guard snapshot.crew == crew,snapshot.channel == channel,!writing,snapshot.source.valid else{throw CrewdClientError(message:"知识引用的原群已切换或正在提交。",kind:.notSubmitted)}
        writing=true;let token=generation;defer{writing=false}
        let params=CrewdValue.object(["requestId":.string(snapshot.id.uuidString.lowercased()),"channelId":.string(snapshot.channel),"expectedLinkRevision":.string(snapshot.revision),"source":try CrewdKnowledgeStore.encode(snapshot.source)])
        let result=try await call(snapshot.operation,params);try check(token)
        let receipt=try validate(result,snapshot:snapshot)
        do{try await refresh()}catch{}
        return receipt
    }
    func lookup(_ snapshot:CrewdChannelKnowledgeDrafts.Snapshot)async throws->CrewdValue? {
        guard snapshot.crew == crew,snapshot.channel == channel else{throw CancellationError()}
        let token=generation
        let value=try await call("resource.requestLookup",.object(["requestId":.string(snapshot.id.uuidString.lowercased())]));try check(token)
        guard value["requestId"].string.flatMap(UUID.init(uuidString:)) == snapshot.id else{throw CrewdClientError(message:"原知识引用请求身份不符。")}
        if value["status"] == .string("notFound") {guard value["operation"] == .null,value["result"] == .null else{throw CrewdClientError(message:"未找到知识引用请求却附带结果。")};return nil}
        guard value["status"] == .string("committed"),value["operation"] == .string(snapshot.operation) else{throw CrewdClientError(message:"原知识引用请求状态或操作不符。")}
        try CrewdPresentationJournal.validateResourceResult(
            value["result"], method:snapshot.operation,
            params:.object(["channelId":.string(snapshot.channel),"source":try CrewdKnowledgeStore.encode(snapshot.source)])
        )
        return value
    }
    private func validate(_ value:CrewdValue,snapshot:CrewdChannelKnowledgeDrafts.Snapshot)throws->CrewdChannelKnowledgeReceipt {
        let result=try CrewdKnowledgeStore.decode(value,CrewdChannelKnowledgeReceipt.self)
        guard result.channelId == snapshot.channel,result.source == snapshot.source,result.linked == snapshot.linked,!result.linkRevision.isEmpty else{throw CrewdClientError(message:"原知识引用回执与原输入不符。")}
        return result
    }
}

/// A document sheet can recover the original resource mutation after the App
/// reopens. Its journal record is only evidence to query, never a send intent.
struct CrewdPendingKnowledgeLink: Identifiable {
    let record: CrewdPresentationJournal.Prepared
    let snapshot: CrewdChannelKnowledgeDrafts.Snapshot
    var id: UUID { record.id }
    var requestID: UUID { snapshot.id }
    var source: CrewdChannelKnowledgeReference { snapshot.source }
    var linked: Bool { snapshot.linked }

    @MainActor static func inChannel(_ records: [CrewdPresentationJournal.Prepared],
                                     scope: CrewdPresentationJournal.Scope?,
                                     crew: String, channel: String) -> [Self] {
        guard let scope, !crew.isEmpty, !channel.isEmpty else { return [] }
        return records.compactMap { record in
            guard record.scope == scope, record.crew == crew,
                  record.params["channelId"] == .string(channel),
                  ["channel.knowledgeLink", "channel.knowledgeUnlink"].contains(record.method),
                  let id = record.params["requestId"].string.flatMap(UUID.init(uuidString:)),
                  let revision = record.params["expectedLinkRevision"].string, !revision.isEmpty,
                  let source = try? CrewdKnowledgeStore.decode(record.params["source"], CrewdChannelKnowledgeReference.self),
                  source.valid else { return nil }
            let snapshot = CrewdChannelKnowledgeDrafts.Snapshot(
                id: id, crew: crew, channel: channel, source: source, revision: revision,
                linked: record.method == "channel.knowledgeLink"
            )
            return Self(record: record, snapshot: snapshot)
        }
    }
}

/// Native access to Core-owned organization objects, separate from SQLite knowledge.
@MainActor final class CrewdManagedDirectoryStore: ObservableObject {
    typealias Wire = CrewdManagedDirectoryWire
    typealias Origin = CrewdKnowledgeStore.Origin
    typealias RPC = CrewdKnowledgeStore.RPC

    struct Invalidation: Equatable {
        let id = UUID()
        fileprivate let origin: Origin
    }
    @Published private(set) var invalidation: Invalidation?

    // The event is only a hint. Cross-crew reads are legal and a row event
    // does not disclose its table, so re-read the active projection.
    func handleEvent(_ name: String, _ data: CrewdValue, connection: UUID) {
        guard let active = origin(), active.connection == connection else { return }
        switch name {
        case "onto.changed":
            guard case .object(let fields) = data, Set(fields.keys) == ["crewId","objectIds"],
                  let crew = fields["crewId"]?.string, Wire.DecisionAbout(kind:.crew,id:crew).valid,
                  case .array(let objects) = fields["objectIds"],
                  (1...2).contains(objects.count) else { return }
            let ids = objects.compactMap(\.string)
            guard ids.count == objects.count, Set(ids).count == ids.count, ids.allSatisfy({ id in
                guard let prefix = id.first, "TODRL".contains(prefix) else { return false }
                return Wire.objectNumber(id,prefix:prefix) != nil
            }) else { return }
        case "events.lagged", "crew.changed", "crew.purged", "channel.changed", "channel.purged": break
        default: return
        }
        invalidation = .init(origin:active)
    }

    func owns(_ candidate: Invalidation, scope: BrowseScope? = nil) -> Bool {
        origin() == candidate.origin && (scope.map { $0.origin == candidate.origin } ?? true)
    }

    struct BrowseScope: Equatable {
        let origin: Origin
        let authority: Wire.Scope
    }
    struct Witness: Codable, Equatable {
        let scope: Wire.Scope
        let crewId: String
        let requestId: String
        let kind: String
        let target: String
        let tableId: String?
        let expectedRevision: String?
        let digest: String
        var presentationCrew: String? = nil
        var riskLesson: RiskLessonWitness? = nil
        var isDecision: Bool {
            ["recordDecision","replaceDecision","withdrawDecision"].contains(kind)
        }
    }
    struct RiskLessonWitness: Codable, Equatable {
        let reviewAt: String?
        let checkedAt: String?
    }
    struct Pending: Identifiable {
        let record: CrewdPresentationJournal.Prepared
        let witness: Witness
        var id: UUID { record.id }
    }
    struct Observation {
        let receipt: Wire.Receipt
        let historical: Bool
    }
    enum Mutation {
        case create(String, [Wire.Field])
        case add(Wire.Table, [Wire.Cell])
        case edit(Wire.Table, Wire.Row, [Wire.Cell])
        case recordDecision(Wire.DecisionDraft)
        case replaceDecision(Wire.Decision, Wire.DecisionDraft)
        case withdrawDecision(Wire.Decision, String)
        case riskLesson(RiskLessonMutation)

        var isDecision: Bool {
            switch self {
            case .recordDecision, .replaceDecision, .withdrawDecision: true
            case .create, .add, .edit, .riskLesson: false
            }
        }

        func encode(crew: String, risksAvailable: Bool = false) throws -> (Wire.Change, String, String?, String?, String) {
            switch self {
            case .create(let title, let fields):
                guard Wire.validDefinition(title: title, fields: fields) else { throw Wire.Problem.invalid }
                let change = Wire.Change(kind: "createTable", title: title, fields: fields)
                return (change, "createTable", nil, nil, try Self.definitionDigest(title, fields))
            case .add(let table, let values):
                guard table.valid, !table.frozen, Wire.exact(crew, table.crewId),
                      Wire.validValues(values, fields: table.fields) else { throw Wire.Problem.invalid }
                return (.init(kind: "addRow", tableId: table.id, values: values), table.id,
                        table.id, nil, try Self.digest(values))
            case .edit(let table, let row, let values):
                guard table.valid, row.matches(table), !row.frozen, Wire.exact(crew, row.crewId),
                      Wire.validValues(values, fields: table.fields) else { throw Wire.Problem.invalid }
                return (.init(kind: "editRow", rowId: row.id, expectedRevision: row.revision, values: values),
                        row.id, table.id, row.revision, try Self.digest(values))
            case .recordDecision(let draft):
                guard Self.allowed(draft, risksAvailable:risksAvailable) else { throw Wire.Problem.invalid }
                return (.init(kind:"recordDecision",draft:draft),"recordDecision",nil,nil,try Self.digest(draft))
            case .replaceDecision(let current,let draft):
                guard current.canChange(crew:crew), Self.allowed(draft, risksAvailable:risksAvailable) else { throw Wire.Problem.invalid }
                return (.init(kind:"replaceDecision",expectedRevision:current.revision,decisionId:current.id,draft:draft),
                        current.id,nil,current.revision,try Self.digest(draft))
            case .withdrawDecision(let current,let reason):
                guard current.canChange(crew:crew), Wire.decisionText(reason) else { throw Wire.Problem.invalid }
                return (.init(kind:"withdrawDecision",expectedRevision:current.revision,decisionId:current.id,reason:reason),
                        current.id,nil,current.revision,try Self.withdrawalDigest(current,reason:reason))
            case .riskLesson: throw Wire.Problem.invalid
            }
        }
        private static func allowed(_ draft: Wire.DecisionDraft, risksAvailable: Bool) -> Bool {
            draft.valid && (risksAvailable || !draft.about.contains { $0.kind == .risk })
        }
        private struct WithdrawalContent: Encodable {
            let draft: Wire.DecisionDraft
            let author: Wire.DecisionAuthor
            let createdAt: String
            let supersedes: String?
            let reason: String
        }
        static func withdrawalDigest(_ decision: Wire.Decision, reason: String) throws -> String {
            try digest(WithdrawalContent(draft:decision.draft,author:decision.author,createdAt:decision.createdAt,
                                         supersedes:decision.supersedes,reason:reason))
        }
        static func definitionDigest(_ title: String, _ fields: [Wire.Field]) throws -> String {
            try digest(Wire.Change(kind: "createTable", title: title, fields: fields))
        }
        static func digest<T: Encodable>(_ value: T) throws -> String {
            try SHA256.hash(data: Wire.canonical(value)).map { String(format: "%02x", $0) }.joined()
        }
    }

    private let origin: () -> Origin?
    private let rpc: RPC
    private let journal: () throws -> CrewdPresentationJournal
    private let decisionsAvailable: () -> Bool
    private let riskLessonsAvailable: () -> Bool
    private let structuredDecisionsAvailable: () -> Bool
    private let messageSourcesAvailable: () -> Bool

    init(origin: @escaping () -> Origin?, rpc: @escaping RPC, journal: @escaping () throws -> CrewdPresentationJournal,
         decisionsAvailable: @escaping () -> Bool = { false },
         riskLessonsAvailable: @escaping () -> Bool = { false },
         structuredDecisionsAvailable: @escaping () -> Bool = { false },
         messageSourcesAvailable: @escaping () -> Bool = { false }) {
        self.origin = origin
        self.rpc = rpc
        self.journal = journal
        self.decisionsAvailable = decisionsAvailable
        self.riskLessonsAvailable = riskLessonsAvailable
        self.structuredDecisionsAvailable = structuredDecisionsAvailable
        self.messageSourcesAvailable = messageSourcesAvailable
    }

    func scope(stillCurrent: @MainActor () -> Bool) async throws -> BrowseScope {
        guard let active = origin(), !active.crew.isEmpty else { throw Wire.Problem.unavailable }
        let scope: Wire.Scope = try await call("managedObject.scope", .object([:]), active, stillCurrent)
        guard let db = active.scope.databaseID, Wire.exact(db, scope.dbId), !scope.restoreEpoch.isEmpty
        else { throw Wire.Problem.invalid }
        return .init(origin: active, authority: scope)
    }
    func tables(_ scope: BrowseScope, after: String?, stillCurrent: @MainActor () -> Bool) async throws -> Wire.TablePage {
        let page: Wire.TablePage = try await call("managedObject.tables",
            .object(["after": after.map(CrewdValue.string) ?? .null]), scope.origin, stillCurrent)
        guard page.items.allSatisfy(\.valid), Wire.validPage(page.items.map(\.id), after: after,
              next: page.nextAfter, prefix: "T") else { throw Wire.Problem.invalid }
        try await check(scope, stillCurrent)
        return page
    }
    func rows(_ scope: BrowseScope, table: String, after: String?, stillCurrent: @MainActor () -> Bool) async throws -> Wire.RowPage {
        guard Wire.objectNumber(table, prefix: "T") != nil else { throw Wire.Problem.invalid }
        let page: Wire.RowPage = try await call("managedObject.rows",
            .object(["tableId": .string(table), "after": after.map(CrewdValue.string) ?? .null]),
            scope.origin, stillCurrent)
        guard page.table.valid, page.table.id == table, page.items.allSatisfy({ $0.matches(page.table) }),
              Wire.validPage(page.items.map(\.id), after: after, next: page.nextAfter, prefix: "O")
        else { throw Wire.Problem.invalid }
        try await check(scope, stillCurrent)
        return page
    }

    func decisions(_ scope: BrowseScope, after: String?, stillCurrent: @MainActor () -> Bool) async throws -> Wire.DecisionPage {
        guard decisionsAvailable() else { throw Wire.Problem.unavailable }
        let includeDefaults=structuredDecisionsAvailable()
        let page: Wire.DecisionPage = try await call("managedObject.decisions",
            decisionParams(["after":after.map(CrewdValue.string) ?? .null],includeDefaults:includeDefaults),scope.origin,stillCurrent)
        guard page.items.allSatisfy({ $0.valid && (includeDefaults || $0.author.kind != "systemDefault") }), Wire.validPage(page.items.map(\.id),after:after,
              next:page.nextAfter,prefix:"D",newestFirst:true) else { throw Wire.Problem.invalid }
        try await check(scope,stillCurrent)
        return page
    }
    func decision(_ scope: BrowseScope, id: String, stillCurrent: @MainActor () -> Bool) async throws -> Wire.Decision {
        guard decisionsAvailable() else { throw Wire.Problem.unavailable }
        guard Wire.objectNumber(id,prefix:"D") != nil else { throw Wire.Problem.invalid }
        let includeDefaults=structuredDecisionsAvailable()
        let object: Wire.Object = try await call("managedObject.get",
            decisionParams(["id":.string(id)],includeDefaults:includeDefaults),scope.origin,stillCurrent)
        guard object.valid, let decision = object.decision, decision.id == id,
              includeDefaults || decision.author.kind != "systemDefault" else { throw Wire.Problem.invalid }
        try await check(scope,stillCurrent)
        return decision
    }

    private func decisionParams(_ fields:[String:CrewdValue],includeDefaults:Bool) -> CrewdValue {
        var params=fields
        if includeDefaults { params["includeSystemDefaults"] = .bool(true) }
        return .object(params)
    }

    func decisionOrigin(_ scope:BrowseScope, decision:Wire.Decision,
                        stillCurrent:@MainActor () -> Bool) async throws -> CrewdStructuredAskWire.Review? {
        guard structuredDecisionsAvailable() else { return nil }
        guard decision.valid, valid(scope.origin,stillCurrent) else { throw Wire.Problem.stale }
        let raw=try await rpc(CrewdStructuredAskWire.originMethod,.object(["id":.string(decision.id)]))
        try await check(scope,stillCurrent)
        if raw == .null {
            guard decision.author.kind != "systemDefault" else { throw Wire.Problem.invalid }
            return nil
        }
        let source=try CrewdStructuredAskWire.Review(raw,crew:decision.crewId)
        guard let capture=source.capture, capture.decisionId==decision.id,
              capture.source.rawValue==decision.author.kind, Wire.exact(source.question,decision.draft.question),
              Wire.exact(source.context,decision.draft.rationale),
              let option=source.option(capture.optionId), Wire.exact(option.label,decision.draft.choice)
        else { throw Wire.Problem.invalid }
        return source
    }

    func riskLessons(_ scope: BrowseScope, lessons: Bool, after: String?,
                     stillCurrent: @MainActor () -> Bool) async throws -> (items:[Wire.RiskLessonEntry],next:String?) {
        guard riskLessonsAvailable() else { throw Wire.Problem.unavailable }
        let params = CrewdValue.object(["after":after.map(CrewdValue.string) ?? .null])
        let items: [Wire.RiskLessonEntry]
        let next: String?
        if lessons {
            let page: Wire.RiskLessonPage<Wire.Lesson> = try await call("managedObject.lessons",params,scope.origin,stillCurrent)
            guard page.items.allSatisfy(\.valid) else { throw Wire.Problem.invalid }
            items = page.items.map(Wire.RiskLessonEntry.lesson)
            next = page.nextAfter
        } else {
            let page: Wire.RiskLessonPage<Wire.Risk> = try await call("managedObject.risks",params,scope.origin,stillCurrent)
            guard page.items.allSatisfy(\.valid) else { throw Wire.Problem.invalid }
            items = page.items.map(Wire.RiskLessonEntry.risk)
            next = page.nextAfter
        }
        guard Wire.validPage(items.map(\.id),after:after,next:next,prefix:lessons ? "L":"R",newestFirst:true)
        else { throw Wire.Problem.invalid }
        try await check(scope,stillCurrent)
        return (items,next)
    }

    func riskLesson(_ scope: BrowseScope, id: String, stillCurrent: @MainActor () -> Bool) async throws -> Wire.RiskLessonEntry {
        guard riskLessonsAvailable() else { throw Wire.Problem.unavailable }
        guard Wire.objectNumber(id,prefix:"R") != nil || Wire.objectNumber(id,prefix:"L") != nil else { throw Wire.Problem.invalid }
        let object: Wire.Object = try await call("managedObject.get",.object(["id":.string(id)]),scope.origin,stillCurrent)
        guard object.valid, let item = object.riskLessonEntry, item.id == id else { throw Wire.Problem.invalid }
        try await check(scope,stillCurrent)
        return item
    }

    func pending(_ scope: BrowseScope) throws -> [Pending] {
        guard valid(scope.origin, { true }) else { throw Wire.Problem.stale }
        let ledger = try journal()
        guard ledger.scope == scope.origin.scope else { throw Wire.Problem.stale }
        return try ledger.unknownRecords().filter {
            $0.method == Wire.observation && $0.scope == ledger.scope &&
                Wire.exact($0.params["presentationCrew"].string ?? $0.crew,scope.origin.crew)
        }.map {
            let witness = try CrewdKnowledgeStore.decode($0.params, Witness.self)
            guard Wire.exact(witness.crewId, $0.crew), witness.scope.dbId == scope.authority.dbId,
                  witness.presentationCrew == nil || witness.kind == "confirmLesson",
                  UUID(uuidString: witness.requestId)?.uuidString.lowercased() == witness.requestId
            else { throw Wire.Problem.invalid }
            return .init(record: $0, witness: witness)
        }
    }

    func submit(_ mutation: Mutation, scope: BrowseScope, stillCurrent: @MainActor () -> Bool) async throws -> Observation {
        guard !mutation.isDecision || decisionsAvailable() else { throw Wire.Problem.unavailable }
        try await check(scope, stillCurrent)
        let request = UUID().uuidString.lowercased()
        let witness: Witness
        let change: Wire.Change
        if case .riskLesson(let operation) = mutation {
            guard riskLessonsAvailable() else { throw Wire.Problem.unavailable }
            (change,witness) = try operation.prepare(scope:scope,request:request)
        } else {
            let crew = scope.origin.crew
            let (wire,target,tableID,revision,digest) = try mutation.encode(crew:crew,risksAvailable:riskLessonsAvailable())
            change = wire
            witness = Witness(scope:scope.authority,crewId:crew,requestId:request,kind:wire.kind,
                              target:target,tableId:tableID,expectedRevision:revision,digest:digest)
        }
        let params = try CrewdKnowledgeStore.encode(Wire.Submit(scope:scope.authority,crewId:witness.crewId,
                                                               requestId:request,change:change))
        let ledger = try journal()
        guard ledger.scope == scope.origin.scope, valid(scope.origin, stillCurrent) else { throw Wire.Problem.stale }
        // The durable observation keeps identity and a payload digest only.
        let prepared = try ledger.prepareManagedDirectory(CrewdKnowledgeStore.encode(witness), crew: witness.crewId)
        do {
            let receipt: Wire.Receipt = try await call("managedObject.submit", params, scope.origin, stillCurrent)
            try verify(receipt, witness)
            try await check(scope, stillCurrent)
            try ledger.finish(prepared, state: "replyObserved")
            return .init(receipt: receipt, historical: false)
        } catch {
            let client = error as? CrewdClientError
            let definitelyRefused = client?.kind == .notSubmitted ||
                (client?.code == -32009 && ["managed_object.row_changed","managed_object.decision_changed","managed_object.risk_changed","managed_object.lesson_changed"]
                    .contains(client?.refusal ?? "")) || ((mutation.isDecision || witness.riskLesson != nil) &&
                    (Self.decisionInputRefused(error) || (client?.code == -32009 && client?.refusal == "resource.frozen")))
            try? ledger.finish(prepared, state: definitelyRefused ? "refused" : "unknown")
            throw error
        }
    }

    static func decisionInputRefused(_ error: Error) -> Bool {
        guard let failure = error as? CrewdClientError else { return false }
        switch (failure.code,failure.refusal) {
        case (-32602,"resource.invalid"), (-32004,"resource.not_found"), (-32003,"resource.forbidden"): return true
        default: return false
        }
    }

    func lookup(_ pending: Pending, scope: BrowseScope, stillCurrent: @MainActor () -> Bool) async throws -> Observation? {
        let ledger = try journal()
        let witness = pending.witness
        guard (!witness.isDecision || decisionsAvailable()) &&
                (witness.riskLesson == nil || riskLessonsAvailable()) else { throw Wire.Problem.unavailable }
        guard ledger.scope == scope.origin.scope, pending.record.scope == ledger.scope,
              Wire.exact(witness.presentationCrew ?? witness.crewId,scope.origin.crew),
              witness.scope.dbId == scope.authority.dbId,
              try self.pending(scope).contains(where: { $0.id == pending.id && $0.witness == witness })
        else { throw Wire.Problem.stale }
        let params = try CrewdKnowledgeStore.encode(LookupParams(scope: witness.scope, crewId: witness.crewId,
                                                               requestId: witness.requestId))
        let result: Wire.Lookup = try await call("managedObject.request.get", params, scope.origin, stillCurrent)
        try await check(scope, stillCurrent)
        switch (result.status, result.receipt) {
        case ("notFound", nil):
            guard witness.scope == scope.authority else { throw Wire.Problem.invalid }
            return nil
        case ("historical", nil):
            guard witness.scope != scope.authority else { throw Wire.Problem.invalid }
            return nil
        case ("known", .some(let receipt)), ("historical", .some(let receipt)):
            guard (result.status == "known") == (witness.scope == scope.authority) else { throw Wire.Problem.invalid }
            try verify(receipt, witness)
            try ledger.acknowledge(pending.record, detail: "已核对组织目录原请求，未再次提交。")
            return .init(receipt: receipt, historical: result.status == "historical")
        default: throw Wire.Problem.invalid
        }
    }

    private struct LookupParams: Encodable {
        let scope: Wire.Scope
        let crewId: String
        let requestId: String
    }
    private func verify(_ receipt: Wire.Receipt, _ original: Witness) throws {
        guard receipt.scope == original.scope, Wire.exact(receipt.crewId, original.crewId),
              receipt.requestId == original.requestId, !receipt.recordedAt.isEmpty, receipt.object.valid
        else { throw Wire.Problem.invalid }
        if original.riskLesson != nil {
            try RiskLessonMutation.verify(receipt,original:original)
            return
        }
        guard receipt.occurrence == nil else { throw Wire.Problem.invalid }
        switch original.kind {
        case "createTable":
            guard let table = receipt.object.table, !table.frozen, Wire.exact(table.crewId, original.crewId),
                  try Mutation.definitionDigest(table.title, table.fields) == original.digest else { throw Wire.Problem.invalid }
        case "addRow", "editRow":
            guard let row = receipt.object.row, !row.frozen, Wire.exact(row.crewId, original.crewId),
                  row.tableId == original.tableId, try Mutation.digest(row.values) == original.digest
            else { throw Wire.Problem.invalid }
            if original.kind == "addRow" {
                guard row.revision == "1" else { throw Wire.Problem.invalid }
            } else {
                guard row.id == original.target, let originalRevision = original.expectedRevision.flatMap(Wire.positive),
                      originalRevision < Int64.max, row.revision == String(originalRevision + 1)
                else { throw Wire.Problem.invalid }
            }
        case "recordDecision", "replaceDecision":
            let prior = original.kind == "replaceDecision" ? original.target : nil
            guard let decision = receipt.object.decision, !decision.frozen, decision.status == .active,
                  Wire.exact(decision.crewId,original.crewId), decision.author.kind == "human",
                  decision.revision == "1", decision.supersedes == prior,
                  decision.createdAt == receipt.recordedAt, try Mutation.digest(decision.draft) == original.digest else { throw Wire.Problem.invalid }
        case "withdrawDecision":
            guard let decision = receipt.object.decision, !decision.frozen, decision.status == .withdrawn,
                  Wire.exact(decision.crewId,original.crewId), decision.id == original.target,
                  let revision = original.expectedRevision.flatMap(Wire.positive), revision < Int64.max,
                  decision.revision == String(revision + 1), let withdrawal = decision.withdrawal,
                  withdrawal.author.kind == "human", withdrawal.at == receipt.recordedAt,
                  try Mutation.withdrawalDigest(decision,reason:withdrawal.reason) == original.digest
            else { throw Wire.Problem.invalid }
        default: throw Wire.Problem.invalid
        }
    }
    private func valid(_ active: Origin, _ stillCurrent: @MainActor () -> Bool) -> Bool {
        !Task.isCancelled && stillCurrent() && origin() == active
    }
    private func call<T: Decodable>(_ method: String, _ value: CrewdValue, _ active: Origin,
                                    _ stillCurrent: @MainActor () -> Bool) async throws -> T {
        guard valid(active, stillCurrent) else { throw Wire.Problem.stale }
        let raw = try await rpc(method, value)
        guard valid(active, stillCurrent) else { throw Wire.Problem.stale }
        return try CrewdKnowledgeStore.decode(raw, T.self)
    }
    private func check(_ expected: BrowseScope, _ stillCurrent: @MainActor () -> Bool) async throws {
        guard valid(expected.origin, stillCurrent),
              try await scope(stillCurrent: stillCurrent) == expected else { throw Wire.Problem.stale }
    }
}

extension CrewdManagedDirectoryStore {
    enum RiskLessonMutation {
        case recordRisk(Wire.RiskDraft, Wire.DecisionAuthor)
        case updateRisk(Wire.Risk, Wire.RiskDraft)
        case assignRisk(Wire.Risk, Wire.DecisionAuthor)
        case reviewRisk(Wire.Risk)
        case setRiskStatus(Wire.Risk, Wire.RiskStatus, String)
        case occurrence(Wire.Risk, Wire.LessonDraft)
        case recordLesson(Wire.LessonDraft)
        case confirmLesson(Wire.Lesson)
        case staleLesson(Wire.Lesson, String)

        func prepare(scope: BrowseScope, request: String) throws -> (Wire.Change,Witness) {
            let selected = scope.origin.crew
            func prepared(_ change: Wire.Change, target: String, crew: String,
                          digest: String, reviewAt: String? = nil, checkedAt: String? = nil) -> (Wire.Change,Witness) {
                (change,Witness(scope:scope.authority,crewId:crew,requestId:request,kind:change.kind,target:target,
                    tableId:nil,expectedRevision:change.expectedRevision,digest:digest,
                    presentationCrew:Wire.exact(crew,selected) ? nil:selected,
                    riskLesson:.init(reviewAt:reviewAt,checkedAt:checkedAt)))
            }
            func risk(_ original: Wire.Risk, change: Wire.Change, expected: RiskFacts, reviewAt: String? = nil) throws -> (Wire.Change,Witness) {
                guard original.valid, !original.frozen, Wire.exact(original.crewId,selected) else { throw Wire.Problem.invalid }
                return prepared(change,target:original.id,crew:original.crewId,digest:try Mutation.digest(expected),reviewAt:reviewAt)
            }
            func manualLesson(_ draft: Wire.LessonDraft) -> Bool {
                draft.valid && !draft.learnedFrom.contains { $0.kind == .risk }
            }
            let human = Wire.DecisionAuthor(kind:"human",sessionId:nil)
            switch self {
            case .recordRisk(let draft,let owner):
                guard draft.valid, owner.valid else { throw Wire.Problem.invalid }
                let expected = RiskFacts(draft:draft,owner:owner,author:human,status:.open)
                return prepared(.init(kind:"recordRisk",owner:owner,riskDraft:draft),target:"recordRisk",
                                crew:selected,digest:try Mutation.digest(expected))
            case .updateRisk(let original,let draft):
                guard original.openForReview, draft.valid else { throw Wire.Problem.invalid }
                var expected = RiskFacts(original)
                expected.draft = draft
                return try risk(original,change:.init(kind:"updateRisk",expectedRevision:original.revision,
                    riskId:original.id,riskDraft:draft),expected:expected,
                    reviewAt:original.owner.kind == "human" ? nil:original.nextReviewAt)
            case .assignRisk(let original,let owner):
                guard original.openForReview, owner.valid, owner != original.owner else { throw Wire.Problem.invalid }
                var expected = RiskFacts(original)
                expected.owner = owner
                return try risk(original,change:.init(kind:"assignRisk",expectedRevision:original.revision,
                    riskId:original.id,owner:owner),expected:expected,reviewAt:original.nextReviewAt)
            case .reviewRisk(let original):
                guard original.openForReview, original.owner.kind == "human" else { throw Wire.Problem.invalid }
                return try risk(original,change:.init(kind:"reviewRisk",expectedRevision:original.revision,
                    riskId:original.id),expected:RiskFacts(original))
            case .setRiskStatus(let original,let status,let reason):
                guard original.canTransition(to:status), Wire.decisionText(reason) else { throw Wire.Problem.invalid }
                var expected = RiskFacts(original)
                expected.status = status
                expected.statusReason = reason
                return try risk(original,change:.init(kind:"setRiskStatus",expectedRevision:original.revision,
                    reason:reason,riskId:original.id,status:status),expected:expected,
                    reviewAt:status == .closed || original.owner.kind != "human" ? original.nextReviewAt:nil)
            case .occurrence(let original,var draft):
                guard original.valid, !original.frozen, original.openForReview,
                      Wire.exact(original.crewId,selected), manualLesson(draft), draft.learnedFrom.count < 8
                else { throw Wire.Problem.invalid }
                let change = Wire.Change(kind:"recordRiskOccurrence",expectedRevision:original.revision,
                                         riskId:original.id,lessonDraft:draft)
                draft.learnedFrom.append(.init(kind:.risk,id:original.id))
                let expected = LessonFacts(draft:draft,author:human,status:.active,riskId:original.id)
                return prepared(change,target:original.id,crew:selected,digest:try Mutation.digest(expected))
            case .recordLesson(let draft):
                guard manualLesson(draft) else { throw Wire.Problem.invalid }
                return prepared(.init(kind:"recordLesson",lessonDraft:draft),target:"recordLesson",crew:selected,
                                digest:try Mutation.digest(LessonFacts(draft:draft,author:human,status:.active)))
            case .confirmLesson(let original):
                guard original.valid, !original.frozen, original.status == .active else { throw Wire.Problem.invalid }
                return prepared(.init(kind:"confirmLesson",expectedRevision:original.revision,lessonId:original.id),
                                target:original.id,crew:original.crewId,digest:try Mutation.digest(LessonFacts(original)))
            case .staleLesson(let original,let reason):
                guard original.valid, !original.frozen, original.status == .active, Wire.exact(original.crewId,selected),
                      Wire.decisionText(reason) else { throw Wire.Problem.invalid }
                var expected = LessonFacts(original)
                expected.status = .stale
                expected.staleReason = reason
                return prepared(.init(kind:"staleLesson",expectedRevision:original.revision,reason:reason,lessonId:original.id),
                    target:original.id,crew:original.crewId,digest:try Mutation.digest(expected),checkedAt:original.lastCheckedAt)
            }
        }

        private struct RiskFacts: Encodable {
            var draft: Wire.RiskDraft
            var owner: Wire.DecisionAuthor
            var author: Wire.DecisionAuthor
            var status: Wire.RiskStatus
            var createdAt: String? = nil
            var statusReason: String? = nil
            var becameLessonId: String? = nil

            init(draft: Wire.RiskDraft, owner: Wire.DecisionAuthor, author: Wire.DecisionAuthor, status: Wire.RiskStatus) {
                self.draft = draft; self.owner = owner; self.author = author; self.status = status
            }
            init(_ item: Wire.Risk, created: Bool = false) {
                self.init(draft:item.draft,owner:item.owner,author:item.author,status:item.status)
                createdAt = created ? nil:item.createdAt
                statusReason = item.statusReason
                becameLessonId = item.becameLessonId
            }
        }
        private struct LessonFacts: Encodable {
            var draft: Wire.LessonDraft
            var author: Wire.DecisionAuthor
            var status: Wire.LessonStatus
            var createdAt: String? = nil
            var staleReason: String? = nil
            var riskId: String?

            init(draft: Wire.LessonDraft, author: Wire.DecisionAuthor, status: Wire.LessonStatus, riskId: String? = nil) {
                self.draft = draft; self.author = author; self.status = status; self.riskId = riskId
            }
            init(_ item: Wire.Lesson, created: Bool = false) {
                self.init(draft:item.draft,author:item.author,status:item.status,riskId:item.riskId)
                createdAt = created ? nil:item.createdAt
                staleReason = item.staleReason
            }
        }

        static func verify(_ receipt: Wire.Receipt, original: Witness) throws {
            guard let witness = original.riskLesson else { throw Wire.Problem.invalid }
            let revision = original.expectedRevision.flatMap(Wire.positive).flatMap { $0 < Int64.max ? String($0 + 1):nil }
            switch original.kind {
            case "recordRisk", "updateRisk", "assignRisk", "reviewRisk", "setRiskStatus":
                let created = original.kind == "recordRisk"
                guard receipt.occurrence == nil, let risk = receipt.object.risk, !risk.frozen,
                      Wire.exact(risk.crewId,original.crewId), risk.updatedAt == receipt.recordedAt,
                      created ? (risk.revision == "1" && risk.createdAt == receipt.recordedAt) :
                        (risk.id == original.target && risk.revision == revision),
                      try Mutation.digest(RiskFacts(risk,created:created)) == original.digest
                else { throw Wire.Problem.invalid }
                if let reviewAt = witness.reviewAt {
                    guard risk.nextReviewAt == reviewAt else { throw Wire.Problem.invalid }
                } else {
                    guard let recorded = Wire.managedTime(receipt.recordedAt), let review = Wire.managedTime(risk.nextReviewAt),
                          abs(review.timeIntervalSince(recorded) - 7 * 24 * 60 * 60) < 0.01
                    else { throw Wire.Problem.invalid }
                }
            case "recordLesson", "confirmLesson", "staleLesson", "recordRiskOccurrence":
                let occurrence = original.kind == "recordRiskOccurrence"
                let created = occurrence || original.kind == "recordLesson"
                guard let lesson = receipt.object.lesson, !lesson.frozen, Wire.exact(lesson.crewId,original.crewId),
                      created ? (lesson.revision == "1" && lesson.createdAt == receipt.recordedAt) :
                        (lesson.id == original.target && lesson.revision == revision),
                      lesson.lastCheckedAt == (witness.checkedAt ?? receipt.recordedAt),
                      try Mutation.digest(LessonFacts(lesson,created:created)) == original.digest
                else { throw Wire.Problem.invalid }
                if occurrence {
                    guard let effect = receipt.occurrence, effect.riskId == original.target,
                          effect.originalRiskRevision == original.expectedRevision, effect.resultRiskRevision == revision,
                          effect.lessonId == lesson.id, effect.lessonRevision == lesson.revision, lesson.riskId == original.target
                    else { throw Wire.Problem.invalid }
                } else if receipt.occurrence != nil { throw Wire.Problem.invalid }
            default: throw Wire.Problem.invalid
            }
        }
    }
}


extension CrewdManagedDirectoryStore {
    typealias MessageWire = CrewdManagedMessageWire
    struct MessageWitness: Codable {
        let original: Witness
        let sourceMessageId: String
        let key: String
        let draft: CrewdValue
        let table: Wire.Table?
    }
    struct MessagePending: Identifiable {
        let record: CrewdPresentationJournal.Prepared
        let witness: MessageWitness
        var id: UUID { record.id }
    }
    struct MessageObservation {
        let receipt: MessageWire.Receipt
        let historical: Bool
    }
    var supportsMessageSources: Bool { messageSourcesAvailable() }

    func messageSource(_ scope: BrowseScope, id: String,
                       stillCurrent: @MainActor () -> Bool) async throws -> MessageWire.Source {
        guard supportsMessageSources else { throw Wire.Problem.unavailable }
        guard valid(scope.origin,stillCurrent) else { throw Wire.Problem.stale }
        let raw = try await rpc(MessageWire.source,.object(["id":.string(id)]))
        try await check(scope,stillCurrent)
        return try .init(raw)
    }

    func messageObjects(_ scope: BrowseScope, message: String, after: String?,
                        stillCurrent: @MainActor () -> Bool) async throws -> MessageWire.Page {
        guard supportsMessageSources, !message.isEmpty else { throw Wire.Problem.unavailable }
        var params: [String:CrewdValue] = ["messageId":.string(message)]
        if let after { params["after"] = .string(after) }
        let page: MessageWire.Page = try await call(MessageWire.objects,.object(params),scope.origin,stillCurrent)
        let ids = page.items.compactMap(\.id)
        guard page.items.count <= 16, ids.count == page.items.count, Set(ids).count == ids.count,
              page.items.allSatisfy(\.valid), page.nextAfter != after || after == nil,
              page.nextAfter == nil || (!page.items.isEmpty && page.nextAfter?.isEmpty == false)
        else { throw Wire.Problem.invalid }
        try await check(scope,stillCurrent)
        return page
    }

    func messagePending(_ scope: BrowseScope, message: String? = nil) throws -> [MessagePending] {
        let ledger = try journal()
        guard ledger.scope == scope.origin.scope, valid(scope.origin,{ true }) else { throw Wire.Problem.stale }
        return try ledger.messageSourceHistory().filter {
            $0.method == MessageWire.observation && $0.scope == ledger.scope &&
                Wire.exact($0.crew,scope.origin.crew)
        }.map { record in
            let witness = try CrewdKnowledgeStore.decode(record.params,MessageWitness.self)
            guard witness.original.scope.dbId == scope.authority.dbId,
                  Wire.exact(witness.original.crewId,record.crew),
                  ["recordDecision","recordRisk","recordLesson","addRow"].contains(witness.original.kind),
                  UUID(uuidString:witness.original.requestId)?.uuidString.lowercased() == witness.original.requestId,
                  !witness.sourceMessageId.isEmpty
            else { throw Wire.Problem.invalid }
            return .init(record:record,witness:witness)
        }.filter { item in
            guard let message else { return true }
            return Wire.exact(item.witness.sourceMessageId,message)
        }
    }

    func recordFromMessage(_ mutation: Mutation, message: String, scope: BrowseScope,
                           stillCurrent: @MainActor () -> Bool) async throws -> MessageObservation {
        guard supportsMessageSources, !message.isEmpty else { throw Wire.Problem.unavailable }
        try await check(scope,stillCurrent)
        let request = UUID().uuidString.lowercased()
        let change: Wire.Change
        let original: Witness
        var table: Wire.Table?
        switch mutation {
        case .recordDecision, .add:
            let encoded = try mutation.encode(crew:scope.origin.crew,risksAvailable:riskLessonsAvailable())
            change = encoded.0
            original = .init(scope:scope.authority,crewId:scope.origin.crew,requestId:request,kind:change.kind,
                             target:encoded.1,tableId:encoded.2,expectedRevision:encoded.3,digest:encoded.4)
            if case .add(let selected,_) = mutation { table = selected }
        case .riskLesson(let operation):
            switch operation {
            case .recordRisk, .recordLesson: break
            default: throw Wire.Problem.invalid
            }
            (change,original) = try operation.prepare(scope:scope,request:request)
        default: throw Wire.Problem.invalid
        }
        let witness = MessageWitness(original:original,sourceMessageId:message,
            key:change.kind + ":" + (original.tableId ?? ""),
            draft:try CrewdKnowledgeStore.encode(change),table:table)
        let params = try CrewdKnowledgeStore.encode(MessageWire.Record(scope:scope.authority,crewId:original.crewId,
            requestId:request,sourceMessageId:message,change:change))
        let ledger = try journal()
        guard ledger.scope == scope.origin.scope, valid(scope.origin,stillCurrent) else { throw Wire.Problem.stale }
        let prepared = try ledger.prepareMessageSource(CrewdKnowledgeStore.encode(witness),crew:original.crewId)
        do {
            let receipt: MessageWire.Receipt = try await call(MessageWire.record,params,scope.origin,stillCurrent)
            try verifyMessageReceipt(receipt,witness:witness)
            try await check(scope,stillCurrent)
            try ledger.finish(prepared,state:"replyObserved")
            return .init(receipt:receipt,historical:false)
        } catch {
            let failed = error as? CrewdClientError
            let refused = failed?.kind == .notSubmitted || Self.decisionInputRefused(error) ||
                (failed?.code == -32009 && failed?.refusal == "resource.frozen")
            try? ledger.finish(prepared,state:refused ? "refused":"unknown")
            throw error
        }
    }

    func lookupMessage(_ pending: MessagePending, scope: BrowseScope,
                       stillCurrent: @MainActor () -> Bool) async throws -> MessageObservation? {
        guard supportsMessageSources else { throw Wire.Problem.unavailable }
        let ledger = try journal(), original = pending.witness.original
        guard ledger.scope == scope.origin.scope, pending.record.scope == ledger.scope,
              try messagePending(scope).contains(where:{ $0.id == pending.id })
        else { throw Wire.Problem.stale }
        let result: MessageWire.Lookup = try await call(MessageWire.lookup,
            CrewdKnowledgeStore.encode(LookupParams(scope:original.scope,crewId:original.crewId,requestId:original.requestId)),
            scope.origin,stillCurrent)
        try await check(scope,stillCurrent)
        switch (result.status,result.receipt) {
        case ("known",.some(let receipt)), ("historical",.some(let receipt)):
            guard (result.status == "known") == (original.scope == scope.authority) else { throw Wire.Problem.invalid }
            try verifyMessageReceipt(receipt,witness:pending.witness)
            try await check(scope,stillCurrent)
            try ledger.acknowledge(pending.record,detail:"已核对消息记录的原请求，未重新提交。")
            return .init(receipt:receipt,historical:result.status == "historical")
        case ("notFound",nil) where original.scope == scope.authority: return nil
        case ("historical",nil) where original.scope != scope.authority: return nil
        default: throw Wire.Problem.invalid
        }
    }

    private func verifyMessageReceipt(_ receipt: MessageWire.Receipt, witness: MessageWitness) throws {
        let original = witness.original
        guard receipt.scope == original.scope, Wire.exact(receipt.crewId,original.crewId),
              receipt.requestId == original.requestId, Wire.exact(receipt.sourceMessageId,witness.sourceMessageId),
              !receipt.recordedAt.isEmpty, receipt.object.valid else { throw Wire.Problem.invalid }
        let expected: String
        switch original.kind {
        case "recordDecision": expected = "decision"
        case "recordRisk": expected = "risk"
        case "recordLesson": expected = "lesson"
        case "addRow": expected = "row"
        default: throw Wire.Problem.invalid
        }
        guard receipt.object.kind == expected,
              original.kind != "addRow" || receipt.object.row?.tableId == original.tableId else { throw Wire.Problem.invalid }
        if receipt.created {
            try verify(.init(scope:receipt.scope,crewId:receipt.crewId,requestId:receipt.requestId,
                recordedAt:receipt.recordedAt,object:receipt.object,occurrence:nil),original)
        }
    }

    func messageDraft(_ pending: MessagePending) throws -> Mutation {
        let draft = pending.witness.draft
        switch pending.witness.original.kind {
        case "recordDecision":
            return .recordDecision(try CrewdKnowledgeStore.decode(draft["draft"],Wire.DecisionDraft.self))
        case "recordRisk":
            return .riskLesson(.recordRisk(try CrewdKnowledgeStore.decode(draft["draft"],Wire.RiskDraft.self),
                try CrewdKnowledgeStore.decode(draft["owner"],Wire.DecisionAuthor.self)))
        case "recordLesson":
            return .riskLesson(.recordLesson(try CrewdKnowledgeStore.decode(draft["draft"],Wire.LessonDraft.self)))
        case "addRow":
            guard let table = pending.witness.table else { throw Wire.Problem.invalid }
            return .add(table,try CrewdKnowledgeStore.decode(draft["values"],[Wire.Cell].self))
        default: throw Wire.Problem.invalid
        }
    }
}

/// Core-owned natural keys. No filesystem lookup or client-side normalization.
@MainActor final class CrewdArtifactStore:ObservableObject {
    typealias Wire=CrewdArtifactWire
    typealias Origin=CrewdKnowledgeStore.Origin
    typealias Scope=CrewdManagedDirectoryStore.BrowseScope
    typealias Problem=CrewdManagedDirectoryWire.Problem
    private let origin:()->Origin?
    private let rpc:CrewdKnowledgeStore.RPC
    private let journal:()throws->CrewdPresentationJournal
    private let available:()->Bool
    private let historyAvailable:()->Bool
    @Published private(set) var historyInvalidation=UUID()
    @Published private(set) var invalidation=UUID()
    init(origin:@escaping ()->Origin?,rpc:@escaping CrewdKnowledgeStore.RPC,journal:@escaping ()throws->CrewdPresentationJournal,available:@escaping ()->Bool,historyAvailable:@escaping ()->Bool = {false}) {
        self.origin=origin;self.rpc=rpc;self.journal=journal;self.available=available;self.historyAvailable=historyAvailable
    }
    var supported:Bool {available()}
    var historySupported:Bool {supported && historyAvailable()}
    func owns(_ scope:Scope)->Bool {supported && origin()==scope.origin}
    func handleEvent(_ name:String,_ data:CrewdValue,connection:UUID) {
        guard let active=origin(),active.connection==connection else {return}
        if name=="artifact.historyChanged" {
            guard historySupported,case .object(let fields)=data,Set(fields.keys)==["crewId","jobId"],
                  data["crewId"].string?.isEmpty==false,data["jobId"].string.map(CrewdArtifactHistoryWire.uuid)==true else {return}
            historyInvalidation=UUID();return
        }
        if ["crew.changed","crew.purged","events.lagged"].contains(name) {historyInvalidation=UUID()}
        if name=="artifact.changed" {
            guard case .object(let fields)=data,Set(fields.keys)==["crewId"],data["crewId"].string?.isEmpty==false else {return}
        } else if !["onto.changed","work.changed","crew.changed","crew.purged","events.lagged"].contains(name) {return}
        invalidation=UUID()
    }
    private func valid(_ active:Origin,_ current:@MainActor ()->Bool)->Bool {supported && current() && origin()==active && !Task.isCancelled}
    private func call<T:Decodable>(_ method:String,_ params:CrewdValue,_ scope:Scope,_ current:@MainActor ()->Bool) async throws->T {
        guard valid(scope.origin,current) else {throw Problem.stale}
        let raw=try await rpc(method,params)
        guard valid(scope.origin,current) else {throw Problem.stale}
        return try CrewdKnowledgeStore.decode(raw,T.self)
    }
    func scope(stillCurrent:@MainActor ()->Bool) async throws->Scope {
        guard supported,let active=origin(),valid(active,stillCurrent) else {throw Problem.unavailable}
        let raw=try await rpc("managedObject.scope",.object([:]))
        guard valid(active,stillCurrent) else {throw Problem.stale}
        let authority=try CrewdKnowledgeStore.decode(raw,CrewdManagedDirectoryWire.Scope.self)
        guard authority.dbId==active.scope.databaseID,!authority.restoreEpoch.isEmpty else {throw Problem.invalid}
        return .init(origin:active,authority:authority)
    }
    private func check(_ expected:Scope,_ current:@MainActor ()->Bool) async throws {
        guard try await scope(stillCurrent:current)==expected else {throw Problem.stale}
    }
    func preview(_ input:Wire.Input,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->Wire.Reference {
        let params:CrewdValue = .object(["crewId":.string(scope.origin.crew),"input":try CrewdKnowledgeStore.encode(input)])
        let reference:Wire.Reference=try await call(Wire.preview,params,scope,stillCurrent)
        guard reference.valid,reference.namespace.kind != "unassigned" || reference.namespace.crewId?.utf8.elementsEqual(scope.origin.crew.utf8)==true else {throw Problem.invalid}
        try await check(scope,stillCurrent);return reference
    }
    func list(_ scope:Scope,after:Wire.Reference?,stillCurrent:@MainActor ()->Bool) async throws->Wire.Page {
        let p=try after.map(CrewdKnowledgeStore.encode) ?? .null
        let page:Wire.Page=try await call(Wire.list,.object(["after":p]),scope,stillCurrent)
        guard page.items.count<=16,page.items.allSatisfy(\.valid),Set(page.items.map(\.id)).count==page.items.count,
              page.nextAfter==nil || (page.items.last?.reference==page.nextAfter && page.nextAfter != after) else {throw Problem.invalid}
        try await check(scope,stillCurrent);return page
    }
    func get(_ reference:Wire.Reference,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->Wire.Artifact {
        let item:Wire.Artifact=try await call(Wire.get,.object(["reference":try CrewdKnowledgeStore.encode(reference)]),scope,stillCurrent)
        guard item.valid,item.reference==reference else {throw Problem.invalid};try await check(scope,stillCurrent);return item
    }
    func edges(_ scope:Scope,reference:Wire.Reference?=nil,subject:Wire.Subject?=nil,after:String?,stillCurrent:@MainActor ()->Bool) async throws->Wire.EdgePage {
        guard (reference != nil) != (subject != nil) else {throw Problem.invalid}
        var p:[String:CrewdValue]=["after":after.map(CrewdValue.string) ?? .null]
        if let reference {p["reference"]=try CrewdKnowledgeStore.encode(reference)}
        if let subject {p["subject"]=try CrewdKnowledgeStore.encode(subject)}
        if historySupported {p["includeLegacySources"] = .bool(true)}
        let page:Wire.EdgePage=try await call(reference==nil ? Wire.subject:Wire.backlinks,.object(p),scope,stillCurrent)
        guard page.items.count<=16,page.items.allSatisfy({$0.valid && (!$0.origin.legacy || historySupported) && (reference==nil || $0.reference==reference) && (subject==nil || $0.subject==subject)}),
              Set(page.items.map(\.id)).count==page.items.count,
              page.nextAfter==nil || (page.nextAfter==page.items.last?.id && page.nextAfter != after) else {throw Problem.invalid}
        try await check(scope,stillCurrent);return page
    }
    struct Witness:Codable {let scope:CrewdManagedDirectoryWire.Scope;let crewId:String;let requestId:String;let change:Wire.Change;let reference:Wire.Reference;let target:String}
    struct Pending:Identifiable {let record:CrewdPresentationJournal.Prepared;let witness:Witness;var id:UUID {record.id}}
    struct Observation {let receipt:Wire.Receipt;let historical:Bool}
    func pending(_ scope:Scope)throws->[Pending] {
        guard valid(scope.origin,{true}) else {throw Problem.stale}
        let ledger=try journal();guard ledger.scope==scope.origin.scope else {throw Problem.stale}
        return try ledger.unknownRecords().filter{$0.method==Wire.observation && $0.scope==ledger.scope && $0.crew.utf8.elementsEqual(scope.origin.crew.utf8)}.map {record in
            let witness=try CrewdKnowledgeStore.decode(record.params,Witness.self)
            guard witness.crewId.utf8.elementsEqual(record.crew.utf8),witness.scope.dbId==scope.authority.dbId,witness.change.valid,witness.reference.valid,
                  UUID(uuidString:witness.requestId)?.uuidString.lowercased()==witness.requestId else {throw Problem.invalid}
            return .init(record:record,witness:witness)
        }
    }
    func submit(_ change:Wire.Change,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->Observation {
        guard change.valid else {throw Problem.invalid}
        try await check(scope,stillCurrent)
        let reference:Wire.Reference
        if let input=change.input {reference=try await preview(input,scope:scope,stillCurrent:stillCurrent)}
        else if let ref=change.reference {
            if change.kind=="linkExisting" {reference=try await get(ref,scope:scope,stillCurrent:stillCurrent).reference}
            else {reference=ref}
        } else {throw Problem.invalid}
        if change.relation == .appliesTo,!reference.key.hasPrefix("file:") {throw Problem.invalid}
        let id=UUID().uuidString.lowercased()
        let target=reference.id+":"+(change.subject?.kind.rawValue ?? "register")+":"+(change.subject?.id ?? "")+":"+(change.relation?.rawValue ?? "")
        let witness=Witness(scope:scope.authority,crewId:scope.origin.crew,requestId:id,change:change,reference:reference,target:target)
        let ledger=try journal();guard ledger.scope==scope.origin.scope,valid(scope.origin,stillCurrent) else {throw Problem.stale}
        let prepared=try ledger.prepareArtifact(CrewdKnowledgeStore.encode(witness),crew:witness.crewId)
        do {
            let receipt:Wire.Receipt=try await call(Wire.submit,CrewdKnowledgeStore.encode(Wire.Submit(scope:witness.scope,crewId:witness.crewId,requestId:id,change:change)),scope,stillCurrent)
            try Self.verify(receipt,witness);try await check(scope,stillCurrent)
            try ledger.finish(prepared,state:"replyObserved")
            return .init(receipt:receipt,historical:false)
        } catch {
            let e=error as? CrewdClientError
            let refused=e?.kind == .notSubmitted || CrewdManagedDirectoryStore.decisionInputRefused(error) || (e?.code == -32009 && ["resource.frozen","artifact.edge_changed"].contains(e?.refusal ?? ""))
            try? ledger.finish(prepared,state:refused ? "refused":"unknown");throw error
        }
    }
    func lookup(_ pending:Pending,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->Observation? {
        guard try self.pending(scope).contains(where:{$0.id==pending.id && Wire.exact($0.witness,pending.witness)}) else {throw Problem.stale}
        let w=pending.witness
        let raw:CrewdValue = .object(["scope":try CrewdKnowledgeStore.encode(w.scope),"crewId":.string(w.crewId),"requestId":.string(w.requestId)])
        let result:Wire.Lookup=try await call(Wire.lookup,raw,scope,stillCurrent);try await check(scope,stillCurrent)
        switch (result.status,result.receipt) {
        case ("known",.some(let receipt)),("historical",.some(let receipt)):
            guard (result.status=="known")==Wire.exact(w.scope,scope.authority) else {throw Problem.invalid}
            try Self.verify(receipt,w);try await check(scope,stillCurrent)
            try journal().acknowledge(pending.record,detail:"已核对产物原请求，未重新提交。")
            return .init(receipt:receipt,historical:result.status=="historical")
        case ("notFound",nil) where Wire.exact(w.scope,scope.authority):return nil
        case ("historical",nil) where !Wire.exact(w.scope,scope.authority):return nil
        default:throw Problem.invalid
        }
    }
    private static func verify(_ r:Wire.Receipt,_ w:Witness)throws {
        guard Wire.exact(r.scope,w.scope),r.crewId.utf8.elementsEqual(w.crewId.utf8),r.requestId==w.requestId,
              Wire.exact(r.change,w.change),r.change.valid,r.artifact.valid,r.artifact.reference==w.reference,!r.recordedAt.isEmpty else {throw Problem.invalid}
        let e=r.effect
        guard e.created != true || r.artifact.createdAt.utf8.elementsEqual(r.recordedAt.utf8) else {throw Problem.invalid}
        switch w.change.kind {
        case "register":guard e.kind=="registered",e.created != nil,e.claimAdded != nil,e.added==nil,e.edge==nil,e.edgeId==nil else {throw Problem.invalid}
        case "link","linkExisting":
            guard w.change.kind != "linkExisting" || e.created==false else {throw Problem.invalid}
            guard e.kind=="linked",e.created != nil,e.added != nil,e.claimAdded==nil,e.edgeId==nil,let edge=e.edge,edge.valid,
                  edge.reference==w.reference,edge.subject==w.change.subject,edge.relation==w.change.relation,
                  edge.crewId.utf8.elementsEqual(w.crewId.utf8),e.added==false || (edge.origin.manual && edge.origin.actor?.kind=="human") else {throw Problem.invalid}
        case "unlink":guard e.kind=="unlinked",e.edgeId==w.change.expectedEdgeId,e.edge==nil,e.created==nil,e.added==nil,e.claimAdded==nil else {throw Problem.invalid}
        default:throw Problem.invalid
        }
    }
}

extension CrewdArtifactStore {
    func readSubject(_ edge:Wire.Edge,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->CrewdValue {
        guard edge.valid,valid(scope.origin,stillCurrent) else {throw Problem.stale}
        let s=edge.subject
        let method=s.kind == .work ? "work.get":s.kind == .requirement ? "todo.get":"managedObject.get"
        var p:[String:CrewdValue]=["id":.string(s.id)]
        if method=="managedObject.get" {p["includeSystemDefaults"] = .bool(true)}
        let raw=try await rpc(method,.object(p));try await check(scope,stillCurrent)
        if method=="managedObject.get" {
            let object=try CrewdKnowledgeStore.decode(raw,CrewdManagedDirectoryWire.Object.self)
            guard object.valid,object.id==s.id,object.kind==s.kind.rawValue,
                  raw[s.kind.rawValue]["crewId"].string?.isEmpty==false else {throw Problem.invalid}
        } else {
            let key=s.kind == .work ? "id":"requirementId"
            guard raw[key].string==s.id,raw["crewId"].string?.isEmpty==false else {throw Problem.invalid}
        }
        return raw
    }
}

extension CrewdArtifactStore {
    typealias History=CrewdArtifactHistoryWire
    struct HistoryWitness:Codable {
        let scope:History.Scope
        let crewId:String
        let requestId:String
        let operation:History.Operation
        let before:History.Job?
        let target:String
    }
    struct HistoryPending:Identifiable {let record:CrewdPresentationJournal.Prepared;let witness:HistoryWitness;var id:UUID {record.id}}
    struct HistoryObservation {let receipt:History.Receipt;let historical:Bool}
    func historyCrews(_ scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->[CrewdValue] {
        guard historySupported,valid(scope.origin,stillCurrent) else {throw Problem.unavailable}
        let raw=try await rpc("crew.list",.object(["all":.bool(true)]));try await check(scope,stillCurrent)
        guard case .array(let rows)=raw else {throw Problem.invalid}
        let ids=rows.compactMap{$0["id"].string}
        guard ids.count==rows.count,ids.allSatisfy({!$0.isEmpty}),Set(ids.map{Data($0.utf8)}).count==ids.count else {throw Problem.invalid}
        return rows
    }
    func historyStatus(_ crew:String,job:String?=nil,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->History.Status {
        guard historySupported,!crew.isEmpty,job.map(History.uuid) ?? true else {throw Problem.unavailable}
        var p:[String:CrewdValue]=["crewId":.string(crew)]
        if let job {p["jobId"] = .string(job)}
        let status:History.Status=try await call(History.status,.object(p),scope,stillCurrent)
        guard Wire.exact(status.scope,scope.authority),status.crewId.utf8.elementsEqual(crew.utf8) else {throw Problem.invalid}
        if let actual=status.job {
            guard actual.valid,actual.crewId.utf8.elementsEqual(crew.utf8),actual.scope.dbId==scope.authority.dbId,
                  job==nil || actual.id==job else {throw Problem.invalid}
        }
        let continuable=status.job.map{Wire.exact($0.scope,scope.authority) && $0.cursor.phase != .complete} ?? false
        guard status.canContinue==continuable else {throw Problem.invalid}
        try await check(scope,stillCurrent);return status
    }
    func historyPending(_ scope:Scope,crew:String?=nil)throws->[HistoryPending] {
        guard historySupported,valid(scope.origin,{true}) else {throw Problem.unavailable}
        let ledger=try journal();guard ledger.scope==scope.origin.scope else {throw Problem.stale}
        return try ledger.unknownRecords().filter{$0.method==History.observation && $0.scope==ledger.scope &&
            (crew==nil || $0.crew.utf8.elementsEqual(crew!.utf8))}.map {record in
            let w=try CrewdKnowledgeStore.decode(record.params,HistoryWitness.self)
            guard w.scope.dbId==scope.authority.dbId,w.crewId.utf8.elementsEqual(record.crew.utf8),History.uuid(w.requestId),
                  w.operation.valid,!w.target.isEmpty else {throw Problem.invalid}
            if let before=w.before {
                guard before.valid,Wire.exact(before.scope,w.scope),before.crewId.utf8.elementsEqual(w.crewId.utf8) else {throw Problem.invalid}
            }
            return .init(record:record,witness:w)
        }
    }
    func beginHistory(_ crew:String,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->HistoryObservation {
        try await historyWrite(.init(kind:"begin"),crew:crew,before:nil,scope:scope,stillCurrent:stillCurrent)
    }
    func stepHistory(_ job:History.Job,limit:Int,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->HistoryObservation {
        guard job.valid,Wire.exact(job.scope,scope.authority),job.cursor.phase != .complete,(1...64).contains(limit) else {throw Problem.stale}
        return try await historyWrite(.init(kind:"step",jobId:job.id,expectedRevision:job.revision,expectedCursor:job.cursor,limit:limit),crew:job.crewId,before:job,scope:scope,stillCurrent:stillCurrent)
    }
    private func historyWrite(_ operation:History.Operation,crew:String,before:History.Job?,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->HistoryObservation {
        guard historySupported,operation.valid,!crew.isEmpty else {throw Problem.unavailable}
        try await check(scope,stillCurrent)
        let uuid=UUID().uuidString.lowercased()
        let target=(try Wire.bytes(scope.authority).base64EncodedString())+":"+crew
        let w=HistoryWitness(scope:scope.authority,crewId:crew,requestId:uuid,operation:operation,before:before,target:target)
        let params:CrewdValue
        if operation.kind=="begin" {params=try CrewdKnowledgeStore.encode(History.BeginParams(scope:w.scope,crewId:crew,requestId:uuid))}
        else {params=try CrewdKnowledgeStore.encode(History.StepParams(scope:w.scope,crewId:crew,requestId:uuid,jobId:operation.jobId!,expectedRevision:operation.expectedRevision!,expectedCursor:operation.expectedCursor!,limit:operation.limit!))}
        let ledger=try journal();guard ledger.scope==scope.origin.scope,valid(scope.origin,stillCurrent) else {throw Problem.stale}
        let prepared=try ledger.prepareArtifactHistory(CrewdKnowledgeStore.encode(w),crew:crew)
        do {
            let receipt:History.Receipt=try await call(operation.kind=="begin" ? History.begin:History.step,params,scope,stillCurrent)
            try Self.verifyHistory(receipt,w);try await check(scope,stillCurrent)
            try ledger.finish(prepared,state:"replyObserved")
            return .init(receipt:receipt,historical:false)
        } catch {
            let e=error as? CrewdClientError
            let refused=e?.kind == .notSubmitted || CrewdManagedDirectoryStore.decisionInputRefused(error) ||
                (e?.code == -32009 && ["resource.frozen","artifact.history_changed"].contains(e?.refusal ?? ""))
            try? ledger.finish(prepared,state:refused ? "refused":"unknown");throw error
        }
    }
    func lookupHistory(_ pending:HistoryPending,scope:Scope,stillCurrent:@MainActor ()->Bool) async throws->HistoryObservation? {
        guard historySupported,try historyPending(scope).contains(where:{$0.id==pending.id && Wire.exact($0.witness,pending.witness)}) else {throw Problem.stale}
        let w=pending.witness
        let p:CrewdValue = .object(["scope":try CrewdKnowledgeStore.encode(w.scope),"crewId":.string(w.crewId),"requestId":.string(w.requestId)])
        let result:History.Lookup=try await call(History.lookup,p,scope,stillCurrent);try await check(scope,stillCurrent)
        switch (result.status,result.receipt) {
        case ("known",.some(let receipt)),("historical",.some(let receipt)):
            guard (result.status=="known")==Wire.exact(w.scope,scope.authority) else {throw Problem.invalid}
            try Self.verifyHistory(receipt,w);try await check(scope,stillCurrent)
            try journal().acknowledge(pending.record,detail:"已核对历史凭据索引原请求；未重发、未继续下一页。")
            return .init(receipt:receipt,historical:result.status=="historical")
        case ("notFound",nil) where Wire.exact(w.scope,scope.authority):return nil
        case ("historical",nil) where !Wire.exact(w.scope,scope.authority):return nil
        default:throw Problem.invalid
        }
    }
    private static func verifyHistory(_ r:History.Receipt,_ w:HistoryWitness)throws {
        guard Wire.exact(r.scope,w.scope),r.crewId.utf8.elementsEqual(w.crewId.utf8),r.requestId==w.requestId,
              Wire.exact(r.operation,w.operation),r.operation.valid,!r.recordedAt.isEmpty,r.job.valid,
              Wire.exact(r.job.scope,w.scope),r.job.crewId.utf8.elementsEqual(w.crewId.utf8) else {throw Problem.invalid}
        let effect=r.effect
        if w.operation.kind=="begin" {
            guard effect.kind=="begun",effect.created != nil,effect.beforeRevision==nil,effect.beforeCursor==nil,effect.delta==nil,w.before==nil else {throw Problem.invalid}
            if effect.created==true {
                guard r.job.revision=="1",r.job.counts.zero,r.job.cursor == .init(phase:.legacyEvents,after:"0"),
                      r.job.lastRequestId==r.requestId else {throw Problem.invalid}
            } else {guard r.job.cursor.phase != .complete else {throw Problem.invalid}}
        } else {
            guard let before=w.before,before.valid,before.cursor.phase != .complete,let revision=w.operation.expectedRevision.flatMap(History.number),revision<UInt64.max,
                  r.job.id==w.operation.jobId,before.id==r.job.id,before.revision==w.operation.expectedRevision,r.job.lastRequestId==r.requestId,
                  effect.kind=="advanced",effect.created==nil,effect.beforeRevision==w.operation.expectedRevision,
                  effect.beforeCursor==w.operation.expectedCursor,before.cursor==w.operation.expectedCursor,
                  let delta=effect.delta,delta.valid,History.number(delta.scanned)!<=UInt64(w.operation.limit!),
                  let total=before.counts.adding(delta),Wire.exact(total,r.job.counts),r.job.revision==String(revision+1),
                  Wire.exact(before.cut,r.job.cut),before.createdAt.utf8.elementsEqual(r.job.createdAt.utf8),
                  r.job.cursor.phase.rank>=before.cursor.phase.rank,
                  r.job.cursor.phase != before.cursor.phase || History.number(r.job.cursor.after)!>History.number(before.cursor.after)! else {throw Problem.invalid}
        }
    }
}
