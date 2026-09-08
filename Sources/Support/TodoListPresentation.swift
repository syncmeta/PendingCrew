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

    // MARK: - 卡片里的 markdown（人类 Todo #119）

    /// 概览卡片正文要渲染的 markdown 片段 —— **在源文本层就截断**。
    ///
    /// ## 为什么不能只靠 `.lineLimit`
    ///
    /// 卡片一直是 `Text(...).lineLimit(3)`，那对一段纯文本是「整段最多 3 行」。
    /// 换成 markdown 之后 `.lineLimit` 变成**逐 block** 生效 —— 一条「标题 + 列表 +
    /// 三个段落」的 Todo，每个 block 各占 3 行，卡片当场撑爆。
    ///
    /// 离屏实测（卡片宽 300pt、`.lineLimit(3)`）：同一段内容纯 `Text` 恒定 42pt，
    /// 渲染成 markdown 是 **527pt，12.5 倍**。所以截断必须发生在渲染之前。
    ///
    /// ## 怎么截
    ///
    /// markdown 是**按行**的，所以按行走、按「估算渲染行数」记预算：
    /// - 中日韩字符按 2 个单位、其余按 1 个，一行放得下 `unitsPerLine` 个单位。
    /// - 空行不计预算，但会被保留成 block 分隔（除非正好落在结尾）。
    /// - **碰到围栏代码块或表格就停** —— 3 行的卡片里它们只会渲染成一坨，
    ///   而且截半个围栏会让 MarkdownUI 把后面所有内容都吞进代码块。
    /// - 预算用完时把当前行按剩余单位切开，末尾补 `…`。
    /// - 第一行自己就超预算也必须留下它的前缀：截成空的会让卡片整片空白。
    ///
    /// 绝大多数 Todo 是没有任何 markdown 记号的短散文（本机账本实测：agent 那本
    /// 720 条里 99 条带记号，人类那本 108 条里 26 条）——**它们原样返回，一个字节不动**。
    static func cardMarkdown(_ text: String, lineBudget: Int, unitsPerLine: Int = 46) -> String {
        let source = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !source.isEmpty, lineBudget > 0 else { return "" }

        var budget = lineBudget * unitsPerLine
        var kept: [String] = []
        var truncated = false

        for line in source.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            // 围栏 / 表格：卡片里放不下，且截半个围栏会污染后面所有内容。
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") || trimmed.hasPrefix("|") {
                truncated = true
                break
            }
            if trimmed.isEmpty {
                if !kept.isEmpty { kept.append("") }   // 保留 block 分隔，行首空行丢掉
                continue
            }
            let units = displayUnits(line)
            if units <= budget {
                kept.append(line)
                // 每一行至少吃掉一整行的预算 —— 三个各占一行的列表项就是 3 行，
                // 不能因为每项都短就当成还剩很多。
                budget -= max(units, unitsPerLine)
                if budget <= 0 { truncated = true; break }
                continue
            }
            // 这一行放不下：切前缀。第一行也走这条 —— 保证卡片不会是空的。
            kept.append(prefix(of: line, units: budget))
            truncated = true
            break
        }

        while kept.last?.isEmpty == true { kept.removeLast() }
        guard !kept.isEmpty else { return prefix(of: source, units: lineBudget * unitsPerLine) + "…" }
        let body = truncated ? closeDanglingInline(kept.joined(separator: "\n")) : kept.joined(separator: "\n")
        return body + (truncated ? "…" : "")
    }

    /// 切在一半的行内记号收尾。
    ///
    /// 按预算切前缀时很容易切在 ``` `code` `` 或 `**粗体**` 的中间，留下一个落单的记号。
    /// MarkdownUI 会把落单记号当**字面字符**渲染出来 —— 卡片上就多出一个莫名其妙的
    /// 反引号或两个星号。截到落单记号之前，宁可少几个字。
    private static func closeDanglingInline(_ text: String) -> String {
        var out = text
        for marker in ["`", "**"] {
            let count = out.components(separatedBy: marker).count - 1
            if count % 2 == 1, let last = out.range(of: marker, options: .backwards) {
                out = String(out[..<last.lowerBound])
            }
        }
        return out.trimmingCharacters(in: .whitespaces)
    }

    /// 显示宽度单位：中日韩全角按 2，其余按 1。
    private static func displayUnits<S: StringProtocol>(_ text: S) -> Int {
        text.unicodeScalars.reduce(0) { $0 + (isWide($1) ? 2 : 1) }
    }

    private static func isWide(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x1100...0x115F, 0x2E80...0xA4CF, 0xAC00...0xD7A3,
             0xF900...0xFAFF, 0xFE30...0xFE4F, 0xFF00...0xFF60, 0xFFE0...0xFFE6,
             0x20000...0x3FFFD:
            return true
        default:
            return false
        }
    }

    private static func prefix<S: StringProtocol>(of text: S, units: Int) -> String {
        guard units > 0 else { return "" }
        var out = ""
        var used = 0
        for character in text {
            let width = displayUnits(String(character))
            if used + width > units { break }
            out.append(character)
            used += width
        }
        return out.isEmpty ? String(text.prefix(1)) : out
    }

    // MARK: - 点进去看哪一条（人类 Todo #122）

    /// 详细窗口这次该显示谁。
    ///
    /// 人类原话：「在外面的没放大的 todo 列表 点进去之后 要能直接显示这个 Todo 的详情
    /// 而不是展示全文」。此前点任何一条进去，看到的都是**又一份完整列表**，他得在里面
    /// 重新找刚点的那条。
    ///
    /// - `focus == nil` → 全列表（顶部「放大看」按钮走这条，那是列表入口）。
    /// - `focus` 指到某条 → 只有那一条。
    /// - **`focus` 指到一条不存在的 #N → 回落全列表**，不是空窗口：窗口开着时那条被删了、
    ///   或药丸换了本账（两本账的 #N 指两件事）都会走到这里，而一个空窗口没有任何出路。
    static func focusedRows(_ rows: [LocalTodoItem], focus: Int?) -> [LocalTodoItem] {
        guard let focus, let hit = rows.first(where: { $0.number == focus }) else { return rows }
        return [hit]
    }

    /// 每条 Todo 共用的本地化时间元信息（概览与详细窗口同一口径）。旧条目的
    /// 更新时间由 `effectiveUpdatedAt` 从既有回应/创建时间回落，不会显示成空白。
    static func metadataText(
        for item: LocalTodoItem,
        locale: Locale = .autoupdatingCurrent,
        timeZone: TimeZone = .autoupdatingCurrent
    ) -> String {
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.timeZone = timeZone
        formatter.dateStyle = .medium
        formatter.timeStyle = .short

        func localized(_ stamp: String) -> String {
            guard let date = CrewTimestamp.parse(stamp) else { return stamp }
            return formatter.string(from: date)
        }
        return "创建 \(localized(item.createdAt)) · 更新 \(localized(item.effectiveUpdatedAt))"
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
