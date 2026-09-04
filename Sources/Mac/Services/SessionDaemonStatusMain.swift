#if os(macOS)
import Foundation

/// `PendingCrew --daemon-status`：只连后台问实况，不起 GUI，也不拉起一个新 daemon。
enum SessionDaemonStatusMain {
    static let flag = "--daemon-status"

    static func runIfRequested(_ argv: [String]) -> Bool {
        guard argv.contains(flag) else { return false }
        MainActor.assumeIsolated {
            do {
                print(try SessionDaemonStatusProbe.query().text)
            } catch {
                // 走 stderr + 非 0 退出码（与 `--daemon-attach` 同一约定）。
                // **说了「不可连接」就不许报成功**：任何 `if PendingCrew --daemon-status`
                // 都会一路走进 then，分不出「后台好着呢」和「后台连不上」。
                FileHandle.standardError.write(
                    Data(("PendingCrew 后台未运行或不可连接：\(error)\n").utf8))
                exit(DaemonExitCode.statusProbeFailed)
            }
        }
        return true
    }
}
#endif
