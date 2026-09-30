#if os(macOS)
import SwiftUI

/// Structured Codex session, close to Codex Desktop's information hierarchy:
/// assistant prose stays full strength; process details become short natural-language
/// activity rows. Exact commands and paths stay collapsed until the user asks for them
/// by opening the row; stdout/diffs remain out of the conversation surface (Todo #90).
struct CodexTranscriptView: View {
    @ObservedObject var transcript: CodexTranscript
    @ObservedObject private var cliVersions = AgentCLIVersionCenter.shared
    fileprivate static let bottomAnchorID = "__codex_bottom__"

    var body: some View {
        VStack(spacing: 0) {
            CodexCLIProvisioningNotice(center: cliVersions)
            ScrollViewReader { proxy in
                ScrollView {
                    CodexTranscriptRows(transcript: transcript)
                }
                .defaultScrollAnchor(.bottom)
                .onChange(of: transcript.items.count) { _, _ in
                    withAnimation { proxy.scrollTo(Self.bottomAnchorID, anchor: .bottom) }
                }
                .onChange(of: transcript.turnActive) { _, _ in
                    withAnimation { proxy.scrollTo(Self.bottomAnchorID, anchor: .bottom) }
                }
            }
        }
    }
}

/// A single action surface for the pre-run member list and an open Codex
/// transcript. Only the core's typed state enables an install action; error
/// prose from a session or a whiteboard entry is never interpreted as one.
struct CodexCLIProvisioningNotice: View {
    @ObservedObject var center: AgentCLIVersionCenter

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let state = center.codexProvisioningState {
                notice(for: state)
            }
            // This owner must outlive `.missing` -> `.awaitingSecondConfirmation`.
            CodexCLIInstallActionButton(center: center)
                .padding(.horizontal, 12)
        }
    }

    @ViewBuilder
    private func notice(for state: CodexCLIProvisioningState) -> some View {
        switch state {
        case .ready:
            EmptyView()
        case let .missing(guide):
            VStack(alignment: .leading, spacing: 5) {
                Text("未找到独立 Codex CLI")
                    .font(Theme.Fonts.footnote.weight(.semibold))
                Text("官方来源：\(guide.registry) · \(guide.package)@\(guide.version)")
                    .font(Theme.Fonts.caption)
                    .textSelection(.enabled)
                HStack {
                    Link("查看官方安装指引", destination: guide.documentation)
                }
            }
            .modifier(ProvisioningNoticeCard())
        case .awaitingSecondConfirmation:
            Text("安装计划已准备，等待第二次确认；尚未下载安装。")
                .modifier(ProvisioningNoticeCard())
        case .installing:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("正在安装并复验 Codex CLI…")
            }
            .modifier(ProvisioningNoticeCard())
        case let .installedNeedsManualSignIn(installed):
            Text("CLI \(installed.version) 已复验。请由你自行完成原生 Codex 登录后再创建 session；这里未验证认证状态。")
                .modifier(ProvisioningNoticeCard())
        case let .failed(message):
            VStack(alignment: .leading, spacing: 5) {
                Text("Codex CLI 安装或检测未完成：\(message)")
                    .foregroundStyle(Theme.Palette.danger)
                    .textSelection(.enabled)
                Button("重新检测") { Task { await center.refresh(.codex) } }
                    .accessibilityLabel("重新检测 Codex CLI")
            }
            .modifier(ProvisioningNoticeCard())
        }
    }
}

/// An action attached to a typed Codex-missing error or the explicit CLI
/// notice. It never accepts a command or package from message prose.
struct CodexCLIInstallActionButton: View {
    @ObservedObject var center: AgentCLIVersionCenter
    @State private var pendingAction: CodexCLIInstallAction?
    @State private var showingConfirmation = false

    var body: some View {
        Group {
            if case .missing = center.codexProvisioningState {
                Button("准备安装…") {
                    guard let action = center.prepareCodexInstall() else { return }
                    pendingAction = action
                    showingConfirmation = true
                }
                .accessibilityLabel("准备安装官方 Codex CLI")
            }
        }
        .confirmationDialog(
            "确认安装官方 Codex CLI？",
            isPresented: $showingConfirmation,
            titleVisibility: .visible
        ) {
            Button("确认安装") {
                guard let pendingAction else { return }
                self.pendingAction = nil
                Task { await center.installCodex(pendingAction) }
            }
            Button("取消", role: .cancel) {
                pendingAction = nil
                Task { await center.refresh(.codex) }
            }
        } message: {
            if let pendingAction {
                Text("来源：\(pendingAction.registry)\n目标：\(pendingAction.target.path)\n参数：\(pendingAction.arguments.joined(separator: " "))\n此操作将安装并复验 CLI；不会读取认证状态或自动启动 session。")
            }
        }
        .onChange(of: showingConfirmation) { _, showing in
            if !showing && pendingAction != nil {
                pendingAction = nil
                Task { await center.refresh(.codex) }
            }
        }
    }
}

private struct ProvisioningNoticeCard: ViewModifier {
    func body(content: Content) -> some View {
        content
            .font(Theme.Fonts.caption)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Theme.Palette.surface)
            .overlay(alignment: .bottom) { Divider() }
    }
}

/// The row list, split out from the `ScrollView` shell so `ImageRenderer` snapshots
/// work (a `ScrollView` renders blank off-screen; the VStack form renders). `lazy`
/// is true on the live path (perf) and false for snapshots (eager render).
struct CodexTranscriptRows: View {
    @ObservedObject var transcript: CodexTranscript
    var lazy: Bool = true

    var body: some View {
        Group {
            if lazy {
                LazyVStack(alignment: .leading, spacing: 10) { content }
            } else {
                VStack(alignment: .leading, spacing: 10) { content }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(maxWidth: 760)
        .padding(.horizontal, 20)
        .padding(.vertical, 22)
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder private var content: some View {
        ForEach(transcript.items) { item in
            row(item).id(item.id)
        }
        if transcript.turnActive {
            workingRow.id("__spinner__")
        }
        Color.clear.frame(height: 1).id(CodexTranscriptView.bottomAnchorID)
    }

    // MARK: - Row dispatch

    @ViewBuilder
    private func row(_ item: CodexThreadItem) -> some View {
        switch item.kind {
        case let .userMessage(text):
            userRow(text, delivery: transcript.inputDelivery[item.id])
        case let .agentMessage(text, _):
            agentRow(text)
        case let .reasoning(summary, content):
            if !pick(summary, content).isEmpty {
                activityRow(icon: "brain.head.profile", text: "进行了分析")
            }
        case let .plan(text):
            signalRow(.accent, "计划", text)
        case let .commandExecution(c):
            expandableActivityRow(CodexActivityPresentation.command(c))
        case let .fileChange(change):
            expandableActivityRow(CodexActivityPresentation.fileChange(change))
        case let .toolCall(name, status):
            expandableActivityRow(CodexActivityPresentation.tool(name: name, status: status))
        case let .webSearch(query):
            expandableActivityRow(CodexActivityPresentation.webSearch(query: query))
        case .unknown:
            EmptyView()   // codex drops internal/unknown signals; we don't echo them
        }
    }

    private func pick(_ a: String?, _ b: String?) -> String {
        if let a, !a.isEmpty { return a }
        if let b, !b.isEmpty { return b }
        return ""
    }

    // MARK: - Conversation (the only full-strength voice)

    /// Agent prose — full-strength markdown, the way codex keeps the model's message
    /// the focus (everything else a recessed annotation). `allowCodeRun:false`: a
    /// transcript is read-only, no chat-style "run" toolbar on code blocks.
    @ViewBuilder
    private func agentRow(_ text: String) -> some View {
        HStack(alignment: .top) {
            MarkdownText(text: text, variant: .codexTranscript, allowCodeRun: false)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    RoundedRectangle(cornerRadius: Theme.Metrics.bubbleRadius,
                                     style: .continuous)
                        .fill(Theme.Palette.surface))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.Metrics.bubbleRadius,
                                     style: .continuous)
                        .strokeBorder(Theme.Palette.hairline, lineWidth: 0.5))
                .frame(maxWidth: Theme.Metrics.readableColumn, alignment: .leading)
            Spacer(minLength: 36)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Human input — full-strength, marked by a leading accent rule (codex's `▌`).
    @ViewBuilder
    private func userRow(_ text: String, delivery: CodexTranscript.InputDelivery?) -> some View {
        HStack(alignment: .top) {
            Spacer(minLength: 36)
            VStack(alignment: .trailing, spacing: 3) {
                Text(text)
                    .font(Theme.Fonts.system(size: 15))
                    .lineSpacing(5)
                    .foregroundStyle(Theme.Palette.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(
                        RoundedRectangle(cornerRadius: Theme.Metrics.bubbleRadius,
                                         style: .continuous)
                            .fill(Theme.Palette.userBubble))
                if let delivery {
                    Text(delivery == .failed ? "发送失败" : delivery == .queued ? "排队中" : "已发送")
                        .font(Theme.Fonts.caption2)
                        .foregroundStyle(delivery == .failed ? Theme.Palette.danger : Theme.Palette.inkMuted)
                        .accessibilityLabel(delivery == .failed ? "消息发送失败" : delivery == .queued ? "消息排队中" : "消息已发送")
                }
            }
            .frame(maxWidth: Theme.Metrics.readableColumn, alignment: .trailing)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }

    // MARK: - Process signals (recessed one-liners)

    enum Dot { case bad, muted, accent }

    private func dotColor(_ d: Dot) -> Color {
        switch d {
        case .bad:    return Theme.Palette.danger
        case .muted:  return Theme.Palette.inkMuted
        case .accent: return Theme.Palette.accent
        }
    }

    /// `• 动词 内容` — the dim one-liner codex uses for exec/tool/search: present but quiet.
    @ViewBuilder
    private func signalRow(_ dot: Dot, _ verb: String, _ body: String) -> some View {
        HStack(alignment: .top, spacing: 7) {
            Circle().fill(dotColor(dot)).frame(width: 5, height: 5).padding(.top, 5)
            Text(verb + "  ").font(Theme.Fonts.caption.weight(.semibold)).foregroundColor(Theme.Palette.inkMuted) +
                Text(body).font(Theme.Fonts.caption).foregroundColor(Theme.Palette.inkMuted)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func activityRow(icon: String, text: String, bad: Bool = false) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .frame(width: 16)
            Text(text)
        }
        .font(Theme.Fonts.footnote)
        .foregroundStyle(bad ? Theme.Palette.danger : Theme.Palette.inkMuted)
        .padding(.vertical, 3)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func expandableActivityRow(_ presentation: CodexActivityPresentation) -> some View {
        if presentation.details.isEmpty {
            activityRow(
                icon: presentation.icon,
                text: presentation.headline,
                bad: presentation.bad)
        } else {
            DisclosureGroup {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(presentation.details) { detail in
                        VStack(alignment: .leading, spacing: 3) {
                            Text(detail.label)
                                .font(Theme.Fonts.caption2.weight(.semibold))
                                .foregroundStyle(Theme.Palette.inkMuted)
                            Text(detail.value)
                                .font(Theme.Fonts.monoSmall)
                                .foregroundStyle(Theme.Palette.ink)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .padding(.leading, 24)
                .padding(.top, 5)
                .padding(.bottom, 3)
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: presentation.icon).frame(width: 16)
                    Text(presentation.headline)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .font(Theme.Fonts.footnote)
                .foregroundStyle(
                    presentation.bad ? Theme.Palette.danger : Theme.Palette.inkMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .tint(presentation.bad ? Theme.Palette.danger : Theme.Palette.inkMuted)
            .padding(.vertical, 3)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder
    private var workingRow: some View {
        HStack(spacing: 7) {
            ProgressView().controlSize(.small)
            Text("运行中…")
                .font(Theme.Fonts.caption.italic())
                .foregroundStyle(Theme.Palette.inkMuted)
        }
        .padding(.vertical, 2)
    }
}

#endif
