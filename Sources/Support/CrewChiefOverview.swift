import Foundation

/// 侧栏「总机长视图」的分段推导（Todo #102 / #109 第一步）。纯 Foundation、不碰
/// store / SwiftUI，单测覆盖。
///
/// ## 它要解决的是「太多太乱」，不是「再来一种排序」
/// 层级视图回答「谁挂在谁下面」，时间流视图回答「刚才哪儿有动静」。这台机器上
/// 已经有 40+ 个 crew，两个问题的答案都是**一份同样长的列表** —— 人要的其实是
/// 第三个问题：「**现在该我管的是哪几件**」。所以这里不排序，是**收敛**：
///
/// 1. **在等你回应** —— 这个 crew 自己那本人类 Todo 还有没回应的条目。
/// 2. **还在跑** —— 最近 `quietAfter` 之内有过动静的。
/// 3. **安静** —— 其余的（含从来没动静过的）。视图默认折起来。
///
/// ## 为什么这一版一个 agent 都不用参与
/// 三段的判据全是**已经在算的机械事实**：未回应条数来自 `CrewHumanTodoAttention`
/// （侧栏黄点的同一份快照），活动时间来自 `CrewActivityTime`（两个老视图的同一
/// 口径）。没有任何一处需要模型判断，所以它**不烧额度、不会算错、agent 没跑也照常
/// 出结果**。让 agent 接管的是后面的「临时分类」，不是这里。
///
/// ## 未回应只看自己那本，不算后代
/// 侧栏黄点会把后代条数沿父边聚合（那是为了「折起来也看得见」）。这里**不能**这么
/// 算：本视图是扁平的，后代自己就在同一份列表里占一行，把它的条数再算进祖先，
/// 同一条 Todo 会在第一段里出现两次，人会以为要处理两件事。
enum CrewChiefOverview {
    /// 一段。顺序即 `allCases` 顺序 —— 段序是固定的，不参与任何排序。
    enum Section: String, CaseIterable, Identifiable, Sendable {
        /// 有没回应的人类 Todo。
        case awaitingHuman
        /// 最近有动静。
        case running
        /// 其余（默认折叠）。
        case quiet

        var id: String { rawValue }

        var title: String {
            switch self {
            case .awaitingHuman: return "在等你回应"
            case .running: return "还在跑"
            case .quiet: return "安静"
            }
        }

        var systemImage: String {
            switch self {
            case .awaitingHuman: return "hand.raised"
            case .running: return "bolt"
            case .quiet: return "moon.zzz"
            }
        }

        /// 「安静」默认折起来 —— 收敛的意义就在于它一开始不占屏幕。
        var collapsedByDefault: Bool { self == .quiet }
    }

    /// 一行。
    struct Entry: Identifiable, Equatable, Sendable {
        let crew: CrewSummary
        /// 这个 crew 自己那本人类 Todo 还有几条没回应。
        let unanswered: Int
        /// 最新活动时间（nil = 从来没动静 / 时间戳脏）。
        let activity: Date?

        var id: String { crew.id }
    }

    /// 一段 + 它的行。
    struct Group: Identifiable, Equatable, Sendable {
        let section: Section
        let entries: [Entry]

        var id: String { section.rawValue }
    }

    /// 「还在跑」的时间窗默认值。取 2 小时：短到能把「还在跑」压成几行（不然它就
    /// 变成第二份全量列表，这个视图也就白做了），长到不会把一个正在等编译/等测试
    /// 的 crew 误判成安静。它是参数，不是常量 —— 视图传进来，测试可以钉别的值。
    static let defaultQuietAfter: TimeInterval = 2 * 60 * 60

    /// 把一堆 crew 收敛成三段。
    ///
    /// - Parameters:
    ///   - crews: 要收进来的 crew（视图侧决定范围：目前是「侧栏当前可见的全部」）。
    ///   - unanswered: 该 crew **自己**那本人类 Todo 的未回应条数（见类型注释）。
    ///   - activity: 该 crew 的最新活动时间；由调用方注入，推导层不碰 IO。
    ///   - now: 现在（测试注入）。
    ///   - quietAfter: 超过这个时长没动静就算「安静」。
    /// - Returns: 段序固定、**空段不返回**（视图不该画一个 0 行的标题）。
    static func sections(
        crews: [CrewSummary],
        unanswered: (CrewSummary) -> Int,
        activity: (CrewSummary) -> Date?,
        now: Date,
        quietAfter: TimeInterval = defaultQuietAfter
    ) -> [Group] {
        var buckets: [Section: [Entry]] = [:]
        for crew in crews {
            let entry = Entry(
                crew: crew,
                unanswered: max(0, unanswered(crew)),
                activity: activity(crew))
            buckets[section(for: entry, now: now, quietAfter: quietAfter), default: []].append(entry)
        }
        return Section.allCases.compactMap { section in
            guard let entries = buckets[section], !entries.isEmpty else { return nil }
            return Group(section: section, entries: entries.sorted { ordered($0, $1, in: section) })
        }
    }

    /// 单行落哪一段。第一段的判据**压过**活动时间：等人回应的哪怕十天没动静，也
    /// 不该被划进「安静」里折起来 —— 那正是它需要被看见的原因。
    static func section(for entry: Entry, now: Date, quietAfter: TimeInterval) -> Section {
        if entry.unanswered > 0 { return .awaitingHuman }
        guard let activity = entry.activity else { return .quiet }
        // 未来时间戳（时钟漂移 / 手改数据）按「刚有动静」算，别掉进安静里。
        return now.timeIntervalSince(activity) <= quietAfter ? .running : .quiet
    }

    /// 段内排序。**全序**（一路比到 id），保证同一份输入每次渲染顺序完全一致 ——
    /// 侧栏最不能忍的就是行自己跳来跳去。
    private static func ordered(_ lhs: Entry, _ rhs: Entry, in section: Section) -> Bool {
        // 第一段先按「欠了几条」多的在前 —— 这一段的问题是"先还哪一笔"。
        if section == .awaitingHuman, lhs.unanswered != rhs.unanswered {
            return lhs.unanswered > rhs.unanswered
        }
        switch (lhs.activity, rhs.activity) {
        case let (l?, r?):
            if l != r { return l > r }
        case (nil, _?):
            return false
        case (_?, nil):
            return true
        case (nil, nil):
            break
        }
        let byTitle = lhs.crew.title.localizedCompare(rhs.crew.title)
        if byTitle != .orderedSame { return byTitle == .orderedAscending }
        return lhs.crew.id < rhs.crew.id
    }
}
