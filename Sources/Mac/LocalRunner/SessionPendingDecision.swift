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
    /// **屏幕上本来有没有编号。**
    ///
    /// 这个字段是补一处「解析器剥掉、渲染器重造」的断层：`options` 里的序号是被
    /// `strip` 掉的，而 `SessionDecisionNotice.post` 渲染时又按 `1. 2. 3.` 重新编了
    /// 一遍。带编号那条路（`parseNumbered`）之所以恰好没错，是因为它**断言过**序号
    /// 必须是连续的 1..N —— 那条断言是那行渲染的正确性前提，可它们隔着这个类型，
    /// 中间没有任何东西说得出这层依赖。
    ///
    /// 于是 2026-09 claude 换成没编号的信任框之后，渲染器凭空造出的
    /// `1. No, exit / 2. Yes, I trust this folder` 就成了**假的可操作性**：机长照着
    /// 发数字，实测屏幕纹丝不动（真正生效的是 nudge 自动补的那个回车，它确认的是
    /// 当前高亮项 —— 默认正是 `No, exit`，一发就把 session 关了）。
    ///
    /// 所以事实必须跟着值走：**每个生产者如实填，渲染按它分叉。**
    let numbered: Bool
    /// 去重指纹：同一个菜单反复重绘 → 同一指纹 → 只报一次。
    ///
    /// 故意**不含** `numbered`：它是同一个框的属性，不是「换了一个框」。
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

    /// claude 选择框底下那句确认脚注（`Enter to confirm · Esc to cancel`）。
    ///
    /// **它是「没有编号的选项块」唯一靠得住的判别式。** 没有它，一个人正打着两行字
    /// 的输入框（光标行 + 续行）跟这种菜单在文本上一模一样，收进来等于把每个正在
    /// 打字的 session 都点亮成「⌛ 等人拍板」。
    ///
    /// 比对前先把空白全去掉：屏幕上是 `Enter to confirm · Esc to cancel`，而去 ANSI 的
    /// 字节尾窗里同一句是 `Entertoconfirm·Esctocancel`（claude 用光标定位摆词，
    /// 词之间那些空格在字节流里根本不存在）。两条来源要用同一把尺子。
    static func isConfirmFooter(_ stripped: String) -> Bool {
        let compact = stripped.filter { !$0.isWhitespace }.lowercased()
        return compact.contains("toconfirm") && compact.contains("tocancel")
    }

    /// 问句往上找多少行。信任框的真问句离选项块有 6～7 行（中间隔着说明段落与
    /// 「Security guide」），只看紧邻那一行会把「Security guide」当成问句发进群 ——
    /// 人看到它完全不知道在等什么。
    static let promptScanDepth = 12

    /// 选项块上方那句问句：**优先带问号的那一行**，找不到就退回最近的一行正文。
    private static func promptAbove(_ lines: [String], blockStart: Int) -> String {
        var nearest = ""
        var scanned = 0
        var k = blockStart - 1
        while k >= 0, scanned < promptScanDepth {
            defer { k -= 1; scanned += 1 }
            let line = lines[k]
            if line.isEmpty { continue }
            if nearest.isEmpty { nearest = line }
            if line.contains("?") || line.contains("？") { return line }
        }
        return nearest
    }

    static func parse(_ plain: String) -> PendingTerminalDecision? {
        let lines = plain
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { strip(String($0)) }
        guard let decision = parseNumbered(lines) ?? parseCursorList(lines) else { return nil }

        // rate-limit 菜单归 `RateLimitMenuScanner` 自动应答，不走找人这条路
        // —— 否则每次撞额度都白喊一次人。两条解析路都要过这道闸。
        let hay = ([decision.prompt] + decision.options).joined(separator: " ").lowercased()
        if RateLimitMenuScanner.menuPhrases.contains(where: { hay.contains($0) }) { return nil }
        return decision
    }

    /// **没有编号的选择框**（2026-09 的 claude 信任框就是这个形状）：
    /// ```
    ///  ❯  No, exit
    ///     Yes, I trust this folder
    ///
    ///  Enter to confirm · Esc to cancel
    /// ```
    /// 判据（同样是缺一不可，理由同上：误报比漏报更伤）：
    /// 1. 末尾附近有那句**确认脚注**（见 `isConfirmFooter`）—— 这条是把它跟「人正在
    ///    输入框里打第二行字」分开的唯一硬证据。
    /// 2. 脚注上方是一段**连续**非空行，至少两行。
    /// 3. 其中**恰好一行**带选择光标（`❯`）—— 两行都带说明这不是单选。
    private static func parseCursorList(_ lines: [String]) -> PendingTerminalDecision? {
        // 1) 脚注：从尾部往上找，正文超过 trailingSlack 行就判它已经滚过去了。
        var footer: Int?
        var trailing = 0
        var i = lines.count - 1
        while i >= 0 {
            let line = lines[i]
            if line.isEmpty { i -= 1; continue }
            if isConfirmFooter(line) { footer = i; break }
            trailing += 1
            if trailing > trailingSlack { return nil }
            i -= 1
        }
        guard let footerIndex = footer else { return nil }

        // 2) 脚注上方那一段连续非空行。
        var end = footerIndex - 1
        while end >= 0, lines[end].isEmpty { end -= 1 }
        guard end >= 0 else { return nil }
        var start = end
        while start - 1 >= 0, !lines[start - 1].isEmpty { start -= 1 }

        let block = Array(lines[start...end])
        guard block.count >= 2 else { return nil }
        // 带编号的走上面那条路；这里只认没编号的，免得同一屏被两条路解析出两份
        // 措辞不同的结果，在上层来回抢。
        guard !block.contains(where: { option($0) != nil }) else { return nil }

        var options: [String] = []
        var cursors = 0
        for line in block {
            guard let first = line.first else { return nil }
            if markers.contains(first) {
                cursors += 1
                let text = String(line.dropFirst()).trimmingCharacters(in: .whitespaces)
                guard !text.isEmpty else { return nil }
                options.append(text)
            } else {
                options.append(line)
            }
        }
        guard cursors == 1 else { return nil }

        return PendingTerminalDecision(
            prompt: promptAbove(lines, blockStart: start), options: options, numbered: false)
    }

    private static func parseNumbered(_ lines: [String]) -> PendingTerminalDecision? {
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

        // 3) 问句 = 选项块上方那句话（优先带问号的那一行，见 `promptAbove`）。
        return PendingTerminalDecision(
            prompt: promptAbove(lines, blockStart: j + 1), options: parsed.map(\.text),
            numbered: true)
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

    /// 菜单在屏幕上稳住多久才算「真在等人」。3s 足够躲开渲染中途的半成品画面，
    /// 又不会让人多等。
    ///
    /// **这是全仓唯一的那个阈值** —— `StartupPromptDelivery.Timing.dialogStable`
    /// 直接引它，别在别处再写一个数字：两条路对「什么时候算真在等人」给出不同答案，
    /// 就会出现「开场投递认为还没定、待决策已经报出去了」这种自相矛盾的现场。
    static let stableWindow: TimeInterval = 3

    /// - Parameter stable: 见 `stableWindow`。
    init(stable: TimeInterval = PendingDecisionTracker.stableWindow) { self.stable = stable }

    func feed(_ bytes: ArraySlice<UInt8>) { stripper.feed(bytes) }

    /// **两条来源，一个出口。**
    ///
    /// - 字节尾窗（`stripper.tail`）：命令审批 / 计划确认这类，claude 是顺着流吐出来的。
    /// - 渲染完的那一屏（`screen`）：claude 的信任框靠**光标定位**摆列，在字节流上
    ///   它是瞎的 —— 序号后面没有空格、行尾是 `\r\r\n`，去 ANSI 之后整个形状就散了
    ///   （实测证据见 `ClaudeInputBox.blockingDialog`）。只有喂进 `Terminal` 让它把列
    ///   位置解出来，那一屏才重新长成人看到的样子。
    ///
    /// 同时命中时怎么办（**别互相抢、别抖**）：
    /// 1. 谁都行的时候认**屏幕**那条 —— 它是权威渲染，措辞跟人看到的一致。
    /// 2. 但**已经在跟踪/已经报出去的那一个优先粘住**：两条来源对同一个菜单会给出
    ///    措辞略有出入的文本（字节流里词间空格是没有的），指纹一变就会重新计时、
    ///    重新报一次 —— 群里刷两条、状态灯闪。粘住之后重绘无害。
    /// 3. **两条都认不出来才算「答完了」。** 屏幕重绘到一半那一拍解析不出来，
    ///    不该把状态清掉。
    ///
    /// - Parameter screen: 渲染完的那一屏（`AgentSessionCore.screenRows()`）。
    ///   `nil` = 调用方拿不到画面，退回只看字节尾窗。
    func poll(now: Date = Date(), screen: [String]? = nil) -> Event? {
        // 屏幕在前 = 谁都行的时候认它。
        let seen = [
            screen.flatMap { ClaudeInputBox.blockingDialog($0) },
            TerminalMenuParser.parse(stripper.tail),
        ].compactMap { $0 }
        let sticky = reported?.fingerprint ?? candidate?.fingerprint
        let chosen = seen.first { $0.fingerprint == sticky } ?? seen.first

        guard let d = chosen else {
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

// MARK: - nudge_session 认得的裸按键

/// **`nudge_session` 能发哪些裸按键 —— 唯一事实源。**
///
/// 待决策消息里「发什么、能不能搬动高亮」那几句是**按这张表算出来的**，不是照抄的。
/// 今天这条 P0 的病根是「一句静态文本去描述一件会变的事实」——上游那一屏会变，
/// 我们已经改成现算了；**而 `nudge` 支持哪些键同样会变**（方向键加不加正等着人拍板）。
/// 要是文案里另写一遍「不认方向键」，加上方向键的那天它就安静地过期，
/// 一模一样的病、换个地方再犯一次。
enum SessionNudgeKeys {
    /// 别名 → 发给 PTY 的字节。**不在这张表里的输入一律当文本发出去、并自动补一个
    /// 回车** —— 那个回车会确认菜单当前高亮的那一项，这正是「发个数字」会把 session
    /// 关掉的原因。
    static let byAlias: [String: [UInt8]] = [
        "enter": [0x0d], "回车": [0x0d], "esc": [0x1b],
    ]

    /// 给人看的键名：**从表里算出来**，不是另写一份名单。
    ///
    /// 这条差点又踩同一个坑 —— 第一版把它写成 `["enter", "esc"].filter { … }`，
    /// 于是往表里加一个键，它不会出现在这份名单里：**一张表、两份名单，缝还在，
    /// 只是挪了个位置。** 变异实测抓到的（往表里加 `down`，消息一边说「只认
    /// enter / esc」一边说「先发 down」）。
    ///
    /// 同一个键有多个别名（`enter` / `回车`）时只露一个：按字节分组，组内取字典序
    /// 最小的那个别名，结果稳定、可测。
    static var displayNames: [String] {
        var seen = Set<[UInt8]>()
        return byAlias.keys.sorted().filter { seen.insert(byAlias[$0]!).inserted }
    }

    /// 表里那些能**搬动菜单高亮**的键（按稳定顺序）。空 = 高亮不在你要的那一项上时
    /// 谁也救不了，只能交给人 —— 消息里那句话就是按这个算的，**不是手写的**。
    static var selectionMoveKeys: [String] {
        ["up", "down", "上", "下"].filter { byAlias[$0] != nil }
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

    /// **按法由这条消息自己算出来，不由任何一句守则去承诺。**
    ///
    /// 今天这个 bug 的成因不是「类型少了个字段」，是**一句静态的指导语去描述一件会变
    /// 的事实**：守则里写着「`nudge_session` 发选项数字」，而上游 11 天里把这个框改了
    /// 三处（编号没了、顺序反了、默认高亮从 `Yes, I trust` 翻成 `No, exit`；两份真字节
    /// fixture 都在库里）。守则停在旧世界，而它是**指导性**的 —— 机长会照着做，
    /// 结果是把一个正在等人救的 session 直接关掉。
    ///
    /// **只要那句话还是静态文本，它就会再次过期。** 所以按法写在这里、按 `numbered`
    /// 现算：这条消息是当时那一屏的产物，跟着屏幕一起变，没有第二个真相源。
    ///
    /// 两条路各自的实测依据（2026-09-06，全新未信任目录 + 真 claude）：
    /// - **有编号**：屏幕上真有 `1.` `2.`，发数字有效。`parseNumbered` 还断言过序号
    ///   必须连续从 1 起，所以这里重新编出来的号跟屏幕上那个对得上。
    /// - **没编号**：按 `1` `2` **屏幕纹丝不动**；而 `nudge_session` 除 `enter`/`esc`
    ///   外一律「发文本 + 自动补一个回车」，真正生效的是那个回车 —— 它确认**当前
    ///   高亮项**。今天信任框的默认高亮是 `No, exit`，所以「发个数字」的净效果是
    ///   把 session 关掉。`esc` 也不是「取消这个框」——实测它让 claude 直接退出。
    ///
    /// **消息里只说该做什么，不断言「有没有某个通道」。** 「今天没有方向键」本身也是
    /// 一句会过期的断言（加不加正等着人拍板）；这里改成按 `SessionNudgeKeys` 算出
    /// **动作**：搬得动就说怎么搬，搬不动就说交给人。表里加一项，这句话自己就变了。
    static func renderOptions(_ options: [String], numbered: Bool) -> [String] {
        guard !options.isEmpty else { return [] }
        guard numbered else {
            return options.map { "  \($0)" } + [
                "怎么答：**这一屏的选项没有编号**。发数字无效（实测画面纹丝不动），"
                + "而 nudge_session 只认这几个裸按键："
                + SessionNudgeKeys.displayNames.map { "`\($0)`" }.joined(separator: " / ")
                + "，其余一律当文本发出去并**自动补一个回车**——那个回车落在**当前高亮的"
                + "那一项**上，未必是你想选的。"
                + "（`esc` 也不是「取消这个框」，实测它让 claude 直接退出。）"
                + "所以**别发数字**。高亮当下就停在你要的那一项上 → 发 `enter`；"
                + (SessionNudgeKeys.selectionMoveKeys.isEmpty
                   ? "不在那一项上 → **交给人**。"
                   : "不在 → 先发 "
                     + SessionNudgeKeys.selectionMoveKeys.map { "`\($0)`" }.joined(separator: " / ")
                     + " 把它移到位，再发 `enter`。"),
            ]
        }
        return options.enumerated().map { "  \($0.offset + 1). \($0.element)" } + [
            "怎么答：这一屏的选项**带编号**，nudge_session 发对应数字即可。",
        ]
    }

    /// - Parameters:
    ///   - question: 在等什么（终端菜单的问句 / codex 的请求描述）。
    ///   - options: 可选项；空 = 不是选择题（如 codex 要求填表单）。
    ///   - numbered: 屏幕上那几项本来有没有编号（见 `renderOptions`）。
    ///   - waitedMinutes: 已等分钟数（升级稿用）。
    static func post(
        stage: Stage, sessionName: String, sessionId: String, isCaptain: Bool,
        question: String, options: [String], numbered: Bool, waitedMinutes: Int
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
        lines.append(contentsOf: renderOptions(options, numbered: numbered))

        let mentions: [String]
        switch stage {
        case .first:
            mentions = isCaptain ? ["human"] : ["captain"]
            // 这里**故意不再复述按法** —— 上面那一行是按当时那一屏算出来的，
            // 是唯一的真相源。在它旁边再写一句静态的「发数字就行」，就是把刚拆掉的
            // 那条会过期的承诺又装回来一份。
            lines.append(isCaptain
                ? "需要人打开这个 session 的终端选一下。"
                : "机长可 inspect_session 看现场；按法照上面那一行（它是按当时那一屏算出来的）。拍不了板就 @人。")
        case .escalate:
            mentions = ["human"]
            lines.append("需要人来定：打开这个 session 的终端直接选，或让机长 nudge_session 代按。")
        }
        lines.append("（session: \(sessionId)）")
        return Post(text: lines.joined(separator: "\n"), mentionKinds: mentions)
    }
}
#endif
