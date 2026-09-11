#if os(macOS)
import Foundation
import XCTest

/// 「要不要问人恢复上次的 session」。
///
/// 人类的规格只有两条（只在「刚更新过」或「上次意外结束」时问、问了才恢复），
/// 但**判定层真正容易错的是第三条**：没东西可恢复时一次都不许问。
/// 一个会在没事时出现的提示，等于把有事时的那次也一起废掉。
final class SessionRestoreOfferTests: XCTestCase {

    private let one = [SessionRestoreOffer.Candidate(sessionId: "s1", crewId: "c1")]
    private let two = [SessionRestoreOffer.Candidate(sessionId: "s1", crewId: "c1"),
                       SessionRestoreOffer.Candidate(sessionId: "s2", crewId: "c2")]

    private func decide(_ exit: ProcessExitClassification,
                        previous: String? = "0.1.34(1)",
                        current: String = "0.1.34(1)",
                        candidates: [SessionRestoreOffer.Candidate]? = nil)
        -> SessionRestoreOffer.Decision {
        SessionRestoreOffer.decide(exit: exit, previousBuild: previous,
                                   currentBuild: current, candidates: candidates ?? one)
    }

    // MARK: - 该问的两种

    func testAsksAfterAnUnexpectedExit() {
        let d = decide(.unexpected)
        XCTAssertTrue(d.shouldAsk)
        XCTAssertEqual(d.reason, .unexpectedExit(.unexpected))
    }

    func testAsksAfterDyingWhileDraining() {
        XCTAssertTrue(decide(.diedWhileDraining).shouldAsk)
    }

    /// 「刚更新过」那一档：上一轮正常退出，但版本变了。
    func testAsksAfterAnUpdateEvenThoughTheExitWasClean() {
        let d = decide(.clean, previous: "0.1.33(9)", current: "0.1.34(1)")
        XCTAssertTrue(d.shouldAsk)
        XCTAssertEqual(d.reason, .justUpdated(from: "0.1.33(9)", to: "0.1.34(1)"))
    }

    // MARK: - 不该问的几种

    func testNeverAsksAfterACleanExitOnTheSameBuild() {
        XCTAssertFalse(decide(.clean).shouldAsk)
    }

    func testNeverAsksOnAFirstEverRun() {
        XCTAssertFalse(decide(.noPriorRun, previous: nil).shouldAsk)
    }

    /// **这条是这个文件的重点。** 崩溃时手上一个 session 都没有是常事
    /// （刚开着窗什么也没干）。那种时候弹「要恢复上次的 session 吗」，
    /// 人点进去发现什么都没有 —— 弹两次他就再也不看这个窗了，
    /// 于是真有事的那一次也一起废掉。
    func testNeverAsksWhenThereIsNothingToRestore() {
        for exit: ProcessExitClassification in [.unexpected, .diedWhileDraining, .clean] {
            let d = decide(exit, previous: "0.1.33(9)", current: "0.1.34(1)", candidates: [])
            XCTAssertFalse(d.shouldAsk, "\(exit)：没东西可恢复却还是弹窗了")
            XCTAssertNil(d.reason)
            XCTAssertTrue(d.message.isEmpty, "不问的时候不该有正文")
        }
    }

    /// 没有上一轮版本（老数据根第一次带印记跑）时，不许把 nil 当成「版本变了」。
    func testAMissingPreviousBuildIsNotAnUpdate() {
        XCTAssertFalse(decide(.clean, previous: nil).shouldAsk,
                       "读不到上一轮版本被当成了「刚更新过」—— 正常退出也会被打扰")
    }

    // MARK: - 两者同时成立时，说哪一个

    /// 更新之后那一轮又崩了：**该告诉他「崩了」，不是「更新了」**。
    /// 说成「更新了」会让他以为这是预期内的，从而漏掉一次真故障。
    func testCrashWinsOverUpdateWhenBothHold() {
        let d = decide(.unexpected, previous: "0.1.33(9)", current: "0.1.34(1)")
        XCTAssertEqual(d.reason, .unexpectedExit(.unexpected))
        XCTAssertFalse(d.message.contains("更新到"), "两者同时成立时报成了「更新」：\(d.message)")
    }

    // MARK: - 正文

    /// 弹窗正文要说清三件事：发生了什么、有几个、点「不恢复」会怎样。
    func testMessageSaysWhatHappenedHowManyAndWhatHappensIfYouDecline() {
        let d = decide(.unexpected, candidates: two)
        XCTAssertTrue(d.message.contains(ProcessExitClassification.unexpected.text),
                      "没说发生了什么：\(d.message)")
        XCTAssertTrue(d.message.contains("2 个"), "没说有几个：\(d.message)")
        XCTAssertTrue(d.message.contains("@"), "没说不恢复之后还能怎么办：\(d.message)")
    }

    func testUpdateMessageNamesBothVersions() {
        let d = decide(.clean, previous: "0.1.33(9)", current: "0.1.34(1)")
        XCTAssertTrue(d.message.contains("0.1.33(9)"), d.message)
        XCTAssertTrue(d.message.contains("0.1.34(1)"), d.message)
    }

    // MARK: - 我们自己换掉后端之后那一次

    /// **打断了就必须问。** 只有后端落后（app 没更新）时，`decide` 会判成
    /// 「正常退出 + 同版」→ 不问；可我们刚刚亲手把他的 session 打断了。
    /// 打断了却不问，是这条链上最难查的那种沉默。
    func testAsksAfterWeOurselvesReplacedTheBackend() {
        let same = decide(.clean)          // 这一档本来不问
        XCTAssertFalse(same.shouldAsk)

        let d = SessionRestoreOffer.afterBackendReplaced(
            oldBuild: "0.1.31(4)", newBuild: "0.1.34(1)", candidates: two)
        XCTAssertTrue(d.shouldAsk, "我们打断了他的 session 却不问")
        XCTAssertEqual(d.reason, .justUpdated(from: "0.1.31(4)", to: "0.1.34(1)"))
        XCTAssertEqual(d.candidates, two)
    }

    /// 但「没东西可恢复就不问」这条仍然管着。
    func testBackendReplacedWithNothingRunningStillDoesNotAsk() {
        XCTAssertFalse(SessionRestoreOffer.afterBackendReplaced(
            oldBuild: "a", newBuild: "b", candidates: []).shouldAsk)
    }

    func testCandidatesComeBackForTheCaller() {
        XCTAssertEqual(decide(.unexpected, candidates: two).candidates, two)
    }
}

/// 恢复跑完之后那句话。**这个类型存在的唯一理由是不让失败被包装成成功。**
final class SessionRestoreOutcomeTests: XCTestCase {

    private func fail(_ id: String) -> SessionRestoreOutcome.Failure {
        .init(sessionId: id, crewId: "c", reason: "No conversation found with session ID: x")
    }

    func testNothingToDoSaysSo() {
        XCTAssertEqual(SessionRestoreOutcome().summary, "没有需要接回的 session。")
    }

    func testAllRestoredSaysHowMany() {
        let o = SessionRestoreOutcome(restored: ["a", "b"])
        XCTAssertTrue(o.allSucceeded)
        XCTAssertEqual(o.summary, "已接回 2 个 session。")
    }

    /// **有失败就必须点名**，不许只给个数字 —— 人得知道是哪几个才去看。
    func testPartialFailureNamesTheOnesThatFailed() {
        let o = SessionRestoreOutcome(restored: ["a"], failures: [fail("b"), fail("c")])
        XCTAssertFalse(o.allSucceeded)
        XCTAssertTrue(o.summary.contains("b"), o.summary)
        XCTAssertTrue(o.summary.contains("c"), o.summary)
        XCTAssertTrue(o.summary.contains("2 个没接回来"), o.summary)
    }

    /// **全失败时一个字都不许出现「已接回」。**
    func testTotalFailureNeverClaimsAnythingWasRestored() {
        let o = SessionRestoreOutcome(failures: [fail("a"), fail("b")])
        XCTAssertFalse(o.summary.contains("已接回"), "全失败却说「已接回」：\(o.summary)")
        XCTAssertTrue(o.summary.contains("都没接回来"), o.summary)
    }

    /// 失败原话要留着，不许被改写成「失败」 —— 调用方要把它原样送进白板。
    func testFailureKeepsTheOriginalWording() {
        let f = fail("a")
        XCTAssertTrue(f.reason.contains("No conversation found"))
    }
}
#endif
