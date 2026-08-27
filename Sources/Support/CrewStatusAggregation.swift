import Foundation

/// crew 级状态点颜色（侧栏色条右上角；Todo #71/#73）。
/// 优先级 红（错误）> 黄（本 crew 或后代有给人类的 Todo）> 绿（正在工作）；
/// 静止时不画。
///
/// ⚠️ 与 session 级状态点（`SessionStatusDot`，头像右下角那颗）是**两套语义**，
/// 尤其**黄色含义完全不同**：这边黄 = 人类 Todo 未回应，那边黄 = session 空闲。
enum CrewStatusDotColor: Equatable {
    /// 任一 session 有健康异常（未登录/额度到顶/拉起失败）。
    case red
    /// 人类 Todo 那本里还有没回应的条目；这一态呼吸。
    case yellow
    /// 任一 session 正在干活（跑 turn）。
    case green

    var breathes: Bool { self == .yellow }
}

/// 参与聚合的单个 session 状态信号 —— 从 `CrewSessionRun` 抽出的纯值快照，
/// 让聚合成为可单测的纯函数（不依赖 SwiftUI / ObservableObject）。
struct CrewSessionStatusSignal: Equatable {
    /// 进程存活（`run.status == .running`）。
    let isAlive: Bool
    /// 正在跑 turn（`run.isWorking`；退出后恒 false）。
    let isWorking: Bool
    /// runner 健康异常（`run.health != nil`：未登录/额度到顶）。
    let hasHealthIssue: Bool
    /// 正等着人回话（`run.awaitingReply != nil`，判定见 `SessionAwaitingReply`）。
    /// 默认 false 兼容旧调用方。
    var isAwaitingReply: Bool = false
}

/// crew 状态点聚合纯函数。sessions 来自 `CrewSessionRunner.runs` 里本 crew 的 run；
/// Todo 范围来自 `CrewHumanTodoAttentionCache` 的后台快照。
enum CrewStatusAggregation {
    /// Todo #73 的主入口：自身或后代任一有未回应条目都黄，且都压过工作绿。
    static func dot(
        sessions: [CrewSessionStatusSignal],
        attention: CrewHumanTodoAttention
    ) -> CrewStatusDotColor? {
        if sessions.contains(where: { $0.hasHealthIssue }) { return .red }
        if attention.hasUnanswered { return .yellow }
        if sessions.contains(where: { $0.isWorking }) { return .green }
        return nil
    }

    /// 取最高优先级：红（健康异常）> 黄（给人类的 Todo）> 绿（干活中）> nil（静止）。
    /// `attentionReason` 形参为旧调用兼容保留，但 Todo #71 起不再控制这颗状态点；
    /// 需要人处理的事项应落人类 Todo，由可追踪、可回应的账本点亮黄色。
    ///
    /// `humanTodoUnanswered` = 那本账的未回应条数快照，由 `CrewStore` 在后台
    /// 指纹门控算好（`CrewHumanTodoAttentionCache`）——**不许在 body 里现读**。
    /// 默认 0 兼容旧调用方。
    static func dot(
        sessions: [CrewSessionStatusSignal],
        attentionReason _: String?,
        humanTodoUnanswered: Int = 0
    ) -> CrewStatusDotColor? {
        dot(
            sessions: sessions,
            attention: CrewHumanTodoAttention(
                ownUnanswered: humanTodoUnanswered,
                descendantUnanswered: 0))
    }
}
// `sessionAttention`（头像右上角未读红点）已删（人类定调 2026-08-08：两个点合成一个）。
// session 级的点现在只剩右下角那颗，语义收口在 `SessionStatusDot`；未读不再点亮任何点。
