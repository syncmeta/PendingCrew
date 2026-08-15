#if os(macOS)
import XCTest

/// 成员状态推导单测（#541）：事故的直接凶手是这条推导 —— 后端没起来但 status
/// 仍是 running、isWorking 为假 → 算出「空闲」，机长照常派活。这里钉死
/// 「拉起失败绝不落进 idle」以及各档的优先级。
final class CrewSessionStateDerivationTests: XCTestCase {

    private let launchFailed = CrewSessionHealth(kind: .launchFailed, detail: "没起来")
    private let authRequired = CrewSessionHealth(kind: .authRequired, detail: "没登录")
    private let rateLimited = CrewSessionHealth(kind: .rateLimited, detail: "限额中")

    // MARK: - 事故复现

    /// 事故本体：拉起失败的 session 不许报「空闲」——「空闲」的语义是
    /// 「起来了、在等活」，拿它当可派活的成员正是活石沉大海的原因。
    func testLaunchFailedNeverLooksIdle() {
        // 后端还没来得及翻 status（stalled：进程活着但零输出）—— 曾经的 idle 陷阱。
        let stalled = CrewSessionStateDerivation.state(
            isRunning: true, health: launchFailed, isWorking: false)
        XCTAssertEqual(stalled, CrewSessionStateDerivation.launchFailed)
        XCTAssertNotEqual(stalled, "idle")
    }

    /// 拉起失败优先于「已退出」：两者都不 running，但语义差得远 ——
    /// 「跑完了」vs「从来没跑起来」，机长要据后者立刻改派。
    func testLaunchFailedBeatsExited() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: false, health: launchFailed, isWorking: false),
            CrewSessionStateDerivation.launchFailed)
    }

    // MARK: - 既有档位不被破坏

    func testExitedWinsOverOtherHealthKinds() {
        // 已退出 + 陈旧的健康异常 → 仍报已退出（既有行为）。
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: false, health: authRequired, isWorking: false),
            "exited")
    }

    func testQuotaHealthIsRateLimitedAndOtherHealthIsError() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: rateLimited, isWorking: false),
            "rateLimited")
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: authRequired, isWorking: false),
            "error")
    }

    /// 健康的 session 才分 working / idle —— idle 在这里才是诚实的。
    func testHealthyRunSplitsWorkingAndIdle() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(isRunning: true, health: nil, isWorking: true),
            "working")
        XCTAssertEqual(
            CrewSessionStateDerivation.state(isRunning: true, health: nil, isWorking: false),
            "idle")
    }

    // MARK: - 待决策 / 待回复排在 working·idle 前面（Todo #6 / #25 层 2）

    /// 卡在终端菜单上、或说完停在问句上的 session 都不吐输出 → `isWorking` 为假，
    /// 落进 idle 就等于告诉机长「它闲着，可以派活」——活会石沉大海。
    func testAwaitingStatesNeverLookIdle() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false, awaitingDecision: true),
            CrewSessionStateDerivation.awaitingDecision)
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false, awaitingReply: true),
            CrewSessionStateDerivation.awaitingReply)
    }

    /// 菜单更具体、机长 nudge 发个数字就能代答 —— 同时成立时报菜单那条。
    func testAwaitingDecisionBeatsAwaitingReply() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false,
                awaitingDecision: true, awaitingReply: true),
            CrewSessionStateDerivation.awaitingDecision)
    }

    /// 「干不了活」比「在等回话」更要紧：撞限额 / 没登录时报那条，别被待回复盖掉。
    func testHealthBeatsAwaitingReply() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: rateLimited, isWorking: false, awaitingReply: true),
            "rateLimited")
    }

    /// 退出了就不再「在等」——残留的待回复不许把灰点谎报成红点。
    func testExitedBeatsAwaitingReply() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: false, health: nil, isWorking: false, awaitingReply: true),
            "exited")
    }

    // MARK: - 机长点名读到的那行

    /// 快照渲染要把「拉起失败」说成人话 + 给出下一步，别只丢个状态词。
    func testRosterSpellsOutLaunchFailure() {
        var snap = CrewSessionsSnapshot()
        snap.crews["c1"] = [CrewSessionsSnapshot.Entry(
            sessionId: "worker-dead", name: "没起来的活", role: "worker", brief: "修个 bug",
            state: CrewSessionStateDerivation.launchFailed,
            healthDetail: "Claude Code 子进程没能启动")]
        let line = snap.renderRoster(crewId: "c1")
        XCTAssertTrue(line.contains("拉起失败"))
        XCTAssertTrue(line.contains("Claude Code 子进程没能启动"), "原因要留痕可读")
        XCTAssertTrue(line.contains("改派"), "要告诉机长下一步")
        XCTAssertFalse(line.contains("空闲"), "绝不能同时被读成空闲")
    }
}
#endif
