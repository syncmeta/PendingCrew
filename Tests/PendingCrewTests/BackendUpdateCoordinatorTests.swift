#if os(macOS)
import Foundation
import XCTest

/// 换代的**执行层**。判定层（`BackendUpdatePlan`）已经单独测过，
/// 这里测的是接线本身 —— 尤其是那条最容易说谎的分支：
/// **想换代但旧后端停不掉时，绝不许返回「换过了」。**
///
/// （这个类型本来写在 `Sources/Mac/Services` 里，那儿进不了 test bundle。
/// 它用到的东西其实全都进得了，所以挪了过来 —— 判定有测试而接线没有，
/// 是这个仓库反复出现的形状。）
@MainActor
final class BackendUpdateCoordinatorTests: XCTestCase {

    private let mine = "0.1.34(1)"

    private func run(_ state: BackendUpdatePlan.BackendState,
                     stop: DaemonStopOutcome = .stopped(pid: 7),
                     logs: UnsafeMutablePointer<[String]>? = nil,
                     announced: UnsafeMutablePointer<[String]>? = nil,
                     stopCalls: UnsafeMutablePointer<Int>? = nil,
                     crews: [String] = ["c1", "c2"])
        -> BackendUpdatePlan.Decision {
        BackendUpdateCoordinator.runIfNeeded(
            appBuild: mine,
            log: { logs?.pointee.append($0) },
            probe: { state },
            stop: { stopCalls?.pointee += 1; return stop },
            affected: { crews },
            announce: { crewId, _ in announced?.pointee.append(crewId) })
    }

    /// **这条是这个文件的重点。** 停不掉却报「换过了」，调用方就会去问人
    /// 「要不要接回来」—— 而根本没断过。人点了「接回来」之后，
    /// 一个还在跑的 session 会被当成要恢复的对象。
    func testRefusesToClaimSuccessWhenTheOldBackendWillNotStop() {
        var stopCalls = 0
        let d = run(.running(build: "0.1.31(4)", pid: 42, sessionCount: 3),
                    stop: .refused("已经发过 SIGTERM，但 pid 42 在 8 秒内没有退出"),
                    stopCalls: &stopCalls)
        XCTAssertEqual(stopCalls, 1, "该去停它")
        XCTAssertFalse(d.shouldReplace, "停不掉却报「换过了」")
        guard case let .leaveAlone(why) = d else { return XCTFail() }
        XCTAssertTrue(why.contains("停不掉"), why)
        XCTAssertTrue(why.contains("SIGTERM"), "把 stopper 的原话吞了：\(why)")
    }

    func testReplacesWhenTheStopSucceeds() {
        var stopCalls = 0
        let d = run(.running(build: "0.1.31(4)", pid: 42, sessionCount: 3),
                    stopCalls: &stopCalls)
        XCTAssertTrue(d.shouldReplace)
        XCTAssertEqual(stopCalls, 1)
    }

    /// 不换的时候**一个副作用都不许有**：不停、不广播。
    func testLeavingAloneTouchesNothing() {
        for state: BackendUpdatePlan.BackendState in
            [.none, .undecidable("握手超时"), .running(build: "0.1.34(1)", pid: 1, sessionCount: 0)] {
            var stopCalls = 0
            var announced: [String] = []
            let d = run(state, logs: nil, announced: &announced, stopCalls: &stopCalls)
            XCTAssertFalse(d.shouldReplace, "\(state)")
            XCTAssertEqual(stopCalls, 0, "\(state)：不该换却去停了后端")
            XCTAssertEqual(announced, [], "\(state)：不该换却往群里广播了")
        }
    }

    /// **`leaveAlone` 也要落日志。** 一个永远换不了代的后台，跟一个每次启动都被
    /// 打断的后台一样糟，只是它安静。
    func testEveryOutcomeIsLoggedIncludingTheQuietOnes() {
        for state: BackendUpdatePlan.BackendState in
            [.none, .undecidable("握手超时"), .running(build: mine, pid: 1, sessionCount: 0)] {
            var logs: [String] = []
            _ = run(state, logs: &logs)
            XCTAssertFalse(logs.isEmpty, "\(state)：什么都没说就走了")
        }
    }

    /// 先说再停。反过来的话，人先看到 session 全断、几秒后才看到解释。
    func testAnnouncesBeforeStopping() {
        var order: [String] = []
        _ = BackendUpdateCoordinator.runIfNeeded(
            appBuild: mine, log: { _ in },
            probe: { .running(build: "0.1.31(4)", pid: 42, sessionCount: 2) },
            stop: { order.append("stop"); return .stopped(pid: 42) },
            affected: { ["c1"] },
            announce: { _, _ in order.append("announce") })
        XCTAssertEqual(order.first, "announce", "先停后说：人先看到断，再看到解释 —— 顺序反了")
    }

    /// 每个受影响的 crew 都要收到，一个不漏、也不重复。
    func testAnnouncesToEveryAffectedCrew() {
        var announced: [String] = []
        _ = run(.running(build: "0.1.31(4)", pid: 42, sessionCount: 2),
                announced: &announced, crews: ["c1", "c2", "c3"])
        XCTAssertEqual(announced, ["c1", "c2", "c3"])
    }

    /// **「有几个」和「通知谁」来自两个不同的源** —— 数目来自握手，名单来自盘上的
    /// registry。它们会不一致（registry 读不动 / 刚被清过），而那时 session 照断不误。
    /// 这条钉住：名单空而数目非零时**必须喊出来**，否则就是「打断了一批人、
    /// 一个都没通知」，且不留痕迹。
    func testShoutsWhenItWillInterruptSessionsButHasNobodyToTell() {
        var logs: [String] = []
        var announced: [String] = []
        _ = run(.running(build: "0.1.31(4)", pid: 42, sessionCount: 3),
                logs: &logs, announced: &announced, crews: [])
        XCTAssertEqual(announced, [])
        XCTAssertTrue(logs.contains { $0.contains("没有人会收到通知") },
                      "打断了 3 个 session 却没人被通知，日志里也一个字没有：\(logs)")
    }

    /// 反面：本来就没有 session 在跑时，不许喊这一嗓子（那会训练人忽略它）。
    func testDoesNotShoutWhenThereWasNothingToInterrupt() {
        var logs: [String] = []
        _ = run(.running(build: "0.1.31(4)", pid: 42, sessionCount: 0),
                logs: &logs, crews: [])
        XCTAssertFalse(logs.contains { $0.contains("没有人会收到通知") }, "\(logs)")
    }
}
#endif
