import Foundation

/// 侧栏「时间流视图」的排序 + 扁平化推导。纯 Foundation、不碰 store / SwiftUI，单测覆盖。
enum CrewTimelineOrdering {
    /// 一行：crew + 它的最新活动时间（nil = 从来没动静过 / 时间戳脏）。
    struct Entry: Identifiable, Equatable {
        let crew: CrewSummary
        let activity: Date?
        var id: String { crew.id }
    }

    /// 扁平列表，**按最新活动倒序**（最新的在最上）。
    ///
    /// - 没有活动时间的（nil）一律沉底，不跟有时间的混排。
    /// - 同一时刻 / 都为 nil 时按标题本地化升序，再按 id —— 保证顺序**稳定**，
    ///   不会因为字典遍历顺序每次渲染跳来跳去。
    /// - `activity` 由调用方注入（视图侧读白板），推导层不碰 IO。
    static func ordered(
        crews: [CrewSummary],
        activity: (CrewSummary) -> Date?
    ) -> [Entry] {
        let entries = crews.map { Entry(crew: $0, activity: activity($0)) }
        return entries.sorted { lhs, rhs in
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

    /// 扁平行的血缘次要行：**直接父 crew 名**（多父用「、」连）。
    ///
    /// 扁平了就看不出层级，所以每行要能读出"它挂在哪儿"。行里已经有两件事在
    /// 说血缘：竖色条（递归二分色）和名字后面那行黄字（标的是**根祖先**）。这里
    /// 补的是黄字**说不出**的那一段 —— 直接父。所以：
    /// - 直接父就是黄字已经标出来的那个（那批）根 → 返回 nil，不重复同一件事；
    /// - 直接父与根不同（三层及以上）→ 显示直接父，人才知道中间挂在谁下面；
    /// - 自己是根 / 父 id 是脏引用（`crewsById` 里查不到）→ nil。
    static func lineageLine(
        for crew: CrewSummary,
        crewsById: [String: CrewSummary],
        rootTitles: [String]
    ) -> String? {
        let parents = crew.parentCrewIds.compactMap { crewsById[$0]?.title }
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        guard !parents.isEmpty else { return nil }
        let roots = Set(rootTitles.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) })
        if Set(parents) == roots { return nil }
        return parents.joined(separator: "、")
    }
}

/// 侧栏用哪种视图列 crew。原始值写进 `UserDefaults`（`@AppStorage`），跟外观模式
/// （`AppearanceMode`）走同一条持久化路子 —— 用户切过一次，下次开 app 还停在那儿。
enum CrewSidebarViewMode: String, CaseIterable, Identifiable {
    /// 机器分组 + 组内 DAG 折叠树（原有形态）。
    case hierarchy
    /// 扁平列表，按最新活动倒序。
    case timeline

    var id: String { rawValue }

    /// 没切过 = 层级视图 —— 新东西作为增量出现，不动任何人现有的肌肉记忆。
    static let `default`: CrewSidebarViewMode = .hierarchy
    static let storageKey = "crew.sidebar.viewMode"

    /// 脏值 / 空串 → 默认视图（别让手改过的 defaults 把侧栏搞空）。
    static func resolve(rawValue: String?) -> CrewSidebarViewMode {
        guard let rawValue, let mode = CrewSidebarViewMode(rawValue: rawValue) else {
            return .default
        }
        return mode
    }

    var label: String {
        switch self {
        case .hierarchy: return "层级"
        case .timeline: return "时间流"
        }
    }

    var systemImage: String {
        switch self {
        case .hierarchy: return "list.bullet.indent"
        case .timeline: return "clock"
        }
    }
}
