import Foundation

/// 侧栏「总机长」视图每行显示的那句话（人类 Todo #136）。
///
/// 人类原话：「机长在往群里发消息的时候，给它填一个字段 这个字段就是直接显示于
/// 原来的最新消息这里的」。
///
/// ## 它比「总机长写的总结」那一版好在哪（这一段解释了这里为什么这么简单）
/// 上一版是总机长在**另一个时刻**给别人写总结 —— 那种东西会比最新消息更旧、会烂，
/// 得配一整套「带时间戳 / 太旧要标出来」的防护。
/// 这一版的状态**和消息是同一次动作产生的**，所以它的时效性天生等于那条消息的
/// 时效性，**永远不会比「最新消息」更旧**。那套防护因此整个不需要。
///
/// ## 但有一个残留的陈旧面，这里必须交代
/// 「这次没填就沿用上一次填的」意味着：**沿用来的那句可能来自三天前那条消息，
/// 而群里已经有更新的消息了。** 它不会凭空烂（不是二手判断），但会随时间脱节。
/// 所以这里**仍然把年龄摆出来** —— 沿用本身没错，错的是让人以为它是刚刚的。
/// 一个默默沿用三天的状态会变成「一直亮着的背景」。
///
/// **不在发送端拦**（比如「隔太久没填就提示」）：那会逼人填一句假的。
enum CrewStatusLine {
    /// 一行要显示的东西。
    struct Line: Equatable, Sendable {
        /// 主文案（年龄已经拼进去了；视图直接画，别再自己拼一遍）。
        let text: String
        /// 这一行是不是「一次都没填过」。视图可以据此画得更淡。
        let isMissing: Bool
    }

    /// 从**这个 crew 的消息里**找出当前生效的那句状态。
    ///
    /// 判据只有一条：**从最新往回找第一条带非空 `crewStatus` 的消息**。
    /// - 找到 → 那句话 + 那条消息的时间（不是「现在」）；
    /// - 一条都没有 → nil。**nil 是「他一次都没填过」，不是「他填了个空的」** ——
    ///   全空白的状态在这里就当没填（写了个空格就算填过，是最廉价的一种假账）。
    ///
    /// `messages` 按时间升序（白板本来就是这个顺序）。
    static func resolve(messages: [(status: String?, createdAt: String)])
        -> (text: String, at: Date?)? {
        for message in messages.reversed() {
            let body = (message.status ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !body.isEmpty else { continue }
            return (body, CrewTimestamp.parse(message.createdAt))
        }
        return nil
    }

    /// 渲染成那一行。
    ///
    /// - Parameters:
    ///   - resolved: `resolve` 的结果；`nil` = 一次都没填过。
    ///   - now: 现在（算年龄用；测试注入）。
    static func make(resolved: (text: String, at: Date?)?, now: Date) -> Line {
        guard let resolved else {
            // **不编**。「还没有」本身是有用的信息：这个机组的机长还没报过状态。
            return Line(text: "还没有", isMissing: true)
        }
        guard let at = resolved.at else {
            // 有话、但那条消息的时间戳解析不出来 —— 说清「不知道多久前」，
            // 别默默当成刚刚的。
            return Line(text: "（不知道多久前）\(resolved.text)", isMissing: false)
        }
        return Line(text: "\(ageText(now.timeIntervalSince(at)))：\(resolved.text)",
                    isMissing: false)
    }

    /// 年龄文案。**永远在正文里**，不是 tooltip —— tooltip 要悬停才看得见，
    /// 而「这句话是多久前报的」是「它还算不算数」的前提。
    static func ageText(_ age: TimeInterval) -> String {
        if age < 60 { return "刚刚" }
        if age < 3600 { return "\(Int(age / 60)) 分钟前" }
        if age < 86_400 { return "\(Int(age / 3600)) 小时前" }
        return "\(Int(age / 86_400)) 天前"
    }
}

/// 写侧：`post_to_crew(crew_status:)` 收下来的那句话（人类 Todo #136）。
///
/// 读侧规则在上面（`CrewStatusLine`），写侧的判定放在同一个文件里 ——
/// 同一个字段的两头分开住，两套口径迟早会打架。
enum CrewStatusIntake {

    /// 侧栏那一行大概露得出多少字。**它是提醒线，不是合法性判据** ——
    /// 超了照写、回执提醒一句。理由：显示宽度不该变成写入端的合法性判据，
    /// 那行以后变宽了，拒收线不会跟着变，于是它会开始拒掉本来能显示的内容。
    static let hintLength = 40
    /// 硬闸。防的是「有人把整段进展粘进来」——那不是一句状态，是一篇报告。
    static let maxLength = 200

    enum Decision: Equatable {
        /// 没填。
        case none
        /// 收下这句话；`hint` 非 nil 时回执里带一句提醒（**照样写进去了**）。
        case accepted(String, hint: String?)
        /// 拒收，并把这句话原样回给调用方。**整条消息都不发** ——
        /// 半截状态（消息发了、状态没落）会让侧栏显示一句过期的话，
        /// 而看的人以为那是刚报的。
        case refused(String)
    }

    /// - Parameters:
    ///   - raw: `crew_status` 参数。
    ///   - isCaptain: 只有机长填得了。侧栏那一行是**这个机组**的状态，
    ///     而机长是唯一为整个机组说话的人；worker 报的是它自己那件活的进展，
    ///     让它写进去，侧栏就会拿一条 worker 的近况冒充整组的状态。
    static func decide(_ raw: Any?, isCaptain: Bool) -> Decision {
        let text = ((raw as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return .none }
        guard isCaptain else {
            return .refused("`crew_status` 只有机长填得了 —— 侧栏那一行是**整个机组**的状态，"
                + "你报的是自己手上这件活。\n**出路**：把这句话写进正文（或标 `progress` "
                + "挂到计划号上），机长会在他下一条发言里把整组的状态报上去。")
        }
        guard text.count <= maxLength else {
            return .refused("`crew_status` 最多 \(maxLength) 字，收到 \(text.count) 字 —— "
                + "这么长的不是一句状态，是一篇报告。\n**出路**：报告发正文，"
                + "`crew_status` 只留一句「现在整组在干什么」。**这条消息也没有发出去。**")
        }
        guard text.count <= hintLength else {
            return .accepted(text, hint: "`crew_status` \(text.count) 字，"
                + "侧栏那一行大概露得出 \(hintLength) 字左右，后面会被截掉 —— "
                + "**已经照原样写进去了**，只是提醒你前 \(hintLength) 字要能自己说清。")
        }
        return .accepted(text, hint: nil)
    }
}
