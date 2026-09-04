#if os(macOS)
import Foundation

/// viewer 那条腿在界面上要用到的两件事。`nil`（没有这个结构）= 根本没有那条腿。
struct ViewerLinkState: Equatable {
    var isConnected: Bool
    var lastError: String?
}

/// **「这个窗口现在管不管事」在界面上是什么态** —— 从闸门裁决直接算出来的**纯判定**。
///
/// ## 为什么是一个纯函数，而不是又一个 `@Published`
///
/// `OrchestrationGate` 落地那一笔，拒绝的理由只到 `@Published` 为止，**全仓没有
/// 任何视图读**。那等于闸门存在、但对用户静默 —— 而「窗口在、什么都不动、不报错」
/// 正是这一整期在修的那种失败。**一个没人读的 `@Published` 和一句没写的日志是
/// 同一个东西。**
///
/// 光「接上界面」还不够：GUI 我们验不了（不许为验证开窗口），所以「接上了」很容易
/// 变成第二个没人看的字段。所以把「显示什么」抽成这个纯判定：
/// **输入是闸门的裁决本身**，输出是屏幕上的态。于是
/// 「锁被别人占着 → 闸门拒绝 → 界面必须是错误态」变成一条跑得出来的测试
/// （`OrchestrationNoticeTests`），而且把 `OrchestrationGate.refuse` 关掉时，
/// **它会连着界面层一起红**，不只是闸门红。
///
/// 测的是**状态**不是像素 —— 像素本来也验不了，但「状态到没到界面层」验得了。
enum OrchestrationNotice: Equatable {
    /// 不占屏。正常 inproc 编排、或已退化成 viewer 且连上了。
    case none
    /// 红：本窗口既没编排也没退化。`detail` 是「谁占着」（pid / 启动时刻 / 数据根）。
    case conflict(detail: String)
    /// 琥珀：已退化成 viewer 但还没接上。可能自己会好（退避重连中），所以是提示不是
    /// 错误 —— 但**不许静默**：「正在连接」和「连不上」在屏幕上必须分得出来。
    case connecting(detail: String)

    /// - Parameters:
    ///   - decision: 编排闸门的裁决。`nil` = 闸门没装（`--daemon` 进程 / 单测）。
    ///   - viewer: viewer 那条腿的状态。`nil` = 没有那条腿（真 inproc 编排）。
    static func resolve(decision: OrchestrationGate.Decision?,
                        viewer: ViewerLinkState?) -> OrchestrationNotice {
        // 冲突压过一切：这个窗口什么都不管，连不连得上后台已经不是重点。
        if case let .conflict(detail) = decision { return .conflict(detail: detail) }
        guard let viewer, !viewer.isConnected else { return .none }
        // 只说「连不上」而不说「本来该连谁」，人还得再查一轮 —— 所以把退化的理由
        // （里面带着 pid / 启动时刻 / 数据根）一并给出来。
        var lines: [String] = []
        if let error = viewer.lastError, !error.isEmpty { lines.append(error) }
        if case let .followDaemon(detail) = decision { lines.append(detail) }
        return .connecting(detail: lines.joined(separator: "\n"))
    }
}
#endif
