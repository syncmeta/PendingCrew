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
///
/// P1 之后它与 `AgentTerminalSession` 同构：一个无画面内核（`.plainShell` 模式 ——
/// 什么都不扫）+ 一个只负责画的 `TerminalMirrorView`。
@MainActor
final class PlainTerminalSession: ObservableObject, SessionBackend {
    let kind: LocalCodingAgentKind = .terminal
    let core: AgentSessionCore
    let mirror: TerminalMirrorView

    var terminalView: TerminalMirrorView { mirror }

    var status: SessionStatus { core.status }
    var statusPublisher: Published<SessionStatus>.Publisher { core.$status }

    let isBusy = false
    var isWorking: Bool { core.isWorking }
    var isWorkingPublisher: Published<Bool>.Publisher { core.$isWorking }

    var health: CrewSessionHealth? { core.health }
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { core.$health }

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
        // argv 走 `SessionConfig(kind: .terminal).argv()` = `["-l"]`（登录 shell）。
        core = AgentSessionCore(
            config: SessionConfig(kind: .terminal),
            mode: .plainShell,
            executable: shell,
            workdir: workdir,
            env: environment)
        mirror = TerminalMirrorView(frame: .zero)
        mirror.core = core
        mirror.terminalDelegate = mirror
        mirror.useNativeScroller()           // 普通终端保留 SwiftTerm 原生滚动条
        core.onOutput = { [weak mirror] slice in
            MainActor.assumeIsolated { mirror?.feedFromCore(slice) }
        }
        // 同 `AgentTerminalSession`：进程一终止就把两份回滚缓冲一起收窄，别让停掉的
        // shell 继续攥着 10000 行的缓冲（见 `TerminatedScrollbackPlan`）。
        core.onExited = { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                let retained = self.mirror.collapseScrollbackAfterExit()
                self.core.changeScrollback(retained)
            }
        }
    }

    /// 协议要求的程序化写入；普通使用路径直接由 SwiftTerm 接收人的键盘输入。
    func send(_ text: String) { core.sendPlainShell(text) }

    func interrupt() { core.interruptPlainShell() }

    func stop() { core.stop() }
}
#endif
