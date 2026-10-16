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

/// An original management scope may be retired while its independent backend
/// remains alive. Explicit application exit may reclaim that same unowned
/// instance; an observer or replacement instance is never stopped.
@MainActor enum CrewdRetiredManagement {
    struct Target: Equatable { let database: String; let instance: String }
    struct Connection {
        let target: Target
        let call: (String, CrewdValue) async throws -> CrewdValue
        let close: () -> Void
    }
    enum Failure: LocalizedError {
        case unconfirmed
        var errorDescription: String? { "原本机后台的退出尚未确认，保留当前窗口以便核对。" }
    }
    @discardableResult static func exit(target: Target, mode: CrewdQuitMode,
                     connect: () async throws -> Connection) async throws -> Bool {
        guard mode != .background else { return false }
        let connection = try await connect()
        defer { connection.close() }
        guard connection.target == target else { return false }
        let status = try await connection.call("lifecycle.status", .object([:]))
        guard let role = status["role"].string, ["owner", "observer"].contains(role),
              case .bool(let ownerPresent) = status["ownerPresent"] else { throw Failure.unconfirmed }
        if role == "observer" {
            guard !ownerPresent else { return false }
            let claimed = try await connection.call("lifecycle.claim", .object([:]))
            guard claimed["role"].string == "owner" else { throw Failure.unconfirmed }
        }
        let reply = try await connection.call("lifecycle.exit", .object([
            "mode": .string(mode == .direct ? "direct" : "safePause"), "timeoutMs": .integer(15000)
        ]))
        guard reply["status"].string == "stopped" else { throw Failure.unconfirmed }
        return true
    }
}
#endif
