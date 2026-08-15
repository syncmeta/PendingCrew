import XCTest
// SessionLaunchParameterEcho.swift 随 LocalRunner 编进 PendingCrewTests target。

/// 「起来了，但启动参数没生效」的首屏判定（人类 Todo #36 的核心那半）。
///
/// 用的是 2026-08-09 在真 PTY 里抓到的 claude 2.1.226 原文，一字未改。
final class SessionLaunchParameterEchoTests: XCTestCase {

    /// 真实首屏：坏 model。注意 claude 把这句写在 TUI 接管之前，空格是在的；
    /// 判定仍走去空白匹配（TUI 接管后的行空格会丢，见 squeeze 的注释）。
    private let badModelScreen = """
        "totally-bogus-model" is not a model this version of Claude Code recognizes, so \
        auto-compact will keep this session within 200k tokens (the context window it assumes). \
        If the model accepts more, append [1m] to the model name for 1M.
        ────────────────────────────────
        Accessing workspace:
        """

    /// 真实首屏：坏 effort。CLI 自己写明了「ignoring it and using the default effort」——
    /// 这就是人类说的静默降级。
    private let badEffortScreen = """
        Warning: Unknown --effort value 'bogus-effort' — ignoring it and using the default \
        effort. Valid values: low, medium, high, xhigh, max.
        ────────────────────────────────
        """

    // MARK: - 命中

    func testBadModelIsDetected() throws {
        let hits = SessionLaunchParameterVerdict.classify(
            badModelScreen, model: "totally-bogus-model", effort: nil)
        XCTAssertEqual(hits.count, 1)
        guard case let .modelUnrecognized(value, quote) = try XCTUnwrap(hits.first) else {
            return XCTFail("应判成 modelUnrecognized")
        }
        XCTAssertEqual(value, "totally-bogus-model")
        XCTAssertTrue(quote.contains("is not a model"), "回执要带 CLI 原话：\(quote)")
    }

    func testBadEffortIsDetectedAsSilentDowngrade() throws {
        let hits = SessionLaunchParameterVerdict.classify(
            badEffortScreen, model: nil, effort: "bogus-effort")
        guard case let .effortIgnored(value, quote) = try XCTUnwrap(hits.first) else {
            return XCTFail("应判成 effortIgnored")
        }
        XCTAssertEqual(value, "bogus-effort")
        XCTAssertTrue(quote.contains("ignoring it"), quote)
        let detail = SessionLaunchParameterProblem.effortIgnored(value: value, quote: quote).detail
        XCTAssertTrue(detail.contains("静默降级"), "白板正文要点名静默降级：\(detail)")
    }

    /// `auto` 是最现实的一种：它在运行时完全合法，只有起 session 时会被降级。
    func testAutoEffortAtLaunchIsDetected() throws {
        let screen = "Warning: Unknown --effort value 'auto' — ignoring it and using the "
            + "default effort. Valid values: low, medium, high, xhigh, max."
        let hits = SessionLaunchParameterVerdict.classify(screen, model: nil, effort: "auto")
        guard case .effortIgnored = try XCTUnwrap(hits.first) else {
            return XCTFail("auto 在启动时被降级，必须报出来")
        }
    }

    func testBothKnobsCanFireTogether() {
        let hits = SessionLaunchParameterVerdict.classify(
            badModelScreen + "\n" + badEffortScreen,
            model: "totally-bogus-model", effort: "bogus-effort")
        XCTAssertEqual(hits.count, 2)
    }

    // MARK: - 不误报（这部分比命中更重要）

    func testNoModelPassedMeansNothingToCheck() {
        // 没显式传 model 就谈不上「传错」——即便屏幕上出现了那句话。
        XCTAssertTrue(SessionLaunchParameterVerdict
            .classify(badModelScreen, model: nil, effort: nil).isEmpty)
    }

    func testPhraseWithoutOurValueDoesNotFire() {
        // agent 自己在终端里聊到这句话（比如正在读这段代码）不该触发。
        // 判据是「短语 + 我们传出去的那个值同时出现」。
        XCTAssertTrue(SessionLaunchParameterVerdict
            .classify(badModelScreen, model: "opus", effort: nil).isEmpty,
            "我们传的是 opus，CLI 抱怨的是别的值 —— 不关我们的事")
        XCTAssertTrue(SessionLaunchParameterVerdict
            .classify(badEffortScreen, model: nil, effort: "high").isEmpty)
    }

    func testCleanStartupSaysNothing() {
        let normal = "╭──────────╮\n│ Claude Code v2.1.226 │\n╰──────────╯\n> "
        XCTAssertTrue(SessionLaunchParameterVerdict
            .classify(normal, model: "opus", effort: "high").isEmpty)
    }

    /// claude 的 TUI 排版靠光标移动，词间空格可能根本没进字节流 —— 压扁后仍要命中。
    func testMatchesEvenWhenTuiDropsSpaces() {
        let squeezed = "\"bogus\"isnotamodelthisversionofClaudeCoderecognizes,so..."
        XCTAssertEqual(SessionLaunchParameterVerdict
            .classify(squeezed, model: "bogus", effort: nil).count, 1)
    }

    // MARK: - 扫描器（时间窗 + 去重）

    private func feed(_ scanner: SessionLaunchParameterScanner, _ s: String,
                      now: Date = Date()) -> [SessionLaunchParameterProblem] {
        scanner.feed(ArraySlice(Array(s.utf8)), now: now)
    }

    func testScannerReportsEachKnobOnlyOnce() {
        let s = SessionLaunchParameterScanner(model: "totally-bogus-model", effort: nil)
        XCTAssertEqual(feed(s, badModelScreen).count, 1)
        XCTAssertEqual(feed(s, badModelScreen).count, 0, "同一类只报一次，别刷屏")
    }

    func testScannerIsIdleWhenNothingWasSpecified() {
        let s = SessionLaunchParameterScanner(model: nil, effort: "")
        XCTAssertTrue(s.isIdle)
        XCTAssertTrue(feed(s, badModelScreen).isEmpty)
    }

    func testScannerStopsAfterLaunchWindow() {
        let started = Date()
        let s = SessionLaunchParameterScanner(model: "totally-bogus-model", effort: nil,
                                              startedAt: started)
        let late = started.addingTimeInterval(SessionLaunchParameterScanner.window + 1)
        XCTAssertTrue(s.isExpired(now: late))
        XCTAssertTrue(feed(s, badModelScreen, now: late).isEmpty,
                      "过了拉起窗口就停扫 —— 长回合里偶然出现同款字眼不该误报")
    }

    func testScannerHandlesSplitChunks() {
        // PTY 是按块到的，短语可能被切成两半。
        let s = SessionLaunchParameterScanner(model: "totally-bogus-model", effort: nil)
        let mid = badModelScreen.index(badModelScreen.startIndex, offsetBy: 40)
        XCTAssertTrue(feed(s, String(badModelScreen[..<mid])).isEmpty)
        XCTAssertEqual(feed(s, String(badModelScreen[mid...])).count, 1)
    }
}
