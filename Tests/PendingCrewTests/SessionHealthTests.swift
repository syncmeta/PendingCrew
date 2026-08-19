import XCTest
// SessionHealth + CodexProtocol 直接编进 PendingCrewTests target，无需 import。

/// runner 健康感知单测（批 C，分诊第 7 点）：
/// PTY 文案扫描（去 ANSI / 跨 chunk / 每 Kind 一次）+ codex account 事件分类。
final class SessionHealthTests: XCTestCase {

    private func feed(_ scanner: SessionHealthScanner, _ s: String) -> [CrewSessionHealth] {
        let bytes = Array(s.utf8)
        return scanner.feed(bytes[...])
    }

    // MARK: - 基本命中

    func testAuthPhraseFiresAuthRequiredOnce() {
        let sc = SessionHealthScanner()
        let hits = feed(sc, "Error: Not logged in. Run claude auth login to authenticate.\n")
        XCTAssertEqual(hits.map(\.kind), [.authRequired])
        // 同一故障 TUI 反复重绘 → 不再重复报。
        XCTAssertTrue(feed(sc, "Not logged in\n").isEmpty)
    }

    func testQuotaPhraseFiresUsageLimit() {
        let sc = SessionHealthScanner()
        let hits = feed(sc, "You've reached your Fable usage limit · resets 3pm\n")
        XCTAssertEqual(hits.map(\.kind), [.usageLimit])
    }

    func testRunLoginVariantsMatch() {
        for line in ["Please run /login to sign in.", "Run /login and retry."] {
            let sc = SessionHealthScanner()
            XCTAssertEqual(feed(sc, line).map(\.kind), [.authRequired], line)
        }
    }

    // MARK: - ANSI / 控制序列剥除

    func testAnsiSgrInsidePhraseIsStripped() {
        let sc = SessionHealthScanner()
        // 短语中间插 SGR 颜色码：run \e[1m/login\e[0m
        let hits = feed(sc, "Please run \u{1b}[1m/login\u{1b}[0m to authenticate")
        XCTAssertEqual(hits.map(\.kind), [.authRequired])
    }

    func testOscTitleSequenceIsStripped() {
        let sc = SessionHealthScanner()
        // OSC 设窗口标题（BEL 结尾）里出现的短语**不该**算命中——它被整段剥掉。
        XCTAssertTrue(feed(sc, "\u{1b}]0;run /login\u{07}normal output").isEmpty)
        // 但正文里的照常命中。
        XCTAssertEqual(feed(sc, "please run /login now").map(\.kind), [.authRequired])
    }

    // MARK: - 跨 chunk 拼接

    func testPhraseSplitAcrossChunksStillMatches() {
        let sc = SessionHealthScanner()
        XCTAssertTrue(feed(sc, "You've reached yo").isEmpty)
        XCTAssertEqual(feed(sc, "ur usage limit").map(\.kind), [.usageLimit])
    }

    // MARK: - 双 Kind / 无命中

    func testBothKindsInOneChunkFireBoth() {
        let sc = SessionHealthScanner()
        let hits = feed(sc, "Invalid API key\nCredit balance is too low\n")
        XCTAssertEqual(Set(hits.map(\.kind)), [.authRequired, .usageLimit])
    }

    func testOrdinaryOutputNoFalsePositive() {
        let sc = SessionHealthScanner()
        let hits = feed(sc, "Running tests… 263 passed.\n$ git status\nnothing to commit\n")
        XCTAssertTrue(hits.isEmpty)
    }

    // MARK: - rate-limit 模态菜单检测（Todo #10 层1）

    private func feedMenu(_ sc: RateLimitMenuScanner, _ s: String, now: Date) -> Bool {
        let bytes = Array(s.utf8)
        return sc.feed(bytes[...], now: now)
    }

    func testRateLimitMenuSlashCommandFires() {
        let sc = RateLimitMenuScanner()
        XCTAssertTrue(feedMenu(sc, "/rate-limit-options\n", now: Date(timeIntervalSince1970: 0)))
    }

    func testRateLimitMenuOptionTextFires() {
        let sc = RateLimitMenuScanner()
        XCTAssertTrue(feedMenu(
            sc, "What do you want to do?\n❯ 1. Stop and wait for limit to reset\n  2. Upgrade your plan\n",
            now: Date(timeIntervalSince1970: 0)))
    }

    func testRateLimitMenuAnsiWrappedStillFires() {
        let sc = RateLimitMenuScanner()
        // TUI 高亮选项：短语中间插 SGR 颜色码也要命中。
        XCTAssertTrue(feedMenu(
            sc, "❯ 1. \u{1b}[1mStop and wait\u{1b}[0m for limit to reset",
            now: Date(timeIntervalSince1970: 0)))
    }

    func testRateLimitMenuRedrawWithinCooldownDoesNotRefire() {
        let sc = RateLimitMenuScanner(cooldown: 30)
        let t0 = Date(timeIntervalSince1970: 0)
        XCTAssertTrue(feedMenu(sc, "Stop and wait for limit to reset", now: t0))
        // 同一菜单每帧重绘 → 冷却期内不再触发（不能对着 PTY 连打 Enter）。
        XCTAssertFalse(feedMenu(sc, "Stop and wait for limit to reset", now: t0.addingTimeInterval(5)))
        // 冷却期满菜单仍在重绘（按键没生效）→ 再次触发 = 内建重试。
        XCTAssertTrue(feedMenu(sc, "Stop and wait for limit to reset", now: t0.addingTimeInterval(31)))
    }

    func testRateLimitMenuOrdinaryOutputNoFalsePositive() {
        let sc = RateLimitMenuScanner()
        XCTAssertFalse(feedMenu(
            sc, "wait for the build to finish… rate limiting the API client\n",
            now: Date(timeIntervalSince1970: 0)))
    }

    // MARK: - codex account 事件分类

    func testCodexAccountRequestClassifiedAsAccount() {
        XCTAssertEqual(
            CodexProtocol.serverRequestKind(method: "account/chatgptAuthTokens/refresh"),
            .account)
        // 原有分类不受影响。
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "item/commandExecution/requestApproval"),
                       .approval)
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "attestation/generate"),
                       .unsupported)
    }
}

/// 「限额中」的恢复判定（机长 2026-07-26 实遇：换模型恢复干活后仍挂着 ⏳ 限额中，
/// 因为唯一的清除路径是几小时后的额度重置唤醒）。
final class QuotaHealthRecoveryTests: XCTestCase {
    private let limited = CrewSessionHealth(kind: .rateLimited, detail: "撞限额")
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    private func recovered(
        _ health: CrewSessionHealth?, at: Date?, since: Date?, after: TimeInterval
    ) -> Bool {
        QuotaHealthRecovery.recovered(
            health: health, quotaHealthAt: at, workingSince: since, now: t0.addingTimeInterval(after))
    }

    func testSustainedWorkAfterLimitClearsIt() {
        // 撞限额 → 隔 10s 开始干活 → 又连着干了 6s：恢复。
        XCTAssertTrue(recovered(limited, at: t0, since: t0.addingTimeInterval(10), after: 16))
    }

    func testShortBurstIsNotEnough() {
        // 只连着干了 3s —— 可能只是 TUI 重绘，不算恢复。
        XCTAssertFalse(recovered(limited, at: t0, since: t0.addingTimeInterval(10), after: 13))
    }

    /// 最要紧的一条：撞限额那一刻本来就在吐输出（正在打印限额报错），
    /// 那段 streak 起点早于撞限额时刻，绝不能当成「已恢复」。
    func testWorkStreakStartedBeforeTheLimitDoesNotCount() {
        XCTAssertFalse(recovered(limited, at: t0, since: t0.addingTimeInterval(-30), after: 60))
    }

    func testIdleSessionStaysLimited() {
        // 真被限额挡住的 session 停在提示符，没有 streak。
        XCTAssertFalse(recovered(limited, at: t0, since: nil, after: 3600))
    }

    func testNonQuotaHealthIsUntouched() {
        // 未登录 / 拉起失败不归这条判定管 —— 它们不会因为「在吐字」就自愈。
        for kind in [CrewSessionHealth.Kind.authRequired, .launchFailed] {
            XCTAssertFalse(recovered(CrewSessionHealth(kind: kind, detail: "x"),
                                     at: t0, since: t0.addingTimeInterval(1), after: 600))
        }
        XCTAssertFalse(recovered(nil, at: t0, since: t0.addingTimeInterval(1), after: 600))
    }

    func testUsageLimitRecoversToo() {
        // 撞墙那档（usageLimit）与卡菜单那档（rateLimited）同一套恢复语义。
        XCTAssertTrue(recovered(CrewSessionHealth(kind: .usageLimit, detail: "额度到顶"),
                                at: t0, since: t0.addingTimeInterval(1), after: 20))
    }

    /// 恢复后点名要如实显示「空闲/干活中」，不再是「限额中」。
    func testDerivedStateFollowsClearedHealth() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(isRunning: true, health: limited, isWorking: true),
            "rateLimited")
        XCTAssertEqual(
            CrewSessionStateDerivation.state(isRunning: true, health: nil, isWorking: true),
            "working")
    }

    // MARK: - ASCII 字节匹配（Todo #59）

    /// 字节匹配必须与老的 `tail.lowercased().contains(phrase)` 在**整张短语表**上
    /// 同判。这条同时覆盖大小写、弯引号多字节、跨 chunk 拼接三种情况。
    func testByteMatchingAgreesWithLowercasedContains() {
        let allPhrases = SessionHealthScanner.authPhrases
            + SessionHealthScanner.quotaPhrases
            + RateLimitMenuScanner.menuPhrases
        let noise = "⎿  · Thinking… 中文 tokens 12345\n  ✻ Welcome  "
        var cases: [String] = [noise, "", "完全无关的一段输出"]
        for p in allPhrases {
            cases.append(noise + p + noise)
            cases.append(noise + p.uppercased() + noise)      // 大小写不敏感
            cases.append(noise + p.prefix(p.count - 1) + noise)  // 差一个字符不该命中
        }
        for text in cases {
            let tail = AnsiPlainTextTail()
            tail.feed(Array(text.utf8)[...])
            let lower = tail.tail.lowercased()
            for p in allPhrases {
                XCTAssertEqual(
                    tail.containsLoweredASCII(AnsiPlainTextTail.loweredNeedle(p)),
                    lower.contains(p),
                    "短语 \(p.debugDescription) 在 \(text.prefix(40).debugDescription) 上判定不一致")
            }
        }
    }

    /// 镜像必须在**截窗之后**仍与 `tail` 指同一段内容 —— 否则截完就再也匹配不上。
    func testLoweredMirrorSurvivesTailTruncation() {
        let tail = AnsiPlainTextTail(tailLimit: 64)
        for _ in 0..<20 { tail.feed(Array("PADDING padding ".utf8)[...]) }
        tail.feed(Array("please Run /Login now".utf8)[...])
        XCTAssertTrue(tail.containsLoweredASCII(AnsiPlainTextTail.loweredNeedle("run /login")))
        XCTAssertEqual(Array(tail.tail.lowercased().utf8).count, tail.loweredASCII.count)
    }

    /// 整窗匹配的耗时红线：11 条短语 × 16K 尾窗，全在主线程、按 session 叠加。
    /// 老实现（每个扫描器 `lowercased()` 整窗 + grapheme 级 `contains`）在这个
    /// 尺寸上 -O 实测 10.2 ms/笔。预算松给，拦的是"改回全窗 String 搜索"。
    func testFullWindowMatchingStaysCheap() {
        let tail = AnsiPlainTextTail(tailLimit: 8192)
        var filler = ""
        while filler.count < 16000 { filler += "⎿  · Thinking… 中文一行 tokens 12345\n" }
        tail.feed(Array(filler.utf8)[...])
        let needles = SessionHealthScanner.authNeedles
            + SessionHealthScanner.quotaNeedles + RateLimitMenuScanner.menuNeedles
        let t0 = DispatchTime.now().uptimeNanoseconds
        for n in needles { _ = tail.containsLoweredASCII(n) }
        let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
        XCTAssertLessThan(ms, 40, "整窗匹配 \(String(format: "%.1f", ms)) ms —— 退回全窗 String 搜索了")
    }
}
