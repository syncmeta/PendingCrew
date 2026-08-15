import Foundation

/// 额度警戒广播的**编排与提醒策略**（人类 Todo #26 / #39）——纯函数，时间/IO 全由调用方喂，
/// `CrewSessionRunner` 只负责取快照 + 把结果写白板。
///
/// **要解决的病**：原来一刀切 —— claude 或 codex 任一阻断窗过 85%，就替 session
/// 下结论「尽快收尾」。这既没看订阅档位，也没看离重置还有多久；Max 5x 周窗 85%、
/// 次日即重置的实况因此触发了一批无谓停工。
///
/// 改成按 runner 分流：
/// - Todo #26 管**发给谁**：单家越线只 @ 那一家；两家都越线另发不定向事实广播。
/// - Todo #39 管**何时发、说什么**：门槛按档位变化，窗口临近重置时抑制；文案只给
///   档位、窗口、百分比、提醒线和重置距离，不替 session 决定收尾/分流。
///
/// 判定沿用 `AgentQuotaWindow.isBlocking`：单模型周窗耗尽可切模型继续，不算停摆。
enum QuotaWarningPlan {

    /// PendingCrew 的**内部提醒策略**，不是 Claude/Codex 公布的绝对额度表。
    ///
    /// Claude 的推导有一条可审计基准：旧 Pro 提醒线 85% 等于留 15% Pro 余量；
    /// 人类实况拍板 Max 5x 周窗约 97% 才值得提醒，恰好仍留约 15% Pro 等价余量
    /// （3% × 5）。Max 20x 同理是 99.25%，取较保守的整数 99%。Team/Enterprise
    /// 没有可靠倍数，只按档位层级采用保守内部值，不声称绝对量。
    ///
    /// Codex 的 app-server 只给 planType + 百分比，不给各档倍数/绝对上限；这里按
    /// Free → Plus → Pro → Business → Enterprise 做递增的**产品提醒梯度**。它只决定
    /// 什么时候把事实送给 session，绝不能拿来反推 requests/tokens。
    static func thresholdPercent(agent: String, subscriptionPlan: String?) -> Int {
        let plan = subscriptionPlan?
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased() ?? ""
        if agent == "claude" {
            if plan.contains("max 20x") { return 99 }
            if plan.contains("max 5x") { return 97 }
            switch plan {
            case "enterprise": return 98
            case "team": return 95
            case "pro": return 85
            default: return 85   // 档位/Max 倍数未知：保守回退，不假装知道容量
            }
        }
        switch plan {
        case "enterprise": return 98
        case "business": return 97
        case "pro": return 95
        case "plus": return 90
        case "free": return 85
        default: return 85       // 未知档位仍保留旧保守线
        }
    }

    /// 距重置进入窗口最后约 1/7 时不再发提醒：周窗最后 24h、5h/session 窗最后
    /// 45min。此时余额即将作废，把「越线」事实推成收尾信号反而会浪费订阅。
    /// 未识别窗口无法知道总长，只在最后 1h 抑制，既不猜窗长也不长期漏报。
    static func nearResetSuppressionInterval(for window: AgentQuotaWindow) -> TimeInterval {
        let label = window.label.lowercased()
        if label.contains("周") || label.contains("week") { return 24 * 3600 }
        if label == "session" || label.contains("5小时") { return 45 * 60 }
        if let minutes = label.split(whereSeparator: { !$0.isNumber }).first.flatMap({ Int($0) }),
           label.contains("分钟") {
            return max(15 * 60, TimeInterval(minutes * 60) / 7)
        }
        return 3600
    }

    /// 一个在跑 session 的最小快照。`agent` 与 `AgentQuotaSnapshot.agent` 同口径
    /// （"claude" / "codex"）——调用方从 `LocalCodingAgentKind` 映射。
    struct RunningSession: Equatable {
        let sessionId: String
        let crewId: String
        let agent: String

        init(sessionId: String, crewId: String, agent: String) {
            self.sessionId = sessionId
            self.crewId = crewId
            self.agent = agent
        }
    }

    /// 一条要发的白板消息。
    struct Message: Equatable {
        let crewId: String
        let text: String
        /// 空 = 广播（不定向）。
        let mentions: [LocalWhiteboardMention]
        /// 去重键：调用方记进已播集合，同一重置周期只喊一次。
        let dedupeKey: String
    }

    private struct Crossing {
        let window: AgentQuotaWindow
        let thresholdPercent: Int
        let reset: Date?
    }

    /// 算出这一轮要发哪些消息。
    ///
    /// - Parameters:
    ///   - claude/codex: 两家的当前快照（`QuotaCenter.shared.claude` / `.codex`）。
    ///   - sessions: 当前 running 的 session 列表。
    ///   - alreadyWarned: 已播过的去重键集合。
    ///   - now: 判定「这个窗是不是已经翻篇」的当下时刻（见 `crossings`）。
    static func compute(
        claude: AgentQuotaSnapshot?,
        codex: AgentQuotaSnapshot?,
        sessions: [RunningSession],
        alreadyWarned: Set<String>,
        now: Date = Date()
    ) -> [Message] {
        let claudeCrossings = crossings(claude, now: now)
        let codexCrossings = crossings(codex, now: now)
        let bothOver = !claudeCrossings.isEmpty && !codexCrossings.isEmpty

        var out: [Message] = []
        out += singleAgentMessages(
            snapshot: claude, crossings: claudeCrossings, sessions: sessions,
            alreadyWarned: alreadyWarned, otherHasHeadroom: !bothOver, now: now)
        out += singleAgentMessages(
            snapshot: codex, crossings: codexCrossings, sessions: sessions,
            alreadyWarned: alreadyWarned, otherHasHeadroom: !bothOver, now: now)

        if bothOver,
           let cKey = claudeCrossings.first.map({ key(agent: "claude", window: $0.window) }),
           let xKey = codexCrossings.first.map({ key(agent: "codex", window: $0.window) }) {
            let bothKey = "both|\(cKey)|\(xKey)"
            if !alreadyWarned.contains(bothKey) {
                let crews = orderedCrewIds(sessions)
                let cFact = claude.flatMap { snapshot in
                    claudeCrossings.first.map { factText(snapshot: snapshot, crossing: $0, now: now) }
                } ?? "Claude Code 快照缺失"
                let xFact = codex.flatMap { snapshot in
                    codexCrossings.first.map { factText(snapshot: snapshot, crossing: $0, now: now) }
                } ?? "Codex 快照缺失"
                let text = "额度事实：Claude Code 与 Codex 各有阻断窗越过本机按档位设置的提醒线。"
                    + "\(cFact)；\(xFact)。提醒线只是 PendingCrew 的通知策略，"
                    + "两家上游都没给可换算的绝对剩余；后续动作由各 session 结合任务自行判断。"
                for crewId in crews {
                    out.append(Message(
                        crewId: crewId, text: text, mentions: [], dedupeKey: bothKey))
                }
            }
        }
        return out
    }

    // MARK: - 内部

    /// 该快照里过警戒线的**阻断**窗（保持 windows 原序）。
    ///
    /// 已经过了自己重置时刻的窗**不算数**（Todo #33）；已进入最后 1/7 的窗也不发
    /// 提醒（Todo #39），因为余额即将作废。说不出重置时刻的窗照常算：不猜时间，
    /// 但文案会明确「上游未给重置时刻」。
    private static func crossings(_ snap: AgentQuotaSnapshot?,
                                  now: Date = Date()) -> [Crossing] {
        guard let snap else { return [] }
        let threshold = thresholdPercent(
            agent: snap.agent, subscriptionPlan: snap.effectiveSubscriptionPlan)
        return snap.windows.compactMap { w in
            guard w.usedPercent >= threshold, w.isBlocking else { return nil }
            guard let raw = w.resetsAt,
                  let reset = QuotaWakeupPlan.parseReset(raw, now: now) else {
                return Crossing(window: w, thresholdPercent: threshold, reset: nil)
            }
            let remaining = reset.timeIntervalSince(now)
            guard remaining > nearResetSuppressionInterval(for: w) else { return nil }
            return Crossing(window: w, thresholdPercent: threshold, reset: reset)
        }
    }

    private static func key(agent: String, window: AgentQuotaWindow) -> String {
        "\(agent)|\(window.label)|\(window.resetsAt ?? "?")"
    }

    /// 人话 runner 名（与 `LocalCodingAgentKind.displayName` 同口径）。
    static func displayName(agent: String) -> String {
        agent == "claude" ? "Claude Code" : "Codex"
    }

    private static func otherAgent(_ agent: String) -> String {
        agent == "claude" ? "codex" : "claude"
    }

    private static func singleAgentMessages(
        snapshot: AgentQuotaSnapshot?,
        crossings: [Crossing],
        sessions: [RunningSession],
        alreadyWarned: Set<String>,
        otherHasHeadroom: Bool,
        now: Date = Date()
    ) -> [Message] {
        guard let snapshot, !crossings.isEmpty else { return [] }
        let agent = snapshot.agent
        let mine = sessions.filter { $0.agent == agent }
        guard !mine.isEmpty else { return [] }   // 这家一个 session 都没在跑 → 没人要通知

        var out: [Message] = []
        for crossing in crossings {
            let w = crossing.window
            let k = key(agent: agent, window: w)
            guard !alreadyWarned.contains(k) else { continue }
            var text = "额度事实：" + factText(snapshot: snapshot, crossing: crossing, now: now) + "。"
            if otherHasHeadroom {
                text += "另一家（\(displayName(agent: otherAgent(agent)))）当前没有阻断窗越过其档位提醒线。"
            }
            text += "提醒线只是 PendingCrew 的通知策略；后续动作由 session 结合任务自行判断。"
            for crewId in orderedCrewIds(mine) {
                let targets = mine.filter { $0.crewId == crewId }
                out.append(Message(
                    crewId: crewId,
                    text: text,
                    mentions: targets.map {
                        LocalWhiteboardMention(kind: "session", targetId: $0.sessionId)
                    },
                    dedupeKey: k))
            }
        }
        return out
    }

    private static func factText(snapshot: AgentQuotaSnapshot, crossing: Crossing,
                                 now: Date) -> String {
        let agent = snapshot.agent
        let name = displayName(agent: agent)
        let w = crossing.window
        let resetText: String
        if let raw = w.resetsAt, let reset = crossing.reset {
            resetText = "，\(raw) 重置（距重置约 \(durationText(reset.timeIntervalSince(now)))）"
        } else if let raw = w.resetsAt {
            resetText = "，重置时刻为 \(raw)（无法解析距离）"
        } else {
            resetText = "，上游未给重置时刻"
        }
        return "\(name)（\(snapshot.subscriptionPlanDescription)）\(w.label)已用 \(w.usedPercent)%"
            + "，本机该档提醒线 \(crossing.thresholdPercent)%\(resetText)。"
            + tierMeaning(agent: agent, plan: snapshot.effectiveSubscriptionPlan)
    }

    private static func tierMeaning(agent: String, plan: String?) -> String {
        let normalized = plan?.lowercased() ?? ""
        if agent == "claude" {
            if normalized.contains("max 20x") {
                return "Max 20x 名义容量约为 Pro 的 20 倍，但绝对剩余仍说不出"
            }
            if normalized.contains("max 5x") {
                return "Max 5x 名义容量约为 Pro 的 5 倍，但绝对剩余仍说不出"
            }
            if normalized == "pro" {
                return "这是 Pro 档百分比；上游没给绝对 requests/tokens，不能换算绝对剩余"
            }
        }
        if let plan, !plan.isEmpty {
            return "这是 \(plan) 档百分比；上游没给该档绝对上限或相对倍数，不能换算绝对剩余"
        }
        return "档位未知，当前采用保守提醒线；百分比不能换算绝对剩余"
    }

    private static func durationText(_ interval: TimeInterval) -> String {
        let minutes = max(1, Int(interval / 60))
        if minutes < 60 { return "\(minutes) 分钟" }
        let hours = minutes / 60
        if hours < 48 { return "\(hours) 小时" }
        let days = hours / 24
        let remainder = hours % 24
        return remainder == 0 ? "\(days) 天" : "\(days) 天 \(remainder) 小时"
    }

    /// 出现顺序去重的 crewId 列表（输出稳定，单测钉得住）。
    private static func orderedCrewIds(_ sessions: [RunningSession]) -> [String] {
        var seen = Set<String>()
        var out: [String] = []
        for s in sessions where !seen.contains(s.crewId) {
            seen.insert(s.crewId)
            out.append(s.crewId)
        }
        return out
    }
}
