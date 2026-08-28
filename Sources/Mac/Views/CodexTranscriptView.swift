#if os(macOS)
import SwiftUI

/// Structured Codex session, close to Codex Desktop's information hierarchy:
/// assistant prose stays full strength; process details become short natural-language
/// activity rows. Raw commands, paths, diffs and stdout are intentionally not rendered
/// in the default conversation surface (Todo #83).
struct CodexTranscriptView: View {
    @ObservedObject var transcript: CodexTranscript
    fileprivate static let bottomAnchorID = "__codex_bottom__"

    var body: some View {
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

/// The row list, split out from the `ScrollView` shell so `ImageRenderer` snapshots
/// work (a `ScrollView` renders blank off-screen; the VStack form renders). `lazy`
/// is true on the live path (perf) and false for snapshots (eager render).
struct CodexTranscriptRows: View {
    @ObservedObject var transcript: CodexTranscript
    var lazy: Bool = true

    var body: some View {
        Group {
            if lazy {
                LazyVStack(alignment: .leading, spacing: 7) { content }
            } else {
                VStack(alignment: .leading, spacing: 7) { content }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
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
            userRow(text)
        case let .agentMessage(text, _):
            agentRow(text)
        case let .reasoning(summary, content):
            if !pick(summary, content).isEmpty {
                activityRow(icon: "brain.head.profile", text: "进行了分析")
            }
        case let .plan(text):
            signalRow(.accent, "计划", text)
        case let .commandExecution(c):
            commandActivity(c)
        case let .fileChange(change):
            activityRow(
                icon: change.status == "failed" ? "exclamationmark.triangle" : "doc.badge.ellipsis",
                text: change.status == "failed" ? "修改档案失败" : "已修改档案",
                bad: change.status == "failed")
        case let .toolCall(_, status):
            activityRow(
                icon: "wrench.and.screwdriver",
                text: status == "failed" ? "调用工具失败" : "已调用工具",
                bad: status == "failed")
        case .webSearch:
            activityRow(icon: "globe", text: "已搜索网页")
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
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .fill(Theme.Palette.surfaceMuted))
                .frame(maxWidth: Theme.Metrics.readableColumn, alignment: .leading)
            Spacer(minLength: 36)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Human input — full-strength, marked by a leading accent rule (codex's `▌`).
    @ViewBuilder
    private func userRow(_ text: String) -> some View {
        HStack(alignment: .top) {
            Spacer(minLength: 36)
            Text(text)
                .font(Theme.Fonts.system(size: 15))
                .lineSpacing(5)
                .foregroundStyle(Theme.Palette.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .fill(Theme.Palette.surfaceMuted))
                .frame(maxWidth: Theme.Metrics.readableColumn, alignment: .leading)
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

    /// app-server 的 `commandActions` 已经把 shell 拆成 read/list/search/unknown。
    /// 这是官方协议提供的语义层，直接复用，不在客户端靠命令字符串猜。
    @ViewBuilder
    private func commandActivity(_ command: CodexThreadItem.CommandExec) -> some View {
        let kinds: Set<CodexThreadItem.CommandAction.Kind> = Set(
            command.actions.map { action in action.kind })
        let failed = command.exitCode.map { code in code != 0 } ?? (command.status == "failed")
        if failed {
            activityRow(icon: "exclamationmark.triangle", text: "执行指令失败", bad: true)
        } else if kinds.contains(.search) {
            activityRow(icon: "magnifyingglass", text: "已搜索档案")
        } else if kinds.contains(.read) || kinds.contains(.listFiles) {
            activityRow(icon: "book", text: "已读取档案")
        } else {
            activityRow(
                icon: "terminal",
                text: command.status == "inProgress" ? "正在执行指令" : "已执行指令")
        }
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
