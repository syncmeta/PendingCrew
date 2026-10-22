#if os(macOS)
import SwiftUI

/// Original session chrome consumes actual selected authority and generation.
/// Structured approvals never send guessed TUI fingerprints or raw keystrokes.
struct CrewdOriginalSessionControls: View {
    @ObservedObject var backend: CrewdOriginalSessionBackend
    @ObservedObject var workspace: CrewdWorkspace
    private var active: Bool {
        workspace.sessionID == backend.sessionID && workspace.client.connected && !workspace.isRetired
    }
    private var profile: CrewdExecutorProfile? { workspace.profileForSession(backend.sessionID) }
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let failure = backend.failure { Text(failure).font(.caption).foregroundStyle(.orange) }
            if let profile {
                Text(profile.title).font(.caption).foregroundStyle(.secondary)
                if !profile.fullHistoryLoad { Text(profile.backend == "codex" && workspace.client.capabilities.contains("session.codex") ? "原历史可在会话控制页只读读取；手动恢复被拒绝时不会创建替代 thread。":profile.recoveryDescription).font(.caption).foregroundStyle(.secondary) }
                if !profile.canStart { Text(profile.unavailableDescription).font(.caption).foregroundStyle(.orange) }
            }
            if workspace.sessions.first(where: { $0["id"].string == backend.sessionID })?["resumeFailedAt"] != nil,
               workspace.sessions.first(where: { $0["id"].string == backend.sessionID })?["resumeFailedAt"] != .null {
                Text(profile?.backend == "codex" ? "此前恢复被拒绝；保留原 thread，不会改为新对话或重发首条任务。":"此前恢复曾被供应商拒绝；后台可能已改为新对话，请核对原记录和系统公告。").font(.caption).foregroundStyle(.orange)
            }
            CrewdCodexSessionControls(workspace:workspace,sessionID:backend.sessionID)
            if backend.status != .running {
                Button("尝试恢复这个会话") { backend.resume() }
                    .disabled(!active || workspace.busy || profile?.canStart != true || workspace.hasOriginalUnknown(method: "session.resume", crew: workspace.crewID) || workspace.controlTargets[backend.sessionID] == nil)
            }
            if active, profile?.structuredDecisions == true, workspace.inspection["runnerRequest"] != .null,
               workspace.inspection["live"]["pendingDecision"]["kind"] == .string("runner_request") {
                CrewdHumanRequestView(workspace: workspace,
                    request: workspace.inspection["runnerRequest"],
                    decision: workspace.inspection["live"]["pendingDecision"])
                    .id((workspace.inspection["runnerRequest"]["runnerRequestId"].string ?? "") + "/" + (workspace.inspection["runnerRequest"]["fingerprint"].string ?? ""))
            }
        }.padding(.horizontal, 8)
        .task(id: backend.sessionID) {
            workspace.selectSession(backend.sessionID)
            await backend.refresh()
        }
    }
}
#endif
