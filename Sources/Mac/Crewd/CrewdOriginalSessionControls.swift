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
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let failure = backend.failure { Text(failure).font(.caption).foregroundStyle(.orange) }
            if backend.status != .running {
                Button("恢复这个原会话") { backend.resume() }
                    .disabled(!active || workspace.busy || workspace.controlTargets[backend.sessionID] == nil)
            }
            if active, workspace.inspection["runnerRequest"] != .null,
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
