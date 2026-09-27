#if os(macOS)
import Foundation

/// 菜单栏品牌图标右侧的数字：全机两本 Todo 账中未解决条目的去重数。
/// 身份是 (crewId, item.id)，不使用各账独立分配的编号。审批与屏幕菜单等待
/// 事项保留为面板分项，但不进入这个数字。
struct HumanAttentionCount: Equatable {
    var approvals: Int = 0
    var screenMenus: Int = 0
    var todos: Int = 0

    var total: Int { todos }

    /// 没事的时候**保持安静**：不显示数字、不加角标。
    /// 图标本身仍在 —— 它是「点一下进去」的入口，消失了人就没地方点。
    var isQuiet: Bool { total == 0 }

    /// 菜单栏图标旁边那个数字。安静时为 nil。
    ///
    /// ⚠️ **这个数和侧栏黄点上的数不是同一个口径，它们本来就不该相等。**
    /// 这个 = **全机** × 两本账的未解决 Todo 去重数；
    /// 侧栏那个（`CrewHumanTodoAttention.badge`）= **单个 crew** × 等人回复的
    /// Todo × 自身+后代。看到两个数不一样是正常的。
    var badge: String? { isQuiet ? nil : String(total) }

    /// 展开后的分项。只列非零的项，审批和屏幕菜单与 Todo 数字分开。
    var lines: [String] {
        var out: [String] = []
        if approvals > 0 { out.append("\(approvals) 件待审批") }
        if screenMenus > 0 { out.append("\(screenMenus) 个 session 卡在框上等你选") }
        if todos > 0 { out.insert("\(todos) 条未解决 Todo", at: 0) }
        return out
    }

    /// 一句话（辅助功能标签 / tooltip）。
    var summary: String {
        lines.isEmpty ? "没有未解决 Todo 或其它等待事项" : lines.joined(separator: "，")
    }
}

enum HumanAttentionTally {
    /// 每拍读共享名册；GUI / daemon 在另一进程新建 crew 后，启动时的
    /// `LocalCrewStore` 内存列表不会自动重载。复用通讯录的失败即抛语义。
    static func loadCrewIds(whiteboardDirectory: URL) throws -> [String] {
        try CrewDirectory.load(whiteboardDirectory: whiteboardDirectory).crews.map(\.id)
    }

    enum TodoReadError: LocalizedError {
        case unreadable(String, TodoLedger)
        var errorDescription: String? {
            switch self {
            case .unreadable(let crewId, let ledger):
                return "\(crewId) 的\(ledger.pillTitle) Todo 账读不出来"
            }
        }
    }

    private struct TodoIdentity: Hashable {
        let crewId: String
        let itemId: String
    }

    struct TodoRecord {
        let crewId: String
        let ledger: TodoLedger
        let item: LocalTodoItem
    }

    static func readRecords(crewIds: [String],
                            read: (String, TodoLedger) -> LocalTodoStore.LedgerRead) throws -> [TodoRecord] {
        var records: [TodoRecord] = []
        for crewId in crewIds {
            for ledger in TodoLedger.allCases {
                switch read(crewId, ledger) {
                case .rows(let rows):
                    records.append(contentsOf: rows.map {
                        TodoRecord(crewId: crewId, ledger: ledger, item: $0)
                    })
                case .unreadable:
                    throw TodoReadError.unreadable(crewId, ledger)
                }
            }
        }
        return records
    }

    /// 同一 crew 的同一条 UUID 跨账只算一次；编号在不同 crew 或账内可重复。
    static func tally(todos: [TodoRecord], pendingApprovalSessionIds: [String],
                      sessionStates: [String: String]) -> HumanAttentionCount {
        let open = todos.filter { !$0.item.isDeleted && !$0.item.isSettled && $0.item.withdrawnAt == nil }
        let identities = Set(open.map { TodoIdentity(crewId: $0.crewId, itemId: $0.item.id) })
        let sessionsWithApproval = Set(pendingApprovalSessionIds)
        let onScreenMenu = sessionStates
            .filter { $0.value == CrewSessionStateDerivation.awaitingDecision }
            .keys.filter { !sessionsWithApproval.contains($0) }
        return HumanAttentionCount(approvals: pendingApprovalSessionIds.count,
                                   screenMenus: onScreenMenu.count, todos: identities.count)
    }

    /// 旧调用兼容入口；菜单栏生产路径使用携带真实条目的 `tally(todos:...)`。
    /// 当前 `ask` / 权限请求已进人类 Todo，生产调用方不再读取旧审批账。
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
