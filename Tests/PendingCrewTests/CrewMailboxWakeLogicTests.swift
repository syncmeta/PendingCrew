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

    // MARK: - busy 那一支到底触发得了吗（本机全历史 0 次，账本里挂着）

    /// `wakeBusyStallAlert` 在全机 47 个白板的全部历史里**一次都没出现过**
    /// （2026-09-07 实测；同期 `wakeFailureAlert` 有 112 次，所以不是统计口径的事）。
    ///
    /// 「从没触发过」有两个完全不同的意思，那个读数分不开：
    /// ① 它防的情况真没发生过；② **它根本触发不了**（判据写错，永远进不去）。
    /// 一个从不发声的东西看起来像「没问题」，实际可能已经退出检测器行列了。
    ///
    /// 下面四条是账本里点名要的那次**构造实验**，一起回答「② 在纯判定这一层成不成立」。
    /// 结论写在最后一条上面。**它们不回答 ①**，也不假装回答。

    /// 收摊时挂着忙碌指示 ⇒ 出来的必须是 busy 那一句。
    /// 选择本身 2026-09-12 之前长在 `CrewSessionRunner` 的一个三元表达式里，
    /// 那个文件不进 test bundle —— 所以这一条在此之前**没有尺子量得到**。
    func test_收摊时还挂着忙碌指示_出来的是busy那一句() {
        let busy = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 7, latestPostId: "p1",
            lastOutputAt: Date(timeIntervalSince1970: 1_000), isBusyNow: true)
        let text = CrewMailboxWakeLogic.unconfirmedAlert(
            latest: busy, targetLabel: "机长", waited: 300)
        XCTAssertEqual(text, CrewMailboxWakeLogic.wakeBusyStallAlert(
            targetLabel: "机长", waited: 300),
            "挂着忙碌指示还喊「疑似卡死」——人会去 nudge 一个正在跑的 session：\(text)")
    }

    /// 反面：什么都没挂（以及一拍都没采到）时仍然是老那一句。
    /// 没有这一条，一个**永远**返回 busy 的实现也会让上面那条绿。
    func test_什么都没挂时_出来的是卡死那一句() {
        let idle = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 7, latestPostId: "p1",
            lastOutputAt: Date(timeIntervalSince1970: 1_000), isBusyNow: false)
        for latest in [idle, nil] {
            let text = CrewMailboxWakeLogic.unconfirmedAlert(
                latest: latest, targetLabel: "机长", waited: 300)
            XCTAssertEqual(text, CrewMailboxWakeLogic.wakeFailureAlert(targetLabel: "机长"),
                           "latest=\(String(describing: latest))：\(text)")
        }
    }

    /// **这一条是整组的关键**：忙碌指示本身**不算**到达证据。
    ///
    /// 若哪天有人把 `isBusyNow` 加进 `receiptVerdict` 的或运算里，一个挂着指示的
    /// 目标就会被判成 confirmed，于是 busy 那一支**永远选不中** —— 变成解释 ②，
    /// 而且盘上不会留下任何痕迹（它本来就是 0 次，谁也看不出少了什么）。
    /// 这条断言是那一刀唯一会碰到的东西。
    func test_忙碌指示本身永远不算到达证据() {
        let t = Date(timeIntervalSince1970: 1_000)
        let baseline = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 7, latestPostId: "p1",
            lastOutputAt: t, isBusyNow: false)
        // 整窗每一拍都只有 isBusyNow 跟基线不同，别的逐字相同。
        let samples = (0..<5).map { _ in
            CrewMailboxWakeLogic.ReceiptEvidence(
                isWorking: false, activityRevision: 7, latestPostId: "p1",
                lastOutputAt: t, isBusyNow: true)
        }
        XCTAssertEqual(
            CrewMailboxWakeLogic.receiptVerdict(baseline: baseline, samples: samples), .failed,
            "忙碌指示被当成了到达证据 —— busy 告警那一支从此永远选不中，而它本来就 0 次，没人看得出来")
    }

    /// 等待的寿命到头时**确实会收摊**（而不是挂着指示就无限等下去）。
    /// 连上上面三条，「② 在纯判定这一层」就被排除了：等得到头、选得中、
    /// 而且忙碌本身不会把它先判成 confirmed。
    ///
    /// ⚠️ **剩下的那半仍然没被证明**，别把这一组读成「已经确认是 ①」：
    /// 真实世界里 `isBusyNow` 能不能在 `lastOutputAt` 一动不动的同时挂满 300 秒，
    /// 这里量不到 —— 那要一次真实现场。
    func test_挂着忙碌指示也等得到头_不会无限等下去() {
        let busy = CrewMailboxWakeLogic.ReceiptEvidence(
            isWorking: false, activityRevision: 7, latestPostId: "p1",
            lastOutputAt: Date(timeIntervalSince1970: 1_000), isBusyNow: true)
        let limit = CrewMailboxWakeLogic.busyWaitLimit
        XCTAssertTrue(CrewMailboxWakeLogic.shouldKeepWaiting(latest: busy, elapsed: limit - 1),
                      "上限之前就收摊了，那条「长静默 ≠ 卡死」的让步等于没有")
        XCTAssertFalse(CrewMailboxWakeLogic.shouldKeepWaiting(latest: busy, elapsed: limit),
                       "挂着忙碌指示就无限等下去 —— busy 告警永远发不出来")
    }
}
#endif
