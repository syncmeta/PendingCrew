import XCTest
// QuotaRingLayout.swift + AgentQuota.swift 直接编进 PendingCrewTests target。

/// 侧栏额度圆环判定层单测（Todo #8/#10）。钉死三件事：
/// ① 颜色阈值不被后人偷偷改；② 窗口→环的映射（Claude 三环 / Codex 一环 /
/// 缺数据不画）；③ 模型名从哪个字段取、怎么大写。
final class QuotaRingLayoutTests: XCTestCase {

    // MARK: - fixtures（形状取自 AgentQuotaTests 的实测输出）

    private func claudeSnapshot(
        session: Int? = 32, weekly: Int? = 58, model: (String, Int)? = ("Fable", 44)
    ) -> AgentQuotaSnapshot {
        var windows: [AgentQuotaWindow] = []
        if let session {
            windows.append(.init(label: "session", usedPercent: session,
                                 resetsAt: "Jul 26 at 4:39am (Asia/Shanghai)"))
        }
        if let weekly {
            windows.append(.init(label: "周(全模型)", usedPercent: weekly,
                                 resetsAt: "Jul 28 at 10:59am (Asia/Shanghai)"))
        }
        if let model {
            windows.append(.init(label: "周(\(model.0))", usedPercent: model.1,
                                 resetsAt: "Jul 28 at 10:59am (Asia/Shanghai)"))
        }
        return AgentQuotaSnapshot(agent: "claude", windows: windows,
                                  fetchedAt: "2026-07-26T10:00:00Z")
    }

    private func codexSnapshot(weekly: Int? = 41) -> AgentQuotaSnapshot {
        var windows: [AgentQuotaWindow] = []
        if let weekly {
            windows.append(.init(label: "周窗", usedPercent: weekly,
                                 resetsAt: "2026-07-28T02:59:00Z"))
        }
        return AgentQuotaSnapshot(agent: "codex", windows: windows,
                                  fetchedAt: "2026-07-26T09:58:00Z")
    }

    // MARK: - ① 颜色阈值（<60 绿 / <80 黄 / <90 橙 / ≥90 红）

    func testQuotaLevelThresholds() {
        XCTAssertEqual(QuotaLevel.of(usedPercent: 0), .ample)
        XCTAssertEqual(QuotaLevel.of(usedPercent: 59), .ample)
        XCTAssertEqual(QuotaLevel.of(usedPercent: 60), .half)
        XCTAssertEqual(QuotaLevel.of(usedPercent: 79), .half)
        XCTAssertEqual(QuotaLevel.of(usedPercent: 80), .near)
        XCTAssertEqual(QuotaLevel.of(usedPercent: 89), .near)
        XCTAssertEqual(QuotaLevel.of(usedPercent: 90), .critical)
        XCTAssertEqual(QuotaLevel.of(usedPercent: 100), .critical)
    }

    func testRingProgressClampedToUnitRange() {
        // 上游给越界值（解析异常 / 新字段语义变化）时环不能画飞。
        let over = QuotaRing(id: "x", caption: "周", windowLabel: "周窗",
                             usedPercent: 140, form: .circle)
        let under = QuotaRing(id: "y", caption: "周", windowLabel: "周窗",
                              usedPercent: -5, form: .circle)
        XCTAssertEqual(over.progress, 1)
        XCTAssertEqual(under.progress, 0)
        XCTAssertEqual(over.level, .critical)
    }

    // MARK: - ② 窗口 → 环

    func testClaudeRendersThreeRingsInOrder() {
        let rings = QuotaRingLayout.claudeRings(claudeSnapshot())
        XCTAssertEqual(rings.map(\.caption), ["5h", "周", "Fable"])
        XCTAssertEqual(rings.map(\.usedPercent), [32, 58, 44])
        // 模型那一格是长条，其余正圆。
        XCTAssertEqual(rings.map(\.form), [.circle, .circle, .stadium])
        XCTAssertEqual(rings.map(\.windowLabel), ["session", "周(全模型)", "周(Fable)"])
        XCTAssertEqual(Set(rings.map(\.id)).count, 3, "id 必须互不相同（ForEach 稳定性）")
    }

    func testCodexRendersOnlyWeeklyRing() {
        let rings = QuotaRingLayout.codexRings(codexSnapshot())
        XCTAssertEqual(rings.map(\.caption), ["周"])
        XCTAssertEqual(rings.first?.usedPercent, 41)
        XCTAssertEqual(rings.first?.form, .circle)
    }

    func testCodexFiveHourWindowIsNotDrawnEvenIfPresent() {
        // codex 侧已无 5 小时分档（人类 2026-07-26 确认）。即便旧 rollout 里
        // 还留着 5 小时窗，也只画周环 —— 别让陈旧数据把 UI 带回旧形态。
        let snap = AgentQuotaSnapshot(agent: "codex", windows: [
            .init(label: "5小时窗", usedPercent: 12, resetsAt: nil),
            .init(label: "周窗", usedPercent: 41, resetsAt: nil),
        ], fetchedAt: "2026-07-26T09:58:00Z")
        XCTAssertEqual(QuotaRingLayout.codexRings(snap).map(\.caption), ["周"])
    }

    func testMissingWindowDrawsNoRing() {
        // Todo #8 ⑦：拿不到就不画，不画空环占位。
        let noModel = QuotaRingLayout.claudeRings(claudeSnapshot(model: nil))
        XCTAssertEqual(noModel.map(\.caption), ["5h", "周"])

        let onlyModel = QuotaRingLayout.claudeRings(claudeSnapshot(session: nil, weekly: nil))
        XCTAssertEqual(onlyModel.map(\.caption), ["Fable"])

        XCTAssertTrue(QuotaRingLayout.claudeRings(nil).isEmpty)
        XCTAssertTrue(QuotaRingLayout.codexRings(nil).isEmpty)
        XCTAssertTrue(QuotaRingLayout.codexRings(codexSnapshot(weekly: nil)).isEmpty)
    }

    // MARK: - ③ 模型名取哪个字段 + 大小写

    func testModelNameExtractedFromPerModelWeeklyLabel() {
        XCTAssertEqual(QuotaRingLayout.modelName(fromWindowLabel: "周(Fable)"), "Fable")
        // 人类明确要求首字母大写。
        XCTAssertEqual(QuotaRingLayout.modelName(fromWindowLabel: "周(fable)"), "Fable")
        // 其余字符原样 —— 不把 GPT-5 / Opus 4.7 之类压平。
        XCTAssertEqual(QuotaRingLayout.modelName(fromWindowLabel: "周(GPT-5)"), "GPT-5")
        XCTAssertEqual(QuotaRingLayout.modelName(fromWindowLabel: "周(opus 4.7)"), "Opus 4.7")
    }

    func testAllModelsAndOtherWindowsAreNotModelWindows() {
        XCTAssertNil(QuotaRingLayout.modelName(fromWindowLabel: "周(全模型)"))
        XCTAssertNil(QuotaRingLayout.modelName(fromWindowLabel: "session"))
        XCTAssertNil(QuotaRingLayout.modelName(fromWindowLabel: "周窗"))
        XCTAssertNil(QuotaRingLayout.modelName(fromWindowLabel: "周()"))
        XCTAssertNil(QuotaRingLayout.modelName(fromWindowLabel: "Current week (Fable)"))
    }

    func testCurrentModelWindowPicksFirstPerModelWeekly() {
        let snap = AgentQuotaSnapshot(agent: "claude", windows: [
            .init(label: "session", usedPercent: 10, resetsAt: nil),
            .init(label: "周(全模型)", usedPercent: 20, resetsAt: nil),
            .init(label: "周(Fable)", usedPercent: 30, resetsAt: nil),
            .init(label: "周(Opus)", usedPercent: 40, resetsAt: nil),
        ], fetchedAt: "2026-07-26T10:00:00Z")
        XCTAssertEqual(QuotaRingLayout.currentModelWindow(snap)?.label, "周(Fable)")
        XCTAssertEqual(QuotaRingLayout.claudeRings(snap).map(\.caption), ["5h", "周", "Fable"])
    }

    // MARK: - 新鲜度文案

    func testFreshnessLabelUsesNewestSnapshot() {
        let now = ISO8601DateFormatter().date(from: "2026-07-26T10:05:00Z")!
        // codex 更旧（09:58），取 claude 的 10:00 → 5 分钟前。
        XCTAssertEqual(
            QuotaRingLayout.freshnessLabel(
                fetchedAt: ["2026-07-26T09:58:00Z", "2026-07-26T10:00:00Z"], now: now),
            "5 分钟前")
    }

    func testFreshnessLabelBuckets() {
        let now = ISO8601DateFormatter().date(from: "2026-07-26T10:00:00Z")!
        func label(_ stamp: String) -> String? {
            QuotaRingLayout.freshnessLabel(fetchedAt: [stamp], now: now)
        }
        XCTAssertEqual(label("2026-07-26T09:59:30Z"), "刚刚")
        XCTAssertEqual(label("2026-07-26T09:59:00Z"), "1 分钟前")
        XCTAssertEqual(label("2026-07-26T09:00:00Z"), "1 小时前")
        XCTAssertEqual(label("2026-07-25T08:00:00Z"), "1 天前")
        // 时钟回拨 / 未来时刻不显示负数。
        XCTAssertEqual(label("2026-07-26T10:30:00Z"), "刚刚")
    }

    func testFreshnessLabelNilWhenNoSnapshot() {
        XCTAssertNil(QuotaRingLayout.freshnessLabel(fetchedAt: [nil, nil]))
        XCTAssertNil(QuotaRingLayout.freshnessLabel(fetchedAt: ["not-a-date"]))
        XCTAssertNil(QuotaRingLayout.freshnessLabel(fetchedAt: []))
    }

    // MARK: - 悬停提示（重置时刻只活在这里）

    func testHelpTextCarriesResetTimesForEveryWindow() {
        // now 要钉在 fixture 的那个时点：这两份快照的重置时刻都在 2026-07-26 之后，
        // 拿真实 now 去看它们早翻篇了，提示里会多出一条「重置时刻都已过去」。
        let now = ISO8601DateFormatter().date(from: "2026-07-26T10:05:00Z")!
        let text = QuotaRingLayout.helpText(claude: claudeSnapshot(), codex: codexSnapshot(),
                                            now: now)
        let unwrapped = try! XCTUnwrap(text)
        XCTAssertTrue(unwrapped.contains("Claude Code："))
        XCTAssertTrue(unwrapped.contains("Codex："))
        // 环上不常驻的重置时刻必须在这里能查到（Todo #8 ⑥）。
        XCTAssertTrue(unwrapped.contains("session 已用 32%（Jul 26 at 4:39am (Asia/Shanghai) 重置）"))
        XCTAssertTrue(unwrapped.contains("周(Fable) 已用 44%"))
        XCTAssertEqual(unwrapped.split(separator: "\n").count, 2)
    }

    func testHelpTextNilWhenBothMissing() {
        XCTAssertNil(QuotaRingLayout.helpText(claude: nil, codex: nil))
    }

    // MARK: - 环下那行：常态「N 分钟前」/ 悬停换成那一项的重置时刻（Todo #14）

    func testFootnoteFallsBackToFreshnessWhenNotHovering() {
        let now = ISO8601DateFormatter().date(from: "2026-07-26T10:05:00Z")!
        XCTAssertEqual(
            QuotaRingLayout.footnote(hovered: nil,
                                     fetchedAt: ["2026-07-26T10:00:00Z"], now: now),
            "5 分钟前")
    }

    func testFootnoteShowsHoveredRingResetTime() throws {
        // 重置时刻是本地时区渲染的，用当前日历构造「今天 / 隔天」的期望值。
        let now = Date(timeIntervalSince1970: 1_785_000_000)
        let sameDay = now.addingTimeInterval(3600)
        let ring = QuotaRing(id: "claude.session", caption: "5h", windowLabel: "session",
                             usedPercent: 32, form: .circle,
                             resetsAt: ISO8601DateFormatter().string(from: sameDay),
                             brand: "Claude Code")
        let text = try XCTUnwrap(
            QuotaRingLayout.footnote(hovered: ring, fetchedAt: ["2026-07-26T10:00:00Z"], now: now))
        XCTAssertTrue(text.hasPrefix("Claude Code 5h · "), text)
        XCTAssertTrue(text.hasSuffix(" 重置"), text)
        // 悬停时绝不再显示读取时刻 —— 那会被误读成重置时刻。
        XCTAssertFalse(text.contains("分钟前"), text)
    }

    /// 两家都有叫「周」的环，文案必须说清是谁的周。
    func testFootnoteDisambiguatesBrandForSameCaption() throws {
        let now = Date(timeIntervalSince1970: 1_785_000_000)
        let codexWeekly = QuotaRingLayout.codexRings(codexSnapshot()).first
        let ring = try XCTUnwrap(codexWeekly)
        let text = try XCTUnwrap(QuotaRingLayout.footnote(hovered: ring, fetchedAt: [], now: now))
        XCTAssertTrue(text.hasPrefix("Codex 周 · "), text)
    }

    /// 上游没给重置时刻（或格式变了）→ 明说未知，不悄悄退回「N 分钟前」。
    func testFootnoteSaysUnknownWhenResetMissingOrUnparsable() throws {
        let now = Date(timeIntervalSince1970: 1_785_000_000)
        func text(_ raw: String?) -> String? {
            QuotaRingLayout.footnote(
                hovered: QuotaRing(id: "x", caption: "周", windowLabel: "周窗", usedPercent: 41,
                                   form: .circle, resetsAt: raw, brand: "Codex"),
                fetchedAt: ["2026-07-26T10:00:00Z"], now: now)
        }
        XCTAssertEqual(text(nil), "Codex 周 · 重置时刻未知")
        XCTAssertEqual(text("下周三吧"), "Codex 周 · 重置时刻未知")
    }

    /// 环带上重置时刻和家名，视图才能只凭「悬停的那个环」拼出文案。
    func testRingsCarryResetTimeAndBrand() throws {
        let claude = QuotaRingLayout.claudeRings(claudeSnapshot())
        XCTAssertEqual(claude.map(\.brand), ["Claude Code", "Claude Code", "Claude Code"])
        XCTAssertEqual(claude.first?.resetsAt, "Jul 26 at 4:39am (Asia/Shanghai)")
        let codex = try XCTUnwrap(QuotaRingLayout.codexRings(codexSnapshot()).first)
        XCTAssertEqual(codex.brand, "Codex")
        XCTAssertEqual(codex.resetsAt, "2026-07-28T02:59:00Z")
    }

    // MARK: - 陈旧标记（按家挂，不许被另一家的新鲜度盖掉）

    /// codex 的额度来自 rollout 文件，只在真跑 codex 时才更新。一个多月没跑，
    /// 数字就是一个多月前的 —— 而下面那行「N 分钟前」取两家最新的读取时刻，
    /// claude 刚查过就会把它一起盖成「刚刚」。所以标记必须按家挂。
    func testStaleBadgeMarksOnlyTheStaleAgent() {
        let now = Date(timeIntervalSince1970: 1_785_000_000)
        let fresh = AgentQuotaSnapshot(
            agent: "claude", windows: [.init(label: "session", usedPercent: 32, resetsAt: nil)],
            fetchedAt: iso(now), producedAt: iso(now))
        let stale = AgentQuotaSnapshot(
            agent: "codex", windows: [.init(label: "周窗", usedPercent: 1, resetsAt: nil)],
            fetchedAt: iso(now),                                   // 刚读到
            producedAt: iso(now.addingTimeInterval(-35 * 86_400)))  // 但数据是 35 天前产的

        XCTAssertNil(QuotaRingLayout.staleBadge(fresh, now: now))
        XCTAssertEqual(QuotaRingLayout.staleBadge(stale, now: now), "35 天前的数据")
        XCTAssertNil(QuotaRingLayout.staleBadge(nil, now: now))
    }

    /// 旧 quota.json 没有 `producedAt` —— 说不出年龄就别标（既不假装新也不假装旧）。
    func testStaleBadgeSilentWhenAgeUnknown() {
        XCTAssertNil(QuotaRingLayout.staleBadge(codexSnapshot()))
    }

    /// 悬停提示里也要说破，并说清为什么会停在那里。
    func testHelpTextSpellsOutStaleness() throws {
        let now = Date(timeIntervalSince1970: 1_785_000_000)
        let stale = AgentQuotaSnapshot(
            agent: "codex", windows: [.init(label: "周窗", usedPercent: 1, resetsAt: nil)],
            fetchedAt: iso(now), producedAt: iso(now.addingTimeInterval(-35 * 86_400)))
        let text = try XCTUnwrap(QuotaRingLayout.helpText(claude: nil, codex: stale, now: now))
        XCTAssertTrue(text.contains("35 天前"), text)
        XCTAssertTrue(text.contains("不是当前值"), text)
    }

    // MARK: - 警示标记（Todo #33：读不到 / 窗口已翻篇 都不许静默）

    /// 取不到数时旧实现一声不吭继续画上一轮的数字 —— 「读不到」和「就是这个数」
    /// 在界面上长得一模一样。必须说破。
    func testWarningBadgeSaysWhenFetchFailed() {
        let now = Date(timeIntervalSince1970: 1_785_000_000)
        let snap = AgentQuotaSnapshot(
            agent: "codex", windows: [.init(label: "周窗", usedPercent: 42, resetsAt: nil)],
            fetchedAt: iso(now), producedAt: iso(now))
        XCTAssertEqual(QuotaRingLayout.warningBadge(snap, failure: "读不到（xxx）", now: now),
                       "读不到，下面是旧值")
        XCTAssertEqual(QuotaRingLayout.warningBadge(nil, failure: "读不到（xxx）", now: now),
                       "读不到")
        XCTAssertNil(QuotaRingLayout.warningBadge(snap, failure: nil, now: now))
    }

    /// 「过了自己的重置时刻」比岁数阈值硬：数据可能只有几十分钟大，窗却已经翻篇，
    /// 那个百分比必然不是现状（codex 停在 87% 就是这么来的）。
    func testWarningBadgeFlagsWindowPastItsReset() {
        let now = ISO8601DateFormatter().date(from: "2026-08-08T11:54:00Z")!
        let expired = AgentQuotaSnapshot(
            agent: "codex",
            windows: [.init(label: "周窗", usedPercent: 87, resetsAt: "2026-08-08T08:00:58Z")],
            fetchedAt: iso(now), producedAt: iso(now.addingTimeInterval(-3600)))
        XCTAssertEqual(QuotaRingLayout.warningBadge(expired, now: now), "窗口已重置，等下一次刷新")
        // 还没到点 → 不标。
        let live = AgentQuotaSnapshot(
            agent: "codex",
            windows: [.init(label: "周窗", usedPercent: 0, resetsAt: "2026-08-15T12:01:41Z")],
            fetchedAt: iso(now), producedAt: iso(now))
        XCTAssertNil(QuotaRingLayout.warningBadge(live, now: now))
    }

    func testHelpTextSpellsOutFailureAndPastReset() throws {
        let now = ISO8601DateFormatter().date(from: "2026-08-08T11:54:00Z")!
        let expired = AgentQuotaSnapshot(
            agent: "codex",
            windows: [.init(label: "周窗", usedPercent: 87, resetsAt: "2026-08-08T08:00:58Z")],
            fetchedAt: iso(now), producedAt: iso(now))
        let text = try XCTUnwrap(QuotaRingLayout.helpText(
            claude: nil, codex: expired, codexError: "读不到（codex app-server 没答上）", now: now))
        XCTAssertTrue(text.contains("读不到"), text)
        XCTAssertTrue(text.contains("旧值"), text)
        XCTAssertTrue(text.contains("重置时刻都已过去"), text)
    }

    /// 一次都没取到过的那一家，也不能从提示里凭空消失。
    func testHelpTextKeepsAgentWithNoSnapshotButAFailure() throws {
        let text = try XCTUnwrap(QuotaRingLayout.helpText(
            claude: nil, codex: nil, codexError: "读不到（codex 没装）"))
        XCTAssertTrue(text.contains("Codex"), text)
        XCTAssertTrue(text.contains("读不到"), text)
    }

    private func iso(_ date: Date) -> String {
        ISO8601DateFormatter().string(from: date)
    }
}
