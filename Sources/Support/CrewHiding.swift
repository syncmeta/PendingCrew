import Foundation

/// 侧栏「**手动**把一个 crew 藏起来」的全部判定（纯函数、可单测）。
///
/// 语义：藏 = **从侧栏消失，聊天记录留着，不真删**。这是**人类界面**的概念，不是
/// 组织结构的改变 —— 藏了的 crew 里的 session 照常干活、照常收发消息，
/// `directory` / `contact` / 每轮注入的组织树一律不受影响。
///
/// **只做手动，不做自动判定。** 「空群自动解散」那条路当初为什么没走（判据是什么、
/// 实测命中几个）留档在 `docs/internal/2026-08-26-crew-hide-manual.md`，不是漏了。
///
/// ## 可见性不是「过滤掉被藏的那一行」
///
/// 侧栏是一棵 DAG 树。把被藏的父**从数组里滤掉**，它的子会因为「父不在本组」而被
/// 当成根**提到顶层**去 —— 那正是明确被否决的做法：侧栏显示的组织架构会和实际汇报
/// 线对不上，**人看到的树是假的**。宁可留一个空父在那儿，也别让组织图撒谎。
///
/// 所以可见性是一次**自根向下的可达性**计算：
/// - 自己被藏 → 不可见，且**不再往下传播**（整棵子树跟着消失）；
/// - 有父边时，至少要有**一个可见的父**才可见（多父 crew 只要还有一条露着的入口
///   就仍然看得见 —— 它确实还进得去）；
/// - 父边指向的 crew 不在这份名单里（跨机器/脏数据）→ 当根看待，与
///   `CrewDAGTreeView` 的口径一致。
enum CrewHiding {
    // MARK: - 可见性

    struct Visibility: Equatable {
        /// 侧栏该画出来的 crew id。
        let visible: Set<String>
        /// 被藏起来、但**入口这一层是露着的**那些 —— 取回它就会原地出现。
        /// 「已隐藏的群」列表列的就是这一批：藏在另一个被藏的群底下的那些不列，
        /// 因为单独取回它们什么也不会发生（列出来就是撒谎），等外层取回了自会露出来。
        let exposedHidden: Set<String>
    }

    /// 一次遍历同时算出「该画谁」和「该在已隐藏列表里列谁」。
    ///
    /// - Parameters:
    ///   - alsoHidden: 额外当成「被藏」处理（「藏了会怎样」的预演）。
    ///   - unhidden: 额外当成「没藏」处理（「取回了会怎样」的预演）。
    static func resolve(_ crews: [CrewSummary],
                        alsoHidden: Set<String> = [],
                        unhidden: Set<String> = []) -> Visibility {
        var childMap: [String: [String]] = [:]
        var known: Set<String> = []
        for crew in crews { known.insert(crew.id) }
        for crew in crews {
            for parent in crew.parentCrewIds where known.contains(parent) {
                childMap[parent, default: []].append(crew.id)
            }
        }
        func hidden(_ crew: CrewSummary) -> Bool {
            if unhidden.contains(crew.id) { return false }
            return crew.manuallyHiddenAt != nil || alsoHidden.contains(crew.id)
        }
        let hiddenIds = Set(crews.filter(hidden).map(\.id))

        // 根 = 没有一条父边指向这份名单里的 crew。
        let structuralRoots = crews
            .filter { !$0.parentCrewIds.contains(where: known.contains) }
            .map(\.id)

        // 走两遍。第一遍**完全不看谁被藏了**，只问「这个 crew 在结构上从根走得到吗」：
        // 走不到的只有一种成因 —— 脏数据成了环（`attachParent` 禁环，但持久化文件这条
        // 缝隙进得来，`LocalCrewStoreDepthTests` 就是从那儿注入的）。环上的 crew
        // **没人藏过它，它就不该从侧栏消失**，所以第二遍把它们也当根喂进去。
        //
        // 两遍不能合成一遍：合了就分不清「走不到是因为成环」还是「走不到是因为祖先
        // 被藏了」，后者恰恰是我们要的行为（藏了父，整棵子树跟着消失）。
        func reach(from seeds: [String], stopAtHidden: Bool) -> (visited: Set<String>, kept: Set<String>) {
            // `visited` 同时当环自保用：第一次访问就定型，重访不带来新信息。
            var visited: Set<String> = []
            var kept: Set<String> = []
            var stack = seeds
            while let id = stack.popLast() {
                guard !visited.contains(id) else { continue }
                visited.insert(id)
                if stopAtHidden, hiddenIds.contains(id) { continue } // 藏了就不往下传播
                kept.insert(id)
                stack.append(contentsOf: childMap[id] ?? [])
            }
            return (visited, kept)
        }

        let structural = reach(from: structuralRoots, stopAtHidden: false).visited
        let seeds = structuralRoots + crews.map(\.id).filter { !structural.contains($0) }
        let (exposed, visible) = reach(from: seeds, stopAtHidden: true)

        return Visibility(visible: visible, exposedHidden: exposed.intersection(hiddenIds))
    }

    /// 侧栏该画的 crew（保持入参顺序，排序归调用方原来那套）。
    static func visible(_ crews: [CrewSummary]) -> [CrewSummary] {
        // 一个都没藏是绝大多数时候的情形 —— 直接原样返回，连遍历都省了。
        guard crews.contains(where: { $0.manuallyHiddenAt != nil }) else { return crews }
        let ids = resolve(crews).visible
        return crews.filter { ids.contains($0.id) }
    }

    /// 藏 `crewId` 这一下会让哪些**此刻可见**的 crew 从侧栏消失（含它自己）。
    static func vanishing(ifHiding crewId: String, in crews: [CrewSummary]) -> Set<String> {
        resolve(crews).visible.subtracting(resolve(crews, alsoHidden: [crewId]).visible)
    }

    /// 取回 `crewId` 会让哪些**此刻不可见**的 crew 回到侧栏（含它自己）。
    /// 也就是「它藏起来的时候，跟着它一起消失的那批」—— 未读要看的正是这一批。
    static func returning(ifUnhiding crewId: String, in crews: [CrewSummary]) -> Set<String> {
        resolve(crews, unhidden: [crewId]).visible.subtracting(resolve(crews).visible)
    }

    // MARK: - 能不能藏（④ 子 crew）

    enum HideDecision: Equatable {
        /// 拦住 —— 这些子 crew **有 session 正在跑**，藏了父它们就没有入口进去了。
        case blocked(activeDescendantTitles: [String])
        /// 可以藏。`alsoHiddenCount` = 跟着一起消失的子 crew 数（0 = 就它自己）。
        case allowed(alsoHiddenCount: Int)
    }

    /// 「这个 crew 现在能不能藏」。
    ///
    /// **选的是「拦住」而不是「连子树一起藏」**（两条路的取舍记在
    /// `docs/internal/2026-08-26-crew-hide-manual.md`）：藏的语义是「这事完了、
    /// 不需要再看见」，而底下有 session 正在跑恰恰说明**没完**。一次右键就把一整棵
    /// 还在干活的子树从侧栏抹掉，人下一秒想去看那个正在跑的子 crew 会找不到入口 ——
    /// 这跟「删了」在体感上没有区别。拦住只让人多做一步（先停掉、或先藏子再藏父），
    /// 而且理由当场说得清。
    ///
    /// 子 crew **全都闲着**时不拦：它们跟着父一起消失，取回父就整棵回来。
    static func decide(hiding crewId: String, in crews: [CrewSummary],
                       activeSessionCrewIds: Set<String>) -> HideDecision {
        let vanishing = vanishing(ifHiding: crewId, in: crews).subtracting([crewId])
        let blockers = crews
            .filter { vanishing.contains($0.id) && activeSessionCrewIds.contains($0.id) }
            .map(\.title)
            .sorted()
        if !blockers.isEmpty { return .blocked(activeDescendantTitles: blockers) }
        return .allowed(alsoHiddenCount: vanishing.count)
    }

    // MARK: - 「已隐藏的群」列表 + 未读（⑤）

    struct HiddenEntry: Identifiable, Equatable {
        let crew: CrewSummary
        /// 藏起来的时刻（解不出来 → nil，排最后）。
        let hiddenAt: Date?
        /// 跟着它一起消失的子 crew 数（0 = 就它自己）。
        let alsoHiddenCount: Int
        /// 藏起来之后（且上次看过之后）这棵子树里有新消息。
        let hasUnread: Bool

        var id: String { crew.id }
    }

    /// 「已隐藏的群」那行展开后要列的内容。
    ///
    /// 未读 = **末条白板消息时间 > max(manuallyHiddenAt, 该 crew 的 lastViewed)**，
    /// 且要看**整棵跟着消失的子树**（藏了父之后，子里来的消息一样是人看不见的动静）。
    ///
    /// - Parameters:
    ///   - lastActivity: crewId → 末条消息时间。**必须喂
    ///     `CrewStore.lastWhiteboardMessages` 那份后台算好的快照**；在这条路上现读
    ///     `LocalWhiteboardStore.list(crewId:)` 解整板 JSON 就是 2026-08-17
    ///     「开久了卡」的完整复现。
    ///   - lastViewed: crewId → 上次点进去看过的时刻（`CrewViewedStore`，UserDefaults）。
    ///     藏起来 ≠ 不再关心：人点进去看过一眼，那个未读提示就该消失 ——
    ///     **一个不可信的计数比没有计数糟**。
    static func hiddenEntries(in crews: [CrewSummary],
                              lastActivity: (String) -> Date?,
                              lastViewed: [String: Date]) -> [HiddenEntry] {
        // 一个都没藏是绝大多数时候的情形 —— 侧栏每次重绘都会问一次，先挡掉。
        guard crews.contains(where: { $0.manuallyHiddenAt != nil }) else { return [] }
        let exposed = resolve(crews).exposedHidden
        guard !exposed.isEmpty else { return [] }
        return crews
            .filter { exposed.contains($0.id) }
            .map { crew in
                let hiddenAt = crew.manuallyHiddenAt.flatMap(CrewTimestamp.parse)
                let subtree = returning(ifUnhiding: crew.id, in: crews)
                // 参照点取两者中较晚的：藏的时刻、以及上次点进去看过的时刻。
                let since = [hiddenAt, lastViewed[crew.id]].compactMap { $0 }.max()
                let unread = subtree.contains { id in
                    guard let at = lastActivity(id) else { return false }
                    guard let since else { return true }
                    return at > since
                }
                return HiddenEntry(
                    crew: crew,
                    hiddenAt: hiddenAt,
                    alsoHiddenCount: subtree.subtracting([crew.id]).count,
                    hasUnread: unread)
            }
            // 最近藏的排最上（解不出时间的垫底）。**不按有没有未读排** —— 来条消息就
            // 让某一行窜到顶，是「藏起来」这个决定被推翻的另一种形式。
            .sorted { a, b in
                switch (a.hiddenAt, b.hiddenAt) {
                case let (x?, y?): return x == y ? a.crew.title < b.crew.title : x > y
                case (nil, _?): return false
                case (_?, nil): return true
                case (nil, nil): return a.crew.title < b.crew.title
                }
            }
    }
}
