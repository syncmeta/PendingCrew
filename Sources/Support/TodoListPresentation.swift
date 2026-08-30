import Foundation

/// 人类 Todo 列表的**纯展示逻辑**（Todo #4/#5/#11）：排序 + 状态→图标映射。
///
/// 抽出来的原因是这两条都是「看得见但容易悄悄回退」的规则 ——
/// 排序（人类抱怨过一次：新建的必须在最上面）与状态图标（提醒事项风格的圆圈）
/// 各自有单测钉住，视图层只负责画。
///
/// 跨平台（Support/）：inspector 概览面板与 Todo 详细窗口共用同一套。
enum TodoListPresentation {
    /// 概览卡片里可以跨平台钉住的视觉契约（Todo #86）。SwiftUI 只读取这组值，
    /// 不在视图里另写一份魔数，避免正文又悄悄展开成全文或卡片退回四角圆角。
    struct OverviewLayout: Equatable {
        enum StatusNumberPlacement: Equatable { case aboveCard }
        enum ResponsePlacement: Equatable { case insideCard }

        struct CardCorners: Equatable {
            let topLeading: Int
            let bottomLeading: Int
            let bottomTrailing: Int
            let topTrailing: Int
        }

        let bodyLineLimit: Int
        let responseLineLimit: Int
        let statusNumberPlacement: StatusNumberPlacement
        let responsePlacement: ResponsePlacement
        let detailButtonTitle: String
        let cardCorners: CardCorners
    }

    static let overviewLayout = OverviewLayout(
        bodyLineLimit: 3,
        responseLineLimit: 1,
        statusNumberPlacement: .aboveCard,
        responsePlacement: .insideCard,
        detailButtonTitle: "放大看",
        cardCorners: .init(topLeading: 0, bottomLeading: 8,
                           bottomTrailing: 8, topTrailing: 8))

    /// 列表顺序：**从新到旧** —— 新建的在最上面。
    ///
    /// 按 `number` 倒序（#N 由 `LocalTodoStore.add` 自增分配，等价于创建顺序，
    /// 且不依赖 `createdAt` 字符串解析）。同号不可能出现（crew 内唯一），
    /// 但仍以 `createdAt` 兜底保证稳定序。
    static func newestFirst(_ items: [LocalTodoItem]) -> [LocalTodoItem] {
        items.sorted {
            $0.number != $1.number ? $0.number > $1.number : $0.createdAt > $1.createdAt
        }
    }

    /// 一条 Todo 的状态图标外观 —— 逻辑照抄提醒事项/Todo App 左侧圆圈。
    struct StatusIcon: Equatable {
        /// SF Symbol 名。空心圆 = `circle`；有填充 = `largecircle.fill.circle`。
        let symbol: String
        /// 是否有填充（进行中 / 已完成）。
        let isFilled: Bool
        /// 是否呼吸（缓慢脉动动画）—— 只有「进行中」呼吸。
        let isBreathing: Bool
        /// 条目正文是否变灰（已完成）。**不加删除线**（人类明确要求）。
        let dimsText: Bool
    }

    /// 状态 → 图标：
    /// - 待办 `pending`（以及任何未知状态）：空心圆、不呼吸、正文正常
    /// - 进行中 `in_progress`：有填充、呼吸、正文正常
    /// - 已完成 `completed`：有填充、不呼吸、正文变灰
    static func statusIcon(_ status: String) -> StatusIcon {
        switch status {
        case "in_progress":
            return StatusIcon(symbol: "largecircle.fill.circle",
                              isFilled: true, isBreathing: true, dimsText: false)
        case "completed":
            return StatusIcon(symbol: "largecircle.fill.circle",
                              isFilled: true, isBreathing: false, dimsText: true)
        default:
            return StatusIcon(symbol: "circle",
                              isFilled: false, isBreathing: false, dimsText: false)
        }
    }

    /// 无障碍/tooltip 文案（复用数据层的中文状态名）。
    static func statusAccessibilityLabel(_ status: String) -> String {
        LocalTodoItem.statusLabel(status)
    }

    /// 概览只露最近一条回应，并折成一行；完整回应仍留给「放大看」。
    static func overviewResponse(for item: LocalTodoItem) -> String? {
        guard let response = item.responses.last else { return nil }
        let namedSender = compactSingleLine(response.senderName ?? "")
        let sender = namedSender.isEmpty
            ? "session:\(response.sessionId.prefix(6))"
            : namedSender
        return "\(sender)：\(compactSingleLine(response.text))"
    }

    private static func compactSingleLine(_ text: String) -> String {
        text.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    }

    /// 建 Todo 时的正文口径（Todo #52：能附图之后，「只贴一张图不打字」成了合法输入）。
    ///
    /// - 有字 → 用人打的字（去首尾空白）；
    /// - 没字但有附件 → 给一条读得懂的占位（全是图 →「（见附图）」，含非图 →
    ///   「（见附件）」）。空正文的条目在列表和「To do +1: #N」里都是一片空白，
    ///   看着像坏了；
    /// - 都没有 → nil，这次没东西可记。
    static func newTodoText(draft: String, attachmentCount: Int, allImages: Bool) -> String? {
        let trimmed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty { return trimmed }
        guard attachmentCount > 0 else { return nil }
        return allImages ? "（见附图）" : "（见附件）"
    }

    /// 空列表时那句话（Todo #62）。两本账的入口完全不同 —— 一本在群聊 composer
    /// 的 Todo 按钮上，另一本只有 agent 加得了，说错了人会到处找不存在的按钮。
    static func emptyHint(_ ledger: TodoLedger) -> String {
        switch ledger {
        case .agent:
            return "还没有条目 —— 在群聊输入框点亮 Todo 按钮，发送即记一条。"
        case .human:
            return "还没有条目 —— 这本账由 agent 加：它遇到要你拍板、又不想停下来干等的事，"
                + "就往这儿记一条（工具 add_human_todo），你有空回应即可。"
        }
    }
}
