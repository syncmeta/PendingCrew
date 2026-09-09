import Foundation

/// 权限类并进 Todo 之后，撞到 gate 的那一刻该怎么办（驾驶舱计划 #75 ②）。
///
/// 人类原话：「权限类可也可以用 todo 来做，如果实在需要人类，也应该通过 todo 引导。」
/// 旧路是 raise 一条待审批 + **阻塞 long-poll 最多一小时**；新路是当场拒、提一条
/// Todo 说清「我要跑 X、为了 Y」，agent 去干别的，人同意后它再来跑。
///
/// # 这里的两条判定都是为了不把改动做成「比现状更糟」
///
/// **hook 每次都重新判，它不知道人已经同意过。** 所以天真的实现会是：
/// 拒 → 提 Todo → 人同意 → agent 重跑 → **又被拒、又提一条 Todo** → …
/// **无限循环，而且每转一圈往人类账上加一条垃圾。**
///
/// 两条判定各堵一半：
/// - `hasGrant` → 放行并**当场把票用掉**（一次同意 = 一次放行）。
/// - `hasPendingRequest` → 已经提过了就别再提，**只拒不写**。
///
/// 票优先于「还挂着一条请求」：人已经同意了，不该被自己那条还没标完的记录挡在外面。
enum PermissionRequestFlow {
    enum Decision: Equatable {
        /// 有票：放行，并把票作废。
        case allowConsumingGrant
        /// 拒绝，但**不再提新的 Todo**（已经有一条挂着了）。
        case denyWithoutFiling
        /// 拒绝，并提一条 Todo 引导人。
        case denyAndFile
    }

    static func decide(hasGrant: Bool, hasPendingRequest: Bool) -> Decision {
        if hasGrant { return .allowConsumingGrant }
        return hasPendingRequest ? .denyWithoutFiling : .denyAndFile
    }
}

/// 从人对那条 Todo 的**自由文本**回应里读「同意了没有」。
///
/// # 方向是保守的，而且这个方向不对称
///
/// 读不准就当**没同意**。把「看不懂」读成同意，代价是替人放行了一件他没点头的事；
/// 读成没同意，最坏只是让 agent 再问一次。**权限这件事上这两个代价不对等**，所以
/// 判据只认明确的词，其余一律 `.unclear`。
///
/// # 条件同意也按**不同意**处理
///
/// 「可以，但先备份」跟「看不懂」是同一类：**他点的头和你要做的事不完全是一回事**。
/// 放行 = 不备份就跑了；不放行 = 再问一次。**代价仍然不对等**，所以上面那条不对称
/// 原样适用。判据宁可宽 —— 宽的那一边代价便宜，这一点上面已经论证过。
///
/// 第一版把它当成已知盲区**钉成了期望值**（断言「条件同意 → granted」）。那是错的
/// 两层：① 原则本来就推得出答案，只是我停在了前一步；② **一个被写成测试的盲区，
/// 跟一个被设计成这样的行为长得一模一样** —— 下一个人读到那条绿断言会以为是设计。
///
/// # 真正剩下的盲区（这条才是没解的）
///
/// 判据只看词，**读不出反讽**（「行啊，你随便」）。要根治得让人在 Todo 上点按钮而不是
/// 写自由文本 —— 那是另一单。
enum PermissionGrantReading {
    enum Reading: Equatable { case granted, refused, unclear }

    /// 拒绝词先判 —— 「不同意」里含着「同意」，先判同意会把它读反。
    private static let refusals = ["不行", "不可以", "不同意", "别跑", "别动", "拒绝", "deny", "no"]
    /// **限定词 = 不是无条件同意**。出现任何一个就不放行（见类型注释里那条不对称）。
    private static let qualifiers = ["但", "不过", "前提", "先", "除非", "只要", "如果", "条件",
                                     "记得", "务必", "注意", "小心", "别忘", "however", "but ", "first"]
    private static let grants = ["同意", "可以", "行", "好", "批准", "放行", "跑吧", "allow", "yes", "ok"]

    static func read(_ reply: String) -> Reading {
        let text = reply.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !text.isEmpty else { return .unclear }
        if refusals.contains(where: { text.contains($0) }) { return .refused }
        // 限定词优先于同意词：「可以，但先备份」里两边都命中，而它**不是**无条件同意。
        if qualifiers.contains(where: { text.contains($0) }) { return .unclear }
        if grants.contains(where: { text.contains($0) }) { return .granted }
        return .unclear
    }
}
