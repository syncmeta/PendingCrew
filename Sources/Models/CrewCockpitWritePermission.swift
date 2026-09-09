import Foundation

/// 谁写得动驾驶舱那本账（人类 Todo #115 的 (D)）。
///
/// ## 起因：一道 `guard` 把四个不同的动作当成了一件事
///
/// `plan_add` / `plan_update` 开头都是 `guard isCaptain`。于是 worker 标
/// `progress` / `blocked` 会被直接拒 —— **而群里绝大多数消息是 worker 发的**，
/// 人类要的「账能及时更新」在那半边根本不成立。
///
/// ## 分法对应一个本来就存在的组织事实
///
/// **报进度的人，和决定「这条存不存在 / 这条算不算完成」的人，本来就不是同一个人。**
///
/// | 动作 | 谁能做 | 为什么 |
/// |---|---|---|
/// | 新增一条计划 | 只有机长 | 板上有哪些条目 = 机长的编排权，也是防淹的闸 |
/// | 追加进展 | worker 也能 | 干活的人才知道进展 —— 这正是「及时」的来源 |
/// | 标卡住 | worker 也能 | 卡住了要立刻可见，等机长转述就晚了 |
/// | 翻完成 | 只有机长 | **完成是验收判断，不是自我声明**（跟凭据闸同一个道理） |
///
/// 防淹那条不是空话：43 个 crew × 每个好几个 worker 全往一块板上**新增条目**，
/// 板会被淹，而人类看的是一块板 —— 淹了等于没有。**追加进展不会淹**（它挂在
/// 已有条目下面），**新增条目会**。
enum CrewCockpitWritePermission {

    enum Decision: Equatable {
        case allowed
        /// 不许写，并把这句话原样回给调用方（**必须给出路**）。
        case refused(String)
    }

    static func decide(category: CrewMessageCategory, isCaptain: Bool) -> Decision {
        guard category.ledger == .cockpit else { return .allowed }
        if isCaptain { return .allowed }
        switch category {
        case .progress, .blocked:
            return .allowed
        case .plan:
            return .refused(
                "`plan` 是**往驾驶舱新增一条计划**，只有机长能做 —— 板上有哪些条目是"
                + "机长的编排权（也是防淹：几十个 worker 各自新增，人类看的那块板就废了）。"
                + "\n**出路**：① 这件事本来就是机长派给你的，那它多半已经在板上了 —— "
                + "用 `progress` 挂到那条计划的 `#N` 上；"
                + "\n② 真是一件板上没有的新事，在群里说一句让机长排（他排完会把 `#N` 给你）；"
                + "\n③ 它其实不构成一件要推进的事，那它是 `finding`。")
        case .done:
            return .refused(
                "`done` 是**把一条计划翻成完成**，只有机长能做 —— **完成是验收判断，"
                + "不是自我声明**（跟「翻 completed 必须带凭据」是同一个道理）。"
                + "\n**出路**：把你做完的事按 `progress` 报上去、带上凭据（commit / 全量读数），"
                + "机长核完会翻。**你报的是事实，他判的是算不算完成 —— 这两件事本来就不该同一个人做。**")
        case .humanTodo, .todoResponse, .handoff, .ack, .question, .finding, .note, .system:
            return .allowed
        }
    }
}

// MARK: - 落账前的纯解析

/// 分类 → 驾驶舱那本账的**纯解析层**（不碰 IO）。
///
/// 只回答三件事：计划号是多少、这条消息该叫什么标题、卡点指向哪本账的第几条。
/// 真正的写在 `McpServer.landOnCockpit` 里走 `CockpitPlanStore` 现成的入口 ——
/// **不新造第三本账，也不新造第二个写入口。**
enum CrewCockpitLanding {

    /// JSON 数字经 `JSONSerialization` 可能是 Int / Double / NSNumber，三种都收。
    static func number(_ raw: Any?) -> Int? {
        if let i = raw as? Int { return i }
        if let n = raw as? NSNumber { return n.intValue }
        if let d = raw as? Double { return Int(d) }
        if let s = raw as? String { return Int(s.trimmingCharacters(in: .whitespaces)) }
        return nil
    }

    /// 群消息 → 计划标题。
    ///
    /// **复用折叠那把尺子**（`CrewMessageFold.derivedSummary` + 同一个 `summaryCap`），
    /// 不另写一套截断规则：同一段文字在气泡收起态和板上条目标题上应该长得一样，
    /// 两把尺子迟早会分叉，而分叉之后没有人会发现。
    static func title(from message: String) -> String {
        if let derived = CrewMessageFold.derivedSummary(message) { return derived }
        let firstLine = message
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty } ?? ""
        let stripped = firstLine
            .trimmingCharacters(in: CharacterSet(charactersIn: "#>-*` \t"))
        if stripped.count <= CrewMessageFold.summaryCap { return stripped }
        return String(stripped.prefix(CrewMessageFold.summaryCap)) + "…"
    }

    /// `blocked_by_ledger` 的取值。两处共用（`plan_update` 与分类落账），
    /// 免得同一个字段在两条路上认不同的词。
    enum LedgerChoice: Equatable {
        case ok(String)
        case refused(String)
    }

    static func blockerLedger(_ raw: Any?) -> LedgerChoice {
        let value = ((raw as? String) ?? "human")
            .trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard ["human", "agent"].contains(value) else {
            return .refused("blocked_by_ledger 只能是 human（你请人类拍板那本）或 agent（人类派给你那本）。")
        }
        return .ok(value)
    }

    /// 标 `progress` 时该不该顺手翻状态。
    ///
    /// **只把「没做」翻成「进行中」，其余一律不动。** 反面很具体：一条 `blocked` 的
    /// 计划被人顺手报了一句进度，如果这里翻回 `in_progress`，
    /// `CockpitPlan.validate` 会**跟着把卡点引用清掉** —— 而人类看板看的正是那个引用
    /// （「卡在哪条待我拍板的事上」）。**报进度不等于解了卡**，解卡是另一个动作。
    static func statusForProgress(current: CockpitPlanStatus?) -> String? {
        current == .notStarted ? CockpitPlanStatus.inProgress.rawValue : nil
    }
}
