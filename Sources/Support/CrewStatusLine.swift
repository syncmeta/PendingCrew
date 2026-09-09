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
