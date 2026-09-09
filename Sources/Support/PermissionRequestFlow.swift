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
/// # ⚠️ 已知读不出来的（钉在测试里，免得被当成已解决）
///
/// **条件同意会被读成无条件同意**（「可以，但先备份」→ `.granted`）。真要治得让人在
/// Todo 上点「同意 / 不同意」而不是写自由文本 —— 那是另一单。这里写下来，是因为
/// 一个没被说出来的盲区，跟一个不存在的盲区长得一模一样。
enum PermissionGrantReading {
    enum Reading: Equatable { case granted, refused, unclear }

    /// 拒绝词先判 —— 「不同意」里含着「同意」，先判同意会把它读反。
    private static let refusals = ["不行", "不可以", "不同意", "别跑", "别动", "拒绝", "deny", "no"]
    private static let grants = ["同意", "可以", "行", "好", "批准", "放行", "跑吧", "allow", "yes", "ok"]

    static func read(_ reply: String) -> Reading {
        let text = reply.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !text.isEmpty else { return .unclear }
        if refusals.contains(where: { text.contains($0) }) { return .refused }
        if grants.contains(where: { text.contains($0) }) { return .granted }
        return .unclear
    }
}
