#if os(iOS)
import SwiftUI
import Combine

/// The original mobile session navigation observes the selected trusted crewd
/// connection. Leaving this screen ends observation, never the remote team.
struct IOSRemoteSessionView: View {
    @EnvironmentObject private var appModel: AppModel
    let crewID: String
    let sessionID: String
    var body: some View {
        Group {
            if let workspace = appModel.crewdBackend?.workspace, workspace.crewID == crewID {
                IOSRemoteSessionDetail(workspace: workspace, sessionID: sessionID)
            } else {
                ContentUnavailableView("远端 session 不可用", systemImage: "wifi.exclamationmark",
                    description: Text(appModel.backendConfigurationError ?? "请先连接这台电脑。"))
            }
        }.navigationTitle(appModel.crewdBackend?.workspace.knownSessions[sessionID]?["displayName"].string ?? "成员会话").navigationBarTitleDisplayMode(.inline)
    }
}

private struct IOSRemoteSessionDetail: View {
    @ObservedObject var workspace: CrewdWorkspace
    let sessionID: String
    @State private var inspection: CrewdValue = .null
    @State private var loadError: String?
    @State private var loading = false
    @State private var controlsBusy = false
    @State private var terminal: CrewdPublicTerminal?
    private var scopeIsCurrent: Bool { workspace.crewID != "" && !workspace.isRetired }
    private var decision: CrewdValue { inspection["live"]["pendingDecision"] }
    private var controlEnabled: Bool { scopeIsCurrent && workspace.client.connected && workspace.client.capabilities.contains("orchestra") && !controlsBusy }
    var body: some View {
        List {
            Section("连接") {
                if loading { HStack{ProgressView();Text("正在读取会话状态")} }
                else if workspace.client.connected && scopeIsCurrent { Label("已安全连接", systemImage: "lock.fill").foregroundStyle(.green) }
                else { Label("连接已断开", systemImage: "wifi.slash").foregroundStyle(.red) }
                if let loadError { Text(loadError).font(.caption).foregroundStyle(.red) }
                Button("刷新会话") { Task { await refresh() } }.disabled(loading || !workspace.client.connected || !scopeIsCurrent)
            }
            Section("终端") {
                if let terminal {
                    CrewdTerminalControlBar(control: terminal)
                    if terminal.snapshot?.kind == .pty {
                        CrewdMobileTerminalView(control: terminal).frame(minHeight: 280)
                    } else {
                        Text(terminal.snapshot?.text ?? "后台尚未提供PTY屏幕。")
                            .font(.system(.footnote, design: .monospaced)).textSelection(.enabled)
                    }
                } else {
                    Text(inspection["screen"].string ?? "正在等待真实会话快照…")
                        .font(.system(.footnote, design: .monospaced)).textSelection(.enabled)
                    Text("当前后台未开放终端控制；仅显示真实只读快照。")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            if !inspection["items"].array.isEmpty {
                Section("成员输出") {
                    ForEach(inspection["items"].array.indices, id: \.self) { index in
                        CrewdRunnerItemView(item: inspection["items"].array[index])
                    }
                }
            }
            if decision != .null {
                Section("终端等待选择") {
                    Text(decision["summary"].string ?? "等待确认")
                    if decision["kind"] == .string("runner_request"), workspace.sessionID == sessionID {
                        CrewdHumanRequestView(workspace: workspace, request: inspection["runnerRequest"], decision: decision)
                            .disabled(!controlEnabled)
                    } else if decision["numbered"] == .bool(true) {
                        ForEach(decision["options"].array.indices, id: \.self) { index in
                            Button(decision["options"].array[index].string ?? "选项") {
                                command("session.answer", fields: ["fingerprint": decision["fingerprint"], "choice": .number(Double(index))])
                            }.disabled(!controlEnabled)
                        }
                    } else {
                        Text("后台未提供可确认的编号选择，请在电脑端核对。")
                    }
                }
            }
            if workspace.profileForSession(sessionID)?.backend == "codex" {
                Section("Codex 会话") {
                    CrewdCodexSessionControls(workspace:workspace,sessionID:sessionID)
                }
            }
            if workspace.profileForSession(sessionID)?.backend == "existingClaudePty"{Section("Claude 受管记录"){CrewdClaudeSessionControls(workspace:workspace,sessionID:sessionID,legacyResume:(controlEnabled && inspection["live"] != .null && inspection["live"]["running"] != .bool(true) && workspace.controlTargets[sessionID] != nil) ? {command("session.resume")}:nil)}}
            Section("会话控制") {
                Button("中断当前回合") { command("session.interrupt") }.disabled(!controlEnabled || inspection["live"]["running"] != .bool(true))
                Button("停止成员", role: .destructive) { command("session.stop", fields: ["reason": .string("人类在移动会话页明确停止")]) }.disabled(!controlEnabled || inspection["live"]["running"] != .bool(true))
                if inspection["live"] != .null, inspection["live"]["running"] != .bool(true),workspace.profileForSession(sessionID)?.backend != "existingClaudePty" {
                    Button("恢复这个原会话") { command("session.resume") }
                        .disabled(!controlEnabled || workspace.controlTargets[sessionID] == nil)
                }
                Text("离开此页面或关闭手机 App 不会停止电脑上的私人团队。")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .listStyle(.insetGrouped).tint(Theme.Palette.accent)
        .task(id: sessionID) {
            workspace.selectSession(sessionID)
            if workspace.client.capabilities.contains("session.terminal") {
                terminal?.retire(); terminal = CrewdPublicTerminal(workspace: workspace, sessionID: sessionID)
            }
            await refresh()
        }
        .onDisappear { terminal?.retire(); terminal = nil }
        .onReceive(workspace.authorityEvents) { event in
            if event.1["sessionId"] == .string(sessionID) { Task { await refresh() } }
        }
    }
    private func refresh() async {
        guard !loading, scopeIsCurrent else { return }
        loading = true; defer { loading = false }
        let epoch = workspace.observationGeneration
        do {
            let next = try await workspace.readValue("session.inspect", .object(["sessionId": .string(sessionID)]))
            guard !Task.isCancelled, scopeIsCurrent, workspace.acceptsObservation(epoch) else { return }
            if workspace.client.capabilities.contains("command.receipts") {
                _ = try await workspace.refreshControlTarget(sessionID)
                guard workspace.acceptsObservation(epoch) else { return }
            }
            inspection = next; loadError = nil
        } catch { loadError = error.localizedDescription }
    }
    private func command(_ method: String, fields: [String: CrewdValue] = [:]) {
        guard controlEnabled else { return }
        let epoch = workspace.observationGeneration
        var params = fields
        if workspace.client.capabilities.contains("command.receipts") {
            guard let target = workspace.controlTargets[sessionID] else { loadError = "原代际尚未读取；未提交控制。"; return }
            if method == "session.interrupt" || method == "session.resume" {
                guard case .object(let value) = target.value else { return }; params = value
            } else if method == "session.answer" {
                // A TUI numbered decision is not a structured runner request.
                loadError = "此选择没有正式结构化请求身份；请取得手工终端控制后处理。"; return
            } else { params["target"] = target.value }
        } else { params["sessionId"] = .string(sessionID) }
        controlsBusy = true
        Task {
            defer { controlsBusy = false }
            guard workspace.acceptsObservation(epoch) else { return }
            do { _ = try await workspace.performValue(method, .object(params)); await refresh() }
            catch { if workspace.acceptsObservation(epoch) { loadError = error.localizedDescription } }
        }
    }

}
#endif
