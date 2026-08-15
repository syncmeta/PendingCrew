#if os(macOS)
import Foundation
import Combine

/// Coding-agent session 的生命周期状态，与后端实现无关。
/// `.running` = agent 进程存活；`.exited(code)` = 已结束。
/// （codex app-server 跨多轮保持 `.running`，只有进程终止才变 `.exited`。）
public enum SessionStatus: Equatable {
    case running
    case exited(Int32?)
}

/// session 终止原因分类（Todo #10 ①）：把「因 hit limit 被打断」从正常结束 /
/// 手动停 / 一般失败里区分出来 —— hit-limit 终止走自动续跑（挂额度重置唤醒），
/// 其余不挂。枚举归 runner 状态线定义（本文件），其他消费方只读不改。
enum SessionExitReason: Equatable {
    /// 用户/机长主动停（`stop()`）。
    case userStopped
    /// 正常结束（exit 0 或 SwiftTerm 拿不到 code 的干净退出）。
    case completed
    /// 非零退出，且与额度无关。
    case failed
    /// 因额度上限被打断（退出前不久刚报过 usageLimit / rateLimited 健康异常）。
    case hitLimit

    /// 分类规则（顺序即优先级）：
    /// 1. 主动停 → `.userStopped`——用户明确不要它跑了，即便当时正限额也不自动续。
    /// 2. 额度类健康异常且**新鲜**（距退出 ≤ `recencyWindow`）→ `.hitLimit`。
    ///    health 是 sticky 首报（每 Kind 一次、不清零），几小时前撞过墙、后来恢复
    ///    正常跑完退出的不能误判成 hit-limit —— 所以必须卡新鲜度。
    /// 3. 其余按 exit code：0/nil → `.completed`，非零 → `.failed`。
    static func classify(
        cancelled: Bool,
        exitCode: Int32?,
        lastHealthKind: CrewSessionHealth.Kind?,
        healthAt: Date?,
        now: Date = Date(),
        recencyWindow: TimeInterval = 600
    ) -> SessionExitReason {
        if cancelled { return .userStopped }
        if let kind = lastHealthKind, kind == .usageLimit || kind == .rateLimited,
           let at = healthAt, now.timeIntervalSince(at) <= recencyWindow {
            return .hitLimit
        }
        return (exitCode ?? 0) == 0 ? .completed : .failed
    }
}

/// `CrewSessionRun` 所需的控制 + 生命周期接口，由终端后端（claude）
/// 与未来的 app-server 后端（codex）共同实现。
/// 视图层（`AgentTerminalView` 等）由具体类型/kind 决定；
/// 本协议只覆盖控制 + 状态两面。
@MainActor
protocol SessionBackend: AnyObject {
    var status: SessionStatus { get }
    var statusPublisher: Published<SessionStatus>.Publisher { get }
    /// 是否正在跑一轮 turn（busy）。事件驱动唤醒（Phase 4b）据此判断：busy 时
    /// 不打断、把@我的定向消息留给下一轮；空闲时立刻注入唤醒。
    ///
    /// - codex（app-server）：有真 turn 生命周期 → `activeTurnId != nil`。
    /// - claude（PTY 终端）：交互式 claude 没有可编程的 turn-state，且 `send()`
    ///   写进的文本由 claude 自己的输入缓冲排队（当前轮跑完后作为下一条 prompt
    ///   执行，不污染进行中的 turn）—— 所以恒返回 `false`（注入随时安全）。
    ///
    /// ⚠️ 「注入随时安全」只对**普通 prompt** 成立。**斜杠命令**（`/model`、`/effort`）
    /// 走的是同一个输入框，忙时同样被排进消息队列 —— 于是它永远不会被当命令执行，
    /// 只会在下一轮变成一句字面文本。要发斜杠命令必须先等空闲，见
    /// `applyProfileSwitch`（#544 根因）。
    var isBusy: Bool { get }
    /// 头像/切换条「干活中 vs 空闲」状态点用的活跃信号 —— **与 `isBusy` 解耦**：
    /// `isBusy` 服务唤醒注入门禁（claude 恒 false 以保证不漏注入），这个只驱动 UI 且
    /// 可观察。codex=turn 进行中；claude=最近 ~1s 还在吐 PTY 输出的近似（干活时
    /// spinner/输出不断 → true，空闲等指令时安静 → false）。`status==.exited` 时恒 false。
    var isWorking: Bool { get }
    var isWorkingPublisher: Published<Bool>.Publisher { get }
    /// 群聊「正在输入」气泡专用的显示态（Todo #24）。与 `isWorking` **分家**：
    /// `isWorking` 是原始活跃信号（唤醒回执采样 / working-idle 上报 / 状态点 /
    /// 限额恢复 streak 都吃它，要的就是即时），给它加迟滞会一起变钝；这条只服务
    /// 一个消费方（`CrewTypingIndicatorRow`），可以放心做防抖 + 心跳重绘过滤。
    ///
    /// - claude（PTY）：`TypingActivityTracker` —— 可见文本指纹 + 不对称迟滞。
    /// - codex（app-server）：有真 turn 生命周期，本来就不抖 → 默认直接复用 `isWorking`。
    var displayIsTyping: Bool { get }
    var displayIsTypingUpdates: AnyPublisher<Bool, Never> { get }
    /// 最近一次探测到的 runner 健康异常（未登录/额度到顶）；nil = 未见异常。
    /// claude = PTY 文案扫描（`SessionHealthScanner`）；codex = `account/*`
    /// server-request。进程存活但干不了活的状态靠它浮出（分诊第 7 点）。
    var health: CrewSessionHealth? { get }
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { get }
    /// 「这个 session 正卡在等人拍板」（Todo #6）。与 `health` 分开是有意的：
    /// health 是 sticky 首报 + 每 Kind 一次（撞墙/没登录那种「持续故障」），而待决策
    /// 是**来去自如**的瞬时态 —— 人一答就该立刻清掉，答完再弹一个还要能再报。
    /// 混进 health 会同时踩两个坑：announce 的 per-Kind 去重让第二个菜单永远不报，
    /// 以及 rearm 语义纠缠不清。
    ///
    /// - claude（PTY）：`PendingDecisionTracker` 从终端画面里认出的选择菜单。
    /// - codex（app-server）：无终端菜单，恒 nil（它的待决策走结构化 server-request，
    ///   在 `handleServerRequest` 当场发群，不需要驻留状态）。
    var pendingDecisionUpdates: AnyPublisher<PendingTerminalDecision?, Never> { get }
    var pendingDecision: PendingTerminalDecision? { get }
    var kind: LocalCodingAgentKind { get }
    func send(_ text: String)
    func interrupt()
    func stop()
    /// 清除额度类健康异常并重新武装检测（额度重置唤醒到点后 runner 调）——
    /// 恢复后别让「限额中」红点谎报。默认 no-op（codex 侧暂无额度类 health）。
    func clearQuotaHealth()
    /// 中途切一个配置档位（model / effort），**返回真实结果**（#544）。
    ///
    /// - claude（PTY）：等终端空闲 → 注入斜杠命令 → 核对回显。忙时注入会被 claude
    ///   收进消息队列、斜杠命令永不执行，所以「等空闲」不是优化而是正确性前提。
    /// - codex（app-server）：协议无中途切换通道（model 绑 thread、effort 绑启动
    ///   参数）→ 默认实现回 `.unsupported`。
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome
}

extension SessionBackend {
    /// 默认「没有终端菜单这回事」——只有 PTY 后端需要覆写。
    var pendingDecisionUpdates: AnyPublisher<PendingTerminalDecision?, Never> {
        Empty(completeImmediately: false).eraseToAnyPublisher()
    }
    var pendingDecision: PendingTerminalDecision? { nil }
    var displayIsTyping: Bool { isWorking }
    var displayIsTypingUpdates: AnyPublisher<Bool, Never> {
        isWorkingPublisher.eraseToAnyPublisher()
    }
    func clearQuotaHealth() {}
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        .unsupported
    }
}
#endif
