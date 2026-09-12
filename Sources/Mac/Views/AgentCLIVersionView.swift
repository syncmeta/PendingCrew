#if os(macOS)
import SwiftUI

/// 一个 harness（claude / codex）在设置里的那一整块。
///
/// ## 为什么不是原来那个「点一下弹个框」
///
/// 人类 2026-09-12 的原话：「**我不希望点击之后再出一个框。**我希望设置里面专门有一个
/// tab 设置编码工具……每一个 harness 都有一块设置的地方，**不用点了才出来**。
/// 要能更新、设置目录、检测。」
///
/// 所以这里把原来藏在 popover 里的全部动作摊平在页面上。名字沿用
/// `AgentCLIVersionView` 没有改 —— 它在 `ViewWiringTests` 的接线表里挂着，
/// 换个名字只是让那张表跟着动一遍，对人没有任何区别。
struct AgentCLIVersionView: View {
    @ObservedObject var center: AgentCLIVersionCenter
    let kind: LocalCodingAgentKind

    @State private var target = ""
    @State private var directory = ""
    @State private var pending: AgentCLIVersionCenter.Action?
    @State private var confirmedInstallation: AgentCLIInstallation?
    @State private var confirming = false

    private var installation: AgentCLIInstallation? { center.installations[kind] }
    private var busy: Bool { center.busy.contains(kind) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            directoryRow
            if let installation {
                pathRows(installation)
                actions(installation)
            }
            Button("重新检测") { Task { await center.refresh(kind) } }
            if let error = center.errors[kind] {
                Text("操作失败 / 检测异常：\(error)\n上方版本若存在，是最后一次成功检测值。")
                    .foregroundStyle(.red).font(.caption).textSelection(.enabled)
            }
            if let result = center.results[kind] {
                ScrollView {
                    Text(result).font(.caption).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 140)
            }
        }
        .disabled(busy)
        .onAppear { directory = LocalCodingAgentExecutable.overrideDirectory(kind) ?? "" }
        .confirmationDialog(
            "确认维护 \(kind.displayName)？", isPresented: $confirming, titleVisibility: .visible
        ) {
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

    // MARK: - 片段

    private var header: some View {
        HStack(spacing: 6) {
            Text(kind.displayName).font(.headline)
            if center.errors[kind] != nil {
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            }
            Text(installation?.version ?? (busy ? "检测中" : "版本未知"))
                .monospacedDigit().foregroundStyle(.secondary)
            if busy { ProgressView().controlSize(.small) }
        }
    }

    /// 「设置目录」。留空 = 自动搜索（登录 shell 的 PATH + 一串常见安装位）。
    /// 填错什么样当场说，不让人保存完再去猜为什么 session 还是起不来。
    private var directoryRow: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                TextField("CLI 所在目录（留空 = 自动搜索）", text: $directory)
                    .textFieldStyle(.roundedBorder)
                Button("选择…") { chooseDirectory() }
                Button("应用") { applyDirectory() }
            }
            if let problem = LocalCodingAgentExecutable.overrideProblem(kind) {
                Text(problem).font(.caption).foregroundStyle(.orange)
            } else if LocalCodingAgentExecutable.overrideDirectory(kind) != nil {
                Text("已指定：自动搜索会被跳过，就用这个目录里的 \(kind.binaryName)。")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func pathRows(_ installation: AgentCLIInstallation) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(installation.executable.path + "\n→ " + installation.resolved.path)
                .font(.caption).textSelection(.enabled)
            Text("检测于 \(installation.checkedAt.formatted(date: .omitted, time: .standard))"
                 + " · " + (installation.native
                            ? "原生安装" : "brew/npm/自定义安装：仅检测，请用原安装渠道管理"))
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func actions(_ installation: AgentCLIInstallation) -> some View {
        if installation.native {
            if kind == .claudeCode {
                TextField("升级目标：留空 / stable / latest / 具体版本", text: $target)
                    .textFieldStyle(.roundedBorder)
            }
            HStack {
                Button("检查更新并升级…") { confirm(.update(target: target), installation) }
                if !installation.rollbackReleases.isEmpty {
                    Menu("回滚到本机保留版本…") {
                        ForEach(installation.rollbackReleases, id: \.self) { release in
                            Button(release) { confirm(.rollback(release: release), installation) }
                        }
                    }
                    .fixedSize()
                }
                if kind == .claudeCode {
                    Button("健康检查（doctor）") {
                        Task { await center.perform(.doctor, installation: installation) }
                    }
                }
            }
        }
    }

    // MARK: - 动作

    private func chooseDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "选择"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        directory = url.path
        applyDirectory()
    }

    private func applyDirectory() {
        LocalCodingAgentExecutable.setOverrideDirectory(directory, for: kind)
        Task { await center.refresh(kind) }
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
