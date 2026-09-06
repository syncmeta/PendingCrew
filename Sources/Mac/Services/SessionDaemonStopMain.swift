#if os(macOS)
import Foundation

/// `PendingCrew --daemon-stop`：停掉常驻后台进程，不起 GUI。
///
/// 同一个二进制的第 N 副身份（`--daemon` / `--daemon-attach` / `--daemon-status`
/// 的同班）。判断和等待都在 `DaemonStopper` 里（那一层进得了 test bundle），
/// 这里只负责「读 argv → 印结果 → 定退出码」。
///
/// **退出码按「期望状态成立了没有」给**，不是按「我做了没做事」：本来就没有后台在跑
/// 也是 0 —— `--daemon-stop && rm -rf 数据根` 这种写法全靠这一点。
enum SessionDaemonStopMain {
    static let flag = "--daemon-stop"

    static func runIfRequested(_ argv: [String]) -> Bool {
        guard argv.contains(flag) else { return false }
        let dataRoot = PendingCrewDaemonPaths.standard().lock.deletingLastPathComponent()
        let outcome = DaemonStopper(dataRoot: dataRoot).stop()
        if outcome.isSuccess {
            print(outcome.text)
        } else {
            StandardErrorText.write("PendingCrew：\(outcome.text)")
        }
        exit(outcome.exitCode)
    }
}
#endif
