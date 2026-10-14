#if os(macOS)
import Foundation
import Combine

/// Window-owned request drafts. Current configuration/grants stay server-owned.
@MainActor final class CrewdRemoteAdmin: ObservableObject {
    struct RequestDraft: Identifiable {
        let id: UUID
        let method: String
        let params: CrewdValue
        let label: String
    }
    @Published private(set) var status: CrewdRemoteStatus?
    @Published private(set) var grants: [CrewdRemoteGrant] = []
    @Published private(set) var invitation: CrewdRemoteEnvelope?
    @Published private(set) var accepted: CrewdRemoteAcceptResult?
    @Published private(set) var unknown: [RequestDraft] = []
    @Published private(set) var busy = false
    @Published var error: String?
    @Published var listenHost = "127.0.0.1"
    @Published var port = "7423"
    @Published var computerLabel = "我的电脑"
    @Published var responseText = ""
    @Published var preview: CrewdRemoteResponse?
    @Published var revokeNotes: [String: String] = [:]
    private var drafts: [String: RequestDraft] = [:]
    private var readEpoch = UUID()
    let workspace: CrewdWorkspace
    init(workspace: CrewdWorkspace) { self.workspace = workspace }
    private func decode<T: Decodable>(_ value: CrewdValue, as: T.Type) throws -> T { try JSONDecoder().decode(T.self, from: JSONEncoder().encode(value)) }
    func refresh() async {
        let epoch = UUID(); readEpoch = epoch
        do {
            let state = try decode(await workspace.client.call("remote.status"), as: CrewdRemoteStatus.self)
            let grants = try decode(await workspace.client.call("remote.grants"), as: [CrewdRemoteGrant].self)
            guard readEpoch == epoch else { return }
            self.status = state; self.grants = grants; error = nil
            if let old = accepted {
                if let current = grants.first(where: { $0.id == old.grant.id }) {
                    accepted = CrewdRemoteAcceptResult(requestId: old.requestId, reused: old.reused, receipt: old.receipt, grant: current)
                } else { accepted = nil }
            }
            if drafts["remote.configure"] == nil { listenHost = state.configuration.listenHost; port = String(state.configuration.port) }
        } catch { if readEpoch == epoch { self.error = error.localizedDescription } }
    }
    func configure(enabled: Bool) async {
        guard let status, let port = UInt16(port), port != 0 else { error = "请先读取设置并填写有效地址和端口。"; return }
        await submit("remote.configure", .object(["expectedRevision": .string(status.configuration.revision), "enabled": .bool(enabled), "listenHost": .string(listenHost), "port": .integer(Int64(port))]), label: enabled ? "允许设备连接" : "关闭设备连接")
    }
    func newInvitation() async {
        // Deliberately retire only the new-invite draft; uncertain older requests
        // remain available for explicit confirmation using their original UUID.
        drafts["remote.invite"] = nil
        await submit("remote.invite", .object(["deviceLabel": .string(computerLabel)]), label: "生成连接邀请")
    }
    func reviewResponse() {
        do {
            let envelope = try CrewdRemoteText.decode(responseText)
            guard let bytes = Data(base64Encoded: envelope.payload), bytes.count <= 65536 else { throw CrewdRemoteError.invalidExchange }
            let value = try JSONDecoder().decode(CrewdRemoteResponse.self, from: bytes)
            let certificate = try CrewdRemoteCertificate.decode(value.clientCertificateDerBase64)
            let verified = try CrewdRemoteCertificate.verify(envelope, certificate: certificate, role: .client, expectedPin: value.clientSpkiSha256, as: CrewdRemoteResponse.self)
            guard let identity = status?.identity, verified.authorityId == identity.authorityId, verified.dbId == identity.dbId,
                  !verified.deviceLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, verified.deviceLabel.utf8.count <= 256 else { throw CrewdRemoteError.wrongIdentity }
            preview = verified; error = nil
        } catch { preview = nil; self.error = error.localizedDescription }
    }
    func approve() async {
        guard preview != nil else { return }
        do {
            let envelope = try CrewdRemoteText.decode(responseText)
            await submit("remote.accept", .object(["responseEnvelope": try JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(envelope))]), label: "批准设备连接")
        } catch { self.error = error.localizedDescription }
    }
    func revoke(_ grant: CrewdRemoteGrant) async {
        let note = (revokeNotes[grant.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !note.isEmpty else { error = "请填写撤销理由。"; return }
        await submit("remote.revoke", .object(["grantId": .string(grant.id), "expectedRevision": .string(grant.revision), "note": .string(note)]), label: "撤销 \(grant.deviceLabel)", idempotent: false)
    }
    func confirm(_ draft: RequestDraft) async { await execute(draft) }
    private func submit(_ method: String, _ base: CrewdValue, label: String, idempotent: Bool = true) async {
        guard !busy else { return }
        let current: RequestDraft
        if let old = drafts[method] {
            var oldBase = old.params
            if case .object(var values) = oldBase { values.removeValue(forKey: "requestId"); oldBase = .object(values) }
            if oldBase == base { current = old }
            else { current = makeDraft(method, base, label, idempotent: idempotent) }
        } else { current = makeDraft(method, base, label, idempotent: idempotent) }
        drafts[method] = current
        await execute(current)
    }
    private func makeDraft(_ method: String, _ params: CrewdValue, _ label: String, idempotent: Bool) -> RequestDraft {
        let id = UUID(); var params = params
        if idempotent, case .object(var values) = params { values["requestId"] = .string(id.uuidString.lowercased()); params = .object(values) }
        return RequestDraft(id: id, method: method, params: params, label: label)
    }
    private func execute(_ draft: RequestDraft) async {
        guard !busy else { return }; busy = true; defer { busy = false }
        readEpoch = UUID()
        do {
            let result = try await workspace.client.call(draft.method, draft.params)
            switch draft.method {
            case "remote.invite": invitation = try decode(result, as: CrewdRemoteEnvelope.self)
            case "remote.accept":
                let value = try decode(result, as: CrewdRemoteAcceptResult.self)
                guard value.requestId == draft.id.uuidString.lowercased() else { throw CrewdRemoteError.wrongIdentity }
                accepted = value
            case "remote.configure": _ = try decode(result, as: CrewdRemoteConfiguration.self)
            case "remote.revoke": _ = try decode(result, as: CrewdRemoteGrant.self)
            default: throw CrewdRemoteError.invalidExchange
            }
            unknown.removeAll { $0.id == draft.id }
            error = nil
            await refresh()
        } catch {
            self.error = error.localizedDescription
            if CrewdClientError.isUncertainSubmission(error), !unknown.contains(where: { $0.id == draft.id }) { unknown.append(draft) }
        }
    }
}
#endif
