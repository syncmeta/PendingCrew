// 纯 Foundation、不带平台门 —— McpServer(跨平台编译)的 get_quota 要用
// AgentQuotaFile;真正 macOS-only 的是 QuotaCenter(轮询/子进程),不在本文件。
import Foundation

/// 一个限额窗口的快照：`label`（人话窗名）+ 已用百分比 + 重置时刻（原样字符串，
/// claude 是本地化人话如 "Jul 5 at 4:39am (Asia/Shanghai)"，codex 是 ISO8601）。
struct AgentQuotaWindow: Codable, Equatable {
    let label: String
    let usedPercent: Int
    let resetsAt: String?

    /// 紧凑重置时刻：同一天只显 24h 时:分（"16:39"），跨天带月/日（"7/5 16:39"）
    /// —— 5h 窗几乎总当天只需时分，周窗常跨天必须带日期消歧。不本地化成 am/pm
    /// （省宽）。`resetsAt` 为 nil 或两种格式都解不出 → nil，调用方照旧留白。
    /// claude 走人话文本、codex 走 ISO8601，此处统一。
    ///
    /// Todo #8 把额度改成圆环后这个格式化一度没人用（重置时刻只留在整块 tooltip
    /// 里）；Todo #14 又要在悬停某个环时把**那一项**的重置时刻显示在环右侧那行，
    /// 于是恢复。
    func compactResetLabel(now: Date = Date()) -> String? {
        guard let raw = resetsAt, let date = Self.parseResetDate(raw, now: now) else { return nil }
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")   // 强制 24h，不随系统落 am/pm
        f.dateFormat = Calendar.current.isDate(date, inSameDayAs: now) ? "HH:mm" : "M/d HH:mm"
        return f.string(from: date)
    }

    /// 把两家不同格式的 `resetsAt` 归一成绝对时刻：codex 是 ISO8601、claude 是
    /// "Jul 5 at 4:39am (Asia/Shanghai)" 人话文本（复用 `ClaudeResetTimeParser`）。
    private static func parseResetDate(_ raw: String, now: Date) -> Date? {
        if let iso = ISO8601DateFormatter().date(from: raw) { return iso }
        return ClaudeResetTimeParser.parse(raw, now: now)
    }

    /// 该窗耗尽是否会**挡住所有工作**（额度警戒广播只看阻断窗）。claude 的
    /// 单模型周窗（"周(Fable)" 等）耗尽后 session 切其它模型就能继续,不构成
    /// 停摆风险 → 非阻断,不警戒;session / 周(全模型) / codex 的 5小时窗、周窗
    /// 以及没认出的新窗名（宁可误报也别漏报真阻断）都算阻断。
    var isBlocking: Bool {
        let isPerModelWeekly =
            label.hasPrefix("周(") && label.hasSuffix(")") && label != "周(全模型)"
        return !isPerModelWeekly
    }
}

/// `/usage` 末尾的使用画像计数。它不是额度上限，不能拿来反推绝对配额；只是把
/// Claude 已经给出的「最近一段时间多少 requests / sessions」原样带给机长参考。
struct AgentQuotaActivity: Codable, Equatable {
    let periodLabel: String
    let requests: Int?
    let sessions: Int?
}

/// 人可以在设置里覆盖自动探到的档位。空字符串 = 自动检测；非空值只作为档位标签，
/// 不附会任何 token / request 绝对量。
enum AgentSubscriptionPlanPreference {
    static let claudeKey = "pendingcrew.quota.claude.subscription-plan"
    static let codexKey = "pendingcrew.quota.codex.subscription-plan"

    static let claudeChoices = ["", "Pro", "Max 5x", "Max 20x", "Team", "Enterprise"]
    static let codexChoices = ["", "Free", "Plus", "Pro", "Business", "Enterprise"]

    static func override(agent: String, defaults: UserDefaults = .standard) -> String? {
        let key = agent == "claude" ? claudeKey : codexKey
        guard let raw = defaults.string(forKey: key)?
            .trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else { return nil }
        return raw
    }

    static func displayName(_ raw: String) -> String {
        switch raw.lowercased() {
        case "free": return "Free"
        case "plus": return "Plus"
        case "pro": return "Pro"
        case "business": return "Business"
        case "team": return "Team"
        case "enterprise": return "Enterprise"
        case "claude_pro", "default_claude_pro": return "Pro"
        case "claude_max", "default_claude_max": return "Max（倍数未知）"
        case "default_claude_max_5x": return "Max 5x"
        case "default_claude_max_20x": return "Max 20x"
        case "claude_team", "default_claude_team": return "Team"
        case "claude_enterprise", "default_claude_enterprise": return "Enterprise"
        default: return raw
        }
    }
}

/// 某个 agent（claude / codex）的额度快照：订阅档位背景 + 所有限额窗口；Claude
/// 另带 requests/sessions 画像。两家都没有剩余 token/request 绝对量。
struct AgentQuotaSnapshot: Codable, Equatable {
    let agent: String              // "claude" | "codex"
    let windows: [AgentQuotaWindow]
    let fetchedAt: String          // ISO8601：我们**读**到这份数据的时刻
    /// 自动探到的订阅档位。claude 来自 ~/.claude.json 的 oauthAccount 档位字段；
    /// codex 来自 app-server / rollout 的 planType。nil = 上游没给，绝不猜。
    let subscriptionPlan: String?
    /// `claude_config` / `codex_account` / `codex_rate_limits` / `codex_rollout`。
    let subscriptionPlanSource: String?
    /// 设置里的人工覆盖；存在时展示和注入以它为准，但自动探测值仍保留以便切回自动。
    let subscriptionPlanOverride: String?
    /// Claude `/usage` 给出的各周期 requests / sessions 画像（当前实测有 Last 24h、
    /// Last 7d）；codex 当前 RPC 不给则 nil。用 optional 保持旧 quota.json 可解码。
    let activities: [AgentQuotaActivity]?

    /// 这份数据**产生**的时刻（ISO8601）。跟 `fetchedAt` 不是一回事，混了就会
    /// 「把一个月前的数字当现状显示」：
    /// - **codex** 的额度是从 `~/.codex/sessions/**/rollout-*.jsonl` 读的，那文件
    ///   只在真跑 codex 时才写。一个多月没跑过 codex，我们每 10 分钟勤快地读一次，
    ///   读到的仍是一个多月前那次的数 —— `fetchedAt` 却显示「刚刚」。
    /// - **claude** 是 `claude -p /usage` 现查现得，等于 `fetchedAt`。
    /// nil = 旧版 quota.json 没这个字段（说不出新鲜度就别猜，照实留白）。
    let producedAt: String?

    init(agent: String, windows: [AgentQuotaWindow], fetchedAt: String,
         producedAt: String? = nil, subscriptionPlan: String? = nil,
         subscriptionPlanSource: String? = nil, subscriptionPlanOverride: String? = nil,
         activities: [AgentQuotaActivity]? = nil) {
        self.agent = agent
        self.windows = windows
        self.fetchedAt = fetchedAt
        self.producedAt = producedAt
        self.subscriptionPlan = subscriptionPlan
        self.subscriptionPlanSource = subscriptionPlanSource
        self.subscriptionPlanOverride = subscriptionPlanOverride
        self.activities = activities
    }

    var effectiveSubscriptionPlan: String? {
        subscriptionPlanOverride ?? subscriptionPlan
    }

    var subscriptionPlanDescription: String {
        guard let plan = effectiveSubscriptionPlan else { return "档位未知（自动通道未提供，设置中也未填写）" }
        if subscriptionPlanOverride != nil { return "\(plan)（手动设置）" }
        switch subscriptionPlanSource {
        case "claude_config": return "\(plan)（自动：Claude 本地账户配置）"
        case "codex_account": return "\(plan)（自动：Codex account/read）"
        case "codex_rate_limits": return "\(plan)（自动：Codex rateLimits）"
        case "codex_rollout": return "\(plan)（自动：Codex rollout）"
        default: return "\(plan)（自动检测）"
        }
    }

    func applyingSubscriptionPlanOverride(_ value: String?) -> AgentQuotaSnapshot {
        AgentQuotaSnapshot(
            agent: agent, windows: windows, fetchedAt: fetchedAt, producedAt: producedAt,
            subscriptionPlan: subscriptionPlan, subscriptionPlanSource: subscriptionPlanSource,
            subscriptionPlanOverride: value, activities: activities)
    }

    /// 数据有多旧（秒）。`producedAt` 缺失或解不出 → nil。
    func ageSeconds(now: Date = Date()) -> TimeInterval? {
        guard let raw = producedAt, let date = ISO8601DateFormatter().date(from: raw) else {
            return nil
        }
        return max(0, now.timeIntervalSince(date))
    }

    /// 超过这个岁数就该明说「这是旧数据」。取 2 小时：额度窗最短是 5 小时滚动窗，
    /// 两小时前的百分比已经完全可能不是现状了。
    static let staleThreshold: TimeInterval = 2 * 3600

    func isStale(now: Date = Date(), threshold: TimeInterval = staleThreshold) -> Bool {
        (ageSeconds(now: now) ?? 0) >= threshold
    }

    /// 这份快照描述的窗口**是否已经翻篇**（所有说得出重置时刻的窗都过点了）。
    ///
    /// Todo #33 的真因就在这：codex 的百分比曾经只从 rollout 文件被动读，窗口在
    /// 服务端重置后我们无从知晓，界面就一直画着过期窗的 87%。「过了自己的重置时刻」
    /// 是**数据自带的**失效证据，比岁数阈值硬：一到点那个百分比必然不是现状。
    ///
    /// 说不出重置时刻的窗不参与判定；一个都判不出 → false（不猜）。只要还有一个窗
    /// 没到点，就不算整份翻篇（那份数据里至少有一格仍是现状）。
    func isPastReset(now: Date = Date()) -> Bool {
        let resets = windows.compactMap { w -> Date? in
            w.resetsAt.flatMap { QuotaWakeupPlan.parseReset($0, now: now) }
        }
        guard !resets.isEmpty else { return false }
        return resets.allSatisfy { $0 <= now }
    }

    /// 给人看的陈旧提示；不陈旧（或说不出岁数）→ nil，调用方照常只显数字。
    /// 不粉饰：说清这是几时的数、以及为什么会停在那里。
    func stalenessNote(now: Date = Date(), threshold: TimeInterval = staleThreshold) -> String? {
        guard let age = ageSeconds(now: now), age >= threshold else { return nil }
        let why = agent == "codex"
            ? "（codex 额度只在真跑 codex 时才落盘，这之后没跑过）"
            : ""
        return "这是 \(Self.humanAge(age)) 的数据，不是当前值\(why)"
    }

    /// 秒 → "38 分钟前" / "5 小时前" / "35 天前"。只给一个量级，不堆精度。
    static func humanAge(_ seconds: TimeInterval) -> String {
        switch seconds {
        case ..<3600:    return "\(Int(seconds / 60)) 分钟前"
        case ..<86_400:  return "\(Int(seconds / 3600)) 小时前"
        default:         return "\(Int(seconds / 86_400)) 天前"
        }
    }

    /// 给 UI / get_quota 工具的一行摘要，如 "session 80% · 周(全模型) 23%"。
    var summary: String {
        windows.map { w in
            w.usedPercent >= 0 ? "\(w.label) \(w.usedPercent)%" : w.label
        }.joined(separator: " · ")
    }

    /// 5 小时滚动窗（claude 的 "session" / codex 的 "5小时窗"）。
    var fiveHourWindow: AgentQuotaWindow? {
        windows.first { $0.label == "session" || $0.label == "5小时窗" }
    }

    /// 周窗（claude 取「全模型」总窗,模型分窗只进 tooltip；codex 的 "周窗"）。
    var weeklyWindow: AgentQuotaWindow? {
        windows.first { $0.label == "周(全模型)" || $0.label == "周窗" }
    }
}

/// quota.json 的文件形状 —— app 的 `QuotaCenter` 定时写、helper 的 `get_quota`
/// 工具读（离线 helper 唯一能碰的共享文件层，与白板/控制通道同 `--dir`）。
struct AgentQuotaFile: Codable, Equatable {
    var claude: AgentQuotaSnapshot? = nil
    var codex: AgentQuotaSnapshot? = nil
    /// 上一轮刷新**没取到**那一家数据时的人话原因（取到 → nil）。
    ///
    /// 失败不许静默（Todo #33）：旧实现是 `if let snap = await x { codex = snap }`，
    /// 取不到就一声不吭继续画上一轮的数字，于是「读不到」和「就是这个数」在界面上
    /// 长得一模一样。把原因带出来，UI / get_quota 才能如实说「这是旧值」。
    /// nil 也可能是旧版 quota.json 没这两个字段 —— 那就照旧不显示，不编造。
    var claudeError: String? = nil
    var codexError: String? = nil
}

/// `claude -p "/usage"` 输出的文本解析（实测 2.1.201：纯本地 control-request，
/// 不烧额度）。行形如：
///   Current session: 80% used · resets Jul 5 at 4:39am (Asia/Shanghai)
///   Current week (all models): 23% used · resets Jul 5 at 10:59am (Asia/Shanghai)
///   Current week (Fable): 18% used · resets Jul 5 at 10:59am (Asia/Shanghai)
/// 含 "% used" 的行解析成窗口；Last 24h / Last 7d 的 requests/sessions 行解析成
/// activities，其余用量画像文字保留在 CLI、不进额度快照。窗名翻译成短中文。
enum ClaudeUsageTextParser {
    static func parse(
        _ text: String, now: Date = Date(), subscriptionPlan: String? = nil
    ) -> AgentQuotaSnapshot? {
        var windows: [AgentQuotaWindow] = []
        var activities: [AgentQuotaActivity] = []
        for rawLine in text.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if let parsed = parseActivity(line) { activities.append(parsed) }
            guard let usedRange = line.range(of: "% used"),
                  let colon = line.range(of: ": ") else { continue }
            let label = String(line[line.startIndex..<colon.lowerBound])
            let percentText = line[colon.upperBound..<usedRange.lowerBound]
            guard let percent = Int(percentText.trimmingCharacters(in: .whitespaces)) else { continue }
            var resets: String? = nil
            if let r = line.range(of: "resets ") {
                resets = String(line[r.upperBound...]).trimmingCharacters(in: .whitespaces)
            }
            windows.append(AgentQuotaWindow(
                label: shortLabel(label), usedPercent: percent, resetsAt: resets))
        }
        guard !windows.isEmpty else { return nil }
        let stamp = ISO8601DateFormatter().string(from: now)
        // claude 是现查现得 —— 读到的时刻就是数据产生的时刻。
        return AgentQuotaSnapshot(
            agent: "claude", windows: windows, fetchedAt: stamp, producedAt: stamp,
            subscriptionPlan: subscriptionPlan,
            subscriptionPlanSource: subscriptionPlan == nil ? nil : "claude_config",
            activities: activities.isEmpty ? nil : activities)
    }

    private static func parseActivity(_ line: String) -> AgentQuotaActivity? {
        let parts = line.split(separator: "·").map {
            String($0).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard parts.count >= 2 else { return nil }
        var requests: Int?
        var sessions: Int?
        for part in parts.dropFirst() {
            let value = part.split(whereSeparator: { !$0.isNumber }).first.flatMap { Int($0) }
            let lower = part.lowercased()
            if lower.contains("request") { requests = value }
            if lower.contains("session") { sessions = value }
        }
        guard requests != nil || sessions != nil else { return nil }
        return AgentQuotaActivity(periodLabel: parts[0], requests: requests, sessions: sessions)
    }

    /// "Current session" → "session"（5 小时滚动窗）；"Current week (all models)" →
    /// "周(全模型)"；"Current week (Fable)" → "周(Fable)"。没认出的原样保留 ——
    /// claude 改文案时宁可显示英文原文也别丢数据。
    static func shortLabel(_ raw: String) -> String {
        if raw == "Current session" { return "session" }
        if raw == "Current week (all models)" { return "周(全模型)" }
        if raw.hasPrefix("Current week ("), raw.hasSuffix(")") {
            let inner = raw.dropFirst("Current week (".count).dropLast()
            return "周(\(inner))"
        }
        return raw
    }
}

/// `~/.claude.json` 的账户档位字段解析。2026-08-09 本机实测：`auth status`
/// 只报 `subscriptionType=max`，这个文件的 `organizationRateLimitTier` 才进一步
/// 区分 `default_claude_max_5x`。只读白名单字段，不碰 token / email。
enum ClaudeAccountPlanParser {
    static func parse(_ data: Data) -> String? {
        guard let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let account = root["oauthAccount"] as? [String: Any] else { return nil }
        for key in ["organizationRateLimitTier", "userRateLimitTier", "seatTier",
                    "subscriptionType", "organizationType"] {
            if let raw = account[key] as? String,
               !raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                return AgentSubscriptionPlanPreference.displayName(raw)
            }
        }
        return nil
    }
}

/// claude `/usage` 输出里重置时刻的**人话文本**解析（"Jul 5 at 4:39am
/// (Asia/Shanghai)" → Date）。撞额度自动挂唤醒钩子要真实的重置时刻,不能拿
/// 展示串糊弄。年份按「未来最近」推断（12 月看 1 月的重置会跨年）。
/// 解析不出 → nil,调用方走保守回退（定期重试）,不猜。
enum ClaudeResetTimeParser {
    static func parse(_ raw: String, now: Date = Date()) -> Date? {
        var text = raw.trimmingCharacters(in: .whitespaces)
        // 尾部时区 "(Asia/Shanghai)" —— 抠出来喂 formatter,剩下 "Jul 5 at 4:39am"。
        var tz = TimeZone.current
        if let open = text.lastIndex(of: "("), text.hasSuffix(")") {
            let name = String(text[text.index(after: open)..<text.index(before: text.endIndex)])
            if let parsed = TimeZone(identifier: name) { tz = parsed }
            text = String(text[..<open]).trimmingCharacters(in: .whitespaces)
        }
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = tz
        // 无年份格式 —— DateFormatter 默认落在 2000 年,parse 后把年份替换成
        // 「>= now 的最近一年」。分/无分两个变体（"4pm" 与 "4:39am" 都出现过）。
        for fmt in ["MMM d 'at' h:mma", "MMM d 'at' ha"] {
            f.dateFormat = fmt
            guard let base = f.date(from: text) else { continue }
            var cal = Calendar(identifier: .gregorian)
            cal.timeZone = tz
            var parts = cal.dateComponents([.month, .day, .hour, .minute], from: base)
            let nowYear = cal.component(.year, from: now)
            for year in [nowYear, nowYear + 1] {
                parts.year = year
                if let candidate = cal.date(from: parts), candidate >= now {
                    return candidate
                }
            }
        }
        return nil
    }
}

/// 撞限额自动唤醒的时刻计算（Todo #10 ①，纯函数可单测）。
/// 输入额度快照里的重置时刻原样字符串（claude 人话文本 / codex ISO8601），
/// 输出唤醒时刻：解析成功且在未来 → **重置 + 1 分钟**；解析不出 / 缺失 / 已过期
/// （陈旧快照）→ **45 分钟退避重试**并标记 `isFallback`，调用方据此 fail-loud
/// （白板报「没拿到重置时刻」），绝不静默丢。唤醒 note 会让 session 先 get_quota
/// 核实,仍受限会自己顺延 —— 退避醒早了不会瞎跑。
enum QuotaWakeupPlan {
    struct Plan: Equatable {
        let fireAt: Date
        /// true = 没拿到可信重置时刻,走的退避重试。
        let isFallback: Bool
    }

    /// 重置后缓冲：+1 分钟（额度窗刚翻新,别掐点撞上仍受限的边缘）。
    static let resetBuffer: TimeInterval = 60
    /// 退避重试间隔：45 分钟（5 小时窗内多试几次,总能撞上重置后）。
    static let fallbackRetry: TimeInterval = 45 * 60

    static func compute(resetsAt: String?, now: Date = Date()) -> Plan {
        if let raw = resetsAt, let reset = parseReset(raw, now: now), reset > now {
            return Plan(fireAt: reset.addingTimeInterval(resetBuffer), isFallback: false)
        }
        return Plan(fireAt: now.addingTimeInterval(fallbackRetry), isFallback: true)
    }

    /// 两家格式归一（同 `AgentQuotaWindow.parseResetDate` 的口径）：先试 ISO8601
    /// （codex），再试 claude 人话文本。
    static func parseReset(_ raw: String, now: Date) -> Date? {
        if let iso = ISO8601DateFormatter().date(from: raw) { return iso }
        return ClaudeResetTimeParser.parse(raw, now: now)
    }
}

/// codex rollout jsonl 的 `token_count.rate_limits` 解析（实测 0.137.0：每轮落盘，
/// 无进程也能读）。形如：
///   {"payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":5.0,
///    "window_minutes":300,"resets_at":1782030455},"secondary":{...},"plan_type":"prolite"}}}
/// 输入整段 jsonl 文本，取**最后一条**带 rate_limits 的记录（最新额度态）。
enum CodexRolloutQuotaParser {
    /// - Parameter producedAt: 这份 rollout **落盘**的时刻（文件 mtime）。codex 只在
    ///   真跑一轮时才写这个文件，所以它就是数据的真实年龄 —— 一个多月没跑 codex，
    ///   这里读到的数就是一个多月前的。不传 = 说不出年龄（照实留白，不假装是现值）。
    static func parse(
        _ jsonl: String, now: Date = Date(), producedAt: Date? = nil
    ) -> AgentQuotaSnapshot? {
        // 倒序找最后一行可解析出 rate_limits 的记录。
        for rawLine in jsonl.split(separator: "\n").reversed() {
            guard rawLine.contains("rate_limits"),
                  let data = rawLine.data(using: .utf8),
                  let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  let limits = rateLimitsDict(in: obj) else { continue }
            var windows: [AgentQuotaWindow] = []
            if let w = window(from: limits["primary"]) { windows.append(w) }
            if let w = window(from: limits["secondary"]) { windows.append(w) }
            guard !windows.isEmpty else { continue }
            let plan = (limits["plan_type"] as? String) ?? (limits["planType"] as? String)
            return AgentQuotaSnapshot(
                agent: "codex", windows: windows,
                fetchedAt: ISO8601DateFormatter().string(from: now),
                producedAt: producedAt.map { ISO8601DateFormatter().string(from: $0) },
                subscriptionPlan: plan.map(AgentSubscriptionPlanPreference.displayName),
                subscriptionPlanSource: plan == nil ? nil : "codex_rollout")
        }
        return nil
    }

    /// 从文件**尾部字节**直接解析（调用方只读尾巴，rollout 动辄几百 KB）。
    ///
    /// 按字节切必然会切在多字节字符中间（rollout 里全是中文），那样整段 UTF-8 解码
    /// 直接返回 nil、这一轮无声无息什么都读不到 —— 往后挪最多 3 个字节（UTF-8 序列
    /// 最长 4 字节）就能对齐到下一个字符边界。第一行本来就可能是半行，按行解析时
    /// 丢掉无所谓。
    static func parseTail(
        _ data: Data, now: Date = Date(), producedAt: Date? = nil
    ) -> AgentQuotaSnapshot? {
        for skip in 0...min(3, data.count) {
            guard let text = String(data: data.dropFirst(skip), encoding: .utf8) else { continue }
            return parse(text, now: now, producedAt: producedAt)
        }
        return nil
    }

    /// rate_limits 可能挂在 payload 下（rollout）或顶层（保守兼容）。
    private static func rateLimitsDict(in obj: [String: Any]) -> [String: Any]? {
        if let payload = obj["payload"] as? [String: Any],
           let l = payload["rate_limits"] as? [String: Any] { return l }
        return obj["rate_limits"] as? [String: Any]
    }

    private static func window(from any: Any?) -> AgentQuotaWindow? {
        CodexRateLimitsDecoder.window(from: any)
    }
}

/// codex 一份 `rate_limits` 字典 → 窗口列表。**两种键名都吃**：
/// - rollout jsonl：snake_case（`used_percent` / `window_minutes` / `resets_at`）
/// - app-server `account/rateLimits/read`：camelCase（`usedPercent` /
///   `windowDurationMins` / `resetsAt`）
/// 同一份语义两套拼写是 codex 自己的通道差异（实测 codex-cli 0.145.0），
/// 收在这一个地方，免得两条读法各写一遍再慢慢漂移。
enum CodexRateLimitsDecoder {
    /// primary（+ secondary，可能为 null）依次成窗；一个都解不出 → 空数组。
    static func windows(from limits: [String: Any]) -> [AgentQuotaWindow] {
        [limits["primary"], limits["secondary"]].compactMap { window(from: $0) }
    }

    /// 数字一律经 NSNumber 取值：JSON 里 `87.0` 和 `0` 解出来是不同类型，
    /// 直接 `as? Double` 会在整数那一侧悄悄失败（app-server 的 `usedPercent: 0`
    /// 就是整数）—— 那正是「解析失败还静默」的经典入口。
    private static func number(_ any: Any?) -> Double? {
        (any as? NSNumber)?.doubleValue
    }

    static func window(from any: Any?) -> AgentQuotaWindow? {
        guard let d = any as? [String: Any],
              let used = number(d["used_percent"] ?? d["usedPercent"]) else { return nil }
        let minutes = number(d["window_minutes"] ?? d["windowDurationMins"]).map { Int($0) } ?? 0
        let label: String
        switch minutes {
        case 300: label = "5小时窗"
        case 10080: label = "周窗"
        default: label = minutes > 0 ? "\(minutes)分钟窗" : "窗口"
        }
        var resets: String? = nil
        if let epoch = number(d["resets_at"] ?? d["resetsAt"]) {
            resets = ISO8601DateFormatter().string(from: Date(timeIntervalSince1970: epoch))
        }
        return AgentQuotaWindow(label: label, usedPercent: Int(used.rounded()), resetsAt: resets)
    }
}

/// codex app-server `account/rateLimits/read` 的 result 解析（实测 codex-cli
/// 0.145.0）。形如：
///   {"rateLimits":{"limitId":"codex","primary":{"usedPercent":0,
///    "windowDurationMins":10080,"resetsAt":1786795301},"secondary":null,…},
///    "rateLimitsByLimitId":{…},"rateLimitResetCredits":{…}}
///
/// 这是 codex **现查现得**的额度真值 —— 与 rollout 文件不同，它不依赖 codex 恰好
/// 跑过一轮，窗口在服务端重置后下一次问就是新数（Todo #33 的修法核心）。
/// 因此 `producedAt` = 问到的此刻，快照既不陈旧也不会「过了自己的重置时刻」。
enum CodexAppServerQuotaParser {
    static func parse(_ result: Any?, now: Date = Date()) -> AgentQuotaSnapshot? {
        guard let obj = result as? [String: Any],
              let limits = (obj["rateLimits"] ?? obj["rate_limits"]) as? [String: Any]
        else { return nil }
        let windows = CodexRateLimitsDecoder.windows(from: limits)
        guard !windows.isEmpty else { return nil }
        let plan = (limits["planType"] as? String) ?? (limits["plan_type"] as? String)
        let stamp = ISO8601DateFormatter().string(from: now)
        return AgentQuotaSnapshot(
            agent: "codex", windows: windows, fetchedAt: stamp, producedAt: stamp,
            subscriptionPlan: plan.map(AgentSubscriptionPlanPreference.displayName),
            subscriptionPlanSource: plan == nil ? nil : "codex_rate_limits")
    }
}
