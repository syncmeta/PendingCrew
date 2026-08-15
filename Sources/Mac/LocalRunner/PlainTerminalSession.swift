#if os(macOS)
import AppKit
import Combine
import Foundation
import SwiftTerm

/// 人直接操作的普通 PTY shell。
///
/// 它只实现 session 的进程生命周期与终端 IO，不扫描 agent 状态、不接世界观/MCP、
/// 不参与白板、唤醒、额度或待决策编排。`CrewSessionRunner` 负责把这种后端只留在
/// session 切换条与 inspector，绝不登记成 crew 成员。
@MainActor
final class PlainTerminalSession: ObservableObject, SessionBackend {
    let kind: LocalCodingAgentKind = .terminal
    let terminalView: ActivityTerminalView

    @Published private(set) var status: SessionStatus = .running
    var statusPublisher: Published<SessionStatus>.Publisher { $status }

    let isBusy = false
    @Published private(set) var isWorking = false
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }

    @Published private(set) var health: CrewSessionHealth?
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { $health }

    private final class Delegate: LocalProcessTerminalViewDelegate {
        var onExit: ((Int32?) -> Void)?
        func processTerminated(source: TerminalView, exitCode: Int32?) { onExit?(exitCode) }
        func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {}
        func setTerminalTitle(source: LocalProcessTerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
    }

    private let delegate = Delegate()
    /// `$SHELL` 必须是可执行的绝对路径；无效时退到系统 zsh。
    nonisolated static func defaultShell(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        isExecutable: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> String? {
        let candidates = [environment["SHELL"], "/bin/zsh"].compactMap { $0 }
        return candidates.first { $0.hasPrefix("/") && isExecutable($0) }
    }

    /// 普通 shell 继承人的环境，但不把 PendingCrew 自己的凭据带进子进程。
    nonisolated static func shellEnvironment(
        _ parent: [String: String] = ProcessInfo.processInfo.environment
    ) -> [String: String] {
        parent.filter { !LocalCodingAgentEnv.isForbidden(key: $0.key) }
    }

    init(shell: String, workdir: String, environment: [String: String]) {
        terminalView = ActivityTerminalView(frame: .zero)
        terminalView.useNativeScroller()
        terminalView.processDelegate = delegate
        delegate.onExit = { [weak self] rawCode in
            Task { @MainActor in
                guard let self, self.status == .running else { return }
                let code = rawCode.map(AgentTerminalSession.decodeWaitStatus)
                self.status = .exited(code)
            }
        }

        var env = environment.map { "\($0.key)=\($0.value)" }
        if !env.contains(where: { $0.hasPrefix("TERM=") }) { env.append("TERM=xterm-256color") }
        terminalView.startProcess(
            executable: shell,
            args: ["-l"],
            environment: env,
            currentDirectory: workdir
        )
        if terminalView.process?.running != true || (terminalView.process?.shellPid ?? 0) <= 0 {
            status = .exited(127)
        }
    }

    /// 协议要求的程序化写入；普通使用路径直接由 SwiftTerm 接收人的键盘输入。
    func send(_ text: String) {
        guard status == .running else { return }
        terminalView.process?.send(data: Array(text.utf8)[...])
    }

    func interrupt() {
        guard status == .running else { return }
        terminalView.process?.send(data: Array<UInt8>([0x03])[...]) // Ctrl-C
    }

    func stop() {
        guard status == .running else { return }
        let pid = terminalView.process?.shellPid ?? 0
        terminalView.terminate()
        status = .exited(nil)
        Task { await terminateTree(pid: pid, graceSeconds: 2.0) }
    }
}
#endif
