#if os(macOS)
import Combine
import Foundation
import SwiftTerm

/// **daemon 进程里的终端后端**：只有内核，没有画面（spec §5.2）。
///
/// `AgentTerminalSession` / `PlainTerminalSession` 是「内核 + `TerminalMirrorView`」
/// 的门面，那个 mirror 是 AppKit 视图 —— 它属于窗口，不属于常驻后台进程。分家之后
/// daemon 里**一个 NSView 都不该有**：没人看的 session 连画都不该画，那正是
/// `docs/tech-debt.md` 第一条（PTY 每批输出都过主线程）要解掉的东西。
///
/// 于是这里是同一批转发，减掉 mirror 那一半。**它不是 `AgentTerminalSession` 的
/// 替代品** —— inproc 模式下仍然走门面那条路，一个字没改；这个类型只在
/// `--daemon` 进程里被造出来。
///
/// 与门面唯一有实质差别的一处是进程退出时的回滚收窄，见 `collapseScrollback()`。
@MainActor
final class HeadlessSessionBackend: ObservableObject, SessionBackend {
    let kind: LocalCodingAgentKind
    let core: AgentSessionCore
    /// 纯 shell 的 `send`/`interrupt` 语义与 agent 不同（不追加回车、Ctrl-C 而非 Esc）。
    private let isPlainShell: Bool

    var status: SessionStatus { core.status }
    var statusPublisher: Published<SessionStatus>.Publisher { core.$status }
    /// 与 `AgentTerminalSession` 一致：PTY 终端没有可编程 turn-state，注入随时安全。
    let isBusy = false
    var isWorking: Bool { core.isWorking }
    var isWorkingPublisher: Published<Bool>.Publisher { core.$isWorking }
    var displayIsTyping: Bool { core.displayIsTyping }
    var displayIsTypingUpdates: AnyPublisher<Bool, Never> {
        core.$displayIsTyping.eraseToAnyPublisher()
    }
    var health: CrewSessionHealth? { core.health }
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { core.$health }
    var pendingDecision: PendingTerminalDecision? { core.pendingDecision }
    var pendingDecisionUpdates: AnyPublisher<PendingTerminalDecision?, Never> {
        core.$pendingDecision.eraseToAnyPublisher()
    }

    init(config: SessionConfig, mode: AgentSessionCore.Mode,
         executable: String, workdir: String, env: [String: String],
         protocolOutputSink: (([UInt8]) -> Void)? = nil) {
        kind = config.kind
        isPlainShell = mode == .plainShell
        core = AgentSessionCore(
            config: config, mode: mode,
            executable: executable, workdir: workdir, env: env,
            protocolOutputSink: protocolOutputSink)
        core.onExited = { [weak self] in
            MainActor.assumeIsolated { self?.collapseScrollback() }
        }
    }

    func send(_ text: String) {
        isPlainShell ? core.sendPlainShell(text) : core.send(text)
    }
    func interrupt() {
        isPlainShell ? core.interruptPlainShell() : core.interrupt()
    }
    func sendRaw(_ bytes: [UInt8]) { core.sendRaw(bytes) }
    func stop() { core.stop() }
    func clearQuotaHealth() { core.clearQuotaHealth() }
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        await core.applyProfileSwitch(cmd)
    }

    /// 进程终止 → 收窄回滚缓冲（`TerminatedScrollbackPlan`，2026-08-18 第二条）。
    ///
    /// 门面那条路是让 mirror 先算（那套反推要 `scrollThumbsize` / `canScroll` 这些
    /// **视图几何**），算出来的行数再喂给内核。daemon 里没有视图，所以这里直接从
    /// 权威缓冲区把同样的两个数算出来 —— SwiftTerm 的 `scrollThumbsize` 定义就是
    /// `max(rows / 缓冲总行数, 0.01)`，`canScroll` 就是「缓冲行数超过一屏」，
    /// 两者都不需要窗口。**判定函数是同一个**，只是喂给它的数换了来路。
    private func collapseScrollback() {
        guard let terminal = core.terminal else { return }
        let rows = terminal.rows
        let total = max(Self.activeLineCount(terminal), rows)
        let retained = TerminatedScrollbackPlan.retainedLines(
            rows: rows,
            thumbSize: rows > 0 ? max(Double(rows) / Double(total), 0.01) : 0,
            canScroll: total > rows)
        core.changeScrollback(retained)
    }

    /// 活跃缓冲区的行数。`Buffer.lines` 是 internal，公开的只有
    /// `getScrollInvariantLine(row:)`（越界返回 nil）—— 所以只能数，不能读。
    /// 一个 session 一生只在退出那一拍走一次，10000 行的循环可以接受。
    private static func activeLineCount(_ t: Terminal) -> Int {
        var row = t.buffer.totalLinesTrimmed
        var count = 0
        while t.getScrollInvariantLine(row: row) != nil {
            row += 1
            count += 1
        }
        return count
    }
}

extension HeadlessSessionBackend: SessionProcessIdentifying {
    var agentProcessIdentifier: Int32 { core.process?.shellPid ?? 0 }
}

extension HeadlessSessionBackend: SessionProtocolTerminalControlling {
    func resizeTerminal(cols: Int, rows: Int) {
        core.resize(cols: cols, rows: rows)
        core.noteViewportChange()
    }
}

extension HeadlessSessionBackend: SessionProtocolScreenTextProviding {
    func screenText(maxLines: Int) -> String { core.screenText(maxLines: maxLines) }
}

extension HeadlessSessionBackend: SessionProtocolTerminalSnapshotProviding {
    func protocolTerminalSnapshot() -> TerminalSnapshotEncoder.Snapshot? { core.snapshot() }
}

extension HeadlessSessionBackend: SessionProtocolLaunchProblemProviding {
    var protocolLaunchParameterProblems: AnyPublisher<SessionLaunchParameterProblem, Never> {
        core.$launchParameterProblem.compactMap { $0 }.eraseToAnyPublisher()
    }
}
#endif
