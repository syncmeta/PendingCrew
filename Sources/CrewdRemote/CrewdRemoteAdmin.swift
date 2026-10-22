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
    @Published private(set) var enrollment:CrewdEnrollment.IssuerSummary?
    @Published var enrollmentHost=""
    var mayIssueEnrollment:Bool { !retired && workspace.dataDirectory != nil && workspace.client.readyForWork && workspace.client.capabilities.contains("remote.enrollment") }
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
    private var retired=false
    let workspace: CrewdWorkspace
    init(workspace: CrewdWorkspace) { self.workspace = workspace }
    func retire() {retired=true;readEpoch=UUID()}
    private func decode<T: Decodable>(_ value: CrewdValue, as: T.Type) throws -> T { try JSONDecoder().decode(T.self, from: JSONEncoder().encode(value)) }
    func refresh() async {
        guard !retired else{return}
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
    private static func enrollmentCall(_ client:CrewdClient,epoch:UUID,method:String,bytes:Data)async throws->Data {
        let params=try JSONDecoder().decode(CrewdValue.self,from:bytes)
        guard client.readyForWork,client.connectionEpoch==epoch else{throw CrewdRemoteError.wrongIdentity}
        let result=try await client.call(method,params)
        return try JSONEncoder().encode(result)
    }
    func readEnrollment() async {
        guard mayIssueEnrollment,let identity=status?.identity,!busy else{return}
        busy=true;defer{busy=false}
        do {let value=try await CrewdEnrollment.shared.issuerOriginal(server:identity)
            guard mayIssueEnrollment,status?.identity==identity else{return};enrollment=value
        }catch{self.error="无法读取原配对链接请求。请解锁设备后再读取，不会生成替代请求。"}
    }
    func createEnrollment() async {
        guard mayIssueEnrollment,let current=status,let identity=current.identity,current.configuration.enabled,current.listening,!busy else{return}
        let host=enrollmentHost.trimmingCharacters(in:.whitespacesAndNewlines)
        guard !host.isEmpty,host != "0.0.0.0",host != "::",host != "127.0.0.1",host != "localhost",host != "::1" else{error="请填写其他设备可访问的真实局域网或公网地址。";return}
        busy=true;defer{busy=false}
        let client=workspace.client,epoch=client.connectionEpoch
        do {
            let value=try await CrewdEnrollment.shared.createHostLink(server:identity,endpoint:.init(host:host,port:current.configuration.port),label:computerLabel,send:{method,bytes in
                try await Self.enrollmentCall(client,epoch:epoch,method:method,bytes:bytes)
            })
            guard mayIssueEnrollment,client.connectionEpoch==epoch,status?.identity==current.identity else{return};enrollment=value;error=nil
        }catch{self.error="链接未生成或结果未确认。请先读取已保存的原请求；如原请求存在，只查询原结果，不要再次生成。"}
    }
    func lookupEnrollment(submitPrepared:Bool=false) async {
        guard mayIssueEnrollment,let current=status,let identity=current.identity,let original=enrollment,!busy else{return}
        busy=true;defer{busy=false};let client=workspace.client,epoch=client.connectionEpoch
        do {
            let send:@Sendable (String,Data)async throws->Data = {method,bytes in
                try await Self.enrollmentCall(client,epoch:epoch,method:method,bytes:bytes)
            }
            let value:CrewdEnrollment.IssuerSummary
            if submitPrepared {value=try await CrewdEnrollment.shared.submitHostOriginal(original.id,server:identity,send:send)}
            else {value=try await CrewdEnrollment.shared.lookupHostLink(original.id,server:identity,send:send)}
            guard mayIssueEnrollment,client.connectionEpoch==epoch,status?.identity==current.identity else{return};enrollment=value;error=nil
        }catch{self.error="原链接结果仍未确认。已保留原请求，不会重发或生成替代token。"}
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
        guard !retired,!busy else { return }
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
        guard !retired,!busy else { return }; busy = true; defer { busy = false }
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
            if !retired {await refresh()}
        } catch {
            self.error = error.localizedDescription
            if CrewdClientError.isUncertainSubmission(error), !unknown.contains(where: { $0.id == draft.id }) { unknown.append(draft) }
        }
    }
}
#endif
