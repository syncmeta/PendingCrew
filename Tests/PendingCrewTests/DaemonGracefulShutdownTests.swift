#if os(macOS)
import XCTest

/// 「人按了停 → daemon 真的停住」这条链。
///
/// 断掉的症状不是崩溃，是**停用命令一直等到超时、报「停不掉」**，而人得自己去
/// `kill -9`。它必须有测试、不能只有注释 —— 抽走它的那次改动通常跟这个功能毫不相干。
///
/// （原来这里还有 4 条是给 launchd 的，开机自启被人类否掉之后一起删了。）
final class DaemonGracefulShutdownTests: XCTestCase {

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

    /// 退出印记那两笔必须落在**正确的两端**：`draining` 在停 session **之前**
    /// （放后面的话，收尾卡死被强杀时盘上还是「在跑」，那次就跟真崩溃分不开了），
    /// `clean` 在真的 `exit` **之前**。
    func testExitMarkerHooksBracketTheDrain() {
        var trace: [String] = []
        let shutdown = DaemonGracefulShutdown(
            budget: 0.01,
            beforeDraining: { trace.append("draining") },
            beforeExit: { trace.append("clean") },
            stopSessions: { trace.append("sessions") },
            releaseHost: { trace.append("host") },
            schedule: { _, work in work() },
            exitProcess: { _ in trace.append("exit") })

        shutdown.begin()

        XCTAssertEqual(trace, ["draining", "sessions", "host", "clean", "exit"],
                       "印记落错了位置 —— 它是「要不要问人恢复」的唯一依据")
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
    /// 永远走不到 `exit(0)`，于是 `--daemon-stop` 会一直等到超时、报「停不掉」，
    /// 人得自己去 `kill -9`（而 `kill -9` 跳过收尾，会把 agent 子进程全变成孤儿）。
    /// 这里用「测试线程（= 主线程）停在信号量上」把主队列**真的**堵死，再看退出走不走得到。
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
                       "主队列被占住时退出就走不到了 —— 那正是「人按了停、它却停不掉」的形状")
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
