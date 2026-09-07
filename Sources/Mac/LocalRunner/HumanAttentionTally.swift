#if os(macOS)
import Foundation

/// 菜单栏那个数字：**有几件事在等人拍板**（P5b·B，人类 Todo #7）。
///
/// 用途人类说得很具体：不开主窗口也能看到有没有事在等他，点一下进去。所以这里只回答
/// 一个问题 —— 「现在有几件事非你不可」。判定全在这一层（纯 Foundation，进得了
/// test bundle），菜单栏那个视图只负责把它显示出来。
///
/// ## 计哪三类，以及**为什么不计第四类**
///
/// 1. `approvals` —— 审批台账里 `status == "pending"` 的条目（`ask` 提的问题 +
///    权限 hook 的放行请求）。
/// 2. `screenMenus` —— 卡在**屏幕上那个框**、点名状态落 `awaitingDecision` 的
///    session。它跟第 1 类不是一本账：审批台账是 MCP 工具写的，这一类是终端画面
///    上明摆着的菜单，没有对应的台账条目。
/// 3. `todos` —— 人类那本 Todo 里还没回应的条数。
///
/// **`awaitingReply` 故意不计。** 它看起来也是「在等人」，但它的判定输入之一
/// 就是「审批台账里本 session 的 pending 条目」（见 `SessionAwaitingReply`）——
/// 把它一起加进来，同一件事会被数两遍。**一个人看到「3 件事在等你」点进去只找到
/// 2 件，下一次他就不信这个数字了**，而一个没人信的数字比没有更糟。
///
/// ## 去重规则写在这里，不靠调用方记得
///
/// 一个 session 理论上可以既有 pending 审批条目、又卡在屏幕菜单上（先问了一句，
/// 又撞上一个框）。那是**一个人要处理的两件事还是一件？**——按人的角度是一件：
/// 他打开那个 session 就都看见了。所以 `screenMenus` 只数**没有** pending 审批
/// 条目的那些 session。
struct HumanAttentionCount: Equatable {
    var approvals: Int = 0
    var screenMenus: Int = 0
    var todos: Int = 0

    var total: Int { approvals + screenMenus + todos }

    /// 没事的时候**保持安静**：不显示数字、不加角标。
    /// 图标本身仍在 —— 它是「点一下进去」的入口，消失了人就没地方点。
    var isQuiet: Bool { total == 0 }

    /// 菜单栏图标旁边那个数字。安静时为 nil。
    var badge: String? { isQuiet ? nil : String(total) }

    /// 展开后的分项。**只列非零的那几项** —— 常年显示「待审批 0」会训练人忽略它。
    var lines: [String] {
        var out: [String] = []
        if approvals > 0 { out.append("\(approvals) 件待审批") }
        if screenMenus > 0 { out.append("\(screenMenus) 个 session 卡在框上等你选") }
        if todos > 0 { out.append("\(todos) 条 Todo 没回") }
        return out
    }

    /// 一句话（辅助功能标签 / tooltip）。
    var summary: String {
        isQuiet ? "没有等你拍板的事" : lines.joined(separator: "，")
    }
}

enum HumanAttentionTally {
    /// - Parameters:
    ///   - pendingApprovalSessionIds: 每个 pending 审批条目对应的 sessionId
    ///     （**按条目来，不是按 session 去重** —— 同一个 session 上两个待审批就是两件事）。
    ///   - sessionStates: `sessionId → 点名状态`（`crew-sessions.json` 的口径）。
    ///   - unansweredTodos: 人类那本 Todo 里没回应的条数。
    static func tally(pendingApprovalSessionIds: [String],
                      sessionStates: [String: String],
                      unansweredTodos: Int) -> HumanAttentionCount {
        let sessionsWithApproval = Set(pendingApprovalSessionIds)
        let onScreenMenu = sessionStates
            .filter { $0.value == CrewSessionStateDerivation.awaitingDecision }
            .keys
            .filter { !sessionsWithApproval.contains($0) }
        return HumanAttentionCount(
            approvals: pendingApprovalSessionIds.count,
            screenMenus: onScreenMenu.count,
            todos: max(0, unansweredTodos))
    }
}
#endif
