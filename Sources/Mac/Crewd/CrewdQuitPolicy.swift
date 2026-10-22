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
        alert.informativeText = "安全暂停：关闭新执行，等待本应用管理的工作安全停止。\n\n直接退出：尽快停止自有工作，未确认的结果保留待核对。\n\n通知停工后退出：先叫他们停工，再显示进度；提示可以关闭，也可明确强制停止。\n\n观察窗口只关闭自身，不改变其他窗口管理的后台。"
        alert.addButton(withTitle: "安全暂停")
        alert.addButton(withTitle: "直接退出")
        alert.addButton(withTitle: "通知停工后退出")
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
            "mode": .string(mode == .direct ? "direct" : "safePause"), "timeoutMs": .integer(120000)
        ]))
        guard reply["status"].string == "stopped" else { throw Failure.unconfirmed }
        return true
    }
}
#endif

#if os(macOS)
/// Retired owned scopes share one authenticated stop socket. A force request
/// must use the same management scope as an already pending cooperative stop.
@MainActor final class CrewdRetiredStopChannel {
    private var opening: Task<CrewdRetiredManagement.Connection, Error>?
    private var connection: CrewdRetiredManagement.Connection?
    private var users = 0
    func exit(target: CrewdRetiredManagement.Target, mode: CrewdQuitMode,
              connect: @escaping () async throws -> CrewdRetiredManagement.Connection) async throws -> Bool {
        users += 1
        defer {
            users -= 1
            if users == 0 { connection?.close(); connection = nil; opening = nil }
        }
        if opening == nil { opening = Task { try await connect() } }
        let actual = try await opening!.value
        connection = actual
        return try await CrewdRetiredManagement.exit(target: target, mode: mode) {
            .init(target: actual.target, call: actual.call, close: {})
        }
    }
}

/// Owns the stop Tasks independently of the progress window's visibility.
@MainActor final class CrewdStopCoordinator {
    enum State: Equatable { case idle, stopping, forcing, unconfirmed(String), stopped }
    private(set) var state: State = .idle
    var changed: ((State) -> Void)?
    var completed: (() -> Void)?
    private let stop: () async throws -> Void
    private let force: () async throws -> Void
    private let mayForce: () -> Bool
    private var stoppingTask: Task<Void, Never>?
    private var forcingTask: Task<Void, Never>?
    init(stop: @escaping () async throws -> Void, force: @escaping () async throws -> Void, mayForce: @escaping () -> Bool) {
        self.stop = stop; self.force = force; self.mayForce = mayForce
    }
    var forceAvailable: Bool {
        guard mayForce() else { return false }
        switch state { case .stopping, .unconfirmed: return true; default: return false }
    }
    func begin(forceImmediately: Bool = false) {
        guard state == .idle else { return }
        transition(.stopping)
        if forceImmediately, mayForce() { requestForce(); return }
        stoppingTask = Task { [self] in
            do { try await stop(); finish() }
            catch {
                guard state != .stopped, forcingTask == nil else { return }
                transition(.unconfirmed(error.localizedDescription))
            }
        }
    }
    func requestForce() {
        guard forceAvailable, forcingTask == nil else { return }
        transition(.forcing)
        forcingTask = Task { [self] in
            do { try await force(); finish() }
            catch {
                guard state != .stopped else { return }
                forcingTask = nil; transition(.unconfirmed(error.localizedDescription))
            }
        }
    }
    private func finish() {
        guard state != .stopped else { return }
        transition(.stopped); completed?()
    }
    private func transition(_ next: State) { state = next; changed?(next) }
}

/// Closing this native panel only hides it. The coordinator and original
/// authenticated connection continue their stop operation until settlement.
@MainActor final class CrewdStopProgressWindow: NSWindowController, NSWindowDelegate {
    private let explanation = NSTextField(wrappingLabelWithString: "已通知本App管理的工作停止；不会接受新任务。关闭此提示不会取消停工，也不会恢复任务。")
    private let spinner = NSProgressIndicator()
    private let forceButton = NSButton(title: "强制关闭 App…", target: nil, action: nil)
    var appExitRequested: (() -> Void)?
    init() {
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 460, height: 220), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        panel.title = "正在叫他们停工"; panel.isReleasedWhenClosed = false
        super.init(window: panel)
        panel.delegate = self
        explanation.font = .systemFont(ofSize:13)
        explanation.textColor = .secondaryLabelColor
        spinner.style = .spinning; spinner.controlSize = .small
        forceButton.target = self; forceButton.action = #selector(forceClicked)
        let row = NSStackView(views: [spinner, forceButton]); row.orientation = .horizontal; row.spacing = 16
        let stack = NSStackView(views: [explanation, row]); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 18; stack.translatesAutoresizingMaskIntoConstraints = false
        let content = panel.contentView!; content.addSubview(stack)
        NSLayoutConstraint.activate([stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24), stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24), stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 22)])
        panel.center()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }
    func present() { showWindow(nil); window?.makeKeyAndOrderFront(nil) }
    func update(_ state: CrewdStopCoordinator.State) {
        // An explicit GUI-only exit is always available, even when an
        // unavailable authority forbids any stronger backend operation.
        forceButton.isEnabled = state != .stopped
        switch state {
        case .idle, .stopping:
            window?.title = "正在叫他们停工"; spinner.startAnimation(nil)
        case .forcing:
            window?.title = "正在强制停止"
            explanation.stringValue = "正在强制停止本App管理的工作，等待实际停止结果。关闭此提示不会取消停工。"
            spinner.startAnimation(nil)
        case .unconfirmed(let message):
            window?.title = "停工尚未确认"; spinner.stopAnimation(nil)
            explanation.stringValue = message + "\n原结果保留待核对，不会恢复或重发任务。可继续等待，或明确选择仅关闭本 App；后台停工结果仍未确认。"
        case .stopped: spinner.stopAnimation(nil); window?.orderOut(nil)
        }
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool { sender.orderOut(nil); return false }
    @objc private func forceClicked() { appExitRequested?() }
}
#endif
