#if os(macOS)
import Foundation

/// 「session 卡在待决策上」的检测 + 通知选靶（Todo #6）。
///
/// **要解决的病**：worker session 在终端里弹出需要人选的东西，如果没人盯右栏，
/// 它就一直干等，人在群聊里完全看不到，session 事实上死掉。
///
/// #491 已经把「走我们自己代码」的三处阻塞点接进群了（`ask` 工具、codex 的
/// `*/requestApproval`、claude 的 PreToolUse 权限钩子）。但 claude 的钩子**只 gate
/// `computer-use` 一个工具**（见 `LocalSessionLaunch` 的 `permGates`），claude 自己在
/// PTY 里弹的那些菜单 —— 命令审批、计划确认、信任文件夹、选登录方式 —— 一个都不经过
/// 它，于是群里一个字都没有。那才是真正让 session 死掉的那条路，本文件补的就是它。
///
/// 形态照终端通知：**只负责亮出来让人知道，不替人拍板**。检测到 → 发群说清在等什么
/// → @ 能处理的人；答不答、怎么答仍归机长/人（机长手上有 `inspect_session` 看现场、
/// `nudge_session` 发按键，本来就能代答）。
///
/// 与 `RateLimitMenuScanner`（#519）的分工：那个是「我们能替它答」的固定菜单（撞额度
/// 时替它选「Stop and wait」），所以自动按键；这里是**拍板类**，只报不答。两者共用
/// `AnsiPlainTextTail` 去 ANSI 底座，rate-limit 菜单在本解析器里被显式排除，免得每次
/// 撞额度都白喊一次人。

// MARK: - 被识别出来的一个「在等人选」现场

struct PendingTerminalDecision: Equatable {
    /// 问句原文（"Do you want to proceed?"）。认不出来时为空串。
    let prompt: String
    /// 选项正文（已剥掉序号与光标标记），按屏幕顺序。
    let options: [String]

    /// 去重指纹：同一个菜单反复重绘 → 同一指纹 → 只报一次。
    var fingerprint: String { ([prompt] + options).joined(separator: "|") }
}

// MARK: - 菜单解析（纯函数）

/// 从一段**去 ANSI 的终端明文**里认出「交互式选择菜单」。
///
/// 判据（缺一不可，全都是为了**不误报**——误报比漏报更伤：群里一刷噪音，
/// 人就不再看通知了）：
/// 1. 末尾附近有一段**连续**的编号行，序号从 1 起连续（`1. / 2. / 3.`）。
/// 2. 至少两个选项（单选项不是选择）。
/// 3. 至少一行带**选择光标**（`❯`）。这条是关键判别式 —— agent 正文里的编号列表
///    满地都是（计划、清单、总结），只有交互菜单才会画光标。
/// 4. 选项块后面最多再跟 `trailingSlack` 行正文（给 box 边框下面的提示行留余量）；
///    再多就说明菜单已经被答掉、输出滚过去了，不再是「在等人选」。
enum TerminalMenuParser {
    /// 选择光标字形。**不含裸 `>`** —— markdown 引用、shell 提示符都用它，收进来必误报。
    static let markers: Set<Character> = ["❯", "›", "▸", "▶"]

    /// TUI 画框用的字符：逐行剥掉两端的框线，露出里面的正文。
    static let boxChars: Set<Character> = [
        "│", "┃", "║", "╭", "╮", "╰", "╯", "─", "━", "═",
        "┌", "┐", "└", "┘", "├", "┤", "╔", "╗", "╚", "╝",
    ]

    /// 选项块后面还能跟几行正文。2 = 给 box 下沿的提示行（"Esc to cancel"）留位，
    /// 但一旦真的开始吐新输出（≥3 行）就判定菜单已被答掉 → 上层随即清状态。
    /// 这个数字直接决定「答完多久状态能回正」，宁小勿大（#545：进得去出不来最伤）。
    static let trailingSlack = 2

    /// 剥掉一行两端的框线与空白。
    static func strip(_ line: String) -> String {
        var s = Substring(line)
        while let f = s.first, f.isWhitespace || boxChars.contains(f) { s = s.dropFirst() }
        while let l = s.last, l.isWhitespace || boxChars.contains(l) { s = s.dropLast() }
        return String(s)
    }

    /// 一行剥干净后是不是选项行。返回 (序号, 正文, 是否带光标)。
    static func option(_ stripped: String) -> (index: Int, text: String, selected: Bool)? {
        var s = Substring(stripped)
        var selected = false
        if let f = s.first, markers.contains(f) {
            selected = true
            s = s.dropFirst()
            while let f2 = s.first, f2.isWhitespace { s = s.dropFirst() }
        }
        var digits = ""
        while let f = s.first, f.isNumber { digits.append(f); s = s.dropFirst() }
        guard !digits.isEmpty, let n = Int(digits), s.first == "." else { return nil }
        s = s.dropFirst()
        guard let sp = s.first, sp.isWhitespace else { return nil }
        while let f = s.first, f.isWhitespace { s = s.dropFirst() }
        let text = String(s)
        guard !text.isEmpty else { return nil }
        return (n, text, selected)
    }

    static func parse(_ plain: String) -> PendingTerminalDecision? {
        let lines = plain
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { strip(String($0)) }

        // 1) 从尾部往上找最后一个选项行；正文超过 trailingSlack 行就判菜单已过期。
        var lastOption: Int?
        var trailing = 0
        var i = lines.count - 1
        while i >= 0 {
            let l = lines[i]
            if l.isEmpty { i -= 1; continue }
            if option(l) != nil { lastOption = i; break }
            trailing += 1
            if trailing > trailingSlack { return nil }
            i -= 1
        }
        guard let end = lastOption else { return nil }

        // 2) 往上收连续选项行（**不跨空行** —— 跨了会把上面另一个块错误粘进来）。
        var parsed: [(index: Int, text: String, selected: Bool)] = []
        var j = end
        while j >= 0, let o = option(lines[j]) {
            parsed.append(o)
            j -= 1
        }
        parsed.reverse()

        guard parsed.count >= 2,
              parsed.enumerated().allSatisfy({ $0.element.index == $0.offset + 1 }),
              parsed.contains(where: { $0.selected })
        else { return nil }

        // 3) 问句 = 选项块上方最近的一行正文（跳过空行/框线）。
        var prompt = ""
        var k = j
        while k >= 0 {
            if lines[k].isEmpty { k -= 1; continue }
            prompt = lines[k]
            break
        }

        let decision = PendingTerminalDecision(prompt: prompt, options: parsed.map(\.text))

        // 4) rate-limit 菜单归 `RateLimitMenuScanner` 自动应答，不走找人这条路。
        let hay = ([prompt] + decision.options).joined(separator: " ").lowercased()
        if RateLimitMenuScanner.menuPhrases.contains(where: { hay.contains($0) }) { return nil }

        return decision
    }
}

// MARK: - 出现 / 消失 的跟踪（有状态，但时间由调用方喂 → 好单测）

/// 喂 PTY 原始字节 + 定期 `poll`，产出「有个菜单在等人」/「等完了」两种事件。
///
/// 为什么用「**同一菜单稳定 N 秒**」而不是「输出静默 N 秒」：TUI 会把同一个菜单反复
/// 重绘，按静默判会永远等不到静默、于是永远报不出来。按指纹稳定判则重绘无害 ——
/// 重绘不改指纹，计时照走；而流式吐字期间压根解析不出菜单，自然不进候选。
final class PendingDecisionTracker {
    enum Event: Equatable {
        case appeared(PendingTerminalDecision)
        case cleared
    }

    private let stripper = AnsiPlainTextTail()
    private let stable: TimeInterval
    private var candidate: PendingTerminalDecision?
    private var candidateSince: Date?
    private var reported: PendingTerminalDecision?

    /// 当前正在等的那个（nil = 没在等）——上层据此翻状态。
    var pending: PendingTerminalDecision? { reported }

    /// - Parameter stable: 菜单在屏幕上稳住多久才算「真在等人」。3s 足够躲开
    ///   渲染中途的半成品画面，又不会让人多等。
    init(stable: TimeInterval = 3) { self.stable = stable }

    func feed(_ bytes: ArraySlice<UInt8>) { stripper.feed(bytes) }

    func poll(now: Date = Date()) -> Event? {
        guard let d = TerminalMenuParser.parse(stripper.tail) else {
            candidate = nil
            candidateSince = nil
            guard reported != nil else { return nil }
            reported = nil
            return .cleared
        }
        guard candidate?.fingerprint == d.fingerprint, let since = candidateSince else {
            candidate = d
            candidateSince = now
            return nil
        }
        guard now.timeIntervalSince(since) >= stable else { return nil }
        guard reported?.fingerprint != d.fingerprint else { return nil }
        reported = d
        return .appeared(d)
    }
}

// MARK: - 通知稿：说什么、@ 谁

/// 一个 session 卡在待决策上时，往群里发的那条消息。
///
/// 选靶规则（需求原话「要么在群里通知人去看，要么由机长代为处理」）：
/// - **worker 卡住 → 先只 @机长**。机长手上有 `inspect_session` / `nudge_session`，
///   能拍的直接拍完；这一步不惊动人 —— 逢事必 @人，通知很快就被无视。
/// - **机长自己卡住 → 直接 @人**。绝不 @ 自己：@机长会触发「目标缺席拉起」，
///   卡着的机长起不来又发一条，就此成环（#541 同款坑）。
/// - **等太久没人管 → 升级 @人**。机长拍不了板/没在跑时的兜底。
enum SessionDecisionNotice {
    enum Stage: Equatable { case first, escalate }

    struct Post: Equatable {
        let text: String
        /// mention kind 列表，顺序即 @ 顺序；空 = 广播。
        let mentionKinds: [String]
    }

    /// 首报后多久还没解决就升级找人。5 分钟：够机长看见并处理一轮，又不至于让人
    /// 干等半天。
    static let escalateAfter: TimeInterval = 300

    static func shouldEscalate(
        raisedAt: Date, now: Date = Date(), after: TimeInterval = escalateAfter
    ) -> Bool {
        now.timeIntervalSince(raisedAt) > after
    }

    /// - Parameters:
    ///   - question: 在等什么（终端菜单的问句 / codex 的请求描述）。
    ///   - options: 可选项；空 = 不是选择题（如 codex 要求填表单）。
    ///   - waitedMinutes: 已等分钟数（升级稿用）。
    static func post(
        stage: Stage, sessionName: String, sessionId: String, isCaptain: Bool,
        question: String, options: [String], waitedMinutes: Int
    ) -> Post {
        var lines: [String] = []
        switch stage {
        case .first:
            lines.append(isCaptain
                ? "\(sessionName)（机长）卡住了，在等一个回复："
                : "\(sessionName) 卡住了，在等人拍板：")
        case .escalate:
            lines.append("\(sessionName) 仍卡着，已等 \(waitedMinutes) 分钟没人处理：")
        }
        if !question.isEmpty { lines.append(question) }
        for (i, o) in options.enumerated() { lines.append("  \(i + 1). \(o)") }

        let mentions: [String]
        switch stage {
        case .first:
            mentions = isCaptain ? ["human"] : ["captain"]
            lines.append(isCaptain
                ? "需要人打开这个 session 的终端选一下。"
                : "机长可 inspect_session 看现场、nudge_session 代答；拍不了板就 @人。")
        case .escalate:
            mentions = ["human"]
            lines.append("需要人来定：打开这个 session 的终端直接选，或让机长 nudge_session 代按。")
        }
        lines.append("（session: \(sessionId)）")
        return Post(text: lines.joined(separator: "\n"), mentionKinds: mentions)
    }
}
#endif
