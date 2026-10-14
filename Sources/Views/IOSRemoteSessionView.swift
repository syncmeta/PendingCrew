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
        }.navigationTitle("Session").navigationBarTitleDisplayMode(.inline)
    }
}

private struct IOSRemoteSessionDetail: View {
    @ObservedObject var workspace: CrewdWorkspace
    let sessionID: String
    @State private var inspection: CrewdValue = .null
    @State private var loadError: String?
    @State private var loading = false
    @State private var controlsBusy = false
    private var scopeIsCurrent: Bool { workspace.crewID != "" && !workspace.isRetired }
    private var decision: CrewdValue { inspection["live"]["pendingDecision"] }
    private var controlEnabled: Bool { scopeIsCurrent && workspace.client.connected && workspace.client.capabilities.contains("orchestra") && !controlsBusy }
    var body: some View {
        List {
            Section("连接") {
                if loading { Label("正在读取", systemImage: "arrow.triangle.2.circlepath") }
                else if workspace.client.connected && scopeIsCurrent { Label("已安全连接", systemImage: "lock.fill").foregroundStyle(.green) }
                else { Label("连接已断开", systemImage: "wifi.slash").foregroundStyle(.red) }
                if let loadError { Text(loadError).font(.caption).foregroundStyle(.red) }
                Button("刷新会话") { Task { await refresh() } }.disabled(loading || !workspace.client.connected || !scopeIsCurrent)
            }
            Section("终端") {
                ScrollView(.horizontal) {
                    Text(inspection["screen"].string ?? "正在等待真实会话快照…")
                        .font(.system(.footnote, design: .monospaced)).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }.frame(minHeight: 180)
                Text("当前为后台提供的只读快照；完整终端输入和实时流尚未交付。")
                    .font(.caption).foregroundStyle(.secondary)
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
            Section("会话控制") {
                Button("中断当前回合") { command("session.interrupt") }.disabled(!controlEnabled || inspection["live"]["running"] != .bool(true))
                Button("停止成员", role: .destructive) { command("session.stop", fields: ["reason": .string("人类在移动会话页明确停止")]) }.disabled(!controlEnabled || inspection["live"]["running"] != .bool(true))
                Text("离开此页面或关闭手机 App 不会停止电脑上的私人团队。")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .task(id: sessionID) { workspace.selectSession(sessionID); await refresh() }
        .onReceive(workspace.authorityEvents) { event in
            if event.1["sessionId"] == .string(sessionID) { Task { await refresh() } }
        }
    }
    private func refresh() async {
        guard !loading, scopeIsCurrent else { return }
        loading = true; defer { loading = false }
        do {
            let next = try await workspace.readValue("session.inspect", .object(["sessionId": .string(sessionID)]))
            guard !Task.isCancelled, scopeIsCurrent else { return }
            inspection = next; loadError = nil
        } catch { loadError = error.localizedDescription }
    }
    private func command(_ method: String, fields: [String: CrewdValue] = [:]) {
        guard controlEnabled else { return }; controlsBusy = true
        var params = fields; params["sessionId"] = .string(sessionID)
        Task {
            defer { controlsBusy = false }
            do { _ = try await workspace.performValue(method, .object(params)); await refresh() }
            catch { loadError = error.localizedDescription }
        }
    }
}
#endif
