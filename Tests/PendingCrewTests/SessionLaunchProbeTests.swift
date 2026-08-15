#if os(macOS)
import XCTest

/// 拉起自检单测（#541）：复现事故那条「进程没起来却报空闲」的判定路径，
/// 并钉住三类失败与「正常」的边界。纯逻辑，不起真进程。
final class SessionLaunchProbeTests: XCTestCase {

    private let deadline: TimeInterval = 25

    // MARK: - 事故复现

    /// 事故本体：fork/exec 没成功 —— 无论过多久、无论怎么观测，都必须立刻判
    /// spawnFailed（此前这里什么都不判，状态停在 running → 点名算出「空闲」）。
    func testSpawnFailedIsImmediateAndBeatsEverythingElse() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: false, processAlive: false, everAlive: false, sawOutput: false,
                elapsed: 0, deadline: deadline),
            .spawnFailed)
        // 即便观察窗还没到、即便诡异地「没 spawn 却报有输出」，也不能装没事。
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: false, processAlive: true, everAlive: true, sawOutput: true,
                elapsed: 0, deadline: deadline),
            .spawnFailed)
    }

    /// 起来了但秒退、退出回调丢了（SwiftTerm 的事件源 activate 早于 setEventHandler
    /// 的竞态；额度耗尽的 CLI 启动即退正好踩中）—— 得自己认账，不能等一个永不到的回调。
    /// 前提是**观测到它活过**（`everAlive`），否则见下面那条误报回归。
    func testDiedSilentlyWhenProcessGoneWithoutExitCallback() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: false, everAlive: true, sawOutput: false,
                elapsed: 0.5, deadline: deadline),
            .diedSilently)
    }

    // MARK: - 误报回归（codex「启动后立刻退出」其实活得好好的）

    /// **本探针自己造的假警报**：codex 后端的看门狗 Task 排在跑 `connection.start()`
    /// 的 Task 之前，第一轮问 `isProcessRunning` 时 `Process.run()` 还没执行，答案
    /// 恒为 false。旧判定「spawned 且进程不在 = diedSilently」于是把**每一个** codex
    /// session 刚拉起就判成「启动后立刻退出」，而进程随后好好跑完一整场
    /// （2026-07-26：被报拉起失败的 session 12 秒后正常发了群消息）。
    /// 没观测到活过 ≠ 死了 —— 观察窗内一律 pending。
    func testNotYetSpawnedIsNotDeath() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: false, everAlive: false, sawOutput: false,
                elapsed: 0, deadline: deadline),
            .pending,
            "第一轮还没 fork 完就报死 = 误报，会让人去重起一个正在干活的 session")
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: false, everAlive: false, sawOutput: false,
                elapsed: deadline - 0.01, deadline: deadline),
            .pending)
    }

    /// 但也不能无限宽容：到截止时刻还从没见它活过 = 压根没起来，照样 fail-loud
    /// （用 spawnFailed 而非 diedSilently —— 它从来没活过，文案不该说「启动后退出」）。
    func testNeverAliveBecomesSpawnFailedAtDeadline() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: false, everAlive: false, sawOutput: false,
                elapsed: deadline, deadline: deadline),
            .spawnFailed)
    }

    /// 观测过活着的进程后来没了，仍然是 diedSilently —— 别为修误报把真死讯也放跑。
    func testEverAliveStillReportsDeathAfterDeadline() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: false, everAlive: true, sawOutput: false,
                elapsed: deadline + 10, deadline: deadline),
            .diedSilently)
    }

    // MARK: - 半死兜底

    /// 进程活着但零输出：截止时刻**之前**继续观察（别把冷启动误报成故障）。
    func testAliveButSilentStaysPendingBeforeDeadline() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: true, everAlive: true, sawOutput: false,
                elapsed: deadline - 0.01, deadline: deadline),
            .pending)
    }

    /// 到点仍零输出 → 半死，必须报（要求 3 的兜底）。边界取 `>=`。
    func testAliveButSilentBecomesStalledAtDeadline() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: true, everAlive: true, sawOutput: false,
                elapsed: deadline, deadline: deadline),
            .stalled)
    }

    // MARK: - 正常路径不许误报

    /// 见过输出 = 确定活着，自检收工；此后进程退出归正常退出回调管，
    /// 不能被自检倒打一耙报成「拉起失败」。
    func testSawOutputIsAliveEvenIfProcessLaterGone() {
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: true, everAlive: true, sawOutput: true,
                elapsed: 0.1, deadline: deadline),
            .alive)
        XCTAssertEqual(
            SessionLaunchProbe.verdict(
                spawned: true, processAlive: false, everAlive: true, sawOutput: true,
                elapsed: 999, deadline: deadline),
            .alive)
    }

    // MARK: - 终局判定 + 文案

    func testTerminalVerdictsStopTheProbe() {
        XCTAssertFalse(SessionLaunchProbe.isTerminal(.pending))
        XCTAssertFalse(SessionLaunchProbe.isTerminal(.alive))
        XCTAssertTrue(SessionLaunchProbe.isTerminal(.spawnFailed))
        XCTAssertTrue(SessionLaunchProbe.isTerminal(.diedSilently))
        XCTAssertTrue(SessionLaunchProbe.isTerminal(.stalled))
    }

    /// 失败原因必须留痕可读：带上工具名、底层错误、以及「别把活挂它身上」的下一步。
    func testFailureDetailCarriesToolAndUnderlyingCause() {
        let d = SessionLaunchProbe.failureDetail(
            .spawnFailed, kind: .claudeCode, deadline: deadline, underlying: "No such file")
        XCTAssertNotNil(d)
        XCTAssertTrue(d!.contains("Claude Code"))
        XCTAssertTrue(d!.contains("No such file"))

        let codex = SessionLaunchProbe.failureDetail(
            .stalled, kind: .codex, deadline: deadline, underlying: nil)
        XCTAssertNotNil(codex)
        XCTAssertTrue(codex!.contains("Codex"))
        XCTAssertTrue(codex!.contains("25"), "半死文案要说清等了多久")
    }

    /// 正常裁决没有失败文案 —— 免得调用方拿着 nil 之外的东西去报警。
    func testNoFailureDetailForHealthyVerdicts() {
        XCTAssertNil(SessionLaunchProbe.failureDetail(.alive, kind: .claudeCode))
        XCTAssertNil(SessionLaunchProbe.failureDetail(.pending, kind: .codex))
    }
}
#endif
