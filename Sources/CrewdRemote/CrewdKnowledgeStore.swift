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
        // SQLite Core authority is the authenticated database identity. Remote
        // pairing authority/grant remain separate in scope and Origin equality.
        var knowledgeAuthorityID: String? { scope.databaseID.flatMap { $0.isEmpty ? nil : $0 } }
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
                      intent.context.authorityId == o.knowledgeAuthorityID, records[intent.id] == nil else { throw Failure.invalid }
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
        guard context.dbId == o.scope.databaseID, context.authorityId == o.knowledgeAuthorityID,
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
    func loadAllViews()async {
        await refresh()
        var seen:Set<String>=[]
        while let next=nextObjectPage {
            guard !Task.isCancelled,seen.insert(next).inserted else{return}
            await list(objectType:"view",after:next)
        }
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
    @discardableResult func submitFromNativeHost(_ change: CrewdDataChange, requestID:UUID?=nil) async -> CrewdDataChangeResult? {
        let o: Origin, token: UUID
        do { (o, token) = try begin() } catch { return nil }
        defer { finish(o, token) }
        do {
            guard let info = workspace else { throw Failure.unavailable }
            guard !hasBlockingPending else { throw Failure.unresolved }
            let (intent, record) = try stage(change, id: requestID ?? UUID(), context: info.context)
            let ledger = try journal()
            let response: CrewdDataChangeResult = try await call("data.submit", CrewdDataSubmitParams(requestId:intent.id.uuidString.lowercased(), context:intent.context, change:change))
            try observe(response, intent:intent, record:record, ledger:ledger)
            guard current(o, token) else { return origin() == o ? response:nil }
            applyObservation(response, intent:intent)
            return response
        } catch { if current(o,token) { renderer.revoke(); status = "未获得完整提交结果；保留原 UUID，不自动重试。" } }
        return nil
    }
    func nativeDocumentStage(channel:String)throws->CrewdPresentationJournal.DocumentStage? {
        guard let o=active,origin() == o else{throw Failure.stale}
        return try journal().currentDocumentStage(crew:o.crew,channel:channel)
    }
    func nativeDocumentResult(_ stage:CrewdPresentationJournal.DocumentStage)throws->CrewdDataChangeResult? {
        guard let o=active,o.scope == stage.scope,o.crew == stage.crew,origin() == o else{throw Failure.stale}
        guard let value=try journal().documentStageResult(stage) else{return nil}
        return try Self.decode(value,CrewdDataChangeResult.self)
    }
    func finishNativeDocument(_ stage:CrewdPresentationJournal.DocumentStage)throws {
        guard let o=active,o.scope == stage.scope,o.crew == stage.crew,origin() == o else{throw Failure.stale}
        try journal().finishDocumentStage(stage)
    }
    /// The original two-stage input lives in the existing journal, beyond a sheet
    /// or process lifetime. Reopening can only adopt/lookup the same save UUID.
    func submitNativeDocument(title:String,body:String,channel:String)async->CrewdDataChangeResult? {
        do {
            guard canSaveNativeDocument, !busy, !hasBlockingPending else { throw Failure.unavailable }
            guard let o=active,origin() == o,let context=workspace?.context else{throw Failure.stale}
            let ledger=try journal()
            let stage=try ledger.prepareDocumentStage(crew:o.crew,channel:channel,title:title,body:body,context:Self.encode(context))
            if try ledger.documentStageAttempted(stage){status="原文档已送交保存；仅核对原请求或加入已保存原文。";return try nativeDocumentResult(stage)}
            guard try Self.decode(stage.context,CrewdDataContext.self) == context else{throw Failure.stale}
            try ledger.markDocumentStageAttempted(stage)
            let change=CrewdDataChange.apply(message:"创建文档",edits:[
                .createBlock(alias:"body",contentType:"text/plain",content:.string(stage.body),sources:[],attributes:.object([:])),
                .createView(alias:"document",title:stage.title,definition:.document(placements:[.set(id:nil,alias:"bodyPlacement",block:.created(alias:"body"),atRevision:nil,layout:.object([:]))],layout:.object([:])),sources:[],attributes:.object([:]))])
            let response=await submitFromNativeHost(change,requestID:stage.id)
            if let response,[.committed,.rejected,.conflicted].contains(response.phase){try ledger.recordDocumentStageResult(Self.encode(response),stage:stage)}
            return response
        }catch{status="文档保存阶段尚未确认，原输入保留："+error.localizedDescription;return nil}
    }
    func lookupNativeDocument(_ stage:CrewdPresentationJournal.DocumentStage)async->CrewdDataChangeResult? {
        let o:Origin,token:UUID
        do{(o,token)=try begin()}catch{return nil}
        defer{finish(o,token)}
        do {
            guard o.scope == stage.scope,o.crew == stage.crew else{throw Failure.stale}
            if let saved=try nativeDocumentResult(stage){return saved}
            let context=try Self.decode(stage.context,CrewdDataContext.self)
            let response:CrewdDataRequestGetResult=try await call("data.request.get",CrewdDataRequestGetParams(requestId:stage.id.uuidString.lowercased(),context:context))
            try checked(response.workspace.context,o)
            guard origin() == o else{throw Failure.stale}
            guard case .known(let result)=response.lookup,[.committed,.rejected,.conflicted].contains(result.phase) else{status="原保存请求尚无已确认结果；保留原文与UUID，不会重发。";return nil}
            let ledger=try journal();try ledger.recordDocumentStageResult(Self.encode(result),stage:stage)
            if let intent=pending.first(where:{$0.id == stage.id}),let record=records[stage.id]{try observe(result,intent:intent,record:record,ledger:ledger);applyObservation(result,intent:intent)}
            return result
        }catch{status="原文档结果未确认；仅保留原UUID核对。";return nil}
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

/// A read projection of Core's real channel placements. The content store is
/// unchanged; linking a view never copies blocks or submits content again.
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
    private var owners:Set<UUID>=[]
    private var refreshTask:Task<Void,Never>?
    private let call:Call
    init(call:@escaping Call){self.call=call}
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
    func lookup(_ snapshot:CrewdChannelKnowledgeDrafts.Snapshot)async throws->CrewdChannelKnowledgeReceipt? {
        guard snapshot.crew == crew,snapshot.channel == channel else{throw CancellationError()}
        let token=generation
        let value=try await call("resource.requestLookup",.object(["requestId":.string(snapshot.id.uuidString.lowercased())]));try check(token)
        guard value["requestId"].string.flatMap(UUID.init(uuidString:)) == snapshot.id else{throw CrewdClientError(message:"原知识引用请求身份不符。")}
        if value["status"] == .string("notFound") {guard value["operation"] == .null,value["result"] == .null else{throw CrewdClientError(message:"未找到知识引用请求却附带结果。")};return nil}
        guard value["status"] == .string("committed"),value["operation"] == .string(snapshot.operation) else{throw CrewdClientError(message:"原知识引用请求状态或操作不符。")}
        let receipt=try validate(value["result"],snapshot:snapshot);do{try await refresh()}catch{};return receipt
    }
    private func validate(_ value:CrewdValue,snapshot:CrewdChannelKnowledgeDrafts.Snapshot)throws->CrewdChannelKnowledgeReceipt {
        let result=try CrewdKnowledgeStore.decode(value,CrewdChannelKnowledgeReceipt.self)
        guard result.channelId == snapshot.channel,result.source == snapshot.source,result.linked == snapshot.linked,!result.linkRevision.isEmpty else{throw CrewdClientError(message:"原知识引用回执与原输入不符。")}
        return result
    }
}
