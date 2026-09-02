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

    /// 2026-08-30 根因红测：状态快照说 idle，但可控后端拒绝第一次 turn/start。
    /// 回调代表「可以推进消费游标」，只能在请求真的受理后执行；重试也只能受理一次。
    @MainActor
    func testRejectedTurnStartRemainsPendingAndIsConsumedOnlyAfterAcceptance() async {
        let backend = ControlledWakeBackend(results: [.retry, .accepted])
        var queue = CrewDeferredWakeQueue()
        let wake = delivery("whiteboard:168a4759")
        var consumed = 0

        guard case let .deliver(first) = queue.submit(wake, isBusy: false) else {
            return XCTFail("状态快照 idle 时应先尝试一次")
        }
        let firstResult = await backend.submitWake(first.text)
        if firstResult == .accepted { consumed += 1 }
        queue.resolve(first, as: firstResult)
        XCTAssertEqual(firstResult, .retry, "可控后端第一次应模拟 turn/start 拒绝")
        XCTAssertEqual(consumed, 0, "拒绝后不能推进消费游标")

        XCTAssertEqual(queue.pendingCount(sessionId: "captain"), 1,
                       "拒绝的 wake 必须回队列，而不能被 delivered 去重账吞掉")
        guard let retry = queue.popWhenIdle(sessionId: "captain") else {
            return XCTFail("无需第二条白板消息，idle 后必须取回原 wake")
        }
        let retryResult = await backend.submitWake(retry.text)
        if retryResult == .accepted { consumed += 1 }
        queue.resolve(retry, as: retryResult)

        XCTAssertEqual(backend.submissionCount, 2,
                       "第一次拒绝必须留在队列并自动重试，不等第二条白板消息")
        XCTAssertEqual(consumed, 1, "只有受理成功才能推进消费，而且只推进一次")
        XCTAssertEqual(backend.fireAndForgetSendCount, 0,
                       "wake 不能再绕回无回执的 send")
        XCTAssertEqual(queue.submit(wake, isBusy: false), .duplicate,
                       "受理后的同一 source id 只能投一次")
    }
}

@MainActor
private final class ControlledWakeBackend: SessionBackend {
    let kind: LocalCodingAgentKind = .codex
    @Published var status: SessionStatus = .running
    var statusPublisher: Published<SessionStatus>.Publisher { $status }
    var isBusy = false
    @Published var isWorking = false
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }
    @Published var health: CrewSessionHealth?
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { $health }

    private var results: [SessionWakeSubmission]
    private(set) var submissionCount = 0
    private(set) var fireAndForgetSendCount = 0

    init(results: [SessionWakeSubmission]) { self.results = results }

    func submitWake(_ text: String) async -> SessionWakeSubmission {
        submissionCount += 1
        return results.isEmpty ? .retry : results.removeFirst()
    }

    func send(_ text: String) { fireAndForgetSendCount += 1 }
    func interrupt() {}
    func stop() { status = .exited(nil) }
    func clearQuotaHealth() {}
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        .unsupported
    }
}
#endif
