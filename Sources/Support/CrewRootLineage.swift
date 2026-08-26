import Foundation

/// 「这个 crew 挂在哪些**根 crew** 之下」—— crew 名字后面那行黄字标注的单一真值。
/// 纯逻辑（Foundation only，无 SwiftUI），侧栏 / crew 列表 / 群聊标题三处共用，单测钉死。
///
/// 标的是**根祖先**（组织树最顶层），不是直接父 crew：`C ← B ← A` 时 C 标的是 A。
/// crew 组织是 DAG（一个 crew 可以认多个父，见 `LocalCrewStore.attachParent`），所以
/// 返回的是**集合而不是单个**：血缘层永远保留全部根；展示层能放下就全显，放不下
/// 才用准确的 `+N` 降级。这里不提前丢任何一个根。
enum CrewRootLineage {
    /// `crewId` 的全部根祖先 id（**不含自己**），按 DFS 前序去重 —— 父边的声明顺序
    /// 就是加入顺序（`attachParent` 是 append），所以输出顺序 = 加入顺序。
    ///
    /// - Parameters:
    ///   - crewId: 要求根的 crew。
    ///   - parents: 每个**已知** crew 的父边表。key 的存在性即「这个 crew 存在」——
    ///     所以传进来的字典必须覆盖全部已知 crew（缺 key 的 id 一律当不存在）。
    /// - Returns: 根 crew id 列表。`crewId` 自己就是根 → 空数组（自己 @ 自己没意义，
    ///   调用方据此不画标注）。
    ///
    /// 边界（都有单测钉住，别让它崩）：
    /// - 未知 `crewId` → 空数组。
    /// - 父 id 不在 `parents` 里（脏数据 / 半同步）→ 那条边当不存在。若某 crew 的父边
    ///   **全是**这种脏引用，它自己就当根 —— 与 `LocalCrewStore.orgTreeLines` 的兜底
    ///   同一口径（别因为一条脏引用把整条谱系判丢）。
    /// - 环（手改 JSON 才可能，`attachParent` 正常路径禁环）：路径 visited 守卫保证
    ///   **一定返回**；纯环上找不到根 → 空数组（那行不标注），不崩、不挂死。
    static func rootIds(of crewId: String, parents: [String: [String]]) -> [String] {
        guard parents[crewId] != nil else { return [] }
        var roots: [String] = []
        var seen: Set<String> = []

        func walk(_ current: String, _ path: Set<String>) {
            // 只沿**存在的**父边往上；脏引用当边不存在。
            let known = (parents[current] ?? []).filter { parents[$0] != nil }
            guard !known.isEmpty else {
                // 到顶了。自己是根 → 不产出（不给自己加 @自己 后缀）。
                guard current != crewId else { return }
                if seen.insert(current).inserted { roots.append(current) }
                return
            }
            let nextPath = path.union([current])
            for parent in known where !nextPath.contains(parent) {
                walk(parent, nextPath)
            }
        }
        walk(crewId, [])
        return roots
    }

    /// `rootIds` 的展示版：直接给标题。喂 `CrewStore.crews`（全量 crew 行）即可；
    /// 列表里查不到标题的 id 会被丢掉（宁可少标一个，也不显示裸 uuid）。
    ///
    /// **本地血缘优先，算不出才用 `CrewSummary.rootCrewTitles` 那份**。后者原是
    /// 服务端下发的，给看不到本地 DAG 的 iPad/iPhone 用；#63 第二期删掉云端整层
    /// 之后恒空，这条回退分支因此不再会被走到 —— 但判定本身留着，重建前后端时
    /// 第二个来源会重新出现在这个位置。
    static func rootTitles(of crewId: String, in crews: [CrewSummary]) -> [String] {
        guard !crews.isEmpty else { return [] }
        var parents: [String: [String]] = [:]
        var titles: [String: String] = [:]
        parents.reserveCapacity(crews.count)
        titles.reserveCapacity(crews.count)
        for crew in crews {
            parents[crew.id] = crew.parentCrewIds
            titles[crew.id] = crew.title
        }
        let local = rootIds(of: crewId, parents: parents).compactMap { titles[$0] }
        if !local.isEmpty { return local }
        return crews.first(where: { $0.id == crewId })?.rootCrewTitles ?? []
    }

    /// 一次把整张列表的标注算完：`crewId → [根标题]`（无根的 crew 不进字典）。
    ///
    /// 列表视图用这个，别在每一行里各调一次 `rootTitles` —— 那样每渲染一行就重建
    /// 一遍父边表，行数一多就是 O(n²) 的白工。**喂全量 crew**（不是某台机器的分组
    /// 子集）：父边可以跨机器，只喂子集会把跨机器的那条谱系判丢。
    static func rootTitlesByCrew(in crews: [CrewSummary]) -> [String: [String]] {
        guard !crews.isEmpty else { return [:] }
        var parents: [String: [String]] = [:]
        var titles: [String: String] = [:]
        parents.reserveCapacity(crews.count)
        titles.reserveCapacity(crews.count)
        for crew in crews {
            parents[crew.id] = crew.parentCrewIds
            titles[crew.id] = crew.title
        }
        var out: [String: [String]] = [:]
        for crew in crews {
            // 本地血缘优先，算不出才用服务端下发的那份 —— 口径与 `rootTitles` 同源，
            // 两边都改才算改。
            let local = rootIds(of: crew.id, parents: parents).compactMap { titles[$0] }
            let roots = local.isEmpty ? crew.rootCrewTitles : local
            if !roots.isEmpty { out[crew.id] = roots }
        }
        return out
    }

    /// 标注文案：`@根1 @根2`（空格分隔，顺序即传入顺序）。无根 → nil（不画）。
    /// 三处展示点共用同一个拼法，避免各写一遍拼出不一样的分隔符。
    static func badgeText(rootTitles: [String]) -> String? {
        CrewRootBadgePresentation.candidateTexts(rootTitles: rootTitles).first
    }
}

/// 根 crew 黄字标注的**展示降级顺序**。这里不估算字体宽度；SwiftUI layout 用每个
/// 候选 `Text` 的真实 intrinsic size 调 `candidateIndexThatFits`，所以动态字体、中文、
/// 英文和不同平台都走同一条规则。
enum CrewRootBadgePresentation {
    /// 用**同一标题字体真实量出来**的名字宽度下限探针。四个全角字 + 省略号约等于
    /// 4–6 个显示字符；这里只提供被 SwiftUI `Layout` 测量的文字，不把点数写死。
    static let titleMinimumWidthProbe = "名字名字…"

    /// 从信息最完整到最精简列候选：
    /// `@A @B @C` → `@A @B +1` → `@A +2`。从不造省略号或裸 `@`。
    static func candidateTexts(rootTitles: [String]) -> [String] {
        let usable = rootTitles
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        guard !usable.isEmpty else { return [] }

        var candidates = [usable.map { "@\($0)" }.joined(separator: " ")]
        guard usable.count > 1 else { return candidates }
        for keptCount in stride(from: usable.count - 1, through: 1, by: -1) {
            let kept = usable.prefix(keptCount).map { "@\($0)" }.joined(separator: " ")
            candidates.append("\(kept) +\(usable.count - keptCount)")
        }
        return candidates
    }

    /// 选第一个真实宽度能完整放下的候选；一个都放不下就返回 nil，整条黄字不画。
    static func candidateIndexThatFits(widths: [Double], availableWidth: Double) -> Int? {
        let available = max(0, availableWidth)
        return widths.firstIndex { $0 <= available + 0.5 }
    }

    struct RowAllocation: Equatable {
        let titleWidth: Double
        let badgeCandidateIndex: Int?
    }

    /// 标题与黄字共用一行时的预算规则。`titleMinimumWidth`、`badgeWidths` 都来自
    /// SwiftUI 对真实字体的测量；这里只决定谁拿多少，不估算字符宽度。
    ///
    /// 标题完整宽度放不下时，先保证它至少拿到下限；黄字只用剩余预算挑完整候选。
    /// 连第一个完整父名都放不下时黄字撤掉，标题拿回整行。
    static func rowAllocation(
        titleIdealWidth: Double,
        titleMinimumWidth: Double,
        badgeWidths: [Double],
        availableWidth: Double,
        spacing: Double
    ) -> RowAllocation {
        let available = max(0, availableWidth)
        let requiredTitle = min(max(0, titleIdealWidth), max(0, titleMinimumWidth))
        guard available + 0.5 >= requiredTitle else {
            return RowAllocation(titleWidth: available, badgeCandidateIndex: nil)
        }

        let badgeBudget = max(0, available - requiredTitle - max(0, spacing))
        guard let index = candidateIndexThatFits(
            widths: badgeWidths,
            availableWidth: badgeBudget
        ) else {
            return RowAllocation(titleWidth: available, badgeCandidateIndex: nil)
        }

        let titleWidth = max(0, available - max(0, badgeWidths[index]) - max(0, spacing))
        return RowAllocation(titleWidth: titleWidth, badgeCandidateIndex: index)
    }
}
