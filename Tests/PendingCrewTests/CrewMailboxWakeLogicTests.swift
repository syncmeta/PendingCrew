#if os(macOS)
import XCTest
// LocalRunner + CrewModels 直接编进 PendingCrewTests target（见 project.yml），无需 import。

/// 事件驱动唤醒纯决策核心（`CrewMailboxWakeLogic.decide`）的单测：
/// 「@我了 + 空闲 → 注入什么、mark 哪些」「busy → 不打断不消费」。
/// 活体 hub→拉 inbox→注入这圈 IO 留 E2E。
final class CrewMailboxWakeLogicTests: XCTestCase {

    private func item(
        id: String,
        status: String = "unread",
        senderKind: String = "user",
        senderSessionId: String? = nil,
        summary: String? = nil
    ) -> CrewMailboxItem {
        CrewMailboxItem(
            id: id, senderKind: senderKind, senderSessionId: senderSessionId,
            messageKind: "chat", summary: summary, status: status,
            createdAt: "2026-06-15T00:00:00Z", payload: nil)
    }

    // MARK: - busy short-circuit

    func testBusyRunNeverInjects() {
        let items = [item(id: "m1", summary: "@你 跑下测试")]
        XCTAssertEqual(CrewMailboxWakeLogic.decide(mailbox: items, isBusy: true), .noop,
                       "busy 时不打断、不消费 mailbox")
    }

    // MARK: - empty / nothing pending

    func testEmptyMailboxIsNoop() {
        XCTAssertEqual(CrewMailboxWakeLogic.decide(mailbox: [], isBusy: false), .noop)
    }

    func testAllDeliveredIsNoop() {
        let items = [
            item(id: "m1", status: "delivered", summary: "old"),
            item(id: "m2", status: "processed", summary: "older"),
        ]
        XCTAssertEqual(CrewMailboxWakeLogic.decide(mailbox: items, isBusy: false), .noop)
    }

    // MARK: - idle + pending → inject

    func testIdleWithPendingInjectsAndMarksThoseIds() {
        let items = [
            item(id: "m1", senderSessionId: "sess-abc123", summary: "帮我看下这个 bug"),
            item(id: "m2", status: "delivered", summary: "已读的旧消息"),
            item(id: "m3", senderKind: "user", summary: "顺便跑下 lint"),
        ]
        guard case let .inject(text, ids) = CrewMailboxWakeLogic.decide(mailbox: items, isBusy: false) else {
            return XCTFail("expected .inject")
        }
        // 只 mark 待处理的（m1, m3）—— 已 delivered 的 m2 不在内。
        XCTAssertEqual(ids, ["m1", "m3"])
        // 注入文本带「有人@你：」前导 + 两条内容 + 发送者标注。
        XCTAssertTrue(text.contains("有人@你："), "missing directed-message header: \(text)")
        XCTAssertTrue(text.contains("session:sess-a"), "session sender label missing: \(text)")
        XCTAssertTrue(text.contains("帮我看下这个 bug"))
        XCTAssertTrue(text.contains("顺便跑下 lint"))
        XCTAssertFalse(text.contains("已读的旧消息"), "delivered item must not be injected")
    }

    func testUnknownStatusTreatedAsPending() {
        // fail-open：未知/老 vocab status 当待处理，宁可多注入也别漏。
        let items = [item(id: "m9", status: "weird_legacy_status", summary: "别漏我")]
        guard case let .inject(_, ids) = CrewMailboxWakeLogic.decide(mailbox: items, isBusy: false) else {
            return XCTFail("expected .inject")
        }
        XCTAssertEqual(ids, ["m9"])
    }

    func testSenderLabelFallbacks() {
        let text = CrewMailboxWakeLogic.renderInjection([
            item(id: "a", senderKind: "human", summary: "h"),
            item(id: "b", senderKind: "captain", summary: "c"),
            item(id: "c", senderKind: "bot", summary: "b"),
        ])
        XCTAssertTrue(text.contains("人类: h"))
        XCTAssertTrue(text.contains("机长: c"))
        XCTAssertTrue(text.contains("bot: b"))
    }

    // MARK: - 项8：近期群聊上下文前置

    func testRecentContextPrependedBeforeDirectedHeader() {
        let recent = [
            LocalWhiteboardMessage(
                id: "w1", senderKind: "captain", senderUserId: nil, senderSessionId: nil,
                category: nil, text: "前情提要", createdAt: "2026-07-13T00:00:00Z"),
        ]
        let text = CrewMailboxWakeLogic.renderInjection(
            [item(id: "m1", summary: "帮我看下")], recent: recent)
        let ctxIdx = text.range(of: "近期群聊：")
        let dirIdx = text.range(of: "有人@你：")
        XCTAssertNotNil(ctxIdx, text)
        XCTAssertNotNil(dirIdx, text)
        XCTAssertTrue(ctxIdx!.lowerBound < dirIdx!.lowerBound, "近期群聊应前置于「有人@你」之前")
        XCTAssertTrue(text.contains("机长: 前情提要"), text)
    }

    func testEmptyRecentKeepsBareDirectedText() {
        // recent 缺省 → 与旧行为一致，不加上下文块。
        let text = CrewMailboxWakeLogic.renderInjection([item(id: "m1", summary: "x")])
        XCTAssertFalse(text.contains("近期群聊："))
        XCTAssertTrue(text.hasPrefix("有人@你："))
    }

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
