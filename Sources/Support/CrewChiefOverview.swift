import Foundation

/// 侧栏「总机长视图」的排序推导（Todo #102 / 人类 #109，口径按 #113 改过）。
/// 纯 Foundation、不碰 store / SwiftUI，单测覆盖。
///
/// ## 这一版**不分类**
/// 第一版曾把 crew 切成三段（在等你回应 / 还在跑 / 安静）。**人类 #113 推翻了它**，
/// 原话：「我希望不要按照 在等回应和在跑这种分类。你应该尽可能地 先不分类 先注重
/// 排序 什么是最近活跃处理的 就放到前面 就是把手头上要用到的 尽可能放前面」。
/// 连同 #109 里他自己写的「有几个机组等着我的回应 那就放到前面」一起作废。
/// 所以这里只剩一件事：**排序**。侧栏不出现任何分组标题。
///
/// ## 「最近活跃」量的是什么：量到的三种，选了有分辨率的那个
/// 2026-09-08 在这台机器上实测过 47 个 crew：
/// - **① 任何人最后一条消息**（本文件用的）—— 覆盖 47/47，有分辨率。
///   但它量的是「哪儿刚有动静」，动静大多是 agent 发的。
/// - **② 人类自己最后一条消息**（`senderKind == "user"`）—— 语义最贴他那句话，
///   但**没有分辨率**：他基本只在一个 crew 里说话，第二名就掉到三天前。
/// - **③ 他最后一次打开某个 crew** —— 语义最贴，**但今天没有这份数据**：
///   `CrewViewedStore.markViewed` 全仓只有一个调用点（侧栏底部「已隐藏的群」那一行）。
///
/// 选 ① 是因为它是唯一**诚实且有分辨率**的信号。真正能回答「手头上要用到的」是 ③，
/// 那要先补埋点、再攒几天数据 —— 换口径时改这一处的注入函数即可，推导层不用动。
///
/// ## agent 的排布是**覆盖层**，不是排序本身
/// #109 那条没被推翻：顺序由总机长那个 session 决定。但它只能**叠在**确定性基础序
/// 上面 —— agent 死了、跑飞了、压根没启动，侧栏照样按 ① 排得好好的。
/// **一个 agent 的判断可以决定「推荐你先看什么」，但不能决定「这台机器上有什么」。**
enum CrewChiefOverview {
    /// 一行：crew + 它的最新活动时间（nil = 从来没动静 / 时间戳脏）。
    struct Entry: Identifiable, Equatable, Sendable {
        let crew: CrewSummary
        let activity: Date?
        /// 这一行是不是被 agent 的排布顶上来的（视图可以据此加个轻标记）。
        var pinnedByArrangement: Bool = false

        var id: String { crew.id }
    }

    /// **确定性基础序**：按最新活动倒序，最新的在最上。
    ///
    /// - 没有活动时间的（nil）一律沉底，不跟有时间的混排。
    /// - 全序（一路比到 id）：同一份输入每次渲染顺序完全一致，行不会自己跳。
    static func baseOrder(
        crews: [CrewSummary],
        activity: (CrewSummary) -> Date?
    ) -> [Entry] {
        crews.map { Entry(crew: $0, activity: activity($0)) }.sorted(by: precedes)
    }

    /// 把 agent 排的那份顺序叠上去。
    ///
    /// - `arrangement` 里**存在的** crew 按它给的顺序排在最前，并标 `pinnedByArrangement`；
    /// - 没被提到的跟在后面，**保持基础序**；
    /// - `arrangement` 里已经不存在的 id **直接忽略**（crew 被删了/被藏了）——
    ///   一份过期的排布不该让任何一行消失，它顶多是「没顶上来」；
    /// - 空排布 / agent 没跑 → 原样返回基础序。**这条是这个覆盖层的地基**：
    ///   它挂了只是不够聪明，不是不能用。
    static func arranged(base: [Entry], arrangement: [String]) -> [Entry] {
        guard !arrangement.isEmpty else { return base }
        let byId = Dictionary(base.map { ($0.crew.id, $0) }, uniquingKeysWith: { a, _ in a })
        var seen = Set<String>()
        var pinned: [Entry] = []
        for id in arrangement where !seen.contains(id) {
            seen.insert(id)
            guard var entry = byId[id] else { continue }
            entry.pinnedByArrangement = true
            pinned.append(entry)
        }
        return pinned + base.filter { !seen.contains($0.crew.id) }
    }

    /// 一步到位：基础序 + 覆盖层。
    static func ordered(
        crews: [CrewSummary],
        activity: (CrewSummary) -> Date?,
        arrangement: [String] = []
    ) -> [Entry] {
        arranged(base: baseOrder(crews: crews, activity: activity), arrangement: arrangement)
    }

    private static func precedes(_ lhs: Entry, _ rhs: Entry) -> Bool {
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
