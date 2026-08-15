import Foundation

/// 群聊「正在输入」气泡的显示态判定（Todo #24）。
///
/// 病根：老实现把「正在输入」等同于 `isWorking` = 「终端最近 1s 内有 PTY 输出」。
/// claude 空闲/等输入时 TUI 仍会周期性重绘（提示行、MCP 提示、清行、光标归位），
/// 每次重绘都点亮那 1s 窗口 → 气泡亮一下灭一下循环往复。
///
/// 两道判据（缺一不可，实测支撑见下）：
///
/// 1. **可见文本指纹**：只有「让画面可见文本发生变化」的输出才算干活。纯光标移动 /
///    清行 / 重绘同一段文字都不算。2026-08-08 在 PTY 里实跑 claude 2.1.226 空闲
///    ~36s 抓到的输出：开屏抖动结束后只剩一条 44 字节的 `ESC[K` + 光标归位，
///    **明文为空**；期间还有若干「同一句提示反复重画」的块。这两类正是闪烁的来源，
///    指纹判据把它们整类排除掉 —— 这是治本的那条，光靠时间迟滞治不了。
/// 2. **不对称迟滞**：起亮即时（第一笔真输出就亮，别让气泡迟到），熄灭要求连续
///    `quietFall` 秒没有新的可见文本。取 2.5s 是因为真回合里两次可见变化的间隔
///    远小于此（spinner 计时行每秒都在变），而 2.5s 也短到「回合真结束了」不会让
///    气泡赖着不走。1s（老阈值）太薄：一次工具调用的思考停顿就能把气泡掐灭。
///
/// 3. **视口重绘宽限**（Todo #32）：终端行列数一变（视图挂载/卸载/改尺寸），SwiftTerm
///    就发 `TIOCSWINSZ` → 子进程收 `SIGWINCH` → CLI **整屏重画**。这一屏是**我们要求
///    它画的**，不是它主动说话，可判据 1 拦不住 —— 重绘出来的明文是**从没见过的新
///    指纹**（记忆条数再多也没用），于是必被判成干活。2026-08-08 PTY 实测 claude
///    2.1.226：纯空闲 12s 吐 **0 字节**、同尺寸重设 winsize 吐 **0 字节**、焦点事件
///    （`ESC[I`/`ESC[O`）吐 **0 字节**，**只有列数真变化**才吐 ~1.6KB 整屏重绘，8/8 次
///    必现，延迟 4–15ms。而 SwiftUI 把 `NSViewRepresentable` 卸载再挂载时，宿主 view
///    必经 `frame → .zero → 真实尺寸`（实测），即**切一次 crew 就有两次行列数变化**。
///    这就是「每次切进 crew，某个 session 的气泡闪 2.5s（= `quietFall`）」的完整链条。
///    所以视口一变就开个宽限窗，窗内到达的输出只进指纹记忆、不算干活。
///
/// 纯逻辑、`now` 外部注入（跟 `PendingDecisionTracker` 一个风格），单测直接喂时间序列。
struct TypingActivityTracker {
    struct Config {
        /// 熄灭前要求的连续安静时长（秒）。
        var quietFall: TimeInterval = 2.5
        /// 「最近见过的画面指纹」记忆条数。>1 是因为心跳重绘常常是**几帧轮流**
        /// （例如提示行 A / 提示行 B 交替重画），只记一条会被 A,B,A,B 骗过去。
        var repaintMemory: Int = 3
        /// 指纹取前多少字符（长输出没必要整段留着比）。
        var signatureLimit: Int = 512
        /// 视口（终端行列数）变化后的重绘宽限窗（秒）—— 窗内的输出算「被我们戳出来的
        /// 重绘」，不算干活。实测 SIGWINCH → 首字节 4–15ms，取 0.4s 有 ~26 倍余量；
        /// 又远小于 `quietFall`，所以回合中途撞上 resize 也丢不掉已亮的气泡
        /// （`lastMeaningfulAt` 不动，宽限期一过真输出立刻续上）。
        var viewportRepaintGrace: TimeInterval = 0.4
    }

    private let config: Config
    /// 最近一次「画面可见文本真的变了」的时刻。
    private(set) var lastMeaningfulAt: Date?
    /// 最近一次视口行列数变化的时刻（宽限窗起点）。
    private(set) var lastViewportChangeAt: Date?
    /// 最近若干条可见文本指纹（FIFO）。
    private var recentSignatures: [String] = []

    init(config: Config = Config()) { self.config = config }

    /// 终端行列数变了（视图挂载/卸载/改尺寸 → SIGWINCH）。调用方在 SwiftTerm 的
    /// `sizeChanged` 里喂 —— 那条回调**先于**子进程的重绘输出到达，宽限窗才盖得住。
    mutating func noteViewportChange(at now: Date) {
        lastViewportChangeAt = now
    }

    /// 喂一段刚到达的输出的**明文**（已去 ANSI；调用方用 `AnsiPlainTextTail`）。
    mutating func feed(plainText: String, at now: Date) {
        let sig = Self.signature(plainText, limit: config.signatureLimit)
        // 纯控制序列 / 只有空白换行 —— 画面看不出变化，是心跳不是干活。
        guard !sig.isEmpty else { return }
        // 见过的画面 = 重绘，同样不算干活（但仍要保留在记忆里，别让轮流重绘穿透）。
        guard !recentSignatures.contains(sig) else { return }
        recentSignatures.append(sig)
        if recentSignatures.count > config.repaintMemory { recentSignatures.removeFirst() }
        // 视口刚变过 —— 这一屏是我们要求它画的，只进记忆不算干活（Todo #32）。
        // 记进记忆是有意的：宽限窗一过要是又原样重画一遍，判据 1 接着挡住。
        if let changed = lastViewportChangeAt,
           now.timeIntervalSince(changed) < config.viewportRepaintGrace { return }
        lastMeaningfulAt = now
    }

    /// 当前是否该显示「正在输入」。`isRunning` 为假（已退出）时恒假。
    func isTyping(isRunning: Bool, now: Date) -> Bool {
        guard isRunning, let at = lastMeaningfulAt else { return false }
        return now.timeIntervalSince(at) < config.quietFall
    }

    /// 回合边界/重启时清空（避免旧指纹压住新一轮的相同首屏）。
    mutating func reset() {
        lastMeaningfulAt = nil
        lastViewportChangeAt = nil
        recentSignatures.removeAll()
    }

    /// 指纹 = 折叠所有空白后的可见文本（前 `limit` 字符）。折叠空白是为了让
    /// 「同一句话换个位置重画」（终端重绘常见）也能认出是同一帧。
    static func signature(_ text: String, limit: Int) -> String {
        let collapsed = text.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
        return collapsed.count > limit ? String(collapsed.prefix(limit)) : collapsed
    }
}
