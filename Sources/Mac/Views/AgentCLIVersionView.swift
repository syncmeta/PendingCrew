#if os(macOS)
import SwiftUI

struct AgentCLIVersionView: View {
    @ObservedObject var center: AgentCLIVersionCenter
    let kind: LocalCodingAgentKind
    @State private var expanded = false
    @State private var target = ""
    @State private var pending: AgentCLIVersionCenter.Action?
    @State private var confirmedInstallation: AgentCLIInstallation?
    @State private var confirming = false

    private var installation: AgentCLIInstallation? { center.installations[kind] }
    private var busy: Bool { center.busy.contains(kind) }

    var body: some View {
        Button {
            expanded.toggle()
        } label: {
            HStack(spacing: 3) {
                if center.errors[kind] != nil { Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange) }
                Text(installation?.version ?? (busy ? "检测中" : "版本未知"))
                    .monospacedDigit()
                Image(systemName: "chevron.down")
            }
            // 字号跟设置页 Form 的正文走（Todo #131 之前它挂在侧栏页脚，那里是
            // 10pt 小字；搬进设置后 10pt 会小得像坏了）。
            .font(.callout)
        }
        .buttonStyle(.plain)
        .help("\(kind.displayName) 版本管理\(center.errors[kind].map { "：" + $0 } ?? "")")
        .popover(isPresented: $expanded) {
            VStack(alignment: .leading, spacing: 12) {
                Text("\(kind.displayName) · 版本管理").font(.headline)
                if let installation {
                    Text("本机版本 \(installation.version)")
                    Text("检测于 \(installation.checkedAt.formatted(date: .omitted, time: .standard))")
                        .font(.caption).foregroundStyle(.secondary)
                    Text(installation.executable.path + "\n→ " + installation.resolved.path)
                        .font(.caption).textSelection(.enabled)
                    Text(installation.native ? "原生安装" : "brew/npm/自定义安装：仅检测，请用原安装渠道管理。")
                        .font(.caption)
                    if installation.native {
                        if kind == .claudeCode {
                            TextField("升级目标：留空 / stable / latest / 具体版本", text: $target)
                        }
                        Button("检查更新并升级…") { confirm(.update(target: target), installation) }
                        if !installation.rollbackReleases.isEmpty {
                            Menu("回滚到本机保留版本…") {
                                ForEach(installation.rollbackReleases, id: \.self) { release in
                                    Button(release) { confirm(.rollback(release: release), installation) }
                                }
                            }
                        }
                    }
                    if kind == .claudeCode {
                        Button("运行健康检查（doctor）") {
                            Task { await center.perform(.doctor, installation: installation) }
                        }
                    }
                }
                Button("重新检测版本") { Task { await center.refresh(kind) } }
                if busy { ProgressView().controlSize(.small) }
                if let error = center.errors[kind] {
                    Text("操作失败 / 检测异常：\(error)\n上方版本若存在，是最后一次成功检测值。")
                        .foregroundStyle(.red).font(.caption).textSelection(.enabled)
                }
                if let result = center.results[kind] {
                    ScrollView { Text(result).font(.caption).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                        .frame(maxHeight: 180)
                }
                Text("自动检测每 10 分钟一次，只读取本机版本，不判断是否最新；升级须由人确认。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(16).frame(width: 420)
            .disabled(busy)
            .confirmationDialog("确认维护 \(kind.displayName)？", isPresented: $confirming, titleVisibility: .visible) {
                Button("确认执行") {
                    if let pending, let confirmedInstallation {
                        Task { await center.perform(pending, installation: confirmedInstallation) }
                    }
                }
                Button("取消", role: .cancel) {}
            } message: {
                Text(confirmationText)
            }
        }
    }

    private var confirmationText: String {
        let action: String
        switch pending {
        case let .update(target): action = "执行 \(kind.binaryName) update\(target.isEmpty ? "" : " " + target)"
        case let .rollback(release): action = "把 Codex current 切换到 \(release)"
        default: action = "维护 CLI"
        }
        return "\(action)。\n执行前必须通过：① PendingCrew 所有 crew 中该 runner 的存活 session 数为 0（含空闲、启动中、等审批）；② 本机进程扫描没有该 runner。检测失败也会拒绝执行。\n维护期间禁止 PendingCrew 启动同类 session。外部终端不受锁控制，请勿同时启动。Unix 已打开的二进制不受符号链接切换影响，旧 session 不会当场崩，但新 session 会用新版，可能出现两版并存，因此要先全部停止。"
    }

    private func confirm(_ action: AgentCLIVersionCenter.Action, _ installation: AgentCLIInstallation) {
        pending = action
        confirmedInstallation = installation
        confirming = true
    }
}
#endif
