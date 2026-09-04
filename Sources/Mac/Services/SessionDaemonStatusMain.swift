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
                print("PendingCrew 后台未运行或不可连接：\(error)")
            }
        }
        return true
    }
}
#endif
