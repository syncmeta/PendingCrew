import XCTest
// SessionAwaitingReply / SessionTurnTrace 直接编进 PendingCrewTests target，无需 import。

/// 「session 在等人回复 → 界面标红」的判定单测（人类 Todo #25 层 2）。
///
/// 病根：session 在终端里问了句话就停住，界面上它跟正常空闲长得一模一样（都是 🟡）。
///
/// 这里钉两件事，**误报那侧的用例比该报那侧还重要** —— 一个老是标红的界面等于没有标红：
/// 1. 三条判据各自认得出（approval 挂着 / 终端菜单 / 收尾停在问句）。
/// 2. 认不错：还在吐字的、进程退了的、正文中间反问一句然后自己干完的，一个都不许红。
/// 3. 进得去也出得来 —— 四条解除路径逐条走一遍（#545 那个「限额中」进得去出不来的坑）。
final class SessionAwaitingReplyTests: XCTestCase {

    // MARK: - 该标红的

    func testPendingApprovalIsAwaiting() {
        let r = SessionAwaitingReply.reason(.init(
            isRunning: true, pendingApprovalSummary: "要不要直接改 main？"))
        XCTAssertEqual(r, .approval("要不要直接改 main？"))
    }

    func testPendingApprovalCountsEvenWhileTerminalStillAnimating() {
        // ask 阻塞期间 claude 的终端照样在转 spinner —— 那不是它在干活，是它在等。
        let r = SessionAwaitingReply.reason(.init(
            isRunning: true, pendingApprovalSummary: "选 A 还是 B？", isProducingOutput: true))
        XCTAssertEqual(r, .approval("选 A 还是 B？"))
    }

    func testTerminalMenuIsAwaiting() {
        let r = SessionAwaitingReply.reason(.init(
            isRunning: true, pendingMenuPrompt: "Do you want to proceed?"))
        XCTAssertEqual(r, .menu("Do you want to proceed?"))
    }

    func testTrailingQuestionIsAwaiting() {
        let r = SessionAwaitingReply.reason(.init(
            isRunning: true, trailingQuestion: "要我接着做 B 吗？"))
        XCTAssertEqual(r, .question("要我接着做 B 吗？"))
    }

    func testApprovalOutranksMenuAndQuestion() {
        // 同时成立时报最确凿的那条：阻塞态 > 画面态 > 推断态。
        let r = SessionAwaitingReply.reason(.init(
            isRunning: true, pendingApprovalSummary: "ask 的问题",
            pendingMenuPrompt: "菜单问句", trailingQuestion: "收尾问句"))
        XCTAssertEqual(r, .approval("ask 的问题"))
    }

    func testMenuOutranksQuestion() {
        let r = SessionAwaitingReply.reason(.init(
            isRunning: true, pendingMenuPrompt: "菜单问句", trailingQuestion: "收尾问句"))
        XCTAssertEqual(r, .menu("菜单问句"))
    }

    // MARK: - 不许标红的（误报那侧 —— 这几条才是这个功能能不能用的关键）

    func testIdleSessionIsNotAwaiting() {
        XCTAssertNil(SessionAwaitingReply.reason(.init(isRunning: true)),
                     "普通空闲不许红 —— 这正是「老是标红等于没标红」的第一道口子")
    }

    func testWorkingSessionIsNotAwaiting() {
        XCTAssertNil(SessionAwaitingReply.reason(.init(
            isRunning: true, isProducingOutput: true)),
            "正在吐字的不许红")
    }

    func testStillProducingOutputSuppressesTrailingQuestion() {
        XCTAssertNil(SessionAwaitingReply.reason(.init(
            isRunning: true, trailingQuestion: "要我接着做吗？", isProducingOutput: true)),
            "上一轮虽然停在问句上，但它已经又开口了 —— 话没说完谈不上等回复")
    }

    func testExitedSessionIsNotAwaiting() {
        XCTAssertNil(SessionAwaitingReply.reason(.init(
            isRunning: false, pendingApprovalSummary: "还挂着的问题",
            pendingMenuPrompt: "还画着的菜单", trailingQuestion: "还留着的问句")),
            "进程都没了，谈不上在等 —— 该是灰点，不是红点")
    }

    func testEmptyTrailingQuestionIsNotAwaiting() {
        XCTAssertNil(SessionAwaitingReply.reason(.init(isRunning: true, trailingQuestion: "")))
    }

    // MARK: - 四条解除路径（进得去必须出得来，#545 的教训）

    func testRecoversWhenApprovalAnswered() {
        var input = SessionAwaitingReply.Input(
            isRunning: true, pendingApprovalSummary: "要不要合 main？")
        XCTAssertNotNil(SessionAwaitingReply.reason(input))
        input.pendingApprovalSummary = nil          // 人答了 → 条目落 answered，不再 pending
        XCTAssertNil(SessionAwaitingReply.reason(input))
    }

    func testRecoversWhenMenuAnswered() {
        var input = SessionAwaitingReply.Input(isRunning: true, pendingMenuPrompt: "Proceed?")
        XCTAssertNotNil(SessionAwaitingReply.reason(input))
        input.pendingMenuPrompt = nil               // 菜单被按掉 → tracker 清
        XCTAssertNil(SessionAwaitingReply.reason(input))
    }

    func testRecoversWhenNudgedAndItStartsTalkingAgain() {
        var input = SessionAwaitingReply.Input(isRunning: true, trailingQuestion: "继续吗？")
        XCTAssertNotNil(SessionAwaitingReply.reason(input))
        input.isProducingOutput = true              // 被 nudge / 人回话 → 又开口了
        XCTAssertNil(SessionAwaitingReply.reason(input))
    }

    func testRecoversWhenProcessStops() {
        var input = SessionAwaitingReply.Input(isRunning: true, trailingQuestion: "继续吗？")
        XCTAssertNotNil(SessionAwaitingReply.reason(input))
        input.isRunning = false                     // 被 stop / 进程退出
        XCTAssertNil(SessionAwaitingReply.reason(input))
    }

    // MARK: - 收尾问句的抽取（严：只认最后一句）

    func testTrailingQuestionTakesTheLastSentence() {
        XCTAssertEqual(
            SessionTurnTrace.trailingQuestion(from: "我把 A 做完了。要不要接着做 B？"),
            "要不要接着做 B？")
    }

    /// 半角问号照认。注意切句**不认半角句点**（`1.5` / `main.swift` / `…` 会被切碎），
    /// 所以整段英文会当成一句 —— 判定仍然对（结尾是问号 = 在等），只是带的原文长一点。
    func testTrailingQuestionAcceptsAsciiQuestionMark() {
        XCTAssertEqual(
            SessionTurnTrace.trailingQuestion(from: "Done with A. Should I start B?"),
            "Done with A. Should I start B?")
    }

    /// 半角句点不切句的另一面：英文段落以句点收尾 → 整段不是问句 → 不红。**没有误报风险**，
    /// 这才是要紧的那侧。
    func testEnglishParagraphEndingInStatementIsNotAQuestion() {
        XCTAssertNil(SessionTurnTrace.trailingQuestion(
            from: "Should I proceed? I already started A, so this is just an FYI."))
    }

    func testTrailingQuestionRejectsMidTextRhetorical() {
        XCTAssertNil(
            SessionTurnTrace.trailingQuestion(from: "这样对吗？我先按 A 做了。"),
            "中途反问一句然后自己干完的不是在等人 —— 层 1 代发与层 2 标红共用这条判据，都不该捞")
    }

    func testTrailingQuestionRejectsPlainStatement() {
        XCTAssertNil(SessionTurnTrace.trailingQuestion(from: "都做完了，已经合进 main。"))
    }

    func testTrailingQuestionRejectsEmpty() {
        XCTAssertNil(SessionTurnTrace.trailingQuestion(from: "   \n  "))
    }

    func testTrailingQuestionTruncatesLongOnes() {
        let long = String(repeating: "很", count: 300) + "？"
        let q = SessionTurnTrace.trailingQuestion(from: long, limit: 20)
        XCTAssertEqual(q?.count, 21, "20 字 + 省略号")
        XCTAssertTrue(q?.hasSuffix("…") == true)
    }
}
