#if os(macOS)
import Foundation

/// `PendingCrew --daemon-attach <sessionId>`：**无界面 viewer 探针**（P5a 的证据工具）。
///
/// 与 `--daemon-status` 同一形状、同一位置（GUI 之前截住，绝不起 NSApplication），
/// 差别只在问什么：status 问「后台在不在、有哪些 session」，attach 问
/// 「**把那个 session 的画面给我看一眼**」—— 而这一眼正是 P5a 唯一还没有可复跑证据的
/// 那一步（viewer 断开 → 重连 → 画面恢复）。加 `--attach-reconnect` 就一次调用打两份。
///
/// 它是探针，不是产品功能：不拉起后台、不改 session 的终端尺寸、不发任何输入。
/// 出错一律打人话 + 非 0 退出码，绝不静默 —— 静默的探针比没有探针更坏。
enum SessionDaemonAttachMain {
    static let flag = SessionDaemonAttachOptions.flag

    /// 退出码：0 成功；2 参数不对；1 跑不通（连不上 / 没这个 session / 超时）。
    static func runIfRequested(_ argv: [String]) -> Bool {
        switch SessionDaemonAttachOptions.parse(argv) {
        case .notRequested:
            return false
        case let .invalid(message):
            FileHandle.standardError.write(Data((message + "\n").utf8))
            exit(2)
        case let .options(options):
            MainActor.assumeIsolated {
                do {
                    print(try SessionDaemonAttachProbe.run(options: options).text)
                } catch {
                    FileHandle.standardError.write(
                        Data(("\(String(describing: error))\n").utf8))
                    exit(1)
                }
            }
            return true
        }
    }
}
#endif
