#if os(macOS)
import Foundation
import XCTest

/// 菜单栏那个数字（P5b·B）。
///
/// 这个数字唯一的价值是**准**：人看到「3 件事在等你」点进去只找到 2 件，
/// 下一次他就不信它了 —— 而一个没人信的数字比没有更糟。所以这里钉的主要是
/// 「什么不算」，不是「什么算」。
final class HumanAttentionTallyTests: XCTestCase {

    func testQuietWhenNothingIsWaiting() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [], sessionStates: ["s1": "working", "s2": "idle"],
            unansweredTodos: 0)
        XCTAssertTrue(count.isQuiet)
        XCTAssertNil(count.badge, "没事的时候还挂个数字，人就学会了无视它")
        XCTAssertEqual(count.lines, [], "常年显示「待审批 0」会训练人忽略这一栏")
    }

    func testCountsTheThreeKinds() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: ["s1", "s2"],
            sessionStates: ["s3": "awaitingDecision", "s4": "working"],
            unansweredTodos: 4)
        XCTAssertEqual(count, HumanAttentionCount(approvals: 2, screenMenus: 1, todos: 4))
        XCTAssertEqual(count.total, 7)
        XCTAssertEqual(count.badge, "7")
    }

    /// 同一个 session 上的两个待审批**是两件事**（按条目数，不按 session 去重）。
    func testTwoApprovalsOnOneSessionAreTwoThings() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: ["s1", "s1"], sessionStates: [:], unansweredTodos: 0)
        XCTAssertEqual(count.approvals, 2)
    }

    /// 一个 session 既有待审批、又卡在屏幕的框上：**按人的角度是一件事**
    /// （他打开那个 session 就都看见了），别数两遍。
    func testASessionWithBothDoesNotGetCountedTwice() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: ["s1"],
            sessionStates: ["s1": "awaitingDecision"],
            unansweredTodos: 0)
        XCTAssertEqual(count.total, 1, "同一个 session 被数了两遍")
        XCTAssertEqual(count.screenMenus, 0)
    }

    /// **`awaitingReply` 不计。** 它的判定输入之一就是审批台账里本 session 的
    /// pending 条目 —— 一起加进来就是把同一件事数两遍。
    func testAwaitingReplyIsDeliberatelyNotCounted() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [],
            sessionStates: ["s1": "awaitingReply", "s2": "awaitingReply"],
            unansweredTodos: 0)
        XCTAssertTrue(count.isQuiet,
                      "awaitingReply 被计进来了 —— 它和待审批是同一件事的两个出口")
    }

    /// 别的状态一律不算「在等人」。**`idle` 尤其**：它的语义是「起来了、在等活」，
    /// 不是在等人。
    func testOtherStatesAreNotWaitingOnAHuman() {
        for state in ["working", "idle", "rateLimited", "error", "launchFailed", "exited"] {
            let count = HumanAttentionTally.tally(
                pendingApprovalSessionIds: [], sessionStates: ["s": state], unansweredTodos: 0)
            XCTAssertTrue(count.isQuiet, "\(state) 被当成了在等人拍板")
        }
    }

    /// 状态字面量必须是点名快照那一份，不是这里另抄一个。
    func testUsesTheRosterStateVocabulary() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [],
            sessionStates: ["s": CrewSessionStateDerivation.awaitingDecision],
            unansweredTodos: 0)
        XCTAssertEqual(count.screenMenus, 1)
    }

    func testSummaryOnlyMentionsWhatIsActuallyWaiting() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [], sessionStates: [:], unansweredTodos: 2)
        XCTAssertEqual(count.summary, "2 条 Todo 没回")
        XCTAssertFalse(count.summary.contains("待审批"))
    }
}
#endif
