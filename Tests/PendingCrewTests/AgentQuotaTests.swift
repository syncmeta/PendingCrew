import XCTest
// AgentQuota.swift 直接编进 PendingCrewTests target。

/// 额度解析单测（#455 批 P4）：claude `/usage` 文本 + codex rollout rate_limits。
/// fixture 均取自 2026-07-05 本机实测输出（非臆造）。
final class AgentQuotaTests: XCTestCase {

    // MARK: - claude /usage 文本

    func testClaudeUsageParsesAllWindows() {
        let text = """
        You are currently using your subscription to power your Claude Code usage
        Current session: 80% used · resets Jul 5 at 4:39am (Asia/Shanghai)
        Current week (all models): 23% used · resets Jul 5 at 10:59am (Asia/Shanghai)
        Current week (Fable): 18% used · resets Jul 5 at 10:59am (Asia/Shanghai)
        Last 24h · 758 requests · 4 sessions
        Last 7d · 1500 requests · 9 sessions
        """
        let snap = ClaudeUsageTextParser.parse(text)
        XCTAssertEqual(snap?.agent, "claude")
        XCTAssertEqual(snap?.windows.count, 3)
        XCTAssertEqual(snap?.windows[0].label, "session")
        XCTAssertEqual(snap?.windows[0].usedPercent, 80)
        XCTAssertEqual(snap?.windows[0].resetsAt, "Jul 5 at 4:39am (Asia/Shanghai)")
        XCTAssertEqual(snap?.windows[1].label, "周(全模型)")
        XCTAssertEqual(snap?.windows[2].label, "周(Fable)")
        XCTAssertEqual(snap?.summary, "session 80% · 周(全模型) 23% · 周(Fable) 18%")
        XCTAssertEqual(snap?.activities, [
            AgentQuotaActivity(periodLabel: "Last 24h", requests: 758, sessions: 4),
            AgentQuotaActivity(periodLabel: "Last 7d", requests: 1500, sessions: 9),
        ])
    }

    func testClaudeUsageActivityKeepsPartialCountsWithoutGuessing() throws {
        let requestsOnly = try XCTUnwrap(ClaudeUsageTextParser.parse("""
        Current session: 10% used
        Last 24h · 12 requests
        """))
        XCTAssertEqual(requestsOnly.activities?.first?.requests, 12)
        XCTAssertNil(requestsOnly.activities?.first?.sessions)
        let noActivity = try XCTUnwrap(
            ClaudeUsageTextParser.parse("Current session: 10% used"))
        XCTAssertNil(noActivity.activities)
    }

    func testClaudeConfigParsesExactMaxMultiplierFromRealFieldShape() {
        let sample = #"{"oauthAccount":{"billingType":"stripe_subscription","seatTier":null,"organizationType":"claude_max","organizationRateLimitTier":"default_claude_max_5x","userRateLimitTier":null}}"#
        XCTAssertEqual(ClaudeAccountPlanParser.parse(Data(sample.utf8)), "Max 5x")
        XCTAssertNil(ClaudeAccountPlanParser.parse(Data("{}".utf8)))
    }

    func testClaudeUsageNoMatchReturnsNil() {
        XCTAssertNil(ClaudeUsageTextParser.parse("Last 24h · 5 requests\nnothing here"))
        XCTAssertNil(ClaudeUsageTextParser.parse(""))
    }

    func testClaudeUnknownWindowLabelKeptVerbatim() {
        let snap = ClaudeUsageTextParser.parse("Some new window: 5% used · resets soon")
        XCTAssertEqual(snap?.windows.first?.label, "Some new window")
        XCTAssertEqual(snap?.windows.first?.usedPercent, 5)
    }

    // MARK: - codex rollout rate_limits

    func testCodexRolloutParsesLatestRateLimits() {
        let jsonl = """
        {"timestamp":"t1","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":5.0,"window_minutes":300,"resets_at":1782030455},"secondary":{"used_percent":15.0,"window_minutes":10080,"resets_at":1782380334},"plan_type":"prolite"}}}
        {"timestamp":"t2","type":"event_msg","payload":{"type":"other"}}
        {"timestamp":"t3","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":9.0,"window_minutes":300,"resets_at":1782030455},"secondary":{"used_percent":16.0,"window_minutes":10080,"resets_at":1782380334},"plan_type":"prolite"}}}
        """
        let snap = CodexRolloutQuotaParser.parse(jsonl)
        XCTAssertEqual(snap?.agent, "codex")
        // 取最后一条（最新）：9% / 16%。
        XCTAssertEqual(snap?.windows.map(\.usedPercent), [9, 16])
        XCTAssertEqual(snap?.windows.map(\.label), ["5小时窗", "周窗"])
        XCTAssertNotNil(snap?.windows[0].resetsAt)
        XCTAssertEqual(snap?.subscriptionPlan, "prolite")
        XCTAssertEqual(snap?.subscriptionPlanSource, "codex_rollout")
    }

    func testCodexRolloutSecondaryNullStillParses() {
        // free 计划实测 secondary 可为 null。
        let jsonl = """
        {"payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":3.0,"window_minutes":10080,"resets_at":1783787895},"secondary":null,"plan_type":"free"}}}
        """
        let snap = CodexRolloutQuotaParser.parse(jsonl)
        XCTAssertEqual(snap?.windows.count, 1)
        XCTAssertEqual(snap?.windows[0].label, "周窗")
        XCTAssertEqual(snap?.windows[0].usedPercent, 3)
    }

    func testCodexRolloutNoRateLimitsReturnsNil() {
        XCTAssertNil(CodexRolloutQuotaParser.parse("{\"payload\":{\"type\":\"token_count\"}}"))
        XCTAssertNil(CodexRolloutQuotaParser.parse(""))
    }

    // MARK: - Todo #33：codex 额度「更新不了」的真因钉死
    //
    // 现象：侧栏 codex 环长期停在 87%，连它自己标的重置时刻过了也不动。
    // 排查结论（2026-08-08 实测，见下面三条断言）：
    //   ① 解析没问题 —— 盘上那条真实 JSON 喂进去，87% 和重置时刻都对；
    //   ② 数据源是**被动**的 —— rollout jsonl 只有 codex CLI 真跑一轮才写，
    //      那之后窗口在服务端翻篇了我们无从知晓，于是永远画着过期窗的百分比；
    //   ③ 真相要**现问** —— app-server `account/rateLimits/read` 现查现得
    //      （实测同一时刻返回 0%：周窗已经重置过了）。

    /// 盘上真实那条（`~/.codex/sessions/2026/08/08/rollout-…-019fdf94-9c9c-….jsonl`
    /// 尾部最后一条 rate_limits，2026-08-08 12:17 落盘）。primary 是周窗、secondary
    /// 为 null、resets_at 是 unix 秒。
    private static let realRolloutLine = """
    {"timestamp":"2026-08-08T04:17:55.279Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1352961},"model_context_window":258400},"rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":87.0,"window_minutes":10080,"resets_at":1786176058},"secondary":null,"credits":{"has_credits":false,"unlimited":false,"balance":"0"},"individual_limit":null,"spend_control_reached":null,"plan_type":"plus","rate_limit_reached_type":null}}}
    """

    /// 数据落盘时刻（文件 mtime）与「人来看」的时刻。
    private static let producedAt = ISO8601DateFormatter().date(from: "2026-08-08T04:17:55Z")!
    private static let lookedAt = ISO8601DateFormatter().date(from: "2026-08-08T11:54:00Z")!

    func testRealCodexRolloutLineParsesWeeklyOnly() throws {
        // ① 解析这一层是好的：机长怀疑的「只认 5 小时窗 / 解析失败静默」都不成立。
        let snap = try XCTUnwrap(CodexRolloutQuotaParser.parse(
            Self.realRolloutLine, now: Self.lookedAt, producedAt: Self.producedAt))
        XCTAssertEqual(snap.agent, "codex")
        XCTAssertEqual(snap.windows.map(\.label), ["周窗"])   // secondary=null → 只有周窗
        XCTAssertEqual(snap.windows[0].usedPercent, 87)
        XCTAssertEqual(snap.windows[0].resetsAt, "2026-08-08T08:00:58Z")
        XCTAssertEqual(snap.weeklyWindow?.usedPercent, 87)   // UI 取的就是这个,取得到
        XCTAssertNil(snap.fiveHourWindow)
    }

    func testRolloutSnapshotIsPastItsOwnResetAndMustNotBeShownAsCurrent() throws {
        // ② 真因：这份数据描述的周窗在 08:00:58Z 就翻篇了，11:54Z 再看那 87%
        //    已经不是现状 —— 而旧实现照旧原样画出来，这就是「更新不了」。
        let snap = try XCTUnwrap(CodexRolloutQuotaParser.parse(
            Self.realRolloutLine, now: Self.lookedAt, producedAt: Self.producedAt))
        XCTAssertTrue(snap.isPastReset(now: Self.lookedAt),
                      "窗口重置时刻已过 → 必须判定为「翻篇了」，不能继续当现状显示")
        // 落盘时刻自己还没过重置，那时它确实是现状。
        XCTAssertFalse(snap.isPastReset(now: Self.producedAt))
    }

    /// app-server `account/rateLimits/read` 的真实返回（2026-08-08 11:59Z 实测；
    /// 注意键名是 camelCase，跟 rollout 的 snake_case 不是一套）。
    private static let realAppServerResult: [String: Any] = [
        "rateLimits": [
            "limitId": "codex",
            "limitName": NSNull(),
            "primary": ["usedPercent": 0, "windowDurationMins": 10080, "resetsAt": 1786795301],
            "secondary": NSNull(),
            "credits": ["hasCredits": false, "unlimited": false, "balance": "0"],
            "planType": "plus",
        ],
    ]

    func testCodexAppServerLiveRateLimitsParse() throws {
        // ③ 现问 codex 拿到的才是真相：同一时刻是 0%（周窗已重置），不是 87%。
        let snap = try XCTUnwrap(
            CodexAppServerQuotaParser.parse(Self.realAppServerResult, now: Self.lookedAt))
        XCTAssertEqual(snap.agent, "codex")
        XCTAssertEqual(snap.windows.map(\.label), ["周窗"])
        XCTAssertEqual(snap.windows[0].usedPercent, 0)
        XCTAssertEqual(snap.windows[0].resetsAt, "2026-08-15T12:01:41Z")
        XCTAssertEqual(snap.subscriptionPlan, "Plus")
        XCTAssertEqual(snap.subscriptionPlanSource, "codex_rate_limits")
        // 现查现得 → producedAt 就是此刻：既不陈旧，也没过重置时刻。
        XCTAssertEqual(snap.producedAt, ISO8601DateFormatter().string(from: Self.lookedAt))
        XCTAssertFalse(snap.isStale(now: Self.lookedAt))
        XCTAssertFalse(snap.isPastReset(now: Self.lookedAt))
    }

    func testCodexAppServerParserRejectsGarbageInsteadOfGuessing() {
        XCTAssertNil(CodexAppServerQuotaParser.parse(nil))
        XCTAssertNil(CodexAppServerQuotaParser.parse(["rateLimits": NSNull()]))
        XCTAssertNil(CodexAppServerQuotaParser.parse(["rateLimits": ["primary": NSNull(),
                                                                    "secondary": NSNull()]]))
    }

    func testRolloutTailCutMidCharacterStillParses() throws {
        // 只读尾部 256KB 是按**字节**切的，切点会落在多字节字符中间。旧实现整段
        // UTF-8 解码返回 nil → 这一轮无声无息什么都没读到，看着就像「不更新」。
        let full = ("{\"payload\":{\"type\":\"agent_message\",\"text\":\"中文中文中文\"}}\n"
                    + Self.realRolloutLine).data(using: .utf8)!
        // 从第 1 个字节起切 —— 落在开头 `{` 之后、后面又有汉字，边界必然不齐。
        for cut in 1...4 {
            let snap = try XCTUnwrap(CodexRolloutQuotaParser.parseTail(full.dropFirst(cut)),
                                     "切在偏移 \(cut) 处应仍能解析出尾部的 rate_limits")
            XCTAssertEqual(snap.windows.map(\.usedPercent), [87])
        }
    }

    func testIsPastResetIgnoresWindowsWithoutResetTimeAndNeverGuesses() {
        // 说不出重置时刻的窗不参与判定；一个都判不出 → false（不猜）。
        let unknown = AgentQuotaSnapshot(
            agent: "codex",
            windows: [AgentQuotaWindow(label: "周窗", usedPercent: 50, resetsAt: nil)],
            fetchedAt: "x")
        XCTAssertFalse(unknown.isPastReset(now: Self.lookedAt))
        // 只要还有一个窗没到点，就不算整份翻篇。
        let mixed = AgentQuotaSnapshot(
            agent: "codex",
            windows: [AgentQuotaWindow(label: "5小时窗", usedPercent: 9,
                                       resetsAt: "2026-08-08T08:00:58Z"),
                      AgentQuotaWindow(label: "周窗", usedPercent: 16,
                                       resetsAt: "2026-08-15T12:01:41Z")],
            fetchedAt: "x")
        XCTAssertFalse(mixed.isPastReset(now: Self.lookedAt))
    }

    // MARK: - 窗口取值（侧栏四段显示 / 自动挂钩用）

    func testFiveHourAndWeeklyWindowSelection() {
        let claude = ClaudeUsageTextParser.parse("""
        Current session: 80% used · resets Jul 5 at 4:39am (Asia/Shanghai)
        Current week (all models): 23% used · resets Jul 5 at 10:59am (Asia/Shanghai)
        Current week (Fable): 18% used · resets Jul 5 at 10:59am (Asia/Shanghai)
        """)
        XCTAssertEqual(claude?.fiveHourWindow?.usedPercent, 80)
        XCTAssertEqual(claude?.weeklyWindow?.usedPercent, 23)   // 全模型总窗,非 Fable 分窗
        let codex = CodexRolloutQuotaParser.parse("""
        {"payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":9.0,"window_minutes":300,"resets_at":1782030455},"secondary":{"used_percent":16.0,"window_minutes":10080,"resets_at":1782380334}}}}
        """)
        XCTAssertEqual(codex?.fiveHourWindow?.usedPercent, 9)
        XCTAssertEqual(codex?.weeklyWindow?.usedPercent, 16)
    }

    // MARK: - 阻断窗分类（额度警戒广播只看阻断窗,人类拍板 2026-07-26）

    func testPerModelWeeklyWindowsAreNonBlocking() {
        // 单模型周窗耗尽可切模型继续 → 非阻断,不触发全 crew 警戒。
        for label in ["周(Fable)", "周(Opus)", "周(Sonnet)"] {
            let w = AgentQuotaWindow(label: label, usedPercent: 100, resetsAt: nil)
            XCTAssertFalse(w.isBlocking, "\(label) 应为非阻断窗")
        }
    }

    func testAllQuotaBlockingWindowsStayBlocking() {
        // 真正挡住所有工作的窗：claude session/周(全模型) + codex 5小时窗/周窗。
        for label in ["session", "周(全模型)", "5小时窗", "周窗"] {
            let w = AgentQuotaWindow(label: label, usedPercent: 100, resetsAt: nil)
            XCTAssertTrue(w.isBlocking, "\(label) 应为阻断窗")
        }
    }

    func testUnknownWindowLabelDefaultsToBlocking() {
        // 没认出的新窗名保守按阻断算：宁可误报,别漏报真停摆。
        for label in ["Some new window", "300分钟窗", "窗口", "周("] {
            let w = AgentQuotaWindow(label: label, usedPercent: 100, resetsAt: nil)
            XCTAssertTrue(w.isBlocking, "\(label) 应保守视为阻断窗")
        }
    }

    // MARK: - claude 重置时刻人话解析（撞墙自动挂钩用）

    func testClaudeResetTimeParsesWithTimezone() throws {
        // now = 2026-07-04 18:00 UTC；"Jul 5 at 4:39am (Asia/Shanghai)" =
        // 2026-07-04 20:39 UTC —— 未来最近年份推断应取 2026。
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        let now = cal.date(from: DateComponents(year: 2026, month: 7, day: 4, hour: 18))!
        let d = try XCTUnwrap(ClaudeResetTimeParser.parse("Jul 5 at 4:39am (Asia/Shanghai)", now: now))
        var sh = Calendar(identifier: .gregorian)
        sh.timeZone = TimeZone(identifier: "Asia/Shanghai")!
        let parts = sh.dateComponents([.year, .month, .day, .hour, .minute], from: d)
        XCTAssertEqual([parts.year, parts.month, parts.day, parts.hour, parts.minute],
                       [2026, 7, 5, 4, 39])
        XCTAssertGreaterThan(d, now)
    }

    func testClaudeResetTimeYearRollover() throws {
        // 12 月底看 "Jan 2 at 3pm" → 应推断成明年,不是今年一月(已过去)。
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        let now = cal.date(from: DateComponents(year: 2026, month: 12, day: 30))!
        let d = try XCTUnwrap(ClaudeResetTimeParser.parse("Jan 2 at 3pm (UTC)", now: now))
        XCTAssertEqual(cal.component(.year, from: d), 2027)
    }

    func testClaudeResetTimeGarbageReturnsNil() {
        XCTAssertNil(ClaudeResetTimeParser.parse("soon-ish"))
        XCTAssertNil(ClaudeResetTimeParser.parse(""))
    }

    // MARK: - 撞限额唤醒时刻计算（Todo #10 ①）

    func testWakeupPlanClaudeHumanTextIsResetPlusOneMinute() throws {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "Asia/Shanghai")!
        let now = cal.date(from: DateComponents(year: 2026, month: 7, day: 4, hour: 18))!
        let reset = try XCTUnwrap(ClaudeResetTimeParser.parse("Jul 5 at 4:39am (Asia/Shanghai)", now: now))
        let plan = QuotaWakeupPlan.compute(resetsAt: "Jul 5 at 4:39am (Asia/Shanghai)", now: now)
        XCTAssertFalse(plan.isFallback)
        XCTAssertEqual(plan.fireAt, reset.addingTimeInterval(60))
    }

    func testWakeupPlanCodexISOIsResetPlusOneMinute() {
        let now = Date(timeIntervalSince1970: 1_782_000_000)
        let resetEpoch = now.addingTimeInterval(3600)
        let iso = ISO8601DateFormatter().string(from: resetEpoch)
        let plan = QuotaWakeupPlan.compute(resetsAt: iso, now: now)
        XCTAssertFalse(plan.isFallback)
        // ISO 秒精度往返：允差 1s 内等于 reset+60。
        XCTAssertEqual(plan.fireAt.timeIntervalSince(now), 3660, accuracy: 1)
    }

    func testWakeupPlanMissingResetFallsBack45Min() {
        let now = Date(timeIntervalSince1970: 0)
        for raw in [nil, "soon-ish", ""] as [String?] {
            let plan = QuotaWakeupPlan.compute(resetsAt: raw, now: now)
            XCTAssertTrue(plan.isFallback)
            XCTAssertEqual(plan.fireAt, now.addingTimeInterval(45 * 60))
        }
    }

    func testWakeupPlanStalePastResetFallsBack() {
        // 陈旧快照：重置时刻已过去 → 不能挂在过去,走退避。
        let now = Date(timeIntervalSince1970: 1_782_000_000)
        let iso = ISO8601DateFormatter().string(from: now.addingTimeInterval(-300))
        let plan = QuotaWakeupPlan.compute(resetsAt: iso, now: now)
        XCTAssertTrue(plan.isFallback)
    }

    // MARK: - snapshot codec（quota.json 给 helper get_quota 读）

    func testSnapshotRoundTripsThroughJSON() throws {
        let snap = AgentQuotaSnapshot(
            agent: "claude",
            windows: [AgentQuotaWindow(label: "session", usedPercent: 80, resetsAt: "soon")],
            fetchedAt: "2026-07-05T01:00:00Z", subscriptionPlan: "Max 5x",
            subscriptionPlanSource: "claude_config",
            activities: [AgentQuotaActivity(
                periodLabel: "Last 24h", requests: 758, sessions: 4)])
        let data = try JSONEncoder().encode(snap)
        let back = try JSONDecoder().decode(AgentQuotaSnapshot.self, from: data)
        XCTAssertEqual(back, snap)
    }

    func testSubscriptionPlanDescriptionOnlyUsesDetectedValue() {
        let detected = AgentQuotaSnapshot(
            agent: "claude", windows: [], fetchedAt: "now", subscriptionPlan: "Max 5x",
            subscriptionPlanSource: "claude_config")
        XCTAssertEqual(detected.subscriptionPlan, "Max 5x")
        XCTAssertTrue(detected.subscriptionPlanDescription.contains("自动"))
        XCTAssertFalse(detected.subscriptionPlanDescription.contains("手动"))
    }

    // MARK: - 陈旧数据不许冒充现状

    /// 病灶：codex 的额度来自 `rollout-*.jsonl`，那文件只有真跑 codex 才写。
    /// 一个多月没跑 codex，我们每 10 分钟勤快地读一次、`fetchedAt` 永远显示「刚刚」，
    /// 读到的却是一个多月前那次的数（实测 2026-07-26 看到重置时刻停在 6/21、6/25）。
    /// `producedAt` 走文件 mtime，据此如实标龄。
    func testCodexSnapshotCarriesRolloutMtimeAsProducedAt() throws {
        let jsonl = #"""
        {"payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":1.0,"window_minutes":10080,"resets_at":1785646463}}}}
        """#
        let wrote = Date(timeIntervalSince1970: 1_782_000_000)
        let snap = try XCTUnwrap(CodexRolloutQuotaParser.parse(jsonl, producedAt: wrote))
        let age = try XCTUnwrap(snap.ageSeconds(now: wrote.addingTimeInterval(35 * 86_400)))
        XCTAssertEqual(age / 86_400, 35, accuracy: 0.01)
        XCTAssertTrue(snap.isStale(now: wrote.addingTimeInterval(35 * 86_400)))
        let note = try XCTUnwrap(snap.stalenessNote(now: wrote.addingTimeInterval(35 * 86_400)))
        XCTAssertTrue(note.contains("35 天前"), "得说清是多久以前的数：\(note)")
        XCTAssertTrue(note.contains("codex"), "还要说清为什么会停在那里：\(note)")
    }

    /// 刚跑过 codex 的数据不该被标成陈旧（别反过来制造噪音）。
    func testFreshCodexSnapshotIsNotFlaggedStale() throws {
        let jsonl = #"""
        {"payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":1.0,"window_minutes":10080,"resets_at":1785646463}}}}
        """#
        let wrote = Date(timeIntervalSince1970: 1_782_000_000)
        let snap = try XCTUnwrap(CodexRolloutQuotaParser.parse(jsonl, producedAt: wrote))
        XCTAssertFalse(snap.isStale(now: wrote.addingTimeInterval(600)))
        XCTAssertNil(snap.stalenessNote(now: wrote.addingTimeInterval(600)))
    }

    /// claude 是 `-p /usage` 现查现得 —— 产生时刻等于读到时刻，永远不该被标陈旧。
    func testClaudeSnapshotProducedAtEqualsFetchedAt() throws {
        let snap = try XCTUnwrap(ClaudeUsageTextParser.parse(
            "Current session: 80% used · resets Jul 5 at 4:39am (Asia/Shanghai)"))
        XCTAssertEqual(snap.producedAt, snap.fetchedAt)
        XCTAssertFalse(snap.isStale())
    }

    /// 旧 quota.json 没有 `producedAt` 字段 —— 必须仍能解码，且**不**假装是新数据，
    /// 也不假装是旧数据：说不出年龄就留白（ageSeconds = nil，不标 ⚠︎）。
    func testLegacySnapshotWithoutProducedAtStillDecodes() throws {
        let legacy = #"""
        {"agent":"codex","windows":[{"label":"周窗","usedPercent":1}],"fetchedAt":"2026-07-26T05:10:56Z"}
        """#
        let snap = try JSONDecoder().decode(
            AgentQuotaSnapshot.self, from: Data(legacy.utf8))
        XCTAssertNil(snap.producedAt)
        XCTAssertNil(snap.ageSeconds())
        XCTAssertFalse(snap.isStale())
        XCTAssertNil(snap.stalenessNote())
    }

    func testHumanAgeUsesOneMagnitude() {
        XCTAssertEqual(AgentQuotaSnapshot.humanAge(38 * 60), "38 分钟前")
        XCTAssertEqual(AgentQuotaSnapshot.humanAge(5 * 3600), "5 小时前")
        XCTAssertEqual(AgentQuotaSnapshot.humanAge(35 * 86_400), "35 天前")
    }
}
