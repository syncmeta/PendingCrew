#if os(macOS)
import Foundation

/// 机长交接的**拉起自检**：新机长到底有没有真的活过来。
///
/// 存在的理由是 2026-09-06 的现场：从 daemon 侧给 crew 44 换 Claude Code 机长，
/// 三次全部报「runner 在 25 秒内没有给出真实就绪信号」并回滚。而 claude 自己的
/// 记录（`~/.claude.json` 该项目条目：`lastSessionId` 对得上、`lastDuration`
/// 27190ms、`lastFpsAverage` 2.01、`lastSessionMetrics.frame_duration_ms_count`
/// 32、`lastGracefulShutdown` false）说明**它渲染了 32 帧、跑满 27.19 秒才被杀**。
/// 也就是说：claude 没有「静默不启动」，是自检看不见它、把它判死了。
///
/// 看不见的原因是自检**按具体类认后端**：只认 `AgentTerminalSession`（inproc 门面）
/// 和 `RemoteSessionBackend`（inproc 经协议）。daemon 里造的是第三种 ——
/// `HeadlessSessionBackend`（见 `CrewSessionRunner` 里 `sessionPublisher.isHeadless`
/// 那个三目），两个 `as?` 都是 nil，于是**这条路在 daemon 里 100% 超时**。
/// 而那三目紧挨着的注释自己写着「两条路共用同一个 `AgentSessionCore`……差的只是
/// 有没有那半画面」—— 知道抽象在哪儿，却在具体类上做判断。
///
/// 这个类型把「读到了什么」和「据此该怎么走」拆成两件可单测的事。
enum CaptainLaunchReadiness {

    /// 这一拍该怎么走。
    enum Step: Equatable {
        /// 拿到真实就绪信号，收工。
        case ready
        /// 还在观察窗内且没有确凿坏消息 —— 再等一拍。
        case keepWaiting
        /// 终局失败，附给人看的原因。
        case failed(String)
    }

    /// 从一个后端读出「它观测到子进程真的活过来了没有」。
    ///
    /// ⚠️ **这里按具体类认后端，daemon 的 `HeadlessSessionBackend` 认不出来。**
    /// 这是 2026-09-06 那个 bug 的原样搬运，先钉成红，再换成协议必答项。
    @MainActor
    static func observedLaunchSignal(_ backend: any SessionBackend) -> Bool {
        if let backend = backend as? AgentTerminalSession,
           backend.core.lastOutputAt != .distantPast { return true }
        if let backend = backend as? RemoteSessionBackend,
           !backend.lastTerminalFrameBytes.isEmpty { return true }
        if let backend = backend as? CodexAppServerBackend,
           backend.isLaunchReady { return true }
        return false
    }

    /// 一拍判定。
    ///
    /// - Parameters:
    ///   - kind: 这个 run 跑的是哪家 runner。
    ///   - isRunning: run 现在还在不在跑。
    ///   - health: run 当前的健康异常（`.launchFailed` 是确凿坏消息，立刻终局）。
    ///   - observedSignal: `observedLaunchSignal(_:)` 读到的那个布尔。
    ///   - ledgerAgentSessionId: 账本里这个 session 的 agent 侧会话号（codex 专用）。
    ///   - elapsed: 距发起过了多久。
    ///   - deadline: 观察窗。
    static func step(
        kind: LocalCodingAgentKind,
        isRunning: Bool,
        health: CrewSessionHealth?,
        observedSignal: Bool,
        ledgerAgentSessionId: String?,
        elapsed: TimeInterval,
        deadline: TimeInterval = SessionLaunchProbe.firstOutputDeadline + 1
    ) -> Step {
        if let health, health.kind == .launchFailed { return .failed(health.detail) }
        guard isRunning else { return .failed("runner 在启动观察窗内退出。") }
        if kind == .terminal { return .failed("纯终端不能当 agent 机长。") }
        if observedSignal { return .ready }
        // codex 的账本兜底：**跨协议边界**的真实 ready 信号 —— app-server 只有握手
        // 拿到 thread id 之后才写这条账，所以它与「后端自报握手完成」等价，但即使
        // 后端被包在协议对面也读得到。claude 那边没有等价的账可读（会话号是我们
        // 自己指定的，写它只证明我们传了参数，不证明 claude 起来了），所以这条
        // 兜底**只对 codex 成立**，不许照搬。
        if kind == .codex, let id = ledgerAgentSessionId, !id.isEmpty { return .ready }
        guard elapsed >= deadline else { return .keepWaiting }
        return .failed(timeoutDetail(kind: kind, elapsed: elapsed))
    }

    /// 超时文案。**必须说出自己观测到了什么**，别只说「没有给出信号」——
    /// 上一版那句把「我没看见」写成了「它没给」，害得排查这条路多花了一整轮
    /// （同一族的还有「启动槽持续被其它唤醒占用」：占着槽的是它自己）。
    static func timeoutDetail(kind: LocalCodingAgentKind, elapsed: TimeInterval) -> String {
        let waited = Int(elapsed.rounded())
        switch kind {
        case .claudeCode:
            return "新机长起来了但自检没等到真实就绪信号：等了 \(waited) 秒，"
                + "期间**一个 PTY 字节都没观测到**（判据 = 终端内核收到过首字节）。"
                + "进程仍在跑 —— 若你在终端里看得见它已经画出界面，那说明自检读错了地方，"
                + "请把这条连同 session id 一起报上来。"
        case .codex:
            return "新机长起来了但自检没等到真实就绪信号：等了 \(waited) 秒，"
                + "app-server 既没握上手拿到 thread id，账本里也没有它的会话号。"
        case .terminal:
            return "纯终端不能当 agent 机长。"
        }
    }
}
#endif
