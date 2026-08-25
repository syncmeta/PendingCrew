import Foundation

/// 侧栏「层级视图」的排序推导（人类 Todo #67）。纯 Foundation、不碰 store / SwiftUI、可单测。
///
/// 人类原话：「sidebar crew 列表 层级 按照最新更新时间而非创建时间排序 从新到旧」。
///
/// # 两条定死的口径
///
/// **① 「最新更新时间」= 那个 crew 白板最后一条消息的时间**，不是 crew 记录自身的
/// `updatedAt` —— 后者被改名、绑定这类事情碰一下就会跳，于是「改个名就窜到顶」，
/// 那不是人心里「这个群有没有动静」的意思。取不到消息时间的回落 `createdAt`
/// （**不是 1970**）：一个从没说过话的 crew 应该待在它该待的年代里，而不是沉到世界尽头。
/// 具体怎么取由调用方注入（`activity` 闭包）—— 这一层不碰 IO。
///
/// **② 父的排序键 = max(自己, 全部后代)。** 一个安静的父部门底下有个正在刷屏的子
/// 部门，如果父按自己的时间沉到底，**那个正在动的子部门就找不到了** —— 层级结构下
/// 这是必然发生的情形，不是边角料。所以活跃度**向上冒泡**。
///
/// # 为什么这里只收一个闭包、一个字节的白板都不读
///
/// 侧栏行以前是在 SwiftUI body 里、主线程上、每个 crew 各解一份整板 JSON ——
/// 那是 2026-08-17「开久了卡」的头号病根（见 `CrewSidebarCrewRow` 里那段注释）。
/// 现在时间一律来自 `CrewStore.lastWhiteboardMessages` 那份**后台按指纹门控算好、
/// 真变了才发布**的快照。**这一层做错的症状不是排序错，是整个 app 重新变卡**，
/// 而且会被当成新问题查半天 —— 所以这里定成纯函数，想读盘都没处读。
enum CrewHierarchyOrdering {

    /// 每个 crew 的排序键：`max(自己的活动时间, 全部后代的活动时间)`。
    ///
    /// **环自保**：crew 的父边是 DAG，理论上不该有环（`attachParent` 禁了），但脏
    /// 数据不该让侧栏转不出来。这里用**逐轮松弛**（把子的键往父上抬，抬到一轮没有
    /// 任何变化为止，最多 crew 个数轮）而不是递归下钻 —— 环在这个算法里天然收敛，
    /// 不需要靠「路径去重」那种一不小心就漏一支的守卫。
    static func activityKeys(
        crews: [CrewSummary],
        activity: (CrewSummary) -> Date?
    ) -> [String: Date] {
        var keys: [String: Date] = [:]
        for crew in crews {
            if let date = activity(crew) { keys[crew.id] = date }
        }
        // 父边：child → 它的那些父。松弛时沿这条边把时间往上抬。
        let edges: [(child: String, parent: String)] = crews.flatMap { crew in
            crew.parentCrewIds.map { (child: crew.id, parent: $0) }
        }
        guard !edges.isEmpty else { return keys }
        for _ in 0..<max(1, crews.count) {
            var changed = false
            for edge in edges {
                guard let childKey = keys[edge.child] else { continue }
                if let parentKey = keys[edge.parent] {
                    if childKey > parentKey { keys[edge.parent] = childKey; changed = true }
                } else {
                    keys[edge.parent] = childKey
                    changed = true
                }
            }
            if !changed { break }
        }
        return keys
    }

    /// 一批**同级** crew 按键从新到旧排。
    ///
    /// 兜底与时间流视图（`CrewTimelineOrdering.ordered`）**同一套**：没有键的沉底、
    /// 同一时刻按标题本地化升序再按 id —— 保证顺序稳定，不会因为字典遍历顺序每次
    /// 渲染跳来跳去。两个视图对「谁更新」的说法必须一致，否则同一个人在两个 tab 里
    /// 会看到两种事实。
    static func sortedSiblings(_ crews: [CrewSummary], keys: [String: Date]) -> [CrewSummary] {
        crews.sorted { lhs, rhs in
            switch (keys[lhs.id], keys[rhs.id]) {
            case let (l?, r?):
                if l != r { return l > r }
            case (nil, _?): return false
            case (_?, nil): return true
            case (nil, nil): break
            }
            let byTitle = lhs.title.localizedCompare(rhs.title)
            if byTitle != .orderedSame { return byTitle == .orderedAscending }
            return lhs.id < rhs.id
        }
    }

    /// `parentId → [子 crew]`，**每个父下各自排好序**。
    ///
    /// 多父 crew 在每个父下各出现一次（引用式，不是唯一归属），所以排序必须**逐个
    /// 父分别成立** —— 只对第一处生效是这类结构最容易漏的地方。
    static func sortedChildMap(
        crews: [CrewSummary],
        keys: [String: Date]
    ) -> [String: [CrewSummary]] {
        var map: [String: [CrewSummary]] = [:]
        for crew in crews {
            for parentId in crew.parentCrewIds { map[parentId, default: []].append(crew) }
        }
        return map.mapValues { sortedSiblings($0, keys: keys) }
    }
}
