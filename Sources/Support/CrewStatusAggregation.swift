import Foundation

/// crew 级状态点颜色（侧栏头像右上角；crew-sidebar-status spec 2026-07-06 §3）。
/// 优先级 红 > 黄 > 绿 > 灰；不满足任何条件（无 session 记录且无 attention）不画。
///
/// ⚠️ 与 session 级状态点（`SessionStatusDot`，头像右下角那颗）是**两套语义**，
/// 尤其**黄色含义完全不同**：这边黄 = 机长 `raise_attention` 点亮的「要人看一眼」，
/// 那边黄 = 这个 session 空闲/排队。改一边时别顺手改另一边。
///
/// 红这边是「**这个群里有人卡着，需要人过来**」的聚合：健康异常，**或**有 session 在等
/// 回复（Todo #25 层 2）。侧栏折起来时人只看得到 crew 这一级，所以 session 级判出的
/// 「待回复」必须能在这里冒出来 —— 否则你得逐个展开才发现有人在等。
enum CrewStatusDotColor: Equatable {
    /// 任一存活 session 健康异常（未登录/额度到顶）**或**正等着人回话。
    case red
    /// 机长点亮的 attention（`raise_attention`）**或**人类 Todo 那本里还有没回应的
    /// 条目（Todo #62 ④）。两个源并联，见 `dot` 的注释。
    case yellow
    /// 任一 session 正在干活（跑 turn）。
    case green
    /// 有 session 记录但全空闲/已退出。
    case gray
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

/// crew 状态点聚合纯函数。数据源：sessions 来自 `CrewSessionRunner.runs`
/// 里本 crew 的 run；attention 来自 `LocalCrewStore` 持久化的 `attentionReason`。
enum CrewStatusAggregation {
    /// 取最高优先级：红（存活且「异常 ∨ 在等回复」）> 黄（attention）> 绿（干活中）>
    /// 灰（有记录但全闲/退）。`nil` = 无任何 session 记录且无 attention —— 不画点，
    /// 避免所有闲置 crew 都挂灰点造成噪音。
    ///
    /// 红的两个来源都卡 `isAlive`：已退出的 session 不再需要人过来，它有自己的灰点。
    /// 黄的**两个源并联**（Todo #62 ④）：机长 `raise_attention` 的理由，**或**
    /// 人类 Todo 那本里还有没回应的条目。
    ///
    /// 为什么不把人类 Todo 塞进 `attentionReason`：那是**单槽 last-write-wins**
    /// （`LocalCrewStore.setAttention`）—— 人类 Todo 往里写会把机长的理由冲掉，
    /// 机长 `clear_attention` 又会顺手把「还有 3 条没人拍板」熄了。并联之后两边
    /// 互不干扰，而且「人类不回应也能按灭」自然成立：`dismissedAt` 一打，
    /// `isUnanswered` 就是 false，这个数自己降到 0（判据不在这里，在
    /// `LocalTodoItem.isUnanswered`）。
    ///
    /// `humanTodoUnanswered` = 那本账的未回应条数快照，由 `CrewStore` 在后台
    /// 指纹门控算好（`CrewHumanTodoAttentionCache`）——**不许在 body 里现读**。
    /// 默认 0 兼容旧调用方。
    static func dot(
        sessions: [CrewSessionStatusSignal],
        attentionReason: String?,
        humanTodoUnanswered: Int = 0
    ) -> CrewStatusDotColor? {
        if sessions.contains(where: {
            $0.isAlive && ($0.hasHealthIssue || $0.isAwaitingReply)
        }) { return .red }
        if let reason = attentionReason, !reason.isEmpty { return .yellow }
        if humanTodoUnanswered > 0 { return .yellow }
        if sessions.contains(where: { $0.isWorking }) { return .green }
        return sessions.isEmpty ? nil : .gray
    }
}
// `sessionAttention`（头像右上角未读红点）已删（人类定调 2026-08-08：两个点合成一个）。
// session 级的点现在只剩右下角那颗，语义收口在 `SessionStatusDot`；未读不再点亮任何点。
