#if os(macOS)
import SwiftUI

struct CrewdJudgmentSettingsTab: View {
    @ObservedObject var window: CrewdRemoteWindow
    var body: some View {
        Group {
            if let workspace = window.workspace {
                CrewdJudgmentSettingsPane(workspace: workspace).id(ObjectIdentifier(workspace))
            } else { Text("请先在主窗口连接可信本机电脑。远程连接不能管理本机判断模型凭据。").padding() }
        }
    }
}
private struct CrewdJudgmentSettingsPane: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject private var client: CrewdClient
    @StateObject private var model: CrewdJudgmentSettingsModel
    @State private var work: Task<Void, Never>?
    @State private var workID = UUID()
    @State private var confirmSubmission = false
    @State private var removeConfiguration = false
    init(workspace: CrewdWorkspace) {
        self.workspace = workspace; client = workspace.client
        _model = StateObject(wrappedValue: CrewdJudgmentSettingsModel(workspace: workspace))
    }
    private func run(_ action: @escaping @MainActor () async -> Void) {
        guard work == nil else { return }
        let token = UUID(); workID = token
        work = Task { await action(); if workID == token { work = nil } }
    }
    private func retire() { workID = UUID(); work?.cancel(); work = nil; model.invalidate() }
    var body: some View {
        Form {
            Section {
                Text("判断模型").font(.headline)
                Text(model.status).font(.callout).accessibilityIdentifier("judgment.settings.status")
                Text("只管理当前可信本机连接的录存设置。读取不会验证密钥、连接服务或调用模型。")
                    .font(.caption).foregroundStyle(.secondary)
                Button("只读刷新") { run { await model.refresh() } }.disabled(model.busy)
                if let view = model.view {
                    Text(credentialLabel(view.credentialState))
                    if view.pendingRequestId != nil { Text("后端存在结果未知的配置操作；保留原请求，仅可明确设置或清除。").foregroundStyle(.orange) }
                }
            }
            Section("配置") {
                Toggle("启用判断模型", isOn: $model.draft.enabled)
                Text("提供方：TypeSafe")
                TextField("HTTPS 服务端点", text: $model.draft.baseUrl)
                TextField("模型", text: $model.draft.model)
                LabeledContent("选择阈值") { TextField("0…1", value: $model.draft.choiceMin, format: .number) }
                LabeledContent("唤醒阈值") { TextField("0…1", value: $model.draft.wakeYes, format: .number) }
                Picker("凭据存储", selection: $model.draft.credentialSource) {
                    Text("系统安全存储").tag("platform")
                    Text("当前私有数据目录").tag("file")
                }
                Picker("本次凭据操作", selection: $model.credentialAction) {
                    Text("保留原凭据").tag(CrewdJudgmentIntent.Action.keep)
                    Text("明确设置新凭据").tag(CrewdJudgmentIntent.Action.set)
                    Text("明确清除凭据").tag(CrewdJudgmentIntent.Action.clear)
                }
                if model.credentialAction == .set {
                    SecureField("新凭据（不会回显或保存为草稿）", text: $model.secretDraft)
                        .accessibilityIdentifier("judgment.settings.secretDraft")
                }
                if model.credentialAction == .keep && !model.canKeep {
                    Text("未知状态、端点或存储改变时不能保留；请明确选择设置或清除。")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Button("提交本次明确操作") { removeConfiguration = false; confirmSubmission = true }
                    .disabled(!model.canSubmit)
                Button("清除凭据并移除配置", role: .destructive) {
                    model.credentialAction = .clear; model.secretDraft = ""
                    removeConfiguration = true; confirmSubmission = true
                }.disabled(!model.adminQualified || model.busy)
            }.disabled(!model.adminQualified || model.busy)
            if !model.records.isEmpty {
                Section("原请求历史（只读核对，不重发）") {
                    ForEach(model.records) { record in
                        HStack {
                            Text(record.id.uuidString.lowercased()).font(.caption).textSelection(.enabled)
                            Spacer()
                            Button("只读核对") { run { await model.refresh(requestID: record.id) } }
                                .disabled(model.busy)
                        }
                    }
                }
            }
        }
        .formStyle(.grouped).padding()
        .confirmationDialog(removeConfiguration ? "明确清除凭据并移除当前配置？" : "提交本次设置？结果未知时不会自动重试。", isPresented: $confirmSubmission, titleVisibility: .visible) {
            Button(removeConfiguration ? "清除并移除" : "提交") { let remove = removeConfiguration; run { await model.submit(removeConfiguration: remove) } }
            Button("取消", role: .cancel) { model.secretDraft = "" }
        }
        .onReceive(client.objectWillChange) { _ in
            // Published changes are delivered before mutation. Recheck after the actor turn.
            Task { @MainActor in if model.synchronizeOrigin() { workID = UUID(); work?.cancel(); work = nil } }
        }
        .onReceive(workspace.objectWillChange) { _ in Task { @MainActor in if model.synchronizeOrigin() { workID = UUID(); work?.cancel(); work = nil } } }
        .onChange(of: model.credentialAction) { _, new in if new != .set { model.secretDraft = "" } }
        .onDisappear { retire() }
    }
    private func credentialLabel(_ state: CrewdJudgmentView.CredentialState) -> String {
        switch state {
        case .missing: return "尚无录存凭据"
        case .recordedUnverified: return "凭据已录存，未验证有效性"
        case .updateUnknown: return "凭据更新结果未知"
        case .unknown: return "凭据录存状态未知"
        }
    }
}
#endif
