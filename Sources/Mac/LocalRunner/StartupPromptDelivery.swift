#if os(macOS)
import Foundation

/// **开场 brief 的投递编排**（P5a）—— 把「送没送到」从赌一次时序改成可观测、可重试。
///
/// ## 它替掉的那条路
///
/// 旧判据是「收到**首批** PTY 字节 + `Task.sleep(100ms)`」然后写正文。首批字节只证明
/// 子进程**开始画**了，不证明**输入框已经画好**；claude 首屏那几次重绘（清屏 / 切
/// alternate screen / 重画输入框）会把这次写入吞掉。更要命的是吞掉之后**没有任何人
/// 知道** —— 不报错、不重试、状态照样 `.running`，点名把它推成「🟡 空闲」，机长照常
/// 派活。2026-09-04 机长在真 daemon 与正常界面模式**两边都实测到**，命中率不低。
///
/// ## 新判据：看画面，不看时钟
///
/// 判据全部作用在**权威终端缓冲区渲染出来的那屏文本**上（`inspect_session` 取的
/// 就是它），所以无画面也成立：
///
/// 1. **就绪** = 屏幕底部真的有一行输入行（`❯` 提示符行，且它不是选择菜单的选项行）。
/// 2. **落地** = 投完正文后回读，那一行**从空变成有东西**。claude 对大段输入会折成
///    `[Pasted text #1 +N lines]` 之类的占位，所以判据是「变了且非空」，不是「逐字
///    匹配 brief」—— 后者会在正常路径上误报。
/// 3. **提交** = 单发回车后回读，那一行**又变了**（通常是被清空）。回车被吃掉时它
///    一动不动，正是要抓的那种坏法。
/// 4. 三步都有**上限**，到顶就翻 health。**最怕的是静默**。
///
/// 时间只用在两处，且都只是「等够了就认输」，不是「等够了就下注」：等就绪的总期限、
/// 每次投递后的回读窗口。**没有任何一处是「睡一会儿然后闭着眼写」。**
///
/// 纯值语义、时间由调用方喂 —— 所以整台状态机可以脱离进程单测。
struct StartupPromptDelivery {

    /// 三个上限。默认值的理由写在各自旁边；调小只该发生在测试里。
    struct Timing: Equatable {
        /// 等输入框就绪的总期限。claude 画完 banner + 输入框远早于此（MCP server
        /// 是画完之后才加载的），给到 90s 是为了把「本机特别慢」也算进去 ——
        /// 超了不是放弃，是**报出来**，随后仍继续等；真就绪了照投不误。
        var readinessDeadline: TimeInterval = 90
        /// 一次写入之后等多久回读判定。TUI 本地回显通常在 300ms 内，2.5s 是给
        /// 「首屏正忙着重绘」留的余量 —— 窗口太短会让**已经落地**的那笔被判成
        /// 失败而重投一遍（输入框里于是有两份 brief），宁可等。
        var verifyWindow: TimeInterval = 2.5
        /// 写完正文之后，**最早**多久才允许下「它落地了」的结论。两个理由，缺一
        /// 不可：
        /// 1. 我们刚写完的那一瞬，TUI 上一帧还没重画完 —— 那一帧属于「写之前」，
        ///    拿它下结论等于在读过期画面。真 claude 会在空框里自己摆一句灰色示例
        ///    提示（`Try "how do I log an error?"`，见 `ClaudeInputBoxFixtureTests`），
        ///    那一下变化跟我们这笔写入毫无关系，却长得一模一样。
        /// 2. claude 按**字节到达时序**区分「粘贴 vs 敲键」：正文与回车挨太近会被
        ///    整段判成粘贴，回车被吞进输入框文本里、不提交（劈分前 `send()` 就为
        ///    这个隔 200ms 单发回车）。
        /// 所以这不是「睡一会儿然后闭着眼写」——判据仍然是回读画面，这里只是给
        /// 「什么时候的画面才作数」划一条底线。
        var submitGap: TimeInterval = 0.5
        /// 写正文 / 发回车各自的最大次数。
        var maxAttempts = 3
        /// 需要人回答的对话框要稳定多久才认。**直接引 `PendingDecisionTracker` 那个
        /// 常量，不在这里另写一个数字** —— 两条路对「什么时候算真在等人」给出不同
        /// 答案，就会出现「开场投递还认为没定、待决策已经报进群了」这种自相矛盾的
        /// 现场。理由也一样：TUI 半成品画面会短暂长得像菜单，判早了是误报。
        var dialogStable: TimeInterval = PendingDecisionTracker.stableWindow

        static let `default` = Timing()
    }

    /// 喂进来的一拍观测量。**全部来自权威缓冲区**，没有一项是时钟推出来的。
    struct Observation {
        /// 输入行提示符后面的内容；`nil` = 屏幕上根本没有输入行（还没就绪）。
        let inputRow: String?
        /// 正文尾部是否已出现在当前屏幕。超长正文会把 `❯` 顶出屏幕，届时它是
        /// 「正文确实落地」的第二条证据；由调用方拿原始 prompt 与权威画面比对。
        let bodyVisible: Bool
        /// 屏幕上有一个需要人回答的对话框（信任提示 / 命令审批这种）。
        let dialogPresent: Bool
        let now: Date
    }

    /// 这一拍该干什么。调用方只负责执行，不做判断。
    enum Step: Equatable {
        /// 什么都别做。
        case idle
        /// 写正文（**不带回车**）。`attempt > 1` = 这是重投，写之前先按 Ctrl-U
        /// 把输入行清干净 —— 否则上一笔要是其实落了一半，两笔会叠成一段乱码。
        case writeBody(attempt: Int)
        /// 单发回车。正文与回车必须分两笔到达：claude 按字节到达时序区分
        /// 「粘贴 vs 敲键」，一笔写入会被整段判成粘贴、回车被吞进文本里不提交。
        case submit(attempt: Int)
        /// 首屏是需要人回答的对话框 —— **不投**，翻 health 说清卡在哪，现场留给
        /// `inspect_session`。只报不答（同 `PendingDecisionTracker` 的分工）。
        case blockedByDialog(String)
        /// 等到期限也没见到输入框。
        case notReady(String)
        /// 正文反复没进输入行，到上限。
        case undelivered(String)
        /// 正文进去了，但回车反复没被接受，到上限。
        case unsubmitted(String)
        /// 送达并提交完成。
        case delivered
    }

    private enum Phase: Equatable {
        case awaitingReady
        /// `notBefore` = 这一笔写入之后最早可以下结论的时刻（见 `Timing.submitGap`）。
        case writing(attempt: Int, notBefore: Date, deadline: Date, baseline: String)
        /// `submittedContent == nil` = 正文太长，提示符已滚出屏幕；提交成功的判据是
        /// 输入框重新出现。非 nil 时仍按输入行内容变化判定。
        case submitting(attempt: Int, deadline: Date, submittedContent: String?)
        case finished
    }

    let timing: Timing
    private var phase: Phase = .awaitingReady
    /// 等就绪的起点；被对话框挡住期间会往后推 —— 卡在等人身上不该再算进「没就绪」。
    private var readinessSince: Date
    private var dialogSince: Date?
    /// 每类 health 只报一次，别把白板刷成一屏同样的话。
    private var reportedDialog = false
    private var reportedNotReady = false

    init(timing: Timing = .default, startedAt: Date = Date()) {
        self.timing = timing
        self.readinessSince = startedAt
    }

    /// 已经出过终局裁决（成功或放弃）——调用方据此把这台机器收掉。
    var isFinished: Bool { phase == .finished }

    mutating func step(_ obs: Observation) -> Step {
        switch phase {
        case .finished:
            return .idle

        case .awaitingReady:
            if obs.dialogPresent {
                let since = dialogSince ?? obs.now
                dialogSince = since
                // 卡在等人回答期间不计「没就绪」的账。
                readinessSince = obs.now
                guard obs.now.timeIntervalSince(since) >= timing.dialogStable else { return .idle }
                guard !reportedDialog else { return .idle }
                reportedDialog = true
                return .blockedByDialog(
                    "开场任务没投出去：首屏是一个需要人回答的对话框（信任此文件夹 / 命令审批这类），"
                    + "brief 投进去会被当成按键吃掉。用 inspect_session 看现场、nudge_session 代答，"
                    + "答完它会自己把开场任务补投出去。")
            }
            dialogSince = nil
            if let row = obs.inputRow {
                phase = .writing(
                    attempt: 1,
                    notBefore: obs.now.addingTimeInterval(timing.submitGap),
                    deadline: obs.now.addingTimeInterval(timing.verifyWindow),
                    baseline: row)
                return .writeBody(attempt: 1)
            }
            guard obs.now.timeIntervalSince(readinessSince) >= timing.readinessDeadline,
                  !reportedNotReady else { return .idle }
            reportedNotReady = true
            return .notReady(
                "开场任务还没投出去：TUI 在吐输出，但等了 \(Int(timing.readinessDeadline))s "
                + "也没在画面上等到输入框。用 inspect_session 看它停在哪一屏。")

        case let .writing(attempt, notBefore, deadline, baseline):
            // 画面正重绘到一半、输入行暂时不在 —— 等下一拍，别当成失败。
            guard let row = obs.inputRow else {
                // crew 开场正文可能长到把提示符顶出当前屏幕。正文尾部仍清楚可见时，
                // 这不是「输入框没画好」，而是落地成功；继续走独立回车提交。
                guard obs.now >= notBefore, obs.bodyVisible else { return .idle }
                phase = .submitting(
                    attempt: 1,
                    deadline: obs.now.addingTimeInterval(timing.verifyWindow),
                    submittedContent: nil)
                return .submit(attempt: 1)
            }
            if obs.now >= notBefore, Self.landed(row: row, baseline: baseline) {
                phase = .submitting(
                    attempt: 1,
                    deadline: obs.now.addingTimeInterval(timing.verifyWindow),
                    submittedContent: row)
                return .submit(attempt: 1)
            }
            guard obs.now >= deadline else { return .idle }
            guard attempt < timing.maxAttempts else {
                phase = .finished
                return .undelivered(
                    "开场任务没送到：正文写了 \(attempt) 次，回读权威缓冲区确认输入行始终是空的"
                    + "（首屏重绘把它吞了）。这个 session 一个字都没收到，派给它的活等于没派出去——"
                    + "请改派或重起。")
            }
            phase = .writing(
                attempt: attempt + 1,
                notBefore: obs.now.addingTimeInterval(timing.submitGap),
                deadline: obs.now.addingTimeInterval(timing.verifyWindow),
                baseline: baseline)
            return .writeBody(attempt: attempt + 1)

        case let .submitting(attempt, deadline, submitted):
            guard let row = obs.inputRow else { return .idle }
            if submitted == nil || row != submitted {
                // 输入行变了（通常是被清空）= 那一笔回车被接受了。
                phase = .finished
                return .delivered
            }
            guard obs.now >= deadline else { return .idle }
            guard attempt < timing.maxAttempts else {
                phase = .finished
                return .unsubmitted(
                    "开场任务卡在输入框里没提交：正文已经进去了，但回车发了 \(attempt) 次都没被接受"
                    + "（多半有个模态挡在前面）。用 inspect_session 看现场。")
            }
            phase = .submitting(
                attempt: attempt + 1,
                deadline: obs.now.addingTimeInterval(timing.verifyWindow),
                submittedContent: submitted)
            return .submit(attempt: attempt + 1)
        }
    }

    /// 「正文真的进了输入行」的判据：那一行**从原样变成了别的东西，且非空**。
    ///
    /// 为什么不逐字匹配 brief：claude 对大段输入会折成 `[Pasted text #1 +N lines]`
    /// 这类占位（开场 brief 恰恰就是大段），逐字匹配会在**正常路径**上判失败，
    /// 于是反复重投 + 误报 health —— 比原来的静默还糟。
    static func landed(row: String, baseline: String) -> Bool {
        !row.isEmpty && row != baseline
    }
}

/// **claude 输入框的识别**（P5a）——「就绪没有」「东西进去没有」这两问的尺子。
///
/// 作用在**渲染完的整屏文本**上（每行一个元素），不是原始字节流：首屏那一串
/// 光标定位 / 清行 / 重绘只有喂进 `Terminal` 才知道最后长什么样，在字节流上认形状
/// 等于自己再写一个终端模拟器。
///
/// 判据本身认不认得**真** claude 的画面，由 `ClaudeInputBoxFixtureTests` 拿录下来的
/// 真字节验（`Tests/Fixtures/tui-claude.bin`）——不凭印象编 ANSI 序列。
enum ClaudeInputBox {

    /// 输入行的提示符字形。`>` 也认（老版本 claude 用的是它）。
    static let markers: Set<Character> = ["❯", "›", ">"]

    /// 从底往上找多少行。输入框下面还压着框线与提示条（「auto mode on … esc to
    /// interrupt」），所以**不能只看最后一行**；给到 15 行也够长输入框折行占的位。
    static let scanDepth = 15

    /// 输入行提示符后面的内容；`nil` = 这一屏上没有输入框（= 还没就绪）。
    ///
    /// 选择菜单的选项行（`❯ 1. Yes, I trust this folder`）**不是**输入行 ——
    /// 撞上就直接判「没就绪」：那一屏在等人回答，谁都不该往里投东西。
    ///
    /// ⚠️ **逐行判断挡不住没有编号的那种框。** 2026-09-06 现录的真信任框长这样：
    /// ```
    ///  ❯  No, exit
    ///     Yes, I trust this folder
    /// ```
    /// 单看 `❯  No, exit` 这一行，它跟「人在输入框里打了字」一模一样 —— 实测
    /// （`ClaudeInputBoxFixtureTests`）这里当场返回了 `"No, exit"`，也就是把 brief
    /// 当按键投进那个框、再回车，而默认高亮正停在 `No, exit` 上。所以判据只能落在
    /// **整屏**这一层：这一屏要是在等人回答，它就不是输入行，一行都不是。
    static func inputRow(_ rows: [String]) -> String? {
        guard blockingDialog(rows) == nil else { return nil }
        for line in normalize(rows).suffix(scanDepth).reversed() {
            let stripped = TerminalMenuParser.strip(line)
            guard let first = stripped.first, markers.contains(first) else { continue }
            if TerminalMenuParser.option(stripped) != nil { return nil }
            return String(stripped.dropFirst()).trimmingCharacters(in: .whitespaces)
        }
        return nil
    }

    /// 超长正文会把 `❯` 提示符滚出当前屏幕。去掉换行/空白后比对正文尾部，既不受
    /// 终端自动折行影响，也不会把普通 banner 重绘误当成正文已落地。
    static func bodyTailVisible(prompt: String, rows: [String]) -> Bool {
        let compactPrompt = prompt.filter { !$0.isWhitespace }
        guard !compactPrompt.isEmpty else { return false }
        let needle = String(compactPrompt.suffix(64))
        let screen = normalize(rows).joined().filter { !$0.isWhitespace }
        return screen.contains(needle)
    }

    /// 这一屏上是不是摆着一个**需要人回答**的对话框（信任此文件夹 / 命令审批 / 选
    /// 登录方式）。菜单形状的判定复用 `TerminalMenuParser`，不另写一套。
    ///
    /// ⚠️ **必须喂渲染完的画面，不能喂去 ANSI 的字节尾窗。** 这不是偏好问题 ——
    /// 拿录下来的真字节实测过（`Tests/Fixtures/tui-claude.bin`）：claude 的信任
    /// 对话框是靠**光标定位**（`\u{1b}[4G` `\u{1b}[7G` …）把词摆到列上的，字节流里
    /// 那一行长这样 `1.Yes,Itrustthisfolder` —— 序号后面根本没有空格，
    /// `TerminalMenuParser.option` 当场判 nil；而且行尾是 `\r\r\n`，去 ANSI 之后
    /// 一行变三行，两个选项之间凭空多出空行，`parse` 的「连续选项块」也断了。
    /// 只有把字节喂进 `Terminal` 让它把列位置解出来，那一屏才重新长成人看到的样子。
    ///
    /// （顺带：跑在生产里的 `PendingDecisionTracker` 吃的正是字节尾窗，所以它**认不出
    /// claude 的信任对话框** —— 那是 Todo #6 那条线自己的账，不在本次修复范围内，
    /// 已单独回报。这里不复用它，就是因为它在这个具体画面上是瞎的。）
    static func blockingDialog(_ rows: [String]) -> PendingTerminalDecision? {
        TerminalMenuParser.parse(normalize(rows).joined(separator: "\n"))
    }

    /// **从没被写过的格子在 SwiftTerm 里读出来是 NUL，不是空格。** claude 是用
    /// 光标定位（`\u{1b}[12G`）跳着把词摆到列上的，跳过去的那些格一次都没被写过，
    /// `translateToString` 于是交出一行 `❯\0\0\01.\0\0Yes,\0I\0trust…`。
    ///
    /// NUL 不是空白字符 —— `TerminalMenuParser` 的 `strip`/`option` 当场判死。
    /// 实测（`ClaudeInputBoxFixtureTests` 拿真字节跑的第一趟）：不做这一步，
    /// **真 claude 的信任对话框一条都认不出来**，而判据自己毫无察觉地返回「没有
    /// 对话框」——又是一种静默。所以这一行不是美化，是判据能不能用的前提。
    static func normalize(_ rows: [String]) -> [String] {
        rows.map { $0.contains("\0") ? $0.replacingOccurrences(of: "\0", with: " ") : $0 }
    }
}
#endif
