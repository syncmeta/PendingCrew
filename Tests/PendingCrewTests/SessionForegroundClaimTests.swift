import XCTest
// SessionForegroundClaim 直接编进 PendingCrewTests target，无需 import。

/// 「新起的 session 不许抢走你正开着的那个」（人类 Todo #42）。
///
/// 病根：`start()` 无条件 `selectedRunId = run.runID`。#481 只挡住了跨 crew 那半，
/// 同一个 crew 里机长 `start_session` 起的新 run 照样把右栏切走。
///
/// 这里钉的是唯一那条分界线：**这一下是不是人自己点的**。
final class SessionForegroundClaimTests: XCTestCase {

    private let openRun = UUID()
    private let rememberedRun = UUID()

    // MARK: - 该跳过去的

    func testUserInitiatedTakesForeground() {
        XCTAssertEqual(
            SessionForegroundClaim.decide(
                isForegroundCrew: true, userInitiated: true,
                foregroundSelection: openRun, paneSelection: nil),
            .selectForeground,
            "人在新建面板里提交的，就是要看它 —— 必须跳过去")
    }

    func testEmptyPaneTakesForegroundEvenWhenProgramStarted() {
        XCTAssertEqual(
            SessionForegroundClaim.decide(
                isForegroundCrew: true, userInitiated: false,
                foregroundSelection: nil, paneSelection: nil),
            .selectForeground,
            "右栏本来空白，没有正在看的东西可被打断 —— 选中它，否则一片空白")
    }

    // MARK: - 不许抢的（#42 的正身）

    func testProgramStartedDoesNotStealSameCrewForeground() {
        XCTAssertEqual(
            SessionForegroundClaim.decide(
                isForegroundCrew: true, userInitiated: false,
                foregroundSelection: openRun, paneSelection: nil),
            .leaveAlone,
            "机长 start_session / @唤醒拉起 在背后起的，不许切走用户正开着的 session")
    }

    func testProgramStartedDoesNotOverwriteBackgroundCrewMemory() {
        XCTAssertEqual(
            SessionForegroundClaim.decide(
                isForegroundCrew: false, userInitiated: false,
                foregroundSelection: openRun, paneSelection: rememberedRun),
            .leaveAlone,
            "后台 crew 记着的选中也别顶掉 —— 用户切回去时该看到他离开时那个")
    }

    // MARK: - 后台 crew：记住但不打断（#481 的语义原样保留）

    func testBackgroundCrewWithoutSelectionRemembersNewRun() {
        XCTAssertEqual(
            SessionForegroundClaim.decide(
                isForegroundCrew: false, userInitiated: false,
                foregroundSelection: openRun, paneSelection: nil),
            .rememberInPane,
            "别的 crew 起的 run 记进它自己的 pane 态，不动当前 crew 的右栏")
    }

    func testUserInitiatedInBackgroundCrewStillOnlyRemembers() {
        XCTAssertEqual(
            SessionForegroundClaim.decide(
                isForegroundCrew: false, userInitiated: true,
                foregroundSelection: openRun, paneSelection: rememberedRun),
            .rememberInPane,
            "人主动起的也不跨 crew 抢前台 —— 只更新那个 crew 记住的选中")
    }
}
