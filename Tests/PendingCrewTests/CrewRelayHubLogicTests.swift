#if os(macOS)
import XCTest
// LocalRunner 源码直接编进 PendingCrewTests target（见 project.yml），无需 import。

/// hub 事件驱动 relay 拉取（CC-P4）的纯核心单测：
/// - `reconcile`：relay 绑定集合 vs 已开 hub 连接集合 → 该开/该关哪些。
/// - `CrewHubPullCoalescer`：hub 事件风暴下的拉取去重 —— 同 crew 一次只有一个
///   拉取在飞；飞行中到达的事件合并成「结束后再拉一次」，不排队不丢。
final class CrewRelayHubLogicTests: XCTestCase {

    // MARK: - reconcile（连接对账）

    func testReconcileOpensNewAndClosesRemoved() {
        let r = CrewRelayHubLogic.reconcile(
            desired: ["a", "b", "c"], connected: ["b", "d"])
        XCTAssertEqual(r.open, ["a", "c"])
        XCTAssertEqual(r.close, ["d"])
    }

    func testReconcileNoopWhenSetsMatch() {
        let r = CrewRelayHubLogic.reconcile(desired: ["a"], connected: ["a"])
        XCTAssertTrue(r.open.isEmpty)
        XCTAssertTrue(r.close.isEmpty)
    }

    func testReconcileClosesAllWhenNothingDesired() {
        let r = CrewRelayHubLogic.reconcile(desired: [], connected: ["a", "b"])
        XCTAssertTrue(r.open.isEmpty)
        XCTAssertEqual(r.close, ["a", "b"])
    }

    // MARK: - CrewHubPullCoalescer（拉取合并去重）

    func testIdleTriggerStartsImmediately() {
        var c = CrewHubPullCoalescer()
        XCTAssertTrue(c.shouldStart())
    }

    func testTriggerWhileRunningIsCoalescedNotStarted() {
        var c = CrewHubPullCoalescer()
        XCTAssertTrue(c.shouldStart())
        // 飞行中的第二、三个事件都不再起新拉取。
        XCTAssertFalse(c.shouldStart())
        XCTAssertFalse(c.shouldStart())
    }

    func testFinishWithPendingRunsExactlyOneMore() {
        var c = CrewHubPullCoalescer()
        XCTAssertTrue(c.shouldStart())
        XCTAssertFalse(c.shouldStart())   // 飞行中来了事件 → 合并
        XCTAssertFalse(c.shouldStart())   // 再来一个 → 仍合并成一次
        XCTAssertTrue(c.didFinish())      // 结束 → 立刻补拉一次
        XCTAssertFalse(c.didFinish())     // 补拉结束、无新事件 → 歇
    }

    func testFinishWithoutPendingGoesIdle() {
        var c = CrewHubPullCoalescer()
        XCTAssertTrue(c.shouldStart())
        XCTAssertFalse(c.didFinish())
        // 回到 idle 后，下个事件重新起飞。
        XCTAssertTrue(c.shouldStart())
    }
}
#endif
