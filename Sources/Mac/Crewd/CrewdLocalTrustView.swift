#if os(macOS)
import SwiftUI

/// Bootstrap requires the daemon's existing genuine Human lineage anchor.
/// Code-admitted reconnects cannot use this form to grant themselves new trust.
struct CrewdLocalTrustView: View {
    @ObservedObject var workspace: CrewdWorkspace
    @State private var identity: CrewdLocalCodeIdentity?
    @State private var trustID = ""
    @State private var busy = false
    @State private var result: String?
    private var candidate: Bool {
        #if PENDINGCREW_NATIVE_CANDIDATE
        return true
        #else
        return false
        #endif
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("本机 App 重连授权").font(.headline)
            Text("需当前真实人类启动链授权。授权只用于新App连接恢复普通命令；不能让新App关闭非其拥有的后台。")
                .font(.caption).foregroundStyle(.secondary)
            if let identity {
                Text(identity.identifier).textSelection(.enabled)
                Text("实际CDHash：\(identity.cdHash)").font(.caption).textSelection(.enabled)
                if let team = identity.team { Text("签名团队：\(team)").font(.caption) }
                if candidate {
                    Button("仅授权这个候选签名重连") { grant(publisher: false) }
                        .disabled(busy || !workspace.hasVerifiedOwnedLocalHelper)
                } else {
                    Button("授权已验证的正式发行者重连") { grant(publisher: true) }
                        .disabled(busy || !identity.publisherEligible || !workspace.hasVerifiedOwnedLocalHelper)
                    if !identity.publisherEligible {
                        Text("正式发行签名、Team或强化运行时尚未验证；不能把临时签名当正式发行者。")
                            .font(.caption).foregroundStyle(.orange)
                    }
                }
            }
            TextField("明确的原授权 trustId", text: $trustID)
            Button("撤销此原授权", role: .destructive) { revoke() }
                .disabled(busy || trustID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !workspace.hasVerifiedOwnedLocalHelper)
            if let result { Text(result).font(.caption).textSelection(.enabled) }
        }.task {
            do { identity = try CrewdLocalCodeIdentity.current() }
            catch { result = error.localizedDescription }
        }
    }
    private func grant(publisher: Bool) {
        guard !busy, let original = identity, workspace.hasVerifiedOwnedLocalHelper,
              publisher ? original.publisherEligible : candidate else { return }
        let epoch = workspace.observationGeneration
        busy = true
        Task {
            defer { busy = false }
            do {
                let current = try CrewdLocalCodeIdentity.current()
                guard current.cdHash == original.cdHash, workspace.acceptsObservation(epoch) else { throw CrewdPublicWire.invalid() }
                let receipt = try await workspace.submitPublicCommand(.localClientTrust, params: .object([
                    "clientPid": .integer(Int64(ProcessInfo.processInfo.processIdentifier)),
                    "mode": .string(publisher ? "publisher" : "exactCode"),
                    "expectedCdHash": .string(current.cdHash),
                    "expectedTeam": publisher ? current.team.map(CrewdValue.string) ?? .null : .null
                ]))
                guard workspace.acceptsObservation(epoch), receipt.phase == .committed, !receipt.historical,
                      let id = receipt.value["result"]["trust"]["id"].string else { throw CrewdClientError(message: "原授权尚未确认；请只读核对回执。") }
                trustID = id; result = "后台已保存此授权：\(id)。真实新PID恢复仍需实际验收。"
            } catch { if workspace.acceptsObservation(epoch) { result = error.localizedDescription } }
        }
    }
    private func revoke() {
        guard !busy, workspace.hasVerifiedOwnedLocalHelper else { return }
        let id = trustID.trimmingCharacters(in: .whitespacesAndNewlines)
        let epoch = workspace.observationGeneration
        busy = true
        Task {
            defer { busy = false }
            do {
                let receipt = try await workspace.submitPublicCommand(.localClientTrustRevoke, params: .object(["trustId": .string(id)]))
                guard workspace.acceptsObservation(epoch), receipt.phase == .committed, !receipt.historical,
                      receipt.value["result"]["trustId"] == .string(id),
                      receipt.value["result"]["revokedInStore"] == .bool(true) else { throw CrewdClientError(message: "原授权撤销尚未确认；请只读核对。") }
                result = "后台已确认原授权SQL撤销；物理连接关闭须另核实际连接，未关闭远程团队。"
            } catch { if workspace.acceptsObservation(epoch) { result = error.localizedDescription } }
        }
    }
}
#endif
