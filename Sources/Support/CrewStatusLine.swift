import Foundation

/// 侧栏「总机长」视图每行显示的那句话（人类 Todo #136，#145 改过口径）。
///
/// ## 这一行现在有三个来源，按优先级
/// 1. **总机长写的摘要**（#145，人类 2026-09-13：「我希望是总机长重新总结和排序」）；
/// 2. **机长自己随消息报的那句状态**（#136，`post_to_crew(crew_status:)`）；
/// 3. 都没有 →「还没有」。**不编**，也不拿最新消息顶上。
///
/// ## 为什么 #136 当初甩掉的那套防护又回来了
/// #136 的理由是：状态和消息同一次动作产生，永远不会比最新消息更旧，所以不需要
/// 陈旧度防护。#145 把「总机长在另一个时刻给别人写总结」要回来了 —— 那种二手的东西
/// **一定会**比之后的消息更旧，当初否掉它的理由原样还在。所以这回防护是硬要求：
/// **写完之后，只要那个机组又有了更新的消息，这句就算过期**，过期的要一眼看得出来。
///
/// 而且 #136 本身也有一个残留的陈旧面：「这次没填就沿用上一次填的」。沿用来的那句
/// 同样适用这条规则（沿用 = 之后又有了没带状态的消息 = 过期）。
///
/// ## 为什么正文里不再拼「N 分钟前：」（人类 2026-09-13）
/// 原来年龄**永远在正文里**，理由是「多久前报的」是「还算不算数」的前提，放 tooltip
/// 要悬停才看得见。人类当面推翻了：「最新一条消息显示这里就不需要再写时间了」——
/// 标题右边本来就有一颗相对时间，两颗时间挤在一行里读不出哪颗是哪颗的。
///
/// **去掉时间不许把旧的伪装成新的。** 原来「年龄」干的活是让人判断还算不算数；
/// 现在这件活由 `isStale` 直接给出结论（界面上变淡 +「已过时」），比让人自己拿
/// 年龄去比更可靠 —— 「3 小时前」算不算旧，取决于这 3 小时里群里有没有新动静，
/// 而那正是这里判的东西。写入/报出的时刻挪进悬停提示，查得到、不占正文。
///
/// **判定全在这里（纯函数），不长在 View 里** —— 长在 View 里的判定进不了 test bundle。
enum CrewStatusLine {
    /// 这一行的话是谁写的。
    enum Source: Equatable, Sendable {
        case chiefSummary
        case captainStatus
        case missing
    }

    /// 一行要显示的东西。
    struct Line: Equatable, Sendable {
        /// 那句话本身（**不含时间**，也不含「已过时」）。
        let text: String
        let source: Source
        /// 写完之后这个机组又有了更新的消息。视图据此变淡，正文前带「已过时」。
        let isStale: Bool
        /// 悬停提示：说清来源 + 写入/报出的时刻 + 过期了的话为什么算过期。
        let help: String

        /// 这一行是不是「一次都没有」。视图可以据此画得更淡。
        var isMissing: Bool { source == .missing }

        /// 视图直接画这个（别自己再拼一遍「已过时」）。
        var displayText: String { isStale ? "已过时 · \(text)" : text }
    }

    /// 渲染成那一行。
    ///
    /// - Parameters:
    ///   - summary: 总机长给这个机组写的摘要；`nil` = 没写过。
    ///   - statusCarrier: 这个机组**当前生效的那句状态**所在的消息
    ///     （`CrewStore.crewStatusCarriers`）；`nil` = 机长一次都没报过。
    ///   - activity: 这个机组**最后一条算数的发言**（见 `Activity` / `CrewActivityMessage`）。
    ///     **不是**「最后一条消息」—— 系统通知、「已送达」「已联系」回执不让它过期。
    static func make(summary: CrewChiefSummary?,
                     statusCarrier: LocalWhiteboardMessage?,
                     activity: Activity) -> Line {
        let status = statusCarrier.flatMap { carrier -> (LocalWhiteboardMessage, String)? in
            // 全空白的状态就当没填（写了个空格就算填过，是最廉价的一种假账）。
            let body = (carrier.crewStatus ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            return body.isEmpty ? nil : (carrier, body)
        }
        let summaryText = summary.map { $0.text.trimmingCharacters(in: .whitespacesAndNewlines) }
        let usableSummary = (summaryText?.isEmpty == false) ? summary : nil

        let summaryStale = usableSummary.map {
            summaryIsStale(writtenAt: $0.writtenAt, activity: activity)
        }
        let statusStale = status.map {
            statusIsStale(carrierId: $0.0.id, activity: activity)
        }

        // 优先级：新鲜的摘要 > 新鲜的状态 > 过期的摘要 > 过期的状态 > 还没有。
        //
        // 「新鲜的状态压过过期的摘要」是这里自己加的一刀，不是字面上的「摘要 > 状态」：
        // 摘要过期 = 写完之后又有了新消息；状态新鲜 = 它就挂在最新那条消息上。
        // 这时状态**严格更新**，还把旧摘要摆在前面，就是拿二手的旧话盖住一手的新话。
        if let s = usableSummary, summaryStale == false {
            return summaryLine(s, stale: false)
        }
        if let (carrier, body) = status, statusStale == false {
            return statusLine(body, carrier: carrier, stale: false)
        }
        if let s = usableSummary {
            return summaryLine(s, stale: true)
        }
        if let (carrier, body) = status {
            return statusLine(body, carrier: carrier, stale: true)
        }
        return Line(text: "还没有", source: .missing, isStale: false,
                    help: "这个机组还没有总机长写的摘要，机长也没报过状态"
                        + "（这里不显示最新消息，也不替他编一句）")
    }

    // MARK: - 过期判定（纯函数）

    /// 这个机组**最后一条算数的发言**，三种情况分开 —— 「不知道」和「一条都没有」
    /// 对过期判定的意思相反，压成一个 `nil` 就会让其中一种说错话。
    ///
    /// ## 为什么不是「最后一条消息」（2026-09-13，父机长抓到的）
    /// 第一版拿的是末条消息，不分是谁发的。而白板上大量是系统通知和工具回执：本机真数据
    /// 14887 条里，系统身份的有 2100+ 条（其中「已送达」1122 条），「已联系」回执 338 条；
    /// 一次读不出来的故障恢复时，一个群一次就补发 74 条系统通知。照第一版装上去，
    /// **每个机组的摘要都会立刻变成「已过时」**，人看到的等于没做。
    enum Activity: Equatable {
        /// 快照里没有这个机组（白板还没读到 / 是空的）→ 证明不了新鲜，当过期。
        case unknown
        /// 读到了白板，但**一条算数的发言都没有**（只有系统通知之类）→ 之后没人说过话。
        case noneYet
        /// 最后一条算数的发言。
        case latest(LocalWhiteboardMessage)
    }

    /// 从 store 那两份快照拼出 `Activity`。视图只调它，不自己拿 nil 判。
    ///
    /// - Parameters:
    ///   - lastMessage: 末条消息（`CrewStore.lastWhiteboardMessages`，**只用来判「读没读到」**）。
    ///   - lastActivity: 最后一条算数的发言（`CrewStore.lastActivityMessages`）。
    static func activity(lastMessage: LocalWhiteboardMessage?,
                         lastActivity: LocalWhiteboardMessage?) -> Activity {
        guard lastMessage != nil else { return .unknown }
        return lastActivity.map(Activity.latest) ?? .noneYet
    }

    /// 总机长的摘要过没过期：**写完之后，这个机组有没有更新的算数发言**。
    ///
    /// - 按**秒**比、而且「同一秒」算过期：消息时间戳只精确到秒，同一秒里谁先谁后
    ///   分不出来 —— 分不出来的时候宁可说旧，不许把旧的说成新的。
    /// - 任何一边的时间解析不出来 → 过期（证明不了它新鲜）。
    /// - 快照里没有这个机组 → 过期。同上：证明不了。
    /// - 读到了、但一条算数的发言都没有 → 新鲜（写完之后没人说过话）。
    static func summaryIsStale(writtenAt: String, activity: Activity) -> Bool {
        guard let written = CrewTimestamp.parse(writtenAt) else { return true }
        switch activity {
        case .unknown:
            return true
        case .noneYet:
            return false
        case .latest(let message):
            guard let last = CrewTimestamp.parse(message.createdAt) else { return true }
            return floor(last.timeIntervalSince1970) >= floor(written.timeIntervalSince1970)
        }
    }

    /// 机长自报的状态过没过期：**它所在那条消息是不是最后一条算数的发言**。
    ///
    /// 用消息 id 比，不用时间比 —— 状态本来就挂在一条具体的消息上，「之后还有没有别人
    /// 说过话」按 id 问是精确的，不受秒级时间戳同秒的影响。
    /// 快照里没有这个机组 → 过期；「一条算数的都没有」却有一句状态，自相矛盾 → 也当过期。
    static func statusIsStale(carrierId: String, activity: Activity) -> Bool {
        guard case .latest(let message) = activity else { return true }
        return carrierId != message.id
    }

    // MARK: - 文案

    private static func summaryLine(_ s: CrewChiefSummary, stale: Bool) -> Line {
        let who = s.bySenderName.map { "总机长（\($0)）" } ?? "总机长"
        let when = clockText(CrewTimestamp.parse(s.writtenAt))
        let help = stale
            ? "已过时：这是\(who)写的摘要，写于 \(when)。写完之后这个机组又有了新消息，"
                + "这句可能已经不对了 —— 按上面的刷新按钮让总机长重写。"
            : "这是\(who)写的摘要，写于 \(when)。写完之后这个机组还没有新消息。"
        return Line(text: s.text.trimmingCharacters(in: .whitespacesAndNewlines),
                    source: .chiefSummary, isStale: stale, help: help)
    }

    private static func statusLine(_ body: String, carrier: LocalWhiteboardMessage,
                                   stale: Bool) -> Line {
        let when = clockText(CrewTimestamp.parse(carrier.createdAt))
        let help = stale
            ? "已过时：这是机长上一次发消息时自己报的状态，报于 \(when)。之后这个机组又有了"
                + "新消息，没再报过。"
            : "这是机长发消息时自己报的状态，报于 \(when)，就挂在这个机组最新那条消息上。"
                + "（不是总机长写的摘要，也不是最新消息的正文）"
        return Line(text: body, source: .captainStatus, isStale: stale, help: help)
    }

    /// 悬停提示里的时刻。格式器进程级复用 —— 这个函数在侧栏 body 里每行都跑，
    /// 每次现造一个 `DateFormatter` 正是 `CrewTimestamp` 顶上记着的那种主线程开销。
    static func clockText(_ date: Date?) -> String {
        guard let date else { return "时间不详" }
        return clockFormatter.string(from: date)
    }

    private static let clockFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "zh_Hans")
        f.dateFormat = "M月d日 HH:mm"
        return f
    }()
}

/// **哪些消息算「这个机组又有人说话了」**（人类 Todo #145 追加，2026-09-13）。
///
/// 只有人类和 agent 的发言算；系统通知、「已送达」回执、「已联系」回执都不算。
///
/// ## 判据只看落盘时的结构字段，不看显示名
/// 显示名什么都可能是：机组可以叫任何名字、人类显示名是「人」、机长是「机长」。
/// 真正不同的字段：
/// - **系统通知 / 「已送达」回执**：写入时 `sessionId == "system"`
///   （`postSystemNotice` 与十几处 `senderName: "系统"` 全是这个形状，逐处核过），
///   新写入还会被正规化成 `senderKind == "pendingcrew"`。老数据两种形状都有，
///   `PendingCrewSystemMessage.isSystem` 两种都认 —— 老数据不用迁移。
/// - **「已联系」回执**：`contact` 工具以**调用它的那个 agent 自己的身份**写回本群，
///   原来是 `category: "progress"`，跟一条真的进展汇报一个字段都不差。所以在写入端补了
///   一个明确标记：`category == contactReceiptCategory`。
///   **老数据里的「已联系」回执没有这个标记，仍然算发言**（本机 338 条）——
///   它们全都早于任何一句摘要（摘要是这一版才有的），只有「装了新 app、但某个 helper
///   还是旧二进制」那段时间里新写的回执会被误算。
enum CrewActivityMessage {
    /// 「已联系」回执的 category。写入端在 `McpServer` 的 `contact` 工具里。
    static let contactReceiptCategory = "contact_receipt"

    /// 这条消息算不算发言。
    static func counts(_ message: LocalWhiteboardMessage) -> Bool {
        if PendingCrewSystemMessage.isSystem(senderKind: message.senderKind,
                                             senderSessionId: message.senderSessionId) {
            return false
        }
        if message.category == contactReceiptCategory { return false }
        return true
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
