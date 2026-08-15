import XCTest
// QuotaWarningPlan.swift 直接编进 PendingCrewTests target。

/// 额度警戒按 runner 分流的编排单测（人类 Todo #26）。
final class QuotaWarningPlanTests: XCTestCase {

    private func snap(_ agent: String, _ windows: [AgentQuotaWindow],
                      plan: String? = nil) -> AgentQuotaSnapshot {
        AgentQuotaSnapshot(
            agent: agent, windows: windows, fetchedAt: "2026-08-08T00:00:00Z",
            subscriptionPlan: plan,
            subscriptionPlanSource: plan == nil ? nil : "test")
    }

    private let claudeHot = AgentQuotaWindow(
        label: "session", usedPercent: 90, resetsAt: "2099-08-08T08:00:58Z")
    private let claudeCool = AgentQuotaWindow(label: "session", usedPercent: 12, resetsAt: nil)
    /// 重置时刻取未来 —— 「窗还没翻篇」是告警成立的前提（Todo #33 起 `crossings`
    /// 会把已经过点的窗剔掉，见 `testExpiredWindowDoesNotWarn`）。
    private let codexHot = AgentQuotaWindow(
        label: "周窗", usedPercent: 87, resetsAt: "2099-08-08T08:00:58Z")
    private let codexCool = AgentQuotaWindow(label: "周窗", usedPercent: 3, resetsAt: nil)

    /// crew A：claude + codex 混编；crew B：只有 codex。
    private let sessions = [
        QuotaWarningPlan.RunningSession(sessionId: "s-c1", crewId: "A", agent: "claude"),
        QuotaWarningPlan.RunningSession(sessionId: "s-x1", crewId: "A", agent: "codex"),
        QuotaWarningPlan.RunningSession(sessionId: "s-x2", crewId: "B", agent: "codex"),
    ]

    // MARK: - 过期窗不告警（Todo #33）

    func testExpiredWindowDoesNotWarn() {
        // 盘上读来的 codex 快照可能停在一个早就翻篇的窗上（87%、16:00 重置，
        // 而现在已是 19:54）—— 拿这个数去喊「收活」等于照着过期数字停摆。
        let expired = AgentQuotaWindow(label: "周窗", usedPercent: 87,
                                       resetsAt: "2026-08-08T08:00:58Z")
        let now = ISO8601DateFormatter().date(from: "2026-08-08T11:54:00Z")!
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [claudeCool]), codex: snap("codex", [expired]),
            sessions: sessions, alreadyWarned: [], now: now)
        XCTAssertTrue(out.isEmpty, "重置时刻已过的窗不该再触发告警")
        // 同一个窗在它还没到点时照常告警。
        let before = ISO8601DateFormatter().date(from: "2026-08-06T04:20:00Z")!
        XCTAssertFalse(QuotaWarningPlan.compute(
            claude: snap("claude", [claudeCool]), codex: snap("codex", [expired]),
            sessions: sessions, alreadyWarned: [], now: before).isEmpty)
    }

    func testWindowWithoutResetTimeStillWarns() {
        // 说不出重置时刻 → 不猜，照常告警（宁可误报也别漏报真阻断）。
        let noReset = AgentQuotaWindow(label: "周窗", usedPercent: 91, resetsAt: nil)
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [claudeCool]), codex: snap("codex", [noReset]),
            sessions: sessions, alreadyWarned: [],
            now: ISO8601DateFormatter().date(from: "2026-08-08T11:54:00Z")!)
        XCTAssertFalse(out.isEmpty)
    }

    // MARK: - 档位门槛与临近重置（Todo #39）

    func testThresholdPolicyIsTierSpecific() {
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(
            agent: "claude", subscriptionPlan: "Pro"), 85)
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(
            agent: "claude", subscriptionPlan: "Max 5x"), 97)
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(
            agent: "claude", subscriptionPlan: "Max 20x"), 99)

        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(agent: "codex", subscriptionPlan: "Free"), 85)
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(agent: "codex", subscriptionPlan: "Plus"), 90)
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(agent: "codex", subscriptionPlan: "Pro"), 95)
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(agent: "codex", subscriptionPlan: "Business"), 97)
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(agent: "codex", subscriptionPlan: "Enterprise"), 98)
        XCTAssertEqual(QuotaWarningPlan.thresholdPercent(agent: "codex", subscriptionPlan: nil), 85)
    }

    func testCrossingUsesEffectiveSubscriptionTier() {
        let now = ISO8601DateFormatter().date(from: "2026-08-08T00:00:00Z")!
        let farReset = "2026-08-10T00:01:00Z"
        let onlyClaude = sessions.filter { $0.agent == "claude" }
        func messages(plan: String, percent: Int) -> [QuotaWarningPlan.Message] {
            QuotaWarningPlan.compute(
                claude: snap("claude", [AgentQuotaWindow(
                    label: "周(全模型)", usedPercent: percent, resetsAt: farReset)], plan: plan),
                codex: nil, sessions: onlyClaude, alreadyWarned: [], now: now)
        }

        XCTAssertTrue(messages(plan: "Pro", percent: 84).isEmpty)
        XCTAssertFalse(messages(plan: "Pro", percent: 85).isEmpty)
        XCTAssertTrue(messages(plan: "Max 5x", percent: 96).isEmpty)
        XCTAssertFalse(messages(plan: "Max 5x", percent: 97).isEmpty)
        XCTAssertTrue(messages(plan: "Max 20x", percent: 98).isEmpty)
        XCTAssertFalse(messages(plan: "Max 20x", percent: 99).isEmpty)
    }

    func testWeeklyWindowNearResetSuppressesWarning() {
        let now = ISO8601DateFormatter().date(from: "2026-08-08T00:00:00Z")!
        let onlyClaude = sessions.filter { $0.agent == "claude" }
        func messages(reset: String) -> [QuotaWarningPlan.Message] {
            QuotaWarningPlan.compute(
                claude: snap("claude", [AgentQuotaWindow(
                    label: "周(全模型)", usedPercent: 99, resetsAt: reset)], plan: "Max 5x"),
                codex: nil, sessions: onlyClaude, alreadyWarned: [], now: now)
        }

        XCTAssertTrue(messages(reset: "2026-08-08T23:59:00Z").isEmpty,
                      "周窗最后 24h 的余额即将作废，不发提醒")
        XCTAssertFalse(messages(reset: "2026-08-09T00:01:00Z").isEmpty,
                       "尚未进入周窗最后 24h，越线仍应把事实送到")
    }

    func testFiveHourWindowUsesShorterNearResetSuppression() {
        let now = ISO8601DateFormatter().date(from: "2026-08-08T00:00:00Z")!
        let onlyCodex = sessions.filter { $0.agent == "codex" }
        func messages(reset: String) -> [QuotaWarningPlan.Message] {
            QuotaWarningPlan.compute(
                claude: nil,
                codex: snap("codex", [AgentQuotaWindow(
                    label: "5小时窗", usedPercent: 96, resetsAt: reset)], plan: "Pro"),
                sessions: onlyCodex, alreadyWarned: [], now: now)
        }

        XCTAssertTrue(messages(reset: "2026-08-08T00:44:00Z").isEmpty)
        XCTAssertFalse(messages(reset: "2026-08-08T00:46:00Z").isEmpty)
    }

    func testWarningTextStatesFactsWithoutPrescribingAction() {
        let now = ISO8601DateFormatter().date(from: "2026-08-08T00:00:00Z")!
        let max = snap("claude", [AgentQuotaWindow(
            label: "周(全模型)", usedPercent: 97,
            resetsAt: "2026-08-10T00:00:00Z")], plan: "Max 5x")
        let out = QuotaWarningPlan.compute(
            claude: max, codex: nil, sessions: sessions,
            alreadyWarned: [], now: now)
        let text = try! XCTUnwrap(out.first?.text)

        XCTAssertTrue(text.contains("Max 5x"), text)
        XCTAssertTrue(text.contains("提醒线 97%"), text)
        XCTAssertTrue(text.contains("距重置约 2 天"), text)
        XCTAssertTrue(text.contains("约为 Pro 的 5 倍"), text)
        XCTAssertTrue(text.contains("绝对剩余仍说不出"), text)
        XCTAssertTrue(text.contains("后续动作由 session 结合任务自行判断"), text)
        XCTAssertFalse(text.contains("尽快收尾"), text)
        XCTAssertFalse(text.contains("重活先缓"), text)
        XCTAssertFalse(text.contains("可以往那边交接"), text)
    }

    // MARK: - 单家告警 = 定向

    func testSingleAgentWarningOnlyReachesCrewsWithThatRunner() {
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [claudeHot]), codex: snap("codex", [codexCool]),
            sessions: sessions, alreadyWarned: [])
        // crew B 只有 codex session → claude 的告警不该出现在它群里。
        XCTAssertEqual(out.map(\.crewId), ["A"])
        let m = out[0]
        XCTAssertEqual(m.mentions, [LocalWhiteboardMention(kind: "session", targetId: "s-c1")])
        XCTAssertTrue(m.text.contains("session已用 90%"))
        XCTAssertTrue(m.text.contains("Codex"), "该陈述另一家没有越过自己的提醒线")
        XCTAssertEqual(m.dedupeKey, "claude|session|2099-08-08T08:00:58Z")
    }

    func testSingleAgentWarningMentionsEveryRunningSessionOfThatRunner() {
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [claudeCool]), codex: snap("codex", [codexHot]),
            sessions: sessions, alreadyWarned: [])
        XCTAssertEqual(out.map(\.crewId), ["A", "B"])
        XCTAssertEqual(out[0].mentions.map(\.targetId), ["s-x1"])   // crew A 里只 @ codex 那个
        XCTAssertEqual(out[1].mentions.map(\.targetId), ["s-x2"])
        XCTAssertFalse(out[0].text.contains("各 session"))
    }

    func testNoRunningSessionOfThatRunnerProducesNothing() {
        let onlyCodex = [QuotaWarningPlan.RunningSession(
            sessionId: "s-x2", crewId: "B", agent: "codex")]
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [claudeHot]), codex: snap("codex", [codexCool]),
            sessions: onlyCodex, alreadyWarned: [])
        XCTAssertTrue(out.isEmpty)
    }

    func testBelowThresholdOrNonBlockingWindowProducesNothing() {
        let perModelWeekly = AgentQuotaWindow(label: "周(Fable)", usedPercent: 99, resetsAt: nil)
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [perModelWeekly,
                                    AgentQuotaWindow(label: "session", usedPercent: 84,
                                                     resetsAt: nil)]),
            codex: nil, sessions: sessions, alreadyWarned: [])
        XCTAssertTrue(out.isEmpty)
    }

    func testDedupeKeySuppressesRepeat() {
        let key = "claude|session|2099-08-08T08:00:58Z"
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [claudeHot]), codex: snap("codex", [codexCool]),
            sessions: sessions, alreadyWarned: [key])
        XCTAssertTrue(out.isEmpty)
    }

    // MARK: - 两家都告警 = 全体收活

    func testBothOverAddsSeparateAllHandsBroadcast() {
        let out = QuotaWarningPlan.compute(
            claude: snap("claude", [claudeHot]), codex: snap("codex", [codexHot]),
            sessions: sessions, alreadyWarned: [])
        let allHands = out.filter { $0.dedupeKey.hasPrefix("both|") }
        // 两家单家告警 + 全体广播（crew A、B 各一条）
        XCTAssertEqual(out.count - allHands.count, 3)
        XCTAssertEqual(allHands.map(\.crewId), ["A", "B"])
        for m in allHands {
            XCTAssertTrue(m.mentions.isEmpty, "全体广播不定向")
            XCTAssertTrue(m.text.contains("各有阻断窗越过"))
        }
        // 两家都越线时，单家文案只陈述本家事实，不把另一家写成未越线。
        for m in out where !m.dedupeKey.hasPrefix("both|") {
            XCTAssertFalse(m.text.contains("当前没有阻断窗越过"))
        }
    }

    func testAllHandsBroadcastHasOwnDedupeKey() {
        let claudeSnap = snap("claude", [claudeHot])
        let codexSnap = snap("codex", [codexHot])
        let first = QuotaWarningPlan.compute(
            claude: claudeSnap, codex: codexSnap, sessions: sessions, alreadyWarned: [])
        let seen = Set(first.map(\.dedupeKey))
        let again = QuotaWarningPlan.compute(
            claude: claudeSnap, codex: codexSnap, sessions: sessions, alreadyWarned: seen)
        XCTAssertTrue(again.isEmpty, "同一重置周期不该刷屏")

        // 单家告警已播过、但全体广播还没播 → 只补全体那条。
        let onlySingles = Set(first.filter { !$0.dedupeKey.hasPrefix("both|") }.map(\.dedupeKey))
        let topUp = QuotaWarningPlan.compute(
            claude: claudeSnap, codex: codexSnap, sessions: sessions, alreadyWarned: onlySingles)
        XCTAssertEqual(topUp.count, 2)
        XCTAssertTrue(topUp.allSatisfy { $0.dedupeKey.hasPrefix("both|") })
    }
}
