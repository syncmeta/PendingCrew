#if os(macOS)
import SwiftUI

/// Structured transcript for a codex app-server session — modeled on codex's own TUI:
/// the assistant's prose is the sole full-strength voice; every process signal
/// (reasoning / command / tool call / file edit / search) recedes into a dim,
/// bullet-prefixed one-liner. Long command output collapses to a head + "… +N 行"
/// (click to expand). Unknown item kinds are dropped — codex's UI filters the
/// protocol stream, it doesn't echo every item 1:1.
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
            CodexReasoningRow(text: pick(summary, content))
        case let .plan(text):
            signalRow(.accent, "计划", text)
        case let .commandExecution(c):
            CodexCommandRow(command: c.command, output: c.aggregatedOutput,
                            exitCode: c.exitCode, status: c.status)
        case let .fileChange(f):
            signalRow(.muted, "改动", f.summary ?? f.status ?? "")
        case let .toolCall(name, status):
            signalRow(status == "failed" ? .bad : .muted, "调用", name)
        case let .webSearch(query):
            signalRow(.muted, "搜索", query ?? "")
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

// MARK: - Reasoning (dim + italic, codex's quietest cell; collapses past ~6 lines)

private struct CodexReasoningRow: View {
    let text: String
    @State private var expanded = false

    var body: some View {
        if text.isEmpty {
            EmptyView()
        } else {
            HStack(alignment: .top, spacing: 7) {
                Circle().fill(Theme.Palette.inkMuted.opacity(0.5)).frame(width: 5, height: 5)
                    .padding(.top, 5)
                VStack(alignment: .leading, spacing: 2) {
                    Text(text)
                        .font(Theme.Fonts.footnote.italic())
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .lineLimit(expanded ? nil : 6)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                    if !expanded && text.count > 280 {
                        Button("展开思考") { expanded = true }
                            .buttonStyle(.plain)
                            .font(Theme.Fonts.caption2)
                            .foregroundStyle(Theme.Palette.accent)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: - Command execution (codex's `• 运行 <cmd>` with collapsed output)

private struct CodexCommandRow: View {
    let command: String
    let output: String?
    let exitCode: Int?
    let status: String?
    @State private var expanded = false
    private let headLines = 6

    private var dotColor: Color {
        if let exitCode { return exitCode == 0 ? Theme.Palette.success : Theme.Palette.danger }
        return Theme.Palette.inkMuted
    }
    private var verb: String {
        if exitCode == nil && (status == nil || status == "inProgress") { return "运行中" }
        return "运行"
    }
    private var lines: [Substring] { (output ?? "").split(separator: "\n", omittingEmptySubsequences: false) }
    private var hiddenCount: Int { max(0, lines.count - headLines) }
    private var hasOutput: Bool { !(output ?? "").isEmpty }

    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            Circle().fill(dotColor).frame(width: 5, height: 5).padding(.top, 5)
            VStack(alignment: .leading, spacing: 3) {
                Text(verb + "  ").font(Theme.Fonts.caption.weight(.semibold)).foregroundColor(Theme.Palette.inkMuted) +
                    Text(command).font(Theme.Fonts.monoSmall).foregroundColor(Theme.Palette.ink)
                if hasOutput {
                    Text((expanded ? lines : Array(lines.prefix(headLines))).joined(separator: "\n"))
                        .font(Theme.Fonts.monoSmall)
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                    if !expanded && hiddenCount > 0 {
                        Button("… +\(hiddenCount) 行") { expanded = true }
                            .buttonStyle(.plain)
                            .font(Theme.Fonts.caption2)
                            .foregroundStyle(Theme.Palette.accent)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
#endif
