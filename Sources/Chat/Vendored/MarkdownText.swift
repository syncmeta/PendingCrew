// VENDORED from PendingBot apps/pendingbot/Sources/Components/MarkdownText.swift @ 43c8ea2e
// 再对齐 = 对照源文件重拷。仅允许偏离打 `// PENDINGCREW SHIM:` 标注。
import SwiftUI
import MarkdownUI
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

// `Theme` is ambiguous at file scope (PendingBot has one, MarkdownUI has one).
// File-private aliases keep call sites readable without leaning on module
// qualifiers. The `chatTheme` static below is `internal`, so its type alias
// must be at least the same access level (fileprivate).
// PENDINGCREW SHIM: 原文是 `PendingBot.Theme`。这里不用模块限定名 ——
// 见 AppThemeAlias.swift（要能同时编进 app 与 test bundle 两个模块）。
fileprivate typealias AppTheme = AppThemeAlias
fileprivate typealias MDTheme = MarkdownUI.Theme

/// Renders chat / log content as full Markdown — headings, lists, tables,
/// blockquotes, fenced code (with a copy + ▶ 运行 toolbar). Bot messages
/// and 来信 articles both use this, parameterized by `variant` —
///
/// - `.chat` keeps the dense, sans-serif rhythm a chat bubble needs.
/// - `.article` switches to an editorial serif body with looser line
///   height, larger heading hierarchy, and italic blockquotes — meant
///   to read like a column rather than a chat reply.
struct MarkdownText: View {
    let text: String
    /// Which typographic register to render in. `.chat` is the default
    /// because the chat surface vastly out-renders 来信 in volume.
    var variant: Variant = .chat
    /// True for the chat surface — turns on the heavyweight code-block
    /// toolbar. False keeps it lightweight (used in 来信 articles where
    /// "run" doesn't make sense).
    var allowCodeRun: Bool = false
    /// Web-search references for inline `[N]` markers. Empty disables the
    /// citation rewrite — the text renders verbatim, including any literal
    /// `[1]`-style brackets the bot might have meant as plain text.
    var citations: [MessageCitation] = []

    // PENDINGCREW SHIM (Todo #56 ⑥): Codex transcript prose is a denser desktop
    // reading surface than a crew chat bubble. Keep its typography opt-in so regular
    // chat, reasoning rows and tool output do not move with it.
    enum Variant { case chat, codexTranscript, article }

    @State private var presentedCitation: PresentedCitation?
    /// Math formulas are rasterised to bitmaps; pin the renderer to the live
    /// scheme so dark mode gets dark-variant ink (and its own cache entries).
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        Markdown(rewrittenText)
            // PENDINGCREW SHIM(代码块不渲染 2026-08-11): 代码块样式**烘进 Theme
            // 本体**（见 `resolvedTheme` / `chatCodeBlock`），不再挂
            // `.markdownBlockStyle(\.codeBlock)`。
            //
            // 原写法是 `.markdownTheme(…)` 在里层、`.markdownBlockStyle(\.codeBlock)`
            // 在外层，而 MarkdownUI 里这两个都是环境值写入：
            //   markdownTheme(t)              → .environment(\.theme, t)          // 整个换掉
            //   markdownBlockStyle(kp){…}     → .environment(\.theme[kp], …)      // 读外层 theme、改一个字段、写回
            // SwiftUI 环境值是「离视图越近的越赢」，所以里层的 markdownTheme 把
            // 外层打的那个补丁整个盖回去了 —— ChatCodeBlock 从来没被实例化过，
            // 围栏代码块一路退到 MarkdownUI `Theme()` 的默认 codeBlock
            // (`BlockStyle { $0.label }` = 拿正文字体直接画)，于是没边框/没底色/
            // 不等宽/没复制按钮、还跟正文一起按字宽折行。行内 `code` 不受影响，
            // 因为它本来就写在 theme 里 —— 这正是「行内好、围栏坏」的成因。
            //
            // 烘进 theme 之后只剩一次环境写入，顺序问题从结构上不存在了。
            .markdownTheme(resolvedTheme)
            // LaTeX math support — see MathRendering.swift. The rewrite turns
            // `\(…\)` / `$$…$$` etc. into `pendingbot-math://` image refs,
            // and these providers typeset them; non-math images fall through
            // to MarkdownUI's defaults.
            // 跨平台:MathRendering 走 SwiftMath → 平台图片(UIImage/NSImage),
            // 两端都内联渲染 LaTeX。
            .markdownImageProvider(
                MathBlockImageProvider(textColor: AppTheme.Palette.ink, fontSize: mathFontSize)
            )
            .markdownInlineImageProvider(
                MathInlineImageProvider(textColor: AppTheme.Palette.ink, fontSize: mathFontSize, scheme: colorScheme)
            )
            // PENDINGCREW SHIM (#443): 原文是 `.textSelection(.enabled)` 常开。
            // macOS 上它给每段文字挂一个真 NSTextField（`SelectionOverlay`），
            // 70 条群聊 = 上百个 NSTextField 每次视图图更新全量 updateNSView ——
            // 0.1.7 那份卡 68.68s 的 hang 报告里主线程 4/11 采样就在这条。
            // 改成由 `crewBubbleSelectable` 环境值决定：群聊里只有指针底下那条挂，
            // 其余调用点（来信正文、Todo 详情…）默认 true，行为不变。
            .crewSelectableText()
            .environment(\.openURL, OpenURLAction { url in
                // Citation taps go through a fake `pendingbot-cite://N`
                // scheme so we can intercept them here and show the source
                // sheet instead of letting Safari try to open them.
                if url.scheme == CitationScheme.scheme,
                   let n = Int(url.host ?? ""),
                   n >= 1, n <= citations.count {
                    presentedCitation = PresentedCitation(n: n, citation: citations[n - 1])
                    Haptics.tap()
                    return .handled
                }
                return .systemAction
            })
            .sheet(item: $presentedCitation) { p in
                CitationSheet(index: p.n, citation: p.citation)
            }
    }

    /// The variant's theme with `ChatCodeBlock` already baked into
    /// `\.codeBlock`. Four precombined statics rather than building a `Theme`
    /// per `body` pass — the chat list re-evaluates bodies constantly (#443),
    /// and handing SwiftUI a freshly-allocated environment value each time
    /// would churn the whole Markdown subtree.
    private var resolvedTheme: MDTheme {
        switch (variant, allowCodeRun) {
        case (.chat, false):    return MarkdownText.chatTheme
        case (.chat, true):     return MarkdownText.chatThemeRunnable
        case (.codexTranscript, false): return MarkdownText.codexTranscriptTheme
        case (.codexTranscript, true):  return MarkdownText.codexTranscriptThemeRunnable
        case (.article, false): return MarkdownText.articleTheme
        case (.article, true):  return MarkdownText.articleThemeRunnable
        }
    }

    private var rewrittenText: String {
        var t = text
        // Math first: turns LaTeX spans into `![](pendingbot-math://…)` image
        // refs. Citation rewrite only touches `[N]` markers, so the two
        // passes don't collide. 跨平台(MathMarkup 纯 Foundation)。
        if MathMarkup.containsMath(t) {
            t = MathMarkup.rewrite(t)
        }
        if !citations.isEmpty {
            t = CitationScheme.rewrite(t, citations: citations)
        }
        return t
    }

    /// Point size for typeset math — matched to each variant's body text so
    /// a formula sits at the same visual weight as the prose around it.
    private var mathFontSize: CGFloat {
        switch variant {
        case .chat: return AppTheme.Fonts.scaled(16)
        case .codexTranscript: return AppTheme.Fonts.scaled(15)
        case .article: return 17
        }
    }

    // ── Themes ─────────────────────────────────────────────────────────────

    // PENDINGCREW SHIM(代码块不渲染 2026-08-11): 每个 variant 一份「已烘入
    // 代码块样式」的成品 theme。`allowCodeRun` 也进 key，因为它是 ChatCodeBlock
    // 的构造参数，不能在渲染时再补。
    fileprivate static let chatTheme = chatThemeBase.chatCodeBlock(allowRun: false, variant: .chat)
    fileprivate static let chatThemeRunnable = chatThemeBase.chatCodeBlock(allowRun: true, variant: .chat)
    fileprivate static let codexTranscriptTheme = codexTranscriptThemeBase
        .chatCodeBlock(allowRun: false, variant: .chat)
    fileprivate static let codexTranscriptThemeRunnable = codexTranscriptThemeBase
        .chatCodeBlock(allowRun: true, variant: .chat)
    fileprivate static let articleTheme = articleThemeBase.chatCodeBlock(allowRun: false, variant: .article)
    fileprivate static let articleThemeRunnable = articleThemeBase.chatCodeBlock(allowRun: true, variant: .article)

    /// Chat-tuned theme: tight vertical rhythm so a short bot reply doesn't
    /// feel like a blog post. Default for `.chat` variant.
    fileprivate static let chatThemeBase: MDTheme = MDTheme()
        .text {
            ForegroundColor(AppTheme.Palette.ink)
            // Matches `Theme.Fonts.body` exactly — both sides of the chat
            // need the same point size or the bot bubble reads larger
            // than the user bubble on Mac Catalyst (Theme.Fonts applies a
            // platform scale; FontSize takes raw points and won't pick
            // it up on its own).
            FontSize(AppTheme.Fonts.scaled(16))
        }
        .link {
            // Citations land here too — see CitationScheme.rewrite, which
            // turns inline `[N]` markers into markdown links whose label is
            // the source's domain wrapped in NBSPs. `accentBg` is the same
            // pale-green wash used on user bubbles, so the inline citations
            // read as light-green pills against the canvas.
            ForegroundColor(AppTheme.Palette.accent)
            BackgroundColor(AppTheme.Palette.accentBg)
            FontSize(.em(0.85))
            FontWeight(.medium)
        }
        .strong { FontWeight(.semibold) }
        .emphasis { FontStyle(.italic) }
        .code {
            FontFamilyVariant(.monospaced)
            FontSize(.em(0.92))
            BackgroundColor(AppTheme.Palette.surfaceMuted)
        }
        .heading1 { configuration in
            configuration.label
                .markdownTextStyle {
                    FontWeight(.semibold)
                    FontSize(22)
                }
                .markdownMargin(top: 6, bottom: 2)
        }
        .heading2 { configuration in
            configuration.label
                .markdownTextStyle {
                    FontWeight(.semibold)
                    FontSize(19)
                }
                .markdownMargin(top: 6, bottom: 2)
        }
        .heading3 { configuration in
            configuration.label
                .markdownTextStyle {
                    FontWeight(.semibold)
                    FontSize(17)
                }
                .markdownMargin(top: 4, bottom: 2)
        }
        .blockquote { configuration in
            configuration.label
                .padding(.leading, 12)
                .padding(.vertical, 4)
                .overlay(alignment: .leading) {
                    Rectangle()
                        .fill(AppTheme.Palette.hairline)
                        .frame(width: 3)
                }
                .markdownTextStyle { ForegroundColor(AppTheme.Palette.inkMuted) }
        }
        .listItem { configuration in
            configuration.label.markdownMargin(top: 2, bottom: 2)
        }
        .paragraph { configuration in
            configuration.label
                .fixedSize(horizontal: false, vertical: true)
                .markdownMargin(top: 0, bottom: 6)
        }

    // PENDINGCREW SHIM (Todo #56 ⑥): only Codex's ordinary assistant prose opts
    // into this theme. One point smaller than crew bubbles, with visibly looser
    // leading for long-form reading; other transcript rows keep their own styles.
    fileprivate static let codexTranscriptThemeBase = chatThemeBase
        .text {
            ForegroundColor(AppTheme.Palette.ink)
            FontSize(AppTheme.Fonts.scaled(15))
        }
        .paragraph { configuration in
            configuration.label
                .fixedSize(horizontal: false, vertical: true)
                .lineSpacing(5)
                .markdownMargin(top: 0, bottom: 6)
        }

    /// Article-tuned theme: editorial serif body, looser line height,
    /// larger heading hierarchy with real space above. Built for the
    /// 来信 detail page where a reader stays on one column for minutes,
    /// not seconds — closer to a magazine column than a chat log.
    ///
    /// Notes on the choices:
    /// - Body is `.serif` 17pt with `lineSpacing 7` — that resolves to
    ///   roughly 1.55× line-height on iOS, which reads cleanly for both
    ///   long Chinese paragraphs and English clauses without the cramped
    ///   feeling 16pt sans gives in long form.
    /// - Headings get progressively wider top margins so a `## section`
    ///   actually breathes apart from the paragraph above it. Bottom
    ///   margins stay tight so the heading still belongs to the paragraph
    ///   it introduces.
    /// - Blockquotes go italic + indented from both sides — the standard
    ///   editorial pull-quote register, distinct from the chat theme's
    ///   muted-aside register.
    /// - Lists get larger inter-item margins and a markdown bullet style
    ///   inherited from MarkdownUI defaults so nested lists still read.
    /// - Tables get a hairline border and per-cell padding so a bot
    ///   that drops a comparison table doesn't end up with mashed cells.
    /// - Thematic breaks render as a centered short hairline rather than
    ///   a full-width rule — feels more like a section break in print.
    fileprivate static let articleThemeBase: MDTheme = MDTheme()
        .text {
            ForegroundColor(AppTheme.Palette.ink)
            FontFamily(.system(.serif))
            FontSize(17)
        }
        .link {
            // Same green pill the chat theme uses — citations and inline
            // links read consistently across both surfaces.
            ForegroundColor(AppTheme.Palette.accent)
            BackgroundColor(AppTheme.Palette.accentBg)
            FontSize(.em(0.88))
            FontWeight(.medium)
        }
        .strong { FontWeight(.semibold) }
        .emphasis { FontStyle(.italic) }
        .code {
            FontFamilyVariant(.monospaced)
            FontSize(.em(0.9))
            BackgroundColor(AppTheme.Palette.surfaceMuted)
        }
        .heading1 { configuration in
            configuration.label
                .markdownTextStyle {
                    FontFamily(.system(.serif))
                    FontWeight(.semibold)
                    FontSize(26)
                }
                .markdownMargin(top: 28, bottom: 10)
        }
        .heading2 { configuration in
            configuration.label
                .markdownTextStyle {
                    FontFamily(.system(.serif))
                    FontWeight(.semibold)
                    FontSize(22)
                }
                .markdownMargin(top: 32, bottom: 8)
        }
        .heading3 { configuration in
            configuration.label
                .markdownTextStyle {
                    FontFamily(.system(.serif))
                    FontWeight(.semibold)
                    FontSize(18)
                }
                .markdownMargin(top: 24, bottom: 6)
        }
        .blockquote { configuration in
            configuration.label
                .padding(.leading, 16)
                .padding(.vertical, 10)
                .overlay(alignment: .leading) {
                    Rectangle()
                        .fill(AppTheme.Palette.accent.opacity(0.55))
                        .frame(width: 2)
                }
                .markdownTextStyle {
                    FontFamily(.system(.serif))
                    FontStyle(.italic)
                    ForegroundColor(AppTheme.Palette.inkMuted)
                }
                .markdownMargin(top: 16, bottom: 16)
        }
        .listItem { configuration in
            configuration.label
                .markdownMargin(top: 6, bottom: 6)
        }
        .paragraph { configuration in
            configuration.label
                .fixedSize(horizontal: false, vertical: true)
                .lineSpacing(7)
                .markdownMargin(top: 0, bottom: 18)
        }
        .thematicBreak {
            HStack {
                Spacer()
                Rectangle()
                    .fill(AppTheme.Palette.hairline)
                    .frame(width: 64, height: 0.5)
                Spacer()
            }
            .padding(.vertical, 12)
        }
        .table { configuration in
            configuration.label
                .fixedSize(horizontal: false, vertical: true)
                .markdownTableBorderStyle(.init(.allBorders, color: AppTheme.Palette.hairline, strokeStyle: .init(lineWidth: 0.5)))
                .markdownTableBackgroundStyle(
                    .alternatingRows(AppTheme.Palette.surface, AppTheme.Palette.surfaceMuted.opacity(0.5))
                )
                .markdownMargin(top: 14, bottom: 18)
        }
        .tableCell { configuration in
            configuration.label
                .markdownTextStyle {
                    FontFamily(.system(.serif))
                    FontSize(15)
                    if configuration.row == 0 {
                        FontWeight(.semibold)
                    }
                }
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
        }
}

// PENDINGCREW SHIM(代码块不渲染 2026-08-11): 把 `ChatCodeBlock` 焊进 theme 的
// `\.codeBlock`。走 `Theme.codeBlock(body:)`(MarkdownUI 自带的 theme builder)
// 而不是 `.markdownBlockStyle(\.codeBlock)` 视图修饰器 —— 后者是环境值补丁，
// 会被里层的 `.markdownTheme(…)` 整体盖掉（这就是本 bug 的根因）。
fileprivate extension MDTheme {
    func chatCodeBlock(allowRun: Bool, variant: MarkdownText.Variant) -> MDTheme {
        codeBlock { configuration in
            ChatCodeBlock(
                content: configuration.content,
                language: configuration.language,
                allowRun: allowRun,
                variant: variant
            )
        }
    }
}

private struct PresentedCitation: Identifiable {
    let n: Int
    let citation: MessageCitation
    var id: Int { n }
}

/// Stable mapping between inline `[N]` markers and the `pendingbot-cite://N`
/// pseudo-URL the markdown renderer trades for tappable links.
fileprivate enum CitationScheme {
    static let scheme = "pendingbot-cite"

    /// Replace every `[N]` (1-based) in `text` with a markdown link whose
    /// label is the citation's domain (e.g. `wikipedia.org`) padded with
    /// NBSPs (`\u{00A0}`) so the inline background color renders with
    /// breathing room — that's what gives the pill its visual padding,
    /// since MarkdownUI's `.link` background is a flat rectangle, not a
    /// shape we can corner-radius. References outside `1...count` are
    /// left as plain text — better to show a stray `[7]` than to dead-link
    /// it. Adjacent citations (`[1][3]` with no separator) get a hair-
    /// space (`\u{2009}`) injected between them so two pills don't smear
    /// into one continuous green bar.
    static func rewrite(_ text: String, citations: [MessageCitation]) -> String {
        // Avoid re-parsing on every layout pass — short-circuit when the
        // text has no candidate.
        guard text.contains("[") else { return text }
        let pattern = try? NSRegularExpression(pattern: #"\[(\d+)\]"#)
        guard let pattern else { return text }
        let ns = text as NSString
        let matches = pattern.matches(in: text, range: NSRange(location: 0, length: ns.length))
        guard !matches.isEmpty else { return text }
        let count = citations.count
        var result = ""
        var cursor = 0
        var lastEmittedCitationEnd = -1   // text-offset where the last pill we wrote ended
        for match in matches {
            let full = match.range
            let nRange = match.range(at: 1)
            let n = Int(ns.substring(with: nRange)) ?? 0
            if full.location > cursor {
                result += ns.substring(with: NSRange(location: cursor, length: full.location - cursor))
            }
            if n >= 1 && n <= count {
                if lastEmittedCitationEnd == full.location {
                    // Adjacent pills — separate them visually so the
                    // backgrounds don't merge.
                    result += "\u{2009}"
                }
                let host = host(for: citations[n - 1].url)
                result += "[\u{00A0}\(host)\u{00A0}](\(scheme)://\(n))"
                lastEmittedCitationEnd = full.location + full.length
            } else {
                result += ns.substring(with: full)
            }
            cursor = full.location + full.length
        }
        if cursor < ns.length {
            result += ns.substring(with: NSRange(location: cursor, length: ns.length - cursor))
        }
        return result
    }

    /// Display host: prefer the bare domain (`example.com`) over the full
    /// hostname (`www.example.com`). Falls back to `?` for malformed urls
    /// so the rewritten markdown stays valid even if the citation slipped
    /// through with a junk url.
    private static func host(for url: String) -> String {
        URL(string: url)?.host?.replacingOccurrences(of: "www.", with: "") ?? "?"
    }
}

/// Bottom sheet shown when the user taps an inline citation chip. Lets the
/// reader skim the title + snippet before deciding to open the source.
private struct CitationSheet: View {
    let index: Int
    let citation: MessageCitation
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    private var hostLabel: String {
        URL(string: citation.url)?.host?.replacingOccurrences(of: "www.", with: "") ?? citation.url
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text("[\(index)]")
                    .font(AppTheme.Fonts.rounded(size: 13, weight: .semibold))
                    .foregroundStyle(AppTheme.Palette.accent)
                Text(hostLabel)
                    .font(AppTheme.Fonts.footnote)
                    .foregroundStyle(AppTheme.Palette.inkMuted)
                Spacer(minLength: 0)
            }
            Text(citation.title)
                .font(AppTheme.Fonts.body)
                .foregroundStyle(AppTheme.Palette.ink)
                .fixedSize(horizontal: false, vertical: true)
            if let snippet = citation.snippet, !snippet.isEmpty {
                Text(snippet)
                    .font(AppTheme.Fonts.footnote)
                    .foregroundStyle(AppTheme.Palette.inkMuted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Button {
                if let u = URL(string: citation.url) {
                    openURL(u)
                    dismiss()
                }
            } label: {
                HStack {
                    Image(systemName: "safari")
                    Text("打开链接")
                }
                .font(AppTheme.Fonts.body)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 10)
                .background(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(AppTheme.Palette.accent.opacity(0.12))
                )
                .foregroundStyle(AppTheme.Palette.accent)
            }
            .buttonStyle(.plain)
            .padding(.top, 4)
        }
        .padding(20)
        #if os(iOS)
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
        #endif
    }
}

// ── Code block ──────────────────────────────────────────────────────────────

/// Fenced code with a hover-style toolbar (lang tag · 复制 · ▶ 运行).
/// Run is offered for `js` / `javascript` / `html` only — anything else
/// shows a static lang chip so users don't tap into a sandbox that can't
/// execute their language.
///
/// `variant` retunes the block for its surrounding surface:
/// - `.chat` keeps the dense bubble-companion treatment.
/// - `.article` switches to editorial register — bigger monospace, more
///   breathing room, quieter chrome — so a code block sits inside a 17pt
///   serif column without reading like a chat snippet glued in.
private struct ChatCodeBlock: View {
    let content: String
    let language: String?
    let allowRun: Bool
    var variant: MarkdownText.Variant = .chat

    @State private var copied = false
    @State private var runnerSheet = false

    private var canRun: Bool {
        guard allowRun, let lang = language?.lowercased() else { return false }
        return ChatCodeBlock.runnableLangs.contains(lang)
    }

    private static let runnableLangs: Set<String> = ["js", "javascript", "html"]

    private var codeFontSize: CGFloat { variant == .article ? 14.5 : 13.5 }
    private var horizontalPadding: CGFloat { variant == .article ? 16 : 12 }
    private var verticalPadding: CGFloat { variant == .article ? 14 : 10 }
    private var cornerRadius: CGFloat { variant == .article ? 6 : 10 }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            toolbar
            ScrollView(.horizontal, showsIndicators: false) {
                Text(content)
                    .font(AppTheme.Fonts.monospaced(size: codeFontSize))
                    .foregroundStyle(AppTheme.Palette.ink)
                    .lineSpacing(variant == .article ? 3 : 1)
                    .padding(.horizontal, horizontalPadding)
                    .padding(.vertical, verticalPadding)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    // PENDINGCREW SHIM (#443): 同上 —— 代码块也是 NSTextField 大户。
                    .crewSelectableText()
            }
        }
        .background(AppTheme.Palette.surfaceMuted.opacity(variant == .article ? 0.6 : 0.85))
        .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .strokeBorder(AppTheme.Palette.hairline, lineWidth: 0.5)
        )
        .padding(.vertical, variant == .article ? 10 : 4)
        #if os(iOS)
        .sheet(isPresented: $runnerSheet) {
            CodeRunnerSheet(content: content, language: language ?? "")
        }
        #endif
    }

    @ViewBuilder
    private var toolbar: some View {
        // In article register, only show the chrome row when there's
        // actually something to show (a language tag or a run button).
        // The 复制 control is already redundant with iOS text selection's
        // long-press menu in the editorial reading mode, and the empty
        // toolbar above each block was the loudest source of "looks
        // chatty" in 来信.
        if variant == .article {
            if let lang = language, !lang.isEmpty {
                HStack(spacing: 0) {
                    Text(lang.lowercased())
                        .font(AppTheme.Fonts.monospaced(size: 10.5))
                        .foregroundStyle(AppTheme.Palette.inkMuted)
                        .tracking(0.4)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, horizontalPadding)
                .padding(.top, 8)
                .padding(.bottom, 2)
            }
        } else {
            HStack(spacing: 10) {
                if let lang = language, !lang.isEmpty {
                    Text(lang.lowercased())
                        .font(AppTheme.Fonts.monospaced(size: 11))
                        .foregroundStyle(AppTheme.Palette.inkMuted)
                }
                Spacer(minLength: 0)
                Button {
                    #if canImport(UIKit)
                    UIPasteboard.general.string = content
                    #elseif canImport(AppKit)
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(content, forType: .string)
                    #endif
                    Haptics.tap()
                    copied = true
                    Task {
                        try? await Task.sleep(nanoseconds: 1_200_000_000)
                        copied = false
                    }
                } label: {
                    Text(copied ? "已复制" : "复制")
                        .font(AppTheme.Fonts.rounded(size: 11, weight: .medium))
                        .foregroundStyle(AppTheme.Palette.inkMuted)
                }
                .buttonStyle(.plain)
                #if os(iOS)
                if canRun {
                    Button {
                        Haptics.tap()
                        runnerSheet = true
                    } label: {
                        Text("▶ 运行")
                            .font(AppTheme.Fonts.rounded(size: 11, weight: .medium))
                            .foregroundStyle(AppTheme.Palette.accent)
                    }
                    .buttonStyle(.plain)
                }
                #endif
            }
            .padding(.horizontal, 10)
            .padding(.top, 6)
            .padding(.bottom, 4)
        }
    }
}
