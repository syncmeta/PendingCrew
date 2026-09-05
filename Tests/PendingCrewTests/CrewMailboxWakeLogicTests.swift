#if os(macOS)
import XCTest
// LocalRunner + CrewModels 直接编进 PendingCrewTests target（见 project.yml），无需 import。

/// 唤醒投递回执纯判定（`CrewMailboxWakeLogic.receiptVerdict` / `wakeFailureAlert`）
/// 的单测。本文件原来还钉着 edge mailbox 决策核心（`decide` / `renderInjection`）
/// 的六条，随 #63 第二期端掉跨端遥控整层一起删 —— 那条路的输入来自 edge
/// `getSessionInbox()`，本地白板永远产不出。
final class CrewMailboxWakeLogicTests: XCTestCase {

    // MARK: - 投递回执（wake-resilience：修「假送达」）

    func testReceiptConfirmedWhenAnySampleWorking() {
        // 注入后窗内任一拍见工作态 → 到达（短 turn 中途结束也不误判失败）。
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            workingSamples: [false, true, false]), .confirmed)
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            workingSamples: [true]), .confirmed)
    }

    func testReceiptFailedWhenWindowStaysQuiet() {
        // 整窗安静（卡模态菜单/进程假死只有注入瞬间回显，采样拍全 false）→ 失败。
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            workingSamples: [false, false, false]), .failed)
    }

    func testReceiptFailedOnNoSamples() {
        // 无采样（run 中途退出等）→ 无到达证据，按失败处理（宁重投不丢件）。
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(workingSamples: []), .failed)
    }

    func testReceiptConfirmedWhenCodexShortTurnFinishesBeforeFirstWorkingSample() {
        let baseline = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 40, latestPostId: "before")
        let afterFastTurn = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 44, latestPostId: "before")
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            baseline: baseline, samples: [afterFastTurn]), .confirmed)
    }

    func testReceiptConfirmedWhenTargetAlreadyPostedToCrewBeforeFirstSample() {
        let baseline = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 7, latestPostId: nil)
        let afterPost = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 7, latestPostId: "posted-by-target")
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            baseline: baseline, samples: [afterPost]), .confirmed)
    }

    func testReceiptStillFailsWhenNoConsumptionEvidenceChanges() {
        let quiet = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 12, latestPostId: "same-post")
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            baseline: quiet, samples: [quiet, quiet]), .failed)
    }

    // MARK: - 长静默 ≠ 卡死（压缩上下文被误判卡死；2026-09-05 人类实测）

    /// 相位对齐的假阴性：采样周期 1s、`isWorking` 判据窗口 1s、claude 状态行
    /// 也 1s 一跳 —— 三个 1 撞一起，相位固定时整窗都可能采在跳字之前。瞬时布尔
    /// 跨不过采样间隙，单调的「最近一次输出时刻」跨得过。
    func testReceiptConfirmedWhenOutputAdvancedButEveryWorkingSampleWasFalse() {
        let t0 = Date(timeIntervalSince1970: 1_000)
        let baseline = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 0, latestPostId: nil, lastOutputAt: t0)
        let quietLookingButAdvanced = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 0, latestPostId: nil,
            lastOutputAt: t0.addingTimeInterval(3))
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            baseline: baseline, samples: [quietLookingButAdvanced]), .confirmed)
    }

    func testReceiptStillFailsWhenOutputClockNeverAdvanced() {
        let t0 = Date(timeIntervalSince1970: 1_000)
        let quiet = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 3, latestPostId: "same", lastOutputAt: t0)
        XCTAssertEqual(CrewMailboxWakeLogic.receiptVerdict(
            baseline: quiet, samples: [quiet, quiet]), .failed)
    }

    /// claude 正在压缩上下文 / 长思考时终端上仍挂着忙碌指示 —— 那是「在做一件长的
    /// 不吐字的事」，不是卡死。窗到头也要继续等，别喊卡死、别把消息退回去重投。
    func testKeepsWaitingWhileTargetStillShowsBusyIndicator() {
        let busy = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 0, latestPostId: nil,
            lastOutputAt: Date(timeIntervalSince1970: 1_000), isBusyNow: true)
        XCTAssertTrue(CrewMailboxWakeLogic.shouldKeepWaiting(
            latest: busy, elapsed: CrewMailboxWakeLogic.receiptWindow))
        XCTAssertTrue(CrewMailboxWakeLogic.shouldKeepWaiting(latest: busy, elapsed: 120))
    }

    /// 没挂忙碌指示（卡模态菜单 / 进程假死）→ 窗到头就是到头，按老口径立刻判失败。
    func testStopsWaitingWhenNothingIndicatesWork() {
        let idle = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 0, latestPostId: nil,
            lastOutputAt: Date(timeIntervalSince1970: 1_000), isBusyNow: false)
        XCTAssertFalse(CrewMailboxWakeLogic.shouldKeepWaiting(
            latest: idle, elapsed: CrewMailboxWakeLogic.receiptWindow))
        XCTAssertTrue(CrewMailboxWakeLogic.shouldKeepWaiting(latest: idle, elapsed: 3))
    }

    /// 「它还挂着忙碌指示，我继续等」这句话有寿命 —— 挂着指示一直不动的也得有个头。
    func testBusyWaitHasALifetime() {
        let busy = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 0, latestPostId: nil,
            lastOutputAt: Date(timeIntervalSince1970: 1_000), isBusyNow: true)
        XCTAssertFalse(CrewMailboxWakeLogic.shouldKeepWaiting(
            latest: busy, elapsed: CrewMailboxWakeLogic.busyWaitLimit))
    }

    func testNoSampleYetKeepsWaitingUntilWindowCloses() {
        XCTAssertTrue(CrewMailboxWakeLogic.shouldKeepWaiting(latest: nil, elapsed: 3))
        XCTAssertFalse(CrewMailboxWakeLogic.shouldKeepWaiting(
            latest: nil, elapsed: CrewMailboxWakeLogic.receiptWindow))
    }

    func testClaudeBusyIndicatorReadFromScreenTail() {
        XCTAssertTrue(CrewMailboxWakeLogic.claudeIsBusy(
            screenTail: "✻ Compacting conversation… (12s · esc to interrupt)"))
        XCTAssertTrue(CrewMailboxWakeLogic.claudeIsBusy(
            screenTail: "* Thinking… (4s · ↑ 1.2k tokens · ESC to interrupt)"))
        XCTAssertFalse(CrewMailboxWakeLogic.claudeIsBusy(
            screenTail: "> \n? for shortcuts"))
        XCTAssertFalse(CrewMailboxWakeLogic.claudeIsBusy(screenTail: ""))
    }

    /// 两种处境不许共用一句话（账本：静默失效第五种穿法）。挂着忙碌指示等到寿命
    /// 上限的那条必须把依据和「可能是误报」写出来，别再一律喊「疑似卡死」。
    func testBusyStallAlertSaysWhatItWatchedAndAdmitsFalseAlarm() {
        let text = CrewMailboxWakeLogic.wakeBusyStallAlert(targetLabel: "机长", waited: 300)
        XCTAssertTrue(text.contains("机长"), text)
        XCTAssertTrue(text.contains("300"), text)
        XCTAssertTrue(text.contains("误报"), text)
        XCTAssertFalse(text.contains("疑似卡死"), text)
        XCTAssertNotEqual(text, CrewMailboxWakeLogic.wakeFailureAlert(targetLabel: "机长"))
    }

    func testWakeFailureAlertMentionsTargetAndSelfHealTools() {
        let text = CrewMailboxWakeLogic.wakeFailureAlert(targetLabel: "限额自愈")
        XCTAssertTrue(text.contains("限额自愈"), text)
        XCTAssertTrue(text.contains("留待重投"), text)
        XCTAssertTrue(text.contains("inspect_session"), text)
        XCTAssertTrue(text.contains("nudge_session"), text)
    }
}
#endif
