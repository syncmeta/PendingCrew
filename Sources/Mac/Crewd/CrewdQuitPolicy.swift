#if os(macOS)
import AppKit

/// One product policy for menu, Dock and AppKit termination requests.
enum CrewdQuitMode: String { case direct, safePause, background }
@MainActor enum CrewdQuitPreference {
    static let key = "pendingcrew.crewd.quitPreference.v1"
    static func choose() -> CrewdQuitMode? {
        if let value = UserDefaults.standard.string(forKey: key), let mode = CrewdQuitMode(rawValue: value) { return mode }
        let alert = NSAlert()
        alert.messageText = "如何退出 PendingCrew？"
        alert.informativeText = "直接退出：尽快停止本应用自有工作，未确认结果保留。安全暂停：关闭新执行并等待安全停止。继续后台：仅退出界面，已有工作继续。观察窗口的退出仅关闭自身，不改变其他窗口管理的后台。"
        alert.addButton(withTitle: "安全暂停")
        alert.addButton(withTitle: "直接退出")
        alert.addButton(withTitle: "继续后台")
        let cancel = alert.addButton(withTitle: "取消")
        cancel.keyEquivalent = "\u{1b}"
        switch alert.runModal() {
        case .alertFirstButtonReturn: return .safePause
        case .alertSecondButtonReturn: return .direct
        case .alertThirdButtonReturn: return .background
        default: return nil
        }
    }
}
#endif
