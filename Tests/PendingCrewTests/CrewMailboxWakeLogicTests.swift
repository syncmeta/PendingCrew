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

    func testWakeFailureAlertMentionsTargetAndSelfHealTools() {
        let text = CrewMailboxWakeLogic.wakeFailureAlert(targetLabel: "限额自愈")
        XCTAssertTrue(text.contains("限额自愈"), text)
        XCTAssertTrue(text.contains("留待重投"), text)
        XCTAssertTrue(text.contains("inspect_session"), text)
        XCTAssertTrue(text.contains("nudge_session"), text)
    }
}
#endif
