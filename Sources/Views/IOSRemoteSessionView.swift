#if os(iOS)
import SwiftUI

/// iOS 的远端 session 详情。`RemoteSessionChannel` 与 crew RPC 由同一个
/// `RemotePendingCrewBackend`/TLS link 驱动；这里不建 transport，也不读 Mac 文件。
struct IOSRemoteSessionView: View {
    @EnvironmentObject private var appModel: AppModel
    let crewID: String
    let sessionID: String

    @State private var channel: RemoteSessionChannel?
    @State private var loadError: String?
    @State private var loading = false

    var body: some View {
        Group {
            if let channel {
                IOSRemoteSessionDetail(
                    channel: channel, retry: { Task { await load(reconnect: true) } })
            } else if loading {
                ProgressView("正在连接远端 session…")
            } else {
                ContentUnavailableView {
                    Label("远端 session 不可用", systemImage: "wifi.exclamationmark")
                } description: {
                    Text(loadError ?? "连接尚未建立。")
                } actions: {
                    Button("重试") { Task { await load(reconnect: true) } }
                }
            }
        }
        .navigationTitle("Session")
        .navigationBarTitleDisplayMode(.inline)
        .task(id: sessionID) { await load(reconnect: false) }
        .onDisappear { appModel.remoteSessionBackend?.closeSession(sessionID: sessionID) }
    }

    @MainActor
    private func load(reconnect: Bool) async {
        guard !loading else { return }
        guard let backend = appModel.remoteSessionBackend else {
            channel = nil
            loadError = "尚未配置已配对的远端 Mac。"
            return
        }
        loading = true
        loadError = nil
        do {
            if reconnect { try await backend.reconnect() }
            channel = try await backend.openSession(sessionID: sessionID)
        } catch {
            channel = nil
            loadError = error.localizedDescription
        }
        loading = false
    }
}

private struct IOSRemoteSessionDetail: View {
    @ObservedObject var channel: RemoteSessionChannel
    let retry: () -> Void

    var body: some View {
        List {
            Section("连接") {
                connectionRow
                if case .disconnected = channel.connectionState {
                    Button("重新连接", action: retry)
                }
            }

            Section("终端") {
                ScrollView(.horizontal) {
                    Text(outputText)
                        .font(.system(.footnote, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(minHeight: 180)
            }

            if let decision = channel.summary?.state.pendingDecision {
                Section("终端等待选择") {
                    Text(decision.prompt)
                    if decision.numbered {
                        ForEach(Array(decision.options.enumerated()), id: \.offset) { index, option in
                            Button(option) { channel.chooseTerminalDecision(optionIndex: index) }
                        }
                    } else {
                        ForEach(decision.options, id: \.self) { Text($0) }
                        Text("这类菜单没有传回当前高亮项；请在 Mac 终端中选择。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }

        }
    }

    private var outputText: String {
        if !channel.terminalText.isEmpty { return channel.terminalText }
        if !channel.transcriptText.isEmpty { return channel.transcriptText }
        switch channel.connectionState {
        case .connecting: return "正在等待首屏…"
        case .connected: return "远端暂未产生输出。"
        case let .disconnected(reason): return "连接已断开：\(reason)"
        }
    }

    @ViewBuilder
    private var connectionRow: some View {
        switch channel.connectionState {
        case .connecting:
            Label("正在连接", systemImage: "arrow.triangle.2.circlepath")
        case .connected:
            Label("已安全连接", systemImage: "lock.fill").foregroundStyle(.green)
        case let .disconnected(reason):
            VStack(alignment: .leading, spacing: 4) {
                Label("连接已断开", systemImage: "wifi.slash").foregroundStyle(.red)
                Text(reason).font(.caption).foregroundStyle(.secondary)
            }
        }
    }

}
#endif
