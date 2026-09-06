#if os(macOS)
import XCTest

/// 「人按了停 → daemon 真的停住 → launchd **不**把它拉回来」这条链（P5b）。
///
/// 这条链断掉的症状不是崩溃，是**人再也停不掉这个后台**：停用命令报成功、进程也确实
/// 退了，然后 launchd 立刻把它拉回来，看起来像"停不掉"，查不出所以然。所以它必须有
/// 测试，不能只有注释 —— 抽走它的那次改动通常跟这个功能毫不相干。
final class DaemonGracefulShutdownTests: XCTestCase {

    // MARK: - launchd 那半：什么样的收场会被拉回来

    /// 链条本身：**我们真正 exit 的那个码**，喂给**我们真正要配的那条策略**，结论必须是
    /// 「不重启」。两边都不许换成字面量 —— 换了这条测试就不再检查任何东西了。
    func testGracefulExitCodeIsOneLaunchdWillNotRestart() {
        let policy = LaunchAgentRestartPolicy.onlyWhenExitWasUnsuccessful
        XCTAssertFalse(
            policy.wouldRestart(after: .exited(DaemonShutdownPolicy.gracefulExitCode)),
            "优雅退出被 launchd 当成异常 —— 人按了停，几秒后它自己回来了")
    }

    /// 「崩了能自己恢复」那半仍然要成立，否则这条策略就退化成"不重启"。
    func testAbnormalEndingsStillGetRestarted() {
        let policy = LaunchAgentRestartPolicy.onlyWhenExitWasUnsuccessful
        XCTAssertTrue(policy.wouldRestart(after: .exited(1)))
        XCTAssertTrue(policy.wouldRestart(after: .killedBySignal(SIGKILL)),
                      "被信号打死（含崩溃）必须算不成功，否则 daemon 崩了就没人拉了")
    }

    /// 为什么不能图省事写 `KeepAlive = true`：那一档会把**正常停用**也拉回来。
    /// 这条测试是那个选项的反例，留着免得有人"简化"。
    func testKeepAliveAlwaysWouldTakeAwayTheAbilityToStop() {
        XCTAssertTrue(
            LaunchAgentRestartPolicy.always
                .wouldRestart(after: .exited(DaemonShutdownPolicy.gracefulExitCode)),
            "KeepAlive=true 下正常退出也会被拉回来 —— 这正是我们不选它的原因")
    }

    /// plist 里落的字面值就是这条策略，不是另写一份。
    func testKeepAlivePlistValueMatchesThePolicy() {
        let value = LaunchAgentRestartPolicy.onlyWhenExitWasUnsuccessful.keepAlivePlistValue
        XCTAssertEqual(value as? [String: Bool], ["SuccessfulExit": false])
        XCTAssertEqual(LaunchAgentRestartPolicy.always.keepAlivePlistValue as? Bool, true)
    }

    // MARK: - 进程那半：收尾真的走得到 exit

    func testDrainsSessionsAndReleasesTheHostBeforeExiting() {
        var trace: [String] = []
        var code: Int32?
        let shutdown = DaemonGracefulShutdown(
            budget: 2.5,
            stopSessions: { trace.append("sessions") },
            releaseHost: { trace.append("host") },
            schedule: { delay, work in
                trace.append("schedule(\(delay))")
                work()          // 假时钟：立刻到点
            },
            exitProcess: { code = $0; trace.append("exit") })

        shutdown.begin()

        XCTAssertEqual(trace, ["sessions", "host", "schedule(2.5)", "exit"],
                       "顺序错了会留下一地没人管的 agent 子进程 —— 放锁/退出必须在停光 session 之后")
        XCTAssertEqual(code, DaemonShutdownPolicy.gracefulExitCode)
    }

    /// 连按两次 ⌃C、或安装脚本补发一次 SIGTERM：不该把 session 再停一遍，
    /// 也不该排第二个退出计时器。
    func testASecondSignalDoesNotDrainTwice() {
        var drains = 0
        var schedules = 0
        let shutdown = DaemonGracefulShutdown(
            budget: 0.01,
            stopSessions: { drains += 1 },
            releaseHost: {},
            schedule: { _, _ in schedules += 1 },   // 不执行，免得 exit 掉测试进程
            exitProcess: { _ in XCTFail("不该在这条用例里退出") })

        shutdown.begin()
        shutdown.begin()

        XCTAssertEqual(drains, 1)
        XCTAssertEqual(schedules, 1)
    }

    /// **这条是这个文件里最重要的一条。**
    ///
    /// 收尾计时器原来挂在 `DispatchQueue.main.asyncAfter` 上 —— 主队列被卡住时它
    /// 永远走不到 `exit(0)`，进程只能被 SIGKILL 收尾，launchd 一看是异常退出，
    /// **把它拉回来**。这里用「测试线程（= 主线程）停在信号量上」把主队列**真的**堵死，
    /// 再看退出走不走得到。
    ///
    /// 变异自证：把 `DaemonGracefulShutdown.offMainQueue` 换回
    /// `DispatchQueue.main.asyncAfter`，这条会在 2 秒后超时变红。
    func testExitDoesNotWaitForTheMainQueue() {
        let exited = DispatchSemaphore(value: 0)
        let code = UnfairBox<Int32?>(nil)
        let shutdown = DaemonGracefulShutdown(
            budget: 0.05,
            stopSessions: {},
            releaseHost: {},
            schedule: DaemonGracefulShutdown.offMainQueue,   // 生产用的那个，不是假的
            exitProcess: { code.value = $0; exited.signal() })

        shutdown.begin()

        // 从这一行起主线程停住 —— 主队列一个 block 都跑不了，正是 `run.stop()`
        // 卡住主队列时的形状。
        XCTAssertEqual(exited.wait(timeout: .now() + 2), .success,
                       "主队列被占住时退出就走不到了 —— 那正是「人按了停、launchd 又把它拉回来」的形状")
        XCTAssertEqual(code.value, DaemonShutdownPolicy.gracefulExitCode)
    }
}

/// 跨线程读写一个值的最小盒子（测试内部用）。
private final class UnfairBox<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: T
    init(_ value: T) { storage = value }
    var value: T {
        get { lock.lock(); defer { lock.unlock() }; return storage }
        set { lock.lock(); storage = newValue; lock.unlock() }
    }
}
#endif
