import Foundation

/// 侧栏「总机长」视图上那个**手动刷新按钮**按下去会发生什么（人类 Todo #145）。
///
/// 人类原话：「再给个手动刷新按钮，手动出发让总机长重新总结、排序」。
///
/// ## 为什么只做「排序」这一半，而且文案里不提「总结」
///
/// 那句话里的「总结」**今天做不到**：总机长手上唯一能写到侧栏的东西是
/// `arrange_crews` 的 `reason`（整份排布共用一句），`CrewArrangement` 里
/// 只有 `crewIds` + 一个 `reason`，**没有「给每个机组各写一句」这种字段**。
///
/// 硬加一个也不是顺手的事 —— `CrewStatusLine` 顶上那段白纸黑字记着：
/// 「总机长在另一个时刻给别人写总结」这个设计**被否过一次**，理由是二手总结
/// 会比最新消息更旧、会烂，得配一整套陈旧度防护；#136 正是为了甩掉那套防护
/// 才改成「机长自己随消息报」。人类现在要把它要回来，那是**产品决定**，
/// 不是这个按钮顺手能定的。
///
/// 所以这个按钮**不承诺它做不到的事**：文案只说「重排」。
/// 缺口与两份互相打架的记录写在
/// `docs/internal/2026-09-12-chief-summary-readings.md`。
///
/// ## 判定为什么不长在 View 里
///
/// 「有没有总机组 / 刚按过要不要拦 / 发什么话」三件都是判定。长在 View 里就进不了
/// test bundle，于是**永远没有人验它按一下到底会不会发出去** —— 本仓里
/// 「规则有测试、接线没有」已经撞过好几次。
enum ChiefResortRequest {

    /// 连按的冷却窗。按一下是让一个 agent 醒过来跑一轮活，不是刷新一张网页 ——
    /// 连点五下不会更快，只会让它收到五条一模一样的话。
    static let cooldown: TimeInterval = 60

    enum Decision: Equatable {
        /// 把这段话发进总机组群聊（**人类身份、不带 @**：无 @ 的人类消息默认唤醒机长，
        /// 走的是现成那条路，不新造唤醒通道）。
        case send(text: String)
        /// 不发 —— 而且**必须把理由说出来**。一个按了没反应的按钮比没有按钮更糟：
        /// 人会以为是它坏了，然后连有反应的那次也不再信。
        case refuse(why: String)
    }

    /// - Parameters:
    ///   - chiefCrewId: 总机组那一层的 crew id；`nil` = 这台机器上还没有那一层。
    ///   - lastRequestedAt: 上一次按下去**真发出去**的时刻；`nil` = 从没按过。
    ///   - now: 现在（测试注入）。
    static func decide(chiefCrewId: String?,
                       lastRequestedAt: Date?,
                       now: Date) -> Decision {
        guard let chiefCrewId, !chiefCrewId.trimmingCharacters(in: .whitespaces).isEmpty else {
            return .refuse(why: "总机组那一层还没在这台机器上建起来，没有人可以请。")
        }
        if let lastRequestedAt {
            // 时钟往回跳时 `age` 会是负的。负数一样落进冷却（刚按过），
            // 但别把负数印给人看。
            let age = max(0, now.timeIntervalSince(lastRequestedAt))
            if age < cooldown {
                let wait = Int((cooldown - age).rounded(.up))
                return .refuse(why: "刚请求过（\(Int(age)) 秒前），再等 \(wait) 秒。"
                               + "总机长要醒过来跑一轮才有新顺序，连按不会更快。")
            }
        }
        return .send(text: requestText)
    }

    /// 发进群里的那句话。
    ///
    /// **它是人类身份的一条普通群消息，不是一条暗号** —— 所以它出现在群聊里，
    /// 人回头翻得到「这个顺序是我几点钟叫它重排的」。写清出处（哪个按钮）也是为此：
    /// 总机长看到它时，得知道这不是人坐在那儿打的字。
    static let requestText = """
        请重新看一遍各机组现在的状况，然后用 arrange_crews 重排侧栏顺序，\
        并在 reason 里写清这次为什么这么排。（侧栏「总机长」视图上的刷新按钮触发）
        """
}
