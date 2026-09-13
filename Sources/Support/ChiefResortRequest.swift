import Foundation
import os

/// 侧栏「总机长」视图上那个**手动刷新按钮**按下去会发生什么（人类 Todo #145）。
///
/// 人类原话：「再给个手动刷新按钮，手动出发让总机长重新总结、排序」；
/// 2026-09-13 当面再说一次：「我希望是总机长重新总结和排序」。
///
/// ## 这一版做全了「总结 + 排序」
///
/// 上一版（c29c70b）只做了排序，文案里刻意不提「总结」，理由是当时没有「给每个机组
/// 各写一句」的字段，而「总机长在另一个时刻给别人写总结」这个设计被否过一次
/// （二手总结会比最新消息更旧、会烂）。人类把它要回来了，所以现在：
/// - 写的地方：`arrange_crews(summaries:)` → `CrewChiefSummaryStore`，每句自带写入时刻；
/// - 防烂：写完之后那个机组又有了新消息就算过期，界面上变淡 +「已过时」
///   （判定在 `CrewStatusLine.summaryIsStale`，有单测）。
/// 当初否掉它的那条理由没有被忘掉，是被那套防护接住了。
///
/// ## 判定为什么不长在 View 里
///
/// 「有没有总机组 / 刚按过要不要拦 / 发什么话 / 回执怎么说」都是判定。长在 View 里就进不了
/// test bundle，于是**永远没有人验它按一下到底会不会发出去** —— 本仓里
/// 「规则有测试、接线没有」已经撞过好几次。
///
/// ## 日志：事后分得清三种情况
///
/// 2026-09-13 人类按了、群里一条都没有，而那时**分不清**是按钮没点着、还是写盘失败被
/// `try?` 吞了（回执永远说「已请总机长重排」）。所以现在按下 / 拒绝 / 写进去 / 没写进去
/// 各记一行，**同一个 subsystem + category**：
///
///     log show --last 1h --info --predicate \
///       'subsystem == "com.pendingname.pendingcrew" AND category == "chief-resort"'
///
/// - 一行都没有 → 没点着；
/// - 只有「按下」、后面跟「没写进去」或什么都没有 → 点着了，没写进去；
/// - 「按下」后面跟「已写进」→ 写进去了（之后总机长醒没醒，看总机组群聊）。
enum ChiefResortRequest {

    /// 这条路上全部日志共用一个 logger —— 查询语句见类型注释。
    static let log = Logger(subsystem: "com.pendingname.pendingcrew", category: "chief-resort")

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
                               + "总机长要醒过来跑一轮才有新摘要和新顺序，连按不会更快。")
            }
        }
        return .send(text: requestText)
    }

    /// 发进群里的那句话。
    ///
    /// **它是人类身份的一条普通群消息，不是一条暗号** —— 所以它出现在群聊里，
    /// 人回头翻得到「这一轮摘要和顺序是我几点钟叫它做的」。写清出处（哪个按钮）也是为此：
    /// 总机长看到它时，得知道这不是人坐在那儿打的字。
    ///
    /// 参数名（`summaries` / `crew_ids` / `reason`）点名写出来：总机长收到的是一条普通
    /// 人类消息，不点名它得自己猜该怎么调。
    static let requestText = """
        请重新总结并排序：先看一遍各机组现在的状况，给每个机组总结一句摘要\
        （一行，40 字左右，说清这个机组现在在干什么 / 卡在哪），然后调一次 arrange_crews —— \
        summaries 里按 crewId 填每个机组那一句，crew_ids 排侧栏顺序，reason 写清这次为什么这么排。\
        （侧栏「总机长」视图上的刷新按钮触发）
        """

    /// 按下去之后显示在按钮旁边的那句回执。
    struct Receipt: Equatable {
        let text: String
        /// 这次算不算「发出去了」、要不要开冷却窗。
        /// **没发出去不开**：人重按会被自己的冷却窗挡住，而那条消息根本没发出去。
        let startsCooldown: Bool
    }

    /// 把「写进总机组群聊」那一下的结果翻成回执。**照实说**，不许一律「已请」。
    ///
    /// - `.success(nil)`：写进去了；
    /// - `.success(incident)`：写进去了，但白板出过事 —— 那句话原样带上；
    /// - `.failure`：没写进去。其中「读不出来但已存进待发件箱」单独说：它会自动补发，
    ///   人再按一次只会在恢复时补出两条，所以这种**要**开冷却窗。
    static func receipt(for result: Result<String?, Error>) -> Receipt {
        switch result {
        case .success(nil):
            return Receipt(text: "已请总机长重新总结并排序（消息已写进总机组群聊）。"
                           + "它要醒过来跑一轮，写回来之前侧栏不会变。",
                           startsCooldown: true)
        case .success(let incident?):
            return Receipt(text: "已请总机长重新总结并排序（消息已写进总机组群聊）。"
                           + "\n⚠️ 但白板出过事：\(incident)",
                           startsCooldown: true)
        case .failure(let error):
            if LocalWhiteboardStore.wasPreservedForRetry(error) {
                return Receipt(text: "还没发出去：总机组群聊此刻读不出来。这条已存进待发件箱，"
                               + "恢复可读时会自动补发 —— 不用再按。\n\(error.localizedDescription)",
                               startsCooldown: true)
            }
            return Receipt(text: "没发出去，总机长收不到、侧栏不会变：\(error.localizedDescription)",
                           startsCooldown: false)
        }
    }

    // MARK: - 日志（四行，同一个 logger）

    /// **点击回调的第一行调它**，在任何判定和写盘之前 —— 冷却挡掉的也算点着了。
    static func logPressed() {
        log.notice("按下刷新按钮")
    }

    static func logRefused(_ why: String) {
        log.notice("拒绝（没发）：\(why, privacy: .public)")
    }

    static func logWritten(crewId: String, incident: String?) {
        log.notice("已写进总机组群聊 crew=\(crewId, privacy: .public) 白板事故=\(incident ?? "无", privacy: .public)")
    }

    static func logWriteFailed(_ detail: String) {
        log.error("没写进去：\(detail, privacy: .public)")
    }
}
