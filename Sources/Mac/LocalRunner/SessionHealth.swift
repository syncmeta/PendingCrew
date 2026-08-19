#if os(macOS)
import Foundation

/// 一条 runner 健康异常：session 还「活着」（进程未退），但底层 agent 已经
/// 干不了活 —— 未登录 / 额度到顶。此前这两类故障只在 PTY 字节流里滚过、
/// app 零感知，用户得进终端肉眼看（分诊第 7 点）。
///
/// 检测面（能可靠拿到什么就报什么，不猜）：
/// - claude（PTY）：`SessionHealthScanner` 对终端输出做去 ANSI 的文案匹配，
///   匹配串全部从本机 claude CLI 2.1.201 二进制 strings 实测核出，非臆测。
/// - codex（app-server）：`account/chatgptAuthTokens/refresh` server-request
///   —— codex 要客户端刷新 ChatGPT token,我们无法提供 = 登录态失效的强信号。
struct CrewSessionHealth: Equatable {
    enum Kind: String, CaseIterable {
        /// 未登录 / 登录失效 / key 无效。
        case authRequired
        /// 额度到顶 / 余额不足。
        case usageLimit
        /// 撞限额卡在 rate-limit 模态菜单 / 等待额度重置（Todo #10）。由
        /// `RateLimitMenuScanner` 命中时翻，不走本扫描器的短语表。与 `usageLimit`
        /// 分开：这类 session 进程活着且已被自动应答「Stop and wait」，语义是
        /// 「限额等待中」而非「干不了活需要人管」——但同样走红点异常链路，不装空闲。
        case rateLimited
        /// 压根没拉起来 / 起来即死 / 起来了但零输出（#541）。与上面三类的区别：
        /// 那些是「跑起来了但干不了活」，这个是**这个 session 从来没活过**——
        /// 派给它的活等于没派出去，机长必须立刻改派。由 `SessionLaunchProbe`
        /// 的终局裁决翻，不走 PTY 短语扫描。
        case launchFailed
    }
    let kind: Kind
    /// 人话说明 + 下一步动作（白板 fail-loud 消息与成员列表副行直接展示）。
    let detail: String

    /// 额度类异常（撞墙/卡限额菜单）——hit-limit 终止识别与自动挂唤醒都认这组。
    var isQuotaRelated: Bool { kind == .usageLimit || kind == .rateLimited }
}

/// 成员实时状态的**唯一**推导（#541）：点名快照 `crew-sessions.json` 与
/// `inspect_session` 共用一份，别再各写一遍 —— 两处口径分叉正是「一边说空闲、
/// 一边说没输出」这类互相打架的来源。
///
/// 优先级（先坏消息后好消息）：拉起失败 → 已退出 → 限额中/异常 → 等人拍板 →
/// 待回复 → 干活中/空闲。拉起失败排在「已退出」前面：它同样已经不 running，但语义差得
/// 远 ——「跑完了」vs「从来没跑起来，活等于没派出去」，机长要据此立刻改派。
///
/// `awaitingDecision` / `awaitingReply` 必须排在 working/idle **前面**（Todo #6 / #25）：
/// 卡在终端菜单上、或问完一句停住的 session 都不吐输出 → `isWorking` 为假 → 会被推成
/// 「🟡 空闲」，机长照常派活，活石沉大海。这正是「session 事实上死掉」在点名里的样子。
///
/// 两者的分工：`awaitingDecision` 是**画面上明摆着的菜单**（机长 nudge 发个数字就能代答）；
/// `awaitingReply` 是「它在等人回话」的一般情形（`ask` 挂着 / 大白话问了一句），判定见
/// `SessionAwaitingReply`。菜单更具体、更可代办，所以排前面。
enum CrewSessionStateDerivation {
    static let launchFailed = "launchFailed"
    static let awaitingDecision = "awaitingDecision"
    static let awaitingReply = "awaitingReply"

    static func state(
        isRunning: Bool, health: CrewSessionHealth?, isWorking: Bool,
        awaitingDecision: Bool = false, awaitingReply: Bool = false
    ) -> String {
        if health?.kind == .launchFailed { return launchFailed }
        if !isRunning { return "exited" }
        if let h = health { return h.isQuotaRelated ? "rateLimited" : "error" }
        if awaitingDecision { return Self.awaitingDecision }
        if awaitingReply { return Self.awaitingReply }
        return isWorking ? "working" : "idle"
    }
}

/// 「限额中」的**恢复**判定 —— 纯函数，好单测。
///
/// 为什么需要它（机长 2026-07-26 实遇）：`rateLimited` / `usageLimit` 此前**进得去
/// 出不来** —— 唯一的清除路径是「额度重置唤醒到点」（`CrewSessionRunner.fire` 里
/// 的 `rearmQuotaHealth`）。机长撞 Fable 周限额后换到 opus 一路正常干活、验收合并
/// 都做完了，点名却仍显示「⏳ 限额中」，因为那条重置唤醒还要几小时才到点。状态谎报
/// 比没有状态更糟：机长按它派活会以为一屋子人都卡着。
///
/// 判据：**撞限额之后才开始的一段连续干活，持续够久**。真被限额挡住的 session 不会
/// 连着吐好几秒输出（它要么停在提示符、要么停在「等重置」的静态画面）；能连着吐就是
/// 在跑回合 = 已经恢复。要求 streak 起点晚于撞限额时刻，是为了不把「打印限额报错
/// 那一阵输出」当成恢复。
///
/// 误判也不致命：清除的同时会重新武装扫描器与白板首报，下次真撞墙照样报。
enum QuotaHealthRecovery {
    /// 连续干活多久算恢复。经验值：claude 跑一个回合轻松过 6s，而「等重置」的静态
    /// 画面不会连续吐 6s 输出。真出现误判再按实测调，别提前调参。
    static let workStreak: TimeInterval = 6

    /// - Parameters:
    ///   - health: 当前健康态（非额度类 / nil 一律不动）。
    ///   - quotaHealthAt: 额度类 health 被置上的时刻。
    ///   - workingSince: 当前这段**连续**干活的起点；不在干活则 nil。
    static func recovered(
        health: CrewSessionHealth?, quotaHealthAt: Date?, workingSince: Date?,
        now: Date = Date(), streak: TimeInterval = workStreak
    ) -> Bool {
        guard health?.isQuotaRelated == true, let at = quotaHealthAt,
              let since = workingSince, since > at else { return false }
        return now.timeIntervalSince(since) >= streak
    }
}

/// 去 ANSI 的滚动明文尾窗 —— `SessionHealthScanner` 与 `RateLimitMenuScanner`
/// 共用的底座：喂 PTY 原始字节，剥 CSI / OSC / 两字节 ESC 序列与控制字符，
/// 维护一段最近明文（跨 chunk 断开的短语在窗内重新连上）。
final class AnsiPlainTextTail {
    private enum AnsiState { case normal, esc, skipOne, csi, osc, oscEsc }
    private var ansiState: AnsiState = .normal
    private(set) var tail = ""
    /// `tail` 的 **ASCII 小写字节镜像**（A–Z→a–z，其余字节原样）。
    ///
    /// 为什么要它（Todo #59）：扫描器原来每笔 PTY 输出都 `tail.lowercased()` 新建
    /// 一份整窗 String，再逐条 `String.contains` —— Foundation 的 `contains` 走
    /// grapheme 级朴素搜索，实测 8K 窗 3 个扫描器一笔要 6.3 ms、16K 窗 10.2 ms，
    /// 全在主线程、按在跑的 session **线性叠加**。短语表全是 ASCII，改成在这份
    /// 字节镜像上搜，11 条短语一共 0.08 ms（快两个数量级）。
    private(set) var loweredASCII: [UInt8] = []
    private let tailLimit: Int

    /// 尾窗上限（字符）。超过 2 倍才截，保证截断永远不会切在
    /// 「刚到达还没匹配过」的短语中间。
    init(tailLimit: Int = 4096) {
        self.tailLimit = tailLimit
    }

    /// 喂一段 PTY 原始字节；返回是否有新明文入窗（false = 全是控制序列，
    /// 调用方可以直接跳过匹配）。
    @discardableResult
    func feed(_ bytes: ArraySlice<UInt8>) -> Bool {
        var plain: [UInt8] = []
        for b in bytes {
            switch ansiState {
            case .normal:
                if b == 0x1b { ansiState = .esc }
                else if b >= 0x20, b != 0x7f { plain.append(b) }   // 含 UTF-8 ≥0x80
                else if b == 0x0a || b == 0x0d { plain.append(0x0a) }
            case .esc:
                switch b {
                case UInt8(ascii: "["): ansiState = .csi
                case UInt8(ascii: "]"): ansiState = .osc
                case UInt8(ascii: "("), UInt8(ascii: ")"), UInt8(ascii: "*"),
                     UInt8(ascii: "+"), UInt8(ascii: "#"), UInt8(ascii: "%"):
                    ansiState = .skipOne                            // 两字节序列,再吃一个
                default: ansiState = .normal                        // ESC+单字节(如 ESC 7)
                }
            case .skipOne:
                ansiState = .normal
            case .csi:
                if (0x40...0x7e).contains(b) { ansiState = .normal }  // final byte
            case .osc:
                if b == 0x07 { ansiState = .normal }                  // BEL 终止
                else if b == 0x1b { ansiState = .oscEsc }
            case .oscEsc:
                ansiState = b == UInt8(ascii: "\\") ? .normal : .osc  // ST 终止
            }
        }
        guard !plain.isEmpty else { return false }
        tail += String(decoding: plain, as: UTF8.self)
        loweredASCII.reserveCapacity(loweredASCII.count + plain.count)
        for b in plain { loweredASCII.append(Self.asciiLowered(b)) }
        if tail.count > tailLimit * 2 {
            tail = String(tail.suffix(tailLimit))
            // 截完从 `tail` 重建镜像，保证两者永远同一段内容（这条每窗只跑一次）。
            loweredASCII = tail.utf8.map(Self.asciiLowered)
        }
        return true
    }

    /// 在 ASCII 小写镜像里做子串搜索。`needle` 必须是**已小写**的 UTF-8 字节
    /// （短语表是常量，调用方预先转好一次即可）。
    func containsLoweredASCII(_ needle: [UInt8]) -> Bool {
        guard !needle.isEmpty, needle.count <= loweredASCII.count else { return false }
        let first = needle[0]
        let last = loweredASCII.count - needle.count
        var i = 0
        while i <= last {
            if loweredASCII[i] == first {
                var j = 1
                while j < needle.count, loweredASCII[i + j] == needle[j] { j += 1 }
                if j == needle.count { return true }
            }
            i += 1
        }
        return false
    }

    private static func asciiLowered(_ b: UInt8) -> UInt8 {
        (b >= 0x41 && b <= 0x5a) ? b + 0x20 : b
    }

    /// 把一条短语转成可以喂给 `containsLoweredASCII` 的字节。短语表是常量，
    /// 每个扫描器**转一次**存成 static，别在 `feed` 里现转。
    static func loweredNeedle(_ phrase: String) -> [UInt8] {
        phrase.utf8.map(asciiLowered)
    }

    /// 清空尾窗（ANSI 状态机保留 —— 序列可能正跨在清空点上）。
    func clear() { tail = ""; loweredASCII = [] }
}

/// claude PTY 输出的健康扫描器：喂原始终端字节，产出**首次**命中的健康异常
/// （每 Kind 只报一次 —— 同一故障 TUI 会反复重绘,不能每帧都刷一条白板消息）。
///
/// 工作方式：
/// 1. 去 ANSI —— 小状态机剥掉 CSI / OSC / 两字节 ESC 序列与控制字符，只留
///    可见文本（UTF-8 多字节 ≥0x80 原样保留）。
/// 2. 滚动尾窗 —— 只保留最近一段明文（跨 chunk 断开的短语在窗内重新连上）。
/// 3. 匹配 —— 小写子串匹配下面的实测短语表。
///
/// 已知边界：终端里**内容级**出现这些短语（比如 agent 在 cat 一段含
/// "run /login" 的文档）会误报 —— 这是提示性 warning 且每 Kind 只一次，
/// 接受这个噪音换「不用进终端就知道挂了」（记 tech-debt）。
final class SessionHealthScanner {

    /// 实测短语表（本机 claude 2.1.201 strings 核出;小写匹配）。
    /// 注意保持短语足够特异 —— 别放 "usage limit" 这种会出现在普通 UI 提示里的宽串。
    static let authPhrases = [
        "run /login",             // "Please run /login to authenticate" 等全家
        "not logged in",          // "Not logged in. Run claude auth login …"
        "invalid api key",
    ]
    static let quotaPhrases = [
        "you've reached your",    // "You've reached your … usage limit"
        "you’ve reached your",    //  同上,弯引号变体
        "usage credit limit reached",
        "out of extra usage",     // "You're out of extra usage"
        "credit balance is too low",
        "credit balance too low",
    ]

    /// 去 ANSI 明文尾窗（短语都 < 64 字符,4K 默认窗绰绰有余）。
    private let stripper = AnsiPlainTextTail()
    private var fired: Set<CrewSessionHealth.Kind> = []

    /// 短语表的**已小写 UTF-8 字节**形式 —— 匹配走 `containsLoweredASCII`
    /// （见 `AnsiPlainTextTail.loweredASCII` 上那段为什么）。短语全是 ASCII
    /// （`you’ve` 的弯引号是多字节，但按字节比一样精确），所以字节比与原来的
    /// `lowercased()` + `contains` 在这张表上等价，有单测钉。
    static let authNeedles = authPhrases.map(AnsiPlainTextTail.loweredNeedle)
    static let quotaNeedles = quotaPhrases.map(AnsiPlainTextTail.loweredNeedle)

    /// 本扫描器只可能翻这两类（`rateLimited` 归 `RateLimitMenuScanner`、
    /// `launchFailed` 归 `SessionLaunchProbe`）——两类都报过就彻底收工。
    private static let scannerKinds: Set<CrewSessionHealth.Kind> = [.authRequired, .usageLimit]

    /// 喂一段 PTY 原始字节，返回**新**命中的健康异常（通常空数组）。
    func feed(_ bytes: ArraySlice<UInt8>) -> [CrewSessionHealth] {
        guard !fired.isSuperset(of: Self.scannerKinds) else { return [] }
        guard stripper.feed(bytes) else { return [] }

        var out: [CrewSessionHealth] = []
        if !fired.contains(.authRequired),
           Self.authNeedles.contains(where: { stripper.containsLoweredASCII($0) }) {
            fired.insert(.authRequired)
            out.append(CrewSessionHealth(
                kind: .authRequired,
                detail: "Claude Code 未登录或登录已失效 —— 打开这个 session 的终端跑 /login 登录后再继续。"))
        }
        if !fired.contains(.usageLimit),
           Self.quotaNeedles.contains(where: { stripper.containsLoweredASCII($0) }) {
            fired.insert(.usageLimit)
            out.append(CrewSessionHealth(
                kind: .usageLimit,
                detail: "Claude Code 额度受限（usage limit / 余额不足）—— 等额度窗口重置或检查订阅后再继续。"))
        }
        return out
    }

    /// 重新武装额度类命中（额度重置唤醒到点后调）——下一个限额窗再撞墙时
    /// 能再次首报（fail-loud），而不是被「每 Kind 一次」永久哑掉。
    func rearmQuota() { fired.remove(.usageLimit) }
}

/// claude 撞限额弹出的 `/rate-limit-options` 模态菜单检测器（Todo #10 层1）。
/// 菜单形如「What do you want to do? ❯ 1. Stop and wait for limit to reset /
/// 2. Upgrade your plan」——弹出后整个 session 卡死等按键，@ 注入全被菜单吞。
/// 命中 → 调用方替 session 选「Stop and wait」按 Enter 自动应答。
///
/// 触发纪律：TUI 每帧重绘同一菜单 → 冷却期（默认 30s）内不重复触发；触发后
/// 清空尾窗，若按键没生效菜单仍在重绘，冷却期满会再次命中 = 内建重试。
final class RateLimitMenuScanner {
    /// 菜单特征短语（小写子串匹配）。两条都足够特异，普通输出不会撞上；
    /// 内容级误报（agent cat 出含这些串的文档）后果只是多按一次 Enter，可接受。
    static let menuPhrases = [
        "/rate-limit-options",
        "stop and wait for limit to reset",
    ]

    /// 同上：已小写 UTF-8 字节形式，匹配走 `containsLoweredASCII`。
    static let menuNeedles = menuPhrases.map(AnsiPlainTextTail.loweredNeedle)

    private let stripper = AnsiPlainTextTail()
    private let cooldown: TimeInterval
    private var lastFiredAt: Date?

    init(cooldown: TimeInterval = 30) {
        self.cooldown = cooldown
    }

    /// 喂一段 PTY 原始字节；返回 true = 新检测到菜单（该自动应答了）。
    func feed(_ bytes: ArraySlice<UInt8>, now: Date = Date()) -> Bool {
        guard stripper.feed(bytes) else { return false }
        guard Self.menuNeedles.contains(where: { stripper.containsLoweredASCII($0) })
        else { return false }
        if let last = lastFiredAt, now.timeIntervalSince(last) < cooldown { return false }
        lastFiredAt = now
        stripper.clear()
        return true
    }
}
#endif
