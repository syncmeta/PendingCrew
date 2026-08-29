#if os(macOS)
import Foundation
import AppKit
import Combine
import SwiftTerm

/// 一个内嵌真终端的 coding-agent 会话：在 PTY 里跑交互式 claude/codex(自动审批)。
/// 读=终端原样显示，控=键盘(SwiftTerm 原生) + 程序化 send()/interrupt() 字节注入。
///
/// **P1 之后它只是一层薄门面**（spec `docs/internal/2026-08-19-backend-split-design.md` §5.2）：
/// 进程、权威缓冲区、六个状态扫描器与拉起自检都在无画面的 `AgentSessionCore` 里，
/// 画面在 `TerminalMirrorView` 里，这里只把 `SessionBackend` 逐条转发给内核，
/// 外加一件仍属于窗口的事 —— 外置滚动条的几何。上层编排一行没改。
@MainActor
final class AgentTerminalSession: ObservableObject, SessionBackend {
    let kind: LocalCodingAgentKind
    /// 无画面内核：进程 + 权威 `Terminal` + 全部扫描器。P4 时它整个搬去后台进程。
    let core: AgentSessionCore
    /// 只负责画的那半：被内核的字节喂养，把键盘/尺寸回传给内核。
    let mirror: TerminalMirrorView

    /// 视图层拿去挂 NSView 的那个对象（`AgentTerminalView` / `CrewSessionRun`）。
    var terminalView: TerminalMirrorView { mirror }

    // MARK: - SessionBackend 转发

    var status: SessionStatus { core.status }
    var statusPublisher: Published<SessionStatus>.Publisher { core.$status }
    /// 唤醒注入门禁用：PTY 终端无可编程 turn-state，交互式 claude 自带输入排队，
    /// 注入随时安全 → 恒 false（main 语义保留，别让 @我的定向消息因 busy 漏注入）。
    var isBusy: Bool { false }
    var isWorking: Bool { core.isWorking }
    var isWorkingPublisher: Published<Bool>.Publisher { core.$isWorking }
    var displayIsTyping: Bool { core.displayIsTyping }
    var displayIsTypingUpdates: AnyPublisher<Bool, Never> { core.$displayIsTyping.eraseToAnyPublisher() }
    var health: CrewSessionHealth? { core.health }
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { core.$health }
    var pendingDecision: PendingTerminalDecision? { core.pendingDecision }
    var pendingDecisionUpdates: AnyPublisher<PendingTerminalDecision?, Never> {
        core.$pendingDecision.eraseToAnyPublisher()
    }
    var launchParameterProblems: AnyPublisher<SessionLaunchParameterProblem, Never> {
        core.$launchParameterProblem.compactMap { $0 }.eraseToAnyPublisher()
    }

    func send(_ text: String) { core.send(text) }
    func interrupt() { core.interrupt() }
    /// 机长 nudge_session 用：向 PTY 发裸按键字节（Enter=0x0d / Esc=0x1b 等），
    /// **不**追加回车 —— 与 `send` 的「正文+隔拍回车」区分，用于模态菜单选择。
    func sendRaw(_ bytes: [UInt8]) { core.sendRaw(bytes) }
    func stop() { core.stop() }
    func clearQuotaHealth() { core.clearQuotaHealth() }
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        await core.applyProfileSwitch(cmd)
    }
    func applyProfileSwitch(
        _ cmd: SessionProfileSwitchCommand, policy: SessionProfileSwitchPolicy
    ) async -> SessionProfileSwitchOutcome {
        await core.applyProfileSwitch(cmd, policy: policy)
    }

    // MARK: - 外置 overlay 滚动条（几何是窗口的事，留在 app 侧）

    /// 外置 overlay 滚动条的几何 + 用户主动信号（SwiftUI `TerminalScrollbarOverlay` 观察）。
    /// 全部取自 SwiftTerm 公开滚动接口，语义：position/thumbSize 都是 0…1。
    struct ScrollState: Equatable {
        /// 是否可滚（有回滚历史且非 alternate buffer）——决定 overlay 是否挂出。
        var canScroll = false
        /// viewport 相对位置 0(最顶)…1(最底)。
        var position: Double = 0
        /// knob 占轨道的比例 0…1（= SwiftTerm `scrollThumbsize`/knobProportion）。
        var thumbSize: Double = 1
        /// 单调递增计数：每次「用户主动滚动」bump 一次。overlay 据此触发淡入 + 重置
        /// 自动隐藏计时；输出自动滚屏不 bump，条不会因 agent 持续吐字一直闪。
        var userScrollTick: Int = 0
    }
    @Published private(set) var scrollState = ScrollState()

    /// `executable` = 已 resolve 的 claude/codex 绝对路径；`workdir` = 工作目录。
    /// argv（auto mode / model / effort）由 `config.argv()` 构建；Claude 开场正文由
    /// 内核等 TUI 首次吐字后经 PTY 发送，不进入 argv。
    init(config: SessionConfig, executable: String, workdir: String, env: [String: String],
         protocolOutputSink: (([UInt8]) -> Void)? = nil) {
        self.kind = config.kind
        self.core = AgentSessionCore(
            config: config, mode: .agent,
            executable: executable, workdir: workdir, env: env,
            protocolOutputSink: protocolOutputSink)
        self.mirror = TerminalMirrorView(frame: .zero)
        mirror.core = core
        mirror.terminalDelegate = mirror     // 自己当自己的 delegate（弱引用，不成环）
        // 内核吐字节 → 画。core 那份是权威、先喂；mirror 可能压根不存在（P4 之后
        // 「没人看的 session」就是这个形状），所以这条是**旁路**，不是主路。
        core.onOutput = { [weak mirror] slice in
            MainActor.assumeIsolated { mirror?.feedFromCore(slice) }
        }
        // 进程终止 → 两份回滚缓冲一起收窄（`TerminatedScrollbackPlan`）。收多少由
        // mirror 算（那套反推要 thumbSize/canScroll 这些视图几何），算出来的行数
        // 同样喂给内核那份 —— 劈分前只有一份缓冲区，收窄它就等于全放掉。
        core.onExited = { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                let retained = self.mirror.collapseScrollbackAfterExit()
                self.core.changeScrollback(retained)
            }
        }
        // 内核 0.6s 轮询的每一拍顺带刷新条的几何：
        // canScroll/thumbSize 会因输出增长而变，但只在 yDisp 变化时才有 scrolled
        // 回调；顶到底持续吐字时 yDisp 每行都动能覆盖，静止但 buffer 变化的边角用
        // 轮询兜一层（非用户主动，只同步几何不点亮）。
        core.onTick = { [weak self] in
            MainActor.assumeIsolated { self?.refreshScrollState(userInitiated: false) }
        }
        // 终端滚动（用户滚 + 输出自动滚）→ 刷新外置条几何。userInitiated 决定是否点亮。
        mirror.onScroll = { [weak self] userInitiated in
            guard let self else { return }
            MainActor.assumeIsolated { self.refreshScrollState(userInitiated: userInitiated) }
        }
        // 内部 scroller 兜底隐藏 + fail-loud：didAddSubview 一般已抓到；若 SwiftTerm 换了
        // 布局（不再用 NSScroller / 私有结构变），这里大声报，别让它静默贴右缘压字。
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 800_000_000)
            guard let self else { return }
            if !self.mirror.hideNativeScroller() {
                assertionFailure("SwiftTerm 内部 NSScroller 未找到——外置滚动条方案需复核 SwiftTerm 布局")
                NSLog("[AgentTerminalSession] 警告：未能隐藏 SwiftTerm 内部滚动条，右缘可能压字")
            }
        }
    }

    /// 从 SwiftTerm 公开接口重算外置条几何；仅在变化时发布，避免输出流时高频 @Published。
    private func refreshScrollState(userInitiated: Bool) {
        let next = ScrollState(
            canScroll: mirror.canScroll,
            position: mirror.scrollPosition,
            thumbSize: Double(mirror.scrollThumbsize),
            userScrollTick: scrollState.userScrollTick + (userInitiated ? 1 : 0)
        )
        if next != scrollState { scrollState = next }
    }

    /// overlay 拖动/点击 → 驱动 SwiftTerm 滚到 0…1 位置，并立即回刷几何（算用户主动）。
    func scrollTerminal(toPosition position: Double) {
        mirror.scroll(toPosition: max(0, min(1, position)))
        refreshScrollState(userInitiated: true)
    }

    /// SwiftTerm 的 `processTerminated` 把 **`waitpid` 的原始 status** 原样当
    /// exitCode 交出来（没走 WEXITSTATUS）—— 于是 exit 2 会显示成 512、exit 127
    /// 显示成 32512。给人看的原因里必须是真的退出码。实现在内核，这里留个别名
    /// 免得调用方跟着搬家。
    static func decodeWaitStatus(_ st: Int32) -> Int32 { AgentSessionCore.decodeWaitStatus(st) }
}

/// SIGTERM 已由调用方（SwiftTerm `terminate()`）发出；本函数等一个宽限期，
/// 若进程仍在就升级到 **SIGKILL**。`killpg` 杀整个进程组——pty 子进程是 setsid
/// 的会话首（组 id == pid），能连带杀掉它拉起的 bash/MCP 子进程；非组首则
/// fallback 单杀。SIGKILL 不可捕获，保证停得掉。
func terminateTree(pid: pid_t, graceSeconds: Double) async {
    guard pid > 0 else { return }
    try? await Task.sleep(nanoseconds: UInt64(max(0, graceSeconds) * 1_000_000_000))
    guard kill(pid, 0) == 0 else { return }     // 已退出，无需升级
    if killpg(pid, SIGKILL) != 0 { kill(pid, SIGKILL) }
}
#endif
