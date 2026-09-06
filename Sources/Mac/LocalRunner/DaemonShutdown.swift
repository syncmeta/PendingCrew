#if os(macOS)
import Foundation

/// 「人按了停 → daemon 真的停住 → launchd **不**把它拉回来」这条链（P5b）。
///
/// ## 为什么这条链要单独有个文件、还要有测试
///
/// P5b 给 daemon 装上 launchd 的 `KeepAlive = { SuccessfulExit = false }`：**只在
/// 异常退出时重启**。于是「正常停用」这个能力，整个挂在一件事上 —— 优雅退出必须
/// 以**成功退出码**收场。它今天是对的，但**这条正确性是借来的**：
///
/// - 借的第一样：`installGracefulShutdown` 那一行恰好写着 `exit(0)`。谁哪天改成
///   `exit(1)`（"顺手把失败也报出来"），或者让它被 SIGKILL 收尾，launchd 眼里
///   就是异常退出 → 立刻拉回来 → **人再也停不掉这个后台**。
/// - 借的第二样（更容易发生，2026-09-07 核 `SessionDaemonMain:243` 时量到的）：
///   那个 `exit(0)` 原本挂在 `DispatchQueue.main.asyncAfter(deadline: .now() + 2.5)`
///   上。**它要走到，得主队列在这 2.5 秒里活着。** 任何一个 `run.stop()` 把主队列
///   卡住、或 launchd 的耐心先到，收尾就走不到 `exit(0)` —— 进程被 SIGKILL 收尾，
///   launchd 看到非正常退出，**于是把它拉回来。人按了停，结果是重启。**
///
/// 第二样这里**直接改掉**（收尾计时器搬离主队列），不是钉住它 —— 借来的前提能还掉
/// 就别只写注释。第一样还不掉（总得有人写 `exit`），所以把那个码收进
/// `DaemonShutdownPolicy.gracefulExitCode`，**同一个值同时喂给真正 exit 的那一行和
/// 判定 launchd 会不会重启的那条策略**：拆成两个字面量的那一刻链就断了，而且断得
/// 没有声音。`DaemonGracefulShutdownTests` 逐条变异自证过。
enum DaemonShutdownPolicy {
    /// 优雅退出的退出码。**别把它换成字面量** —— 见类型注释。
    static let gracefulExitCode: Int32 = DaemonExitCode.ok

    /// 停 session 的预算：到点无论停没停干净都退。
    ///
    /// 停不掉的那些由下一轮的孤儿核对兜底 —— **赖在这里不退才是最坏的结局**，
    /// 因为 launchd 那边等不到成功退出。
    static let drainBudget: TimeInterval = 2.5
}

/// launchd `KeepAlive` 的语义，写成**可判定**的形状。
///
/// 之所以要把它变成代码而不是一句注释：这条策略的正确性只有在「拿真实的收场喂给
/// 它」时才检查得了，而真实的收场就是 `DaemonShutdownPolicy.gracefulExitCode`。
enum LaunchAgentRestartPolicy: Equatable {
    /// `KeepAlive = true`：**永远拉回来**。正常退出也拉。
    /// 这一档就是「装上撤不掉」的那个后台，我们要的不是它。
    case always
    /// `KeepAlive = { SuccessfulExit = false }`：只在**上一次退出不成功**时拉回来。
    case onlyWhenExitWasUnsuccessful

    /// 进程是怎么收场的。
    enum Termination: Equatable {
        case exited(Int32)
        /// 被信号打死（SIGKILL / 崩溃）。launchd 一律算不成功。
        case killedBySignal(Int32)
    }

    func wouldRestart(after termination: Termination) -> Bool {
        switch self {
        case .always:
            return true
        case .onlyWhenExitWasUnsuccessful:
            switch termination {
            case let .exited(code): return code != 0
            case .killedBySignal: return true
            }
        }
    }

    /// 写进 LaunchAgent plist 的 `KeepAlive` 值。
    var keepAlivePlistValue: Any {
        switch self {
        case .always: return true
        case .onlyWhenExitWasUnsuccessful: return ["SuccessfulExit": false]
        }
    }
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
