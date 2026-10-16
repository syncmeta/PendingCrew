import Foundation
import Combine
import CryptoKit

/// SQLite authority client. This model never opens the knowledge database locally.
@MainActor final class CrewdKnowledgeStore: ObservableObject, CrewdKnowledgeViewAuthority {
    struct Origin: Equatable {
        let scope: CrewdPresentationJournal.Scope
        let observation: UUID
        let connection: UUID
        let crew: String
    }
    struct Intent: Codable, Equatable, Identifiable {
        let id: UUID
        let context: CrewdDataContext
        let method: String? // operation identity only; never a replay payload
        let changeDigest: String // no replay payload or view code in the observation journal
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
    @Published private(set) var nextObjectPage: String?
    private(set) var detailIsSnapshot = false
    private(set) var detailReference: CrewdDataRef?
    var canEditDetail: Bool { !detailIsSnapshot && detailReference?.namespace == workspace?.context.namespace }
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
        pending = []; result = nil; busy = false; records = [:]; nextObjectPage = nil
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
        history = nil; snapshots = nil; preview = nil; nextObjectPage = nil
    }
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
    private func begin() throws -> (Origin, UUID) {
        guard !busy, let o = origin(), !o.crew.isEmpty else { throw Failure.unavailable }
        if active != o {
            invalidate()
            let stored = try journal().unknownRecords().filter { $0.method == "data.submit.observation" && $0.scope == o.scope && $0.crew == o.crew }
            for record in stored {
                let intent = try Self.decode(record.params, Intent.self)
                guard intent.context.crewId == o.crew, intent.context.dbId == o.scope.databaseID,
                      intent.context.authorityId == o.scope.authorityID, records[intent.id] == nil else { throw Failure.invalid }
                records[intent.id] = record; pending.append(intent)
            }
            active = o
        }
        let token = UUID(); operation = token; busy = true; return (o, token)
    }
    private func finish(_ o: Origin, _ token: UUID) {
        guard active == o, operation == token else { return }
        if origin() == o { busy = false } else { invalidate() }
    }
    private func checked(_ context: CrewdDataContext, _ o: Origin) throws {
        guard context.dbId == o.scope.databaseID, context.authorityId == o.scope.authorityID,
              context.crewId == o.crew, !context.namespace.isEmpty, !context.restoreEpoch.isEmpty, !context.generation.isEmpty else { throw Failure.invalid }
    }
    private func sameAuthority(_ a: CrewdDataContext, _ b: CrewdDataContext) -> Bool {
        a.dbId == b.dbId && a.authorityId == b.authorityId && a.restoreEpoch == b.restoreEpoch && a.crewId == b.crewId && a.namespace == b.namespace
    }
    func refresh() async {
        let o: Origin, token: UUID
        do { (o, token) = try begin() } catch { status = "知识数据或原提交历史当前不可用。"; return }
        defer { finish(o, token) }
        do {
            let info = try Self.decode(try await perform("data.workspace.get", Self.encode(CrewdDataWorkspaceGetParams(crewId: o.crew))), CrewdDataWorkspaceInfo.self)
            try checked(info.context, o)
            guard current(o, token) else { return }
            if let old = workspace, old.context != info.context { renderer.revoke(); detail = nil; objects = [] }
            workspace = info
            guard info.state == "ready" else { status = info.state == "uninitialized" ? "知识工作区尚未初始化；读取不会创建数据库。" : "知识工作区不可用；未退回旧库。"; return }
            let page = try Self.decode(try await perform("data.object.list", Self.encode(CrewdDataObjectListParams(context: info.context, objectType: "view", after: nil, limit: 50, atRevision: nil, includeArchived: false))), CrewdDataObjectPage.self)
            guard current(o, token), page.context == info.context else { throw Failure.stale }
            objects = page.items; nextObjectPage = page.next
            status = "已读取当前数据视图；未改动数据。"
        } catch { if current(o, token) { status = "读取未完成或权限拒绝；未初始化或切换数据来源。" } }
    }
    func list(objectType: String, after: String? = nil, includeArchived: Bool = false) async {
        await read { context in
            let page: CrewdDataObjectPage = try await self.call("data.object.list", CrewdDataObjectListParams(context: context, objectType: objectType, after: after, limit: 50, atRevision: nil, includeArchived: includeArchived))
            return { self.objects = after == nil ? page.items : self.objects + page.items; self.nextObjectPage = page.next }
        }
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
    private func call<P: Encodable, R: Decodable>(_ method: String, _ params: P) async throws -> R {
        let input = try Self.encode(params)
        let output = try await perform(method, input)
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
    private func stage(_ change: CrewdDataChange, id: UUID, context: CrewdDataContext) throws -> (Intent, CrewdPresentationJournal.Prepared) {
        guard let o = active, origin() == o else { throw Failure.stale }
        try checked(context, o)
        let bytes = try JSONEncoder().encode(change)
        guard bytes.count <= 8_388_608 else { throw Failure.invalid }
        let intent = Intent(id: id, context: context, method: "data.submit", changeDigest: SHA256.hash(data:bytes).map{String(format:"%02x",$0)}.joined())
        let record = try journal().prepare(method: "data.submit.observation", params: Self.encode(intent), crew: o.crew)
        records[id] = record; pending.append(intent)
        return (intent, record)
    }
    private func observe(_ response: CrewdDataChangeResult, intent: Intent, record: CrewdPresentationJournal.Prepared, ledger: CrewdPresentationJournal) throws {
        let sameScope = response.context.dbId == intent.context.dbId && response.context.authorityId == intent.context.authorityId && response.context.crewId == intent.context.crewId && response.context.namespace == intent.context.namespace
        guard UUID(uuidString:response.requestId) == intent.id, sameScope,
              intent.method == "data.restore" || response.context.restoreEpoch == intent.context.restoreEpoch else { throw Failure.invalid }
        if response.phase == .committed {
            guard response.beforeGeneration == intent.context.generation, response.afterGeneration == response.context.generation else { throw Failure.invalid }
        }
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
    func submitFromNativeHost(_ change: CrewdDataChange) async {
        let o: Origin, token: UUID
        do { (o, token) = try begin() } catch { return }
        defer { finish(o, token) }
        do {
            guard let info = workspace else { throw Failure.unavailable }
            guard !hasBlockingPending else { throw Failure.unresolved }
            let (intent, record) = try stage(change, id: UUID(), context: info.context)
            let ledger = try journal()
            let response: CrewdDataChangeResult = try await call("data.submit", CrewdDataSubmitParams(requestId:intent.id.uuidString.lowercased(), context:intent.context, change:change))
            try observe(response, intent:intent, record:record, ledger:ledger)
            guard current(o, token) else { return }
            applyObservation(response, intent:intent)
        } catch { if current(o,token) { renderer.revoke(); status = "未获得完整提交结果；保留原 UUID，不自动重试。" } }
    }
    func lookup(_ id: UUID) async {
        let o: Origin, token: UUID
        do { (o,token) = try begin() } catch { return }
        defer { finish(o,token) }
        do {
            guard let intent = pending.first(where:{$0.id == id}), let record = records[id], workspace != nil else { throw Failure.invalid }
            let ledger = try journal()
            let response: CrewdDataRequestGetResult = try await call("data.request.get", CrewdDataRequestGetParams(requestId:id.uuidString.lowercased(), context:intent.context))
            try checked(response.workspace.context,o)
            guard current(o,token) else { return }
            workspace = response.workspace
            switch response.lookup {
            case .known(let original): try observe(original,intent:intent,record:record,ledger:ledger); applyObservation(original,intent:intent)
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
