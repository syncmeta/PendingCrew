#if os(macOS)
import Darwin
import Foundation

/// `PendingCrew --daemon-stop`：**给人用的停用入口**（P5b）。
///
/// ## 为什么这条命令必须存在
///
/// 在它之前，停后台的唯一办法是自己 `pgrep` 出 pid 再 `kill -TERM`
/// （`scripts/release/install-local-update.sh` 就是这么做的）。只要 daemon 还是由
/// app 拉起来的，这还能忍 —— 退出 app 就完了。**但 P5b 给它装上开机自启之后，
/// 「我想关掉它」就成了一个必须有正式入口的能力**：没有入口的自启，等于装上撤不掉。
/// 所以这条命令跟自启是同一笔账，不是顺手加的糖。
///
/// ## 它只做一件事：替人找到那个 pid，然后发 SIGTERM
///
/// 停用的**机制**不在这里，在 `SessionDaemonMain.installGracefulShutdown`
/// （停光 session → 放锁 → 以 `DaemonShutdownPolicy.gracefulExitCode` 退出）。
/// 这里不新造第二条停用通路 —— 一条命令一个 `kill(2)`，别的什么都不做。
///
/// ## 三件事上它跟「随手写个 pkill」不一样
///
/// 1. **只停 daemon**。同一把编排锁现在也可能被 app 窗口拿着；分不清的话这条命令
///    会去 SIGTERM 一个 GUI 进程（`SessionDaemonControl.runningDaemonPid` 那条注释
///    记的就是这个坑）。
/// 2. **「我读不到」绝不说成「没在跑」**。见 `SessionOrchestratorLock.Presence`。
/// 3. **不升级到 SIGKILL**。停不下来就大声说停不下来 —— SIGKILL 会跳过收尾，
///    留下一地没人管的 agent 子进程，而那正是优雅退出存在的全部理由。
enum DaemonStopOutcome: Equatable {
    /// 发了 SIGTERM，并且确认它真的走了。
    case stopped(pid: Int32)
    /// 本来就没有后台在跑 —— **期望状态已经成立**，这是成功。
    case alreadyStopped(String)
    /// 我们没停掉任何东西，而且不该假装停掉了。
    case refused(String)

    /// 退出码按「**期望状态成立了没有**」给，跟 `DaemonExitCode.forDaemonStart`
    /// 同一条规矩：`--daemon-stop && rm -rf 数据根` 这种写法全靠它分岔。
    var exitCode: Int32 {
        switch self {
        case .stopped, .alreadyStopped: return DaemonExitCode.ok
        case .refused: return DaemonExitCode.failed
        }
    }

    var isSuccess: Bool { exitCode == DaemonExitCode.ok }

    var text: String {
        switch self {
        case let .stopped(pid):
            return "已停止 PendingCrew 后台进程（pid \(pid)）。"
        case let .alreadyStopped(detail):
            return detail
        case let .refused(detail):
            return detail
        }
    }
}

/// 停用动作本体。三个系统调用（探锁 / `kill` / 再探锁）全部走注入点，
/// 于是「停不掉时报什么」这件事进得了单测 —— 不需要真的起一个 daemon 再打死它。
struct DaemonStopper {
    var presence: () -> SessionOrchestratorLock.Presence
    /// 发 SIGTERM。返回 0 成功，否则是 errno。
    var sendTerm: (Int32) -> Int32
    /// 等一小会儿再看。
    var tick: () -> Void
    var now: () -> Date
    /// 等它放锁的上限。默认与 `SessionDaemonControl.stopRunningDaemon` 一致；
    /// 必须**大于** `DaemonShutdownPolicy.drainBudget`，否则我们会在它正常收尾的
    /// 半路上宣布"停不掉"。
    var timeout: TimeInterval = 8

    init(dataRoot: URL) {
        presence = { SessionOrchestratorLock.presence(dataRoot: dataRoot) }
        sendTerm = { pid in kill(pid, SIGTERM) == 0 ? 0 : errno }
        tick = { usleep(100_000) }
        now = Date.init
    }

    init(presence: @escaping () -> SessionOrchestratorLock.Presence,
         sendTerm: @escaping (Int32) -> Int32,
         tick: @escaping () -> Void,
         now: @escaping () -> Date,
         timeout: TimeInterval = 8) {
        self.presence = presence
        self.sendTerm = sendTerm
        self.tick = tick
        self.now = now
        self.timeout = timeout
    }

    func stop() -> DaemonStopOutcome {
        switch presence() {
        case .none:
            return .alreadyStopped("PendingCrew 后台进程没有在运行。")
        case let .undecidable(detail):
            // **这条不许报成功。** 我们不知道后台在不在，而调用方多半要拿这个退出码
            // 决定下一步删不删数据。
            return .refused("停不了：说不清后台在不在运行 —— \(detail)")
        case let .heldByUnknown(detail):
            return .refused("停不了：有进程正在管理这个数据根，但认不出是谁 —— \(detail)")
        case let .held(holder) where holder.kind != "daemon":
            // 占着锁的是 app 窗口。它不是后台进程，这条命令不该去打它。
            return .refused(
                "没有后台进程在运行，但 PendingCrew 界面（pid \(holder.pid)）正在管理这个数据根。"
                + "\n要停的话请退出 PendingCrew 界面（⌘Q）。")
        case let .held(holder):
            return terminate(pid: holder.pid)
        }
    }

    private func terminate(pid: Int32) -> DaemonStopOutcome {
        let code = sendTerm(pid)
        if code != 0 {
            if code == ESRCH {
                // 刚才还在，发信号时已经没了 —— 期望状态成立。
                return .alreadyStopped("PendingCrew 后台进程已经退出。")
            }
            return .refused("发信号给 pid \(pid) 失败：\(String(cString: strerror(code)))")
        }
        let deadline = now().addingTimeInterval(timeout)
        while now() < deadline {
            switch presence() {
            case .none:
                return .stopped(pid: pid)
            case let .held(holder) where holder.pid != pid:
                return .stopped(pid: pid)      // 锁已经换人，那个 daemon 走了
            case .held, .heldByUnknown:
                break                          // 还在收尾，继续等
            case let .undecidable(detail):
                return .refused("发过 SIGTERM 了，但确认不了它有没有停："
                    + "\(detail)（pid \(pid)）")
            }
            tick()
        }
        // **不升级到 SIGKILL** —— 见类型注释第 3 条。
        return .refused(
            "已经发过 SIGTERM，但 pid \(pid) 在 \(Int(timeout)) 秒内没有退出。"
            + "\n没有继续升级到 SIGKILL：那会跳过收尾，把它底下的 agent 子进程全变成孤儿。"
            + "\n它多半卡在停某个 session 上；确要强杀请自己执行 `kill -9 \(pid)`，"
            + "然后用 `--daemon-status` 确认没有残留。")
    }
}

/// 往 stderr 写一行。
///
/// **不用 `FileHandle.standardError.write(_:)`** —— 那是 ObjC 的 `writeData:`，
/// 写失败时抛的是 Swift 接不住的 `NSFileHandleOperationException`
/// （2026-09-05 那次 daemon 被 EPIPE 打死就是这个，见 `CodexPipeWrite`）。
/// CLI 的 stderr 完全可能是一根管子（`--daemon-stop 2>&1 | head`），对端一走就断。
/// `fputs` 只会返回 EOF，不会把进程带走。
///
/// 仓库里还有 5 处 `FileHandle.standardError.write`（见 `docs/tech-debt.md`）——
/// 它们该迁到这里来，但那是另一件事，不在这一笔里做。
enum StandardErrorText {
    static func write(_ text: String) {
        fputs(text.hasSuffix("\n") ? text : text + "\n", stderr)
    }
}
#endif
