#if os(macOS)
import SwiftUI

/// 一个 runner 在设置里的版本、默认配置与维护操作。
struct AgentCLIVersionView: View {
    @ObservedObject var center: AgentCLIVersionCenter
    let kind: LocalCodingAgentKind

    @State private var target = ""
    @State private var pending: AgentCLIVersionCenter.Action?
    @State private var confirmedInstallation: AgentCLIInstallation?
    @State private var confirming = false
    @State private var defaultModel = ""
    @State private var defaultFastMode = false
    @ObservedObject private var catalog = ModelCatalogCenter.shared

    private var installation: AgentCLIInstallation? { center.installations[kind] }
    private var busy: Bool { center.busy.contains(kind) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            launchDefaults
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
        .onAppear {
            defaultModel = AgentLaunchPreferences.model(for: kind) ?? ""
            defaultFastMode = AgentLaunchPreferences.fastMode(for: kind)
        }
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

    private var launchDefaults: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("新 session 默认配置").font(.subheadline.weight(.medium))
            Picker("默认模型", selection: $defaultModel) {
                Text("沿用 \(kind.displayName) 当前默认").tag("")
                if !defaultModel.isEmpty,
                   !SessionLaunchOptions.models(for: kind, catalog: catalog.file).contains(defaultModel) {
                    Text(defaultModel).tag(defaultModel)
                }
                ForEach(SessionLaunchOptions.models(for: kind, catalog: catalog.file), id: \.self) { model in
                    Text(SessionLaunchOptions.displayName(for: model, catalog: catalog.file)).tag(model)
                }
            }
            .onChange(of: defaultModel) { _, value in
                UserDefaults.standard.set(value, forKey: "pendingcrew.defaultModel.\(kind.rawValue)")
                Task { await catalog.refresh() }
            }
            Toggle("默认开启快速模式", isOn: $defaultFastMode)
                .onChange(of: defaultFastMode) { _, value in
                    UserDefaults.standard.set(value, forKey: "pendingcrew.defaultFastMode.\(kind.rawValue)")
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
