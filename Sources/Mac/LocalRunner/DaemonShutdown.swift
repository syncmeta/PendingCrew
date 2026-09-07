#if os(macOS)
import Foundation

/// 「人按了停 → daemon 真的停住」这条链。
///
/// ## 收尾的两条约束，以及为什么它们值得有测试
///
/// SIGTERM 之后的收尾是：停光 session → 放锁 → 到点 `exit`。
/// 它原本借了一样东西：`exit` 挂在 `DispatchQueue.main.asyncAfter(deadline: .now() + 2.5)`
/// 上，**要走到它，主队列得在这 2.5 秒里活着**。任何一个 `run.stop()` 把主队列卡住，
/// 收尾就走不到 `exit`，进程只能被 SIGKILL 收尾 —— 而 `--daemon-stop` 会一直等到
/// 超时、报「停不掉」，人得自己去 `kill -9`。
///
/// **这一样直接还掉**：收尾计时器搬到 global 队列，不再问主队列借命
/// （`DaemonGracefulShutdownTests` 用「测试线程停在信号量上」把主队列真的堵死来钉它）。
///
/// > 2026-09-07 记：这个文件原来还有一半是给 launchd 的 —— 开机自启那版要靠
/// > 「优雅退出必须是成功退出码」才不会被 `KeepAlive` 拉回来。人类否掉常驻方向之后
/// > 没有 launchd 了，那半（`LaunchAgentRestartPolicy`）连同它的测试一起删了。
/// > **退出码本身留着**：`--daemon-stop` 和安装脚本仍然读它。
enum DaemonShutdownPolicy {
    /// 优雅退出的退出码。`--daemon-stop` 与安装脚本都按它判「停干净了没有」。
    static let gracefulExitCode: Int32 = DaemonExitCode.ok

    /// 停 session 的预算：到点无论停没停干净都退。
    ///
    /// 停不掉的那些由下一轮的孤儿核对兜底 —— **赖在这里不退才是最坏的结局**：
    /// 停用命令会一直等到超时，然后告诉人「停不掉，自己 kill -9」。
    static let drainBudget: TimeInterval = 2.5
}

/// 收到 SIGTERM/SIGINT 之后的收尾：停光 session → 放锁 → 到点退出。
///
/// 拆出来是为了让上面那条链**在单测里跑得到真身**：`SessionDaemonMain` 进不了
/// test bundle，而这里进得了（`Sources/Mac/LocalRunner` 整个编进 test target）。
final class DaemonGracefulShutdown {
    /// 到点执行 —— **注入点存在的唯一理由是让测试换掉时钟**；生产用的那个
    /// 默认值（`offMainQueue`）才是这个类型的重点。
    typealias Schedule = (TimeInterval, @escaping () -> Void) -> Void

    /// **不在主队列上**。见类型注释里「借的第二样」。
    static let offMainQueue: Schedule = { delay, work in
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + delay, execute: work)
    }

    private let budget: TimeInterval
    private let stopSessions: () -> Void
    private let releaseHost: () -> Void
    private let schedule: Schedule
    private let exitProcess: (Int32) -> Void
    private let lock = NSLock()
    private var begun = false

    init(budget: TimeInterval = DaemonShutdownPolicy.drainBudget,
         stopSessions: @escaping () -> Void,
         releaseHost: @escaping () -> Void,
         schedule: @escaping Schedule = DaemonGracefulShutdown.offMainQueue,
         exitProcess: @escaping (Int32) -> Void = { exit($0) }) {
        self.budget = budget
        self.stopSessions = stopSessions
        self.releaseHost = releaseHost
        self.schedule = schedule
        self.exitProcess = exitProcess
    }

    /// 幂等：连按两次 ⌃C（或安装脚本补发一次 SIGTERM）不该把 session 再停一遍、
    /// 也不该排第二个退出计时器。
    func begin() {
        lock.lock()
        if begun {
            lock.unlock()
            return
        }
        begun = true
        lock.unlock()

        stopSessions()
        releaseHost()
        schedule(budget) { [exitProcess] in
            exitProcess(DaemonShutdownPolicy.gracefulExitCode)
        }
    }
}
#endif
