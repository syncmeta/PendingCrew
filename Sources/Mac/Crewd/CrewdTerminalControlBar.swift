import SwiftUI

/// Explicit Human custody controls on the original session surface. Closing the
/// surface disposes only this authenticated observation/control connection.
struct CrewdTerminalControlBar: View {
    @ObservedObject var control: CrewdPublicTerminal
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Label(control.connected ? "后台终端观察" : "终端未连接", systemImage: "terminal")
                Spacer()
                if control.leaseID == nil {
                    Button("取得手工控制") { Task { await control.claim() } }
                        .disabled(control.busy || !control.connected || control.snapshot?.manualAvailable != true)
                } else {
                    Button("结束手工控制") { Task { await control.release() } }.disabled(control.busy)
                }
                Button("重新读取屏幕") {
                    control.deactivate()
                    Task { await control.connect() }
                }.disabled(control.busy)
            }
            if let failure = control.failure { Text(failure).font(.caption).foregroundStyle(.secondary) }
            if !control.unresolvedInput.isEmpty {
                DisclosureGroup("本页面未提交字节：\(control.unresolvedInput.count) · 不自动重发") {
                    Text(control.unresolvedInput.base64EncodedString()).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    Text("这是本页面内存中的原始字节Base64；已提交的未知输入另在原请求核对中耐久保留。")
                        .font(.caption2).foregroundStyle(.secondary)
                }.font(.caption).foregroundStyle(.orange)
            }
            Text("手工控制限时；写入仅确认PTY内核接受字节，不代表应用执行成功。")
                .font(.caption2).foregroundStyle(.secondary)
        }.padding(8)
        .task { await control.connect() }
        .onDisappear { control.deactivate() }
    }
}
