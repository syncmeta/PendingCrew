#if os(macOS)
import Foundation
import Combine

/// The wire02 configuration contains no credential. Never add a secret field here.
struct CrewdJudgmentConfig: Codable, Equatable {
    var enabled: Bool
    var provider: String
    var baseUrl: String
    var model: String
    var choiceMin: Double
    var wakeYes: Double
    var credentialSource: String
    static let empty = Self(enabled: false, provider: "typesafe", baseUrl: "", model: "", choiceMin: 0.5, wakeYes: 0.5, credentialSource: "platform")
    var valid: Bool {
        guard provider == "typesafe", ["platform", "file"].contains(credentialSource),
              !model.isEmpty, model.utf8.count <= 256, baseUrl.utf8.count <= 2048,
              choiceMin.isFinite, wakeYes.isFinite, (0...1).contains(choiceMin), (0...1).contains(wakeYes),
              let u = URLComponents(string: baseUrl), u.scheme == "https", let host = u.host, !host.isEmpty,
              u.user == nil, u.password == nil, u.query == nil, u.fragment == nil else { return false }
        return true
    }
    func sameCredentialBinding(as other: Self?) -> Bool {
        guard let other else { return false }
        // Conservative frontend comparison. The server owns endpoint normalization.
        return provider == other.provider && baseUrl == other.baseUrl && credentialSource == other.credentialSource
    }
}

struct CrewdJudgmentView: Decodable, Equatable {
    struct Settings: Decodable, Equatable { let revision: String?; let config: CrewdJudgmentConfig? }
    enum CredentialState: String, Decodable { case missing, recordedUnverified = "recorded_unverified", updateUnknown = "update_unknown", unknown }
    let settings: Settings
    let credentialState: CredentialState
    let pendingRequestId: String?
}
struct CrewdJudgmentResult: Decodable {
    enum Outcome: String, Decodable { case committed, unknown }
    let historical: Bool
    let requestId: String
    let outcome: Outcome
    let view: CrewdJudgmentView
}
struct CrewdJudgmentResponse: Decodable {
    struct Lookup: Decodable {
        enum Status: String, Decodable { case known, notFound = "not_found", historical }
        let status: Status
        let result: CrewdJudgmentResult?
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            status = try c.decode(Status.self, forKey: .status)
            result = try c.decodeIfPresent(CrewdJudgmentResult.self, forKey: .result)
            guard (status == .known) == (result != nil) else { throw DecodingError.dataCorruptedError(forKey: .result, in: c, debugDescription: "Closed lookup shape required") }
        }
        private enum CodingKeys: String, CodingKey { case status, result }
    }
    let view: CrewdJudgmentView
    let requestLookup: Lookup?
}

/// Immutable observation metadata, never a replay payload. No credential value exists in this type.
struct CrewdJudgmentIntent: Codable, Equatable, Identifiable {
    enum Action: String, Codable, CaseIterable { case keep, set, clear }
    let id: UUID
    let expectedRevision: String?
    let config: CrewdJudgmentConfig?
    let action: Action
    let runtimeBindingID: String
}

@MainActor
final class CrewdJudgmentSettingsModel: ObservableObject {
    struct Origin: Equatable {
        let scope: CrewdPresentationJournal.Scope
        let runtime: String
        let connection: UUID
        let observation: UUID
    }
    enum Failure: Error { case unavailable, invalid, journal }
    typealias RPC = (String, CrewdValue) async throws -> CrewdValue
    @Published private(set) var view: CrewdJudgmentView?
    @Published private(set) var records: [CrewdJudgmentIntent] = []
    @Published private(set) var adminQualified = false
    @Published private(set) var busy = false
    @Published private(set) var status = "尚未读取本机判断模型设置。"
    @Published var draft = CrewdJudgmentConfig.empty
    @Published var credentialAction = CrewdJudgmentIntent.Action.keep
    @Published var secretDraft = ""
    private let origin: () -> Origin?
    private let rpc: RPC
    private let journalFactory: (Origin) throws -> CrewdPresentationJournal
    private var active: Origin?
    private var journal: CrewdPresentationJournal?
    private var operation = UUID()
    private var unresolved = Set<UUID>()

    init(origin: @escaping () -> Origin?, rpc: @escaping RPC, journal: @escaping (Origin) throws -> CrewdPresentationJournal) {
        self.origin = origin; self.rpc = rpc; journalFactory = journal
    }
    convenience init(workspace: CrewdWorkspace) {
        self.init(origin: { [weak workspace] in
            guard let w = workspace, w.dataDirectory != nil, w.originalJournalReady,
                  w.acceptsObservation(w.observationGeneration), let scope = w.presentationScope,
                  let db = w.client.runtimeIdentity["dbId"].string, scope.databaseID == db,
                  let runtime = w.client.runtimeIdentity["runtimeBindingId"].string else { return nil }
            return Origin(scope: scope, runtime: runtime, connection: w.client.connectionEpoch, observation: w.observationGeneration)
        }, rpc: { [weak workspace] method, params in
            guard let w = workspace, w.dataDirectory != nil, w.originalJournalReady,
                  w.acceptsObservation(w.observationGeneration) else { throw Failure.unavailable }
            return try await w.client.call(method, params)
        }, journal: { [weak workspace] origin in
            guard let directory = workspace?.dataDirectory else { throw Failure.unavailable }
            return try CrewdPresentationJournal(root: directory.appendingPathComponent("frontend-presentation/judgment-settings-observations"), scope: origin.scope)
        })
    }
    private func decode<T: Decodable>(_ value: CrewdValue, as type: T.Type) throws -> T {
        try JSONDecoder().decode(type, from: JSONEncoder().encode(value))
    }
    private func encode<T: Encodable>(_ value: T) throws -> CrewdValue {
        try JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(value))
    }
    /// Called on origin/connection retirement and when the Settings pane disappears.
    func invalidate() {
        operation = UUID(); active = nil; journal = nil; adminQualified = false
        secretDraft = ""; busy = false; view = nil; records = []; unresolved = []
        draft = .empty; credentialAction = .keep
        status = "连接或权限已变化；未重发任何设置。请重新读取。"
    }
    @discardableResult func synchronizeOrigin() -> Bool {
        if active != nil && origin() != active { invalidate(); return true }; return false
    }
    private func begin() throws -> (Origin, UUID) {
        guard !busy, let current = origin() else { throw Failure.unavailable }
        if active != current {
            invalidate(); let ledger = try journalFactory(current)
            let restored = try ledger.unknownRecords().filter { $0.scope == current.scope && $0.method == "judgment.settings.set.observation" }
            var intents: [CrewdJudgmentIntent] = []
            for record in restored {
                let intent = try decode(record.params, as: CrewdJudgmentIntent.self)
                guard !intents.contains(where: { $0.id == intent.id }) else { throw Failure.journal }
                intents.append(intent)
            }
            active = current; journal = ledger; records = intents; unresolved = Set(intents.map(\.id))
        }
        let token = UUID(); operation = token; busy = true
        return (current, token)
    }
    private func current(_ o: Origin, _ token: UUID) -> Bool { active == o && origin() == o && operation == token }
    private func finishBusy(_ o: Origin, _ token: UUID) {
        guard active == o, operation == token else { return }
        if origin() == o { busy = false } else { invalidate() }
    }
    func refresh(requestID: UUID? = nil) async {
        let o: Origin, token: UUID
        do { (o, token) = try begin() } catch { secretDraft = ""; adminQualified = false; status = "仅可信本机 Human 连接且历史可用时可读取。"; return }
        defer { finishBusy(o, token) }
        do {
            // Pure lookup only: this method never invokes settings.set.
            let params: CrewdValue = requestID.map { .object(["requestId": .string($0.uuidString.lowercased())]) } ?? .null
            let response = try decode(try await rpc("judgment.settings.get", params), as: CrewdJudgmentResponse.self)
            guard current(o, token) else { return }
            if let id = requestID {
                guard records.contains(where: { $0.id == id }), let lookup = response.requestLookup else { throw Failure.invalid }
                switch lookup.status {
                case .known:
                    guard let result = lookup.result, UUID(uuidString: result.requestId) == id else { throw Failure.invalid }
                    if !result.historical && result.outcome == .committed { unresolved.remove(id) }
                    status = result.historical ? "原请求属于历史状态；未将旧结果恢复为当前设置。" : (result.outcome == .committed ? "已只读核对原请求；不代表模型或凭据有效。" : "原请求结果未知；仅可新建明确的设置或清除操作。")
                case .notFound: status = "未找到原请求；未重发。需要人类明确新建操作。"
                case .historical: status = "原请求属于历史记录；未重发或升级为当前状态。"
                }
            } else { guard response.requestLookup == nil else { throw Failure.invalid }; status = "已读取录存状态；未验证凭据或调用模型。" }
            // Current view is distinct from the lookup's original/historical result.
            view = response.view; draft = response.view.settings.config ?? .empty; adminQualified = true
        } catch {
            guard current(o, token) else { return }
            secretDraft = ""; adminQualified = false; status = "设置读取未完成或被权限拒绝；未提交或重发。"
        }
    }
    var canKeep: Bool {
        guard let view, unresolved.isEmpty, view.pendingRequestId == nil,
              view.credentialState == .recordedUnverified else { return false }
        return draft.sameCredentialBinding(as: view.settings.config)
    }
    var canSubmit: Bool {
        adminQualified && !busy && origin() == active && draft.valid &&
        (credentialAction != .keep || canKeep) &&
        (credentialAction != .set || (!secretDraft.isEmpty && secretDraft.utf8.count <= 8192))
    }
    /// One explicit Human action gets exactly one UUID. Cancellation/lost response is not replayable.
    func submit(removeConfiguration: Bool = false) async {
        guard adminQualified, !busy, origin() == active, let old = view,
              removeConfiguration ? credentialAction == .clear : canSubmit else { secretDraft = ""; status = "本次尚未提交：请核对连接、配置和明确的凭据操作。"; return }
        let secret = secretDraft; secretDraft = ""
        let o: Origin, token: UUID
        do { (o, token) = try begin() } catch { status = "本次尚未提交：原连接或历史不可用。"; return }
        defer { finishBusy(o, token) }
        let intent = CrewdJudgmentIntent(id: UUID(), expectedRevision: old.settings.revision, config: removeConfiguration ? nil : draft, action: credentialAction, runtimeBindingID: o.runtime)
        let ledger = journal
        let prepared: CrewdPresentationJournal.Prepared
        do {
            guard let ledger, current(o, token) else { throw Failure.unavailable }
            prepared = try ledger.prepare(method: "judgment.settings.set.observation", params: encode(intent), crew: "")
            records.append(intent); unresolved.insert(intent.id)
        } catch { if current(o, token) { status = "本次尚未提交：非秘密意图未能可靠保存。" }; return }
        guard current(o, token) else { return } // durable record remains available for pure lookup
        var update: [String: CrewdValue] = ["action": .string(intent.action.rawValue)]
        if intent.action == .set { update["value"] = .string(secret) }
        let params: CrewdValue
        do { params = .object(["requestId": .string(intent.id.uuidString.lowercased()), "expectedRevision": intent.expectedRevision.map(CrewdValue.string) ?? .null, "config": try intent.config.map(encode) ?? .null, "credentialUpdate": .object(update)]) }
        catch { if current(o, token) { status = "未能构建请求；保留原意图供只读核对。" }; return }
        do {
            let result = try decode(try await rpc("judgment.settings.set", params), as: CrewdJudgmentResult.self)
            guard UUID(uuidString: result.requestId) == intent.id else { throw Failure.invalid }
            // Complete only the original ledger. Never publish to a new connection/workspace.
            try ledger?.finish(prepared, state: result.outcome == .committed && !result.historical ? "replyObserved" : "unknown")
            guard current(o, token) else { return }
            if result.outcome == .committed && !result.historical { unresolved.remove(intent.id) }
            adminQualified = false // a fresh formal get is needed before another mutation
            status = result.historical ? "返回历史结果；请只读刷新当前状态。" : (result.outcome == .committed ? "已录存设置；尚未验证模型或凭据。请刷新。" : "提交结果未知；保留原 UUID，仅只读核对或明确新操作。")
        } catch {
            try? ledger?.finish(prepared, state: "unknown")
            guard current(o, token) else { return }
            adminQualified = false; status = "提交结果未知；原 UUID 已保留，未自动重试。请只读核对。"
        }
    }
}
#endif
