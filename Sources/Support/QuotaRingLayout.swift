// 纯 Foundation、不带平台门 —— 侧栏额度圆环的**判定层**，编进 PendingCrewTests
// 单测。视图（QuotaRingsFooter）只负责画，「画哪些环、环里写什么字、算哪一档
// 颜色、快照多旧」这几件事全部收在这里，免得判定逻辑散进 SwiftUI body 里没法测。
import Foundation

/// 余量档位 —— 额度配色阈值的单一真值。
///
/// 阈值自 #455 起就是这四档，Todo #8 只换了呈现方式（文字表格 → 圆环），
/// **阈值不动**：已用 <60 充裕 / <80 过半 / <90 将尽 / ≥90 告急。
enum QuotaLevel: String, Equatable {
    case ample      // <60 —— 宽裕
    case half       // <80 —— 过半
    case near       // <90 —— 快到
    case critical   // ≥90 —— 告急

    static func of(usedPercent: Int) -> QuotaLevel {
        switch usedPercent {
        case ..<60: return .ample
        case ..<80: return .half
        case ..<90: return .near
        default:    return .critical
        }
    }
}

/// 侧栏底部的一个额度环。
struct QuotaRing: Equatable, Identifiable {
    /// 环的外形：正圆（"5h" / "周" 两个字塞得下）或 stadium 长条（模型名长，
    /// 正圆里挤不开 —— 进度沿整个长条外轮廓走）。
    enum Form: Equatable { case circle, stadium }

    /// ForEach 的稳定 id：`agent.窗名`（同一 agent 内窗名唯一）。
    let id: String
    /// 环里平时显示的字（"5h" / "周" / "Fable"）。悬停时换成百分比。
    let caption: String
    /// 悬停/无障碍用的完整窗名（"session" / "周(全模型)" / "周(Fable)"）。
    let windowLabel: String
    let usedPercent: Int
    let form: Form
    /// 这一项的重置时刻原样字符串（claude 人话 / codex ISO8601）；悬停到本环时
    /// 环下面那行显示它（Todo #14）。上游没给 → nil。
    var resetsAt: String? = nil
    /// 哪一家（"Claude Code" / "Codex"）—— 两家都有叫「周」的环，悬停文案里得
    /// 说清是谁的周。
    var brand: String = ""

    var level: QuotaLevel { .of(usedPercent: usedPercent) }
    /// 进度 0…1（百分比可能被上游给成越界值，钳死免得环画飞）。
    var progress: Double { min(max(Double(usedPercent) / 100, 0), 1) }
}

/// 额度快照 → 环列表 / 新鲜度文案的映射。
enum QuotaRingLayout {

    // MARK: - 快照 → 环

    /// claude 一行三个环：5h（`session` 窗）→ 周（`周(全模型)` 窗）→ 当前模型
    /// （`周(<模型名>)` 分窗，长条）。
    ///
    /// **拿不到的窗就不画那个环**（Todo #8 ⑦：不画空环占位 —— 空环看着像坏了）。
    /// 所以 claude 没登录/解析失败时这一行整个消失，而不是三个灰圈。
    static func claudeRings(_ snapshot: AgentQuotaSnapshot?) -> [QuotaRing] {
        guard let snapshot else { return [] }
        var rings: [QuotaRing] = []
        if let w = snapshot.fiveHourWindow {
            rings.append(ring(agent: "claude", window: w, caption: "5h", form: .circle))
        }
        if let w = snapshot.weeklyWindow {
            rings.append(ring(agent: "claude", window: w, caption: "周", form: .circle))
        }
        if let w = currentModelWindow(snapshot), let name = modelName(fromWindowLabel: w.label) {
            rings.append(ring(agent: "claude", window: w, caption: name, form: .stadium))
        }
        return rings
    }

    /// codex 一行只有周窗 —— codex 侧已经没有 5 小时分档了（人类 2026-07-26 确认），
    /// 拿不到周窗（免费档 / 没跑过 codex）就整行不画。
    static func codexRings(_ snapshot: AgentQuotaSnapshot?) -> [QuotaRing] {
        guard let snapshot, let w = snapshot.weeklyWindow else { return [] }
        return [ring(agent: "codex", window: w, caption: "周", form: .circle)]
    }

    private static func ring(agent: String, window: AgentQuotaWindow,
                             caption: String, form: QuotaRing.Form) -> QuotaRing {
        QuotaRing(id: "\(agent).\(window.label)", caption: caption,
                  windowLabel: window.label, usedPercent: window.usedPercent, form: form,
                  resetsAt: window.resetsAt,
                  brand: agent == "claude" ? "Claude Code" : "Codex")
    }

    /// 「当前模型」那一格取哪个窗：claude `/usage` 除了 `周(全模型)` 还会额外列出
    /// **当前所用模型**的周分窗（`周(Fable)`）—— 取第一个非全模型的周分窗即可。
    /// 换了模型下一轮刷新自然跟着换。没有分窗（老版 CLI / 该模型无独立配额）→ nil。
    static func currentModelWindow(_ snapshot: AgentQuotaSnapshot) -> AgentQuotaWindow? {
        snapshot.windows.first { modelName(fromWindowLabel: $0.label) != nil }
    }

    /// `周(Fable)` → `Fable`；`周(全模型)` 是总窗不是模型窗 → nil；其它窗名 → nil。
    /// 首字母大写（人类明确要求 "F 要大写"）—— 其余字符原样保留，免得把
    /// `GPT-5` 之类的大小写压平。
    static func modelName(fromWindowLabel label: String) -> String? {
        guard label.hasPrefix("周("), label.hasSuffix(")") else { return nil }
        let inner = String(label.dropFirst(2).dropLast())
        guard !inner.isEmpty, inner != "全模型" else { return nil }
        return inner.prefix(1).uppercased() + inner.dropFirst()
    }

    // MARK: - 新鲜度

    /// 环下面那行「N 分钟前」：取两家快照里**最新**的 `fetchedAt`。
    /// 两家都没有 → nil（调用方连同刷新按钮一起隐藏）。
    /// 时钟回拨/未来时刻按「刚刚」处理，不显示负数。
    static func freshnessLabel(fetchedAt stamps: [String?], now: Date = Date()) -> String? {
        let dates = stamps.compactMap { $0 }.compactMap { ISO8601DateFormatter().date(from: $0) }
        guard let newest = dates.max() else { return nil }
        let seconds = now.timeIntervalSince(newest)
        if seconds < 60 { return "刚刚" }
        let minutes = Int(seconds / 60)
        if minutes < 60 { return "\(minutes) 分钟前" }
        let hours = minutes / 60
        if hours < 24 { return "\(hours) 小时前" }
        return "\(hours / 24) 天前"
    }

    // MARK: - 环下那行显示什么（常态 / 悬停）

    /// 环下面那行的文案：**没悬停**时是「N 分钟前」（数据几时读的），**悬停到某个
    /// 环**时换成那一项的重置时刻（Todo #14 —— 「用了多少」看环、「几点回血」看这行，
    /// 且只说鼠标指着的那一项）。
    ///
    /// 两家都有叫「周」的环，所以带上家名消歧。重置时刻解不出（上游没给 / 格式变了）
    /// 就明说未知，不退回「N 分钟前」—— 悬停中却显示读取时刻会让人误读成重置时刻。
    static func footnote(hovered: QuotaRing?, fetchedAt stamps: [String?],
                         now: Date = Date()) -> String? {
        guard let ring = hovered else {
            return freshnessLabel(fetchedAt: stamps, now: now)
        }
        let window = AgentQuotaWindow(label: ring.windowLabel,
                                      usedPercent: ring.usedPercent, resetsAt: ring.resetsAt)
        let reset = window.compactResetLabel(now: now).map { "\($0) 重置" } ?? "重置时刻未知"
        let name = ring.brand.isEmpty ? ring.caption : "\(ring.brand) \(ring.caption)"
        return "\(name) · \(reset)"
    }

    // MARK: - 陈旧标记

    /// 某一家的**数据年龄**标记，挂在那一行环后面（如 "35 天前的数据"）。不老 → nil。
    ///
    /// 为什么要**按家**标而不是共用下面那行「N 分钟前」：那行说的是「我们几点去读
    /// 的」，取的还是两家里最新的一个——claude 刚查过就会把 codex 一个多月没动的
    /// 数字一起盖成「刚刚」。而 codex 的额度来自 rollout 文件，只有真跑 codex 才更新，
    /// 于是界面在替陈旧数据打包票（实测 2026-07-26：重置时刻停在 6/21、6/25，
    /// 而那行显示「刚刚」）。判定和口径在 `AgentQuotaSnapshot`，这里只裁一句短的。
    static func staleBadge(_ snapshot: AgentQuotaSnapshot?, now: Date = Date()) -> String? {
        guard let snapshot, let age = snapshot.ageSeconds(now: now),
              age >= AgentQuotaSnapshot.staleThreshold else { return nil }
        return "\(AgentQuotaSnapshot.humanAge(age))的数据"
    }

    /// 一行环后面的**警示标记**（陈旧标记的超集）。优先级从硬到软：
    ///
    /// 1. **读不到** —— 这一轮压根没取到数（`failure != nil`）。还留着上一轮快照就
    ///    明说「下面是旧值」，一次都没取到过就只说读不到。绝不让「读不到」和
    ///    「就是这个数」在界面上长得一样（Todo #33）。
    /// 2. **窗口已翻篇** —— 快照里所有窗都过了自己的重置时刻，那个百分比必然不是
    ///    现状。这条比岁数阈值硬：数据可能只有半小时大，窗却已经重置过了。
    /// 3. **数据陈旧** —— 岁数超阈值（沿用 `staleBadge`）。
    ///
    /// 三条都不占 → nil，照常只显数字。
    static func warningBadge(_ snapshot: AgentQuotaSnapshot?, failure: String? = nil,
                             now: Date = Date()) -> String? {
        if failure != nil {
            return snapshot == nil ? "读不到" : "读不到，下面是旧值"
        }
        guard let snapshot else { return nil }
        if snapshot.isPastReset(now: now) { return "窗口已重置，等下一次刷新" }
        return staleBadge(snapshot, now: now)
    }

    // MARK: - 悬停提示

    /// 整块的 `.help()` 悬停提示：完整窗名 + 已用百分比 + 重置时刻，陈旧的那家
    /// 再补一句说清有多旧、为什么停在那里。
    /// 重置时刻**只**活在这里（Todo #8 ⑥：不常驻，环里那一格已经被窗名和百分比
    /// 轮流占着）。两家都没数据 → nil。
    static func helpText(claude: AgentQuotaSnapshot?, codex: AgentQuotaSnapshot?,
                         claudeError: String? = nil, codexError: String? = nil,
                         now: Date = Date()) -> String? {
        let lines = [(claude, claudeError, "Claude Code"), (codex, codexError, "Codex")]
            .compactMap { snap, failure, name -> String? in
                guard let snap else {
                    // 一次都没取到过：说清读不到，别整块消失得像没这回事。
                    return failure.map { "\(name)：\($0)" }
                }
                let windows = snap.windows.map { w in
                    "\(w.label) 已用 \(w.usedPercent)%" + (w.resetsAt.map { "（\($0) 重置）" } ?? "")
                }.joined(separator: "；")
                var notes: [String] = []
                if let failure { notes.append("\(failure)；下面是上一轮的旧值") }
                if snap.isPastReset(now: now) {
                    notes.append("这些窗口的重置时刻都已过去，百分比不是现状")
                }
                if let stale = snap.stalenessNote(now: now) { notes.append(stale) }
                let suffix = notes.map { "\n  ⚠︎ \($0)" }.joined()
                return "\(name)：\(windows)\(suffix)"
            }
        return lines.isEmpty ? nil : lines.joined(separator: "\n")
    }
}
