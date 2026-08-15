#if os(macOS)
import XCTest

final class CrewDeferredWakeQueueTests: XCTestCase {
    private func delivery(
        _ source: String = "whiteboard:m1", target: String = "captain", text: String = "人：在吗"
    ) -> CrewDeferredWakeQueue.Delivery {
        .init(key: "\(source)|target:\(target)", targetSessionId: target, text: text)
    }

    /// 事故回归：busy 时只到一条消息，随后无第二条白板事件；仅 busy→idle
    /// 也必须产出一次补投，重复 idle / 重复扫描都不能再投。
    func testBusyMessageIsDeliveredOnceAfterIdleWithoutAnotherMessage() {
        var queue = CrewDeferredWakeQueue()
        XCTAssertEqual(queue.submit(delivery(), isBusy: true), .deferred)
        XCTAssertEqual(queue.pendingCount(sessionId: "captain"), 1)

        XCTAssertEqual(queue.popWhenIdle(sessionId: "captain"), delivery())
        XCTAssertNil(queue.popWhenIdle(sessionId: "captain"))
        XCTAssertEqual(queue.submit(delivery(), isBusy: false), .duplicate)
    }

    func testIdleMessageDeliversImmediately() {
        var queue = CrewDeferredWakeQueue()
        XCTAssertEqual(queue.submit(delivery(), isBusy: false), .deliver(delivery()))
        XCTAssertNil(queue.popWhenIdle(sessionId: "captain"))
    }

    func testPendingMessagesDrainOnePerIdleTransition() {
        var queue = CrewDeferredWakeQueue()
        XCTAssertEqual(queue.submit(delivery("m1"), isBusy: true), .deferred)
        XCTAssertEqual(queue.submit(delivery("m2", text: "第二条"), isBusy: true), .deferred)
        XCTAssertEqual(queue.popWhenIdle(sessionId: "captain")?.key, "m1|target:captain")
        XCTAssertEqual(queue.pendingCount(sessionId: "captain"), 1)
        XCTAssertEqual(queue.popWhenIdle(sessionId: "captain")?.key, "m2|target:captain")
    }

    func testSameMessageCanWakeDifferentTargetsOnceEach() {
        var queue = CrewDeferredWakeQueue()
        XCTAssertEqual(queue.submit(delivery("m1", target: "a"), isBusy: true), .deferred)
        XCTAssertEqual(queue.submit(delivery("m1", target: "b"), isBusy: true), .deferred)
        XCTAssertEqual(queue.popWhenIdle(sessionId: "a")?.targetSessionId, "a")
        XCTAssertEqual(queue.popWhenIdle(sessionId: "b")?.targetSessionId, "b")
    }
}
#endif
