import Foundation

/// 一条 Todo「落地」的**剧本**（Todo #62）：步骤顺序、每一步没成该说什么、群里那行
/// 挂什么 mention —— 全在这一份里。四个调用点各做各的 I/O，但顺序和措辞只有一份。
///
/// ## 为什么必须抽出来
/// 这是同一族事故的第三次预防。#577 是「没落盘就宣布」：条目其实没写进去，群里
/// 那行照发，人以为记下了。同族的第二个形状在 helper 上验到了 —— helper（`--mcp-serve`）
/// 主线程从头到尾卡在 `readLine` 里，没有 run loop，**fire-and-forget 的 `Task`
/// 一声不吭、永远不跑**（`@MainActor` 的 async hop 直接挂死；换 `dispatchMain()`
/// 泵着就通，病因钉死是主线程被占）。两个形状是同一件事：**「什么都没发生」看起来
/// 像成功了**。
///
/// 所以这里不只收顺序，**失败路径一起收**：
///   * `receipt` 是回执的**唯一出口**。走到哪一步就只说得出哪一步的话 ——
///     没落账就拿不到「已记入」那句，落了账没发出群就必须带上那句警示。
///   * `Step` 的顺序写死，`order` 有单测钉住。
///
/// ## 为什么是「无 actor、无平台条件」的纯类型
/// `CrewLocalTodoLanding` 是 `@MainActor` + `#if os(macOS)` + `async`，helper 里
/// **链接得上但跑不起来**（上面那条实测）。会漂移的从来不是 I/O 那几行，是文案和
/// 顺序 —— 把纯的那部分抽出来，四个调用点各写各的 I/O，漂移面就没了。纯类型还能单测。
enum TodoLandingFlow {

    /// 这次落地在干什么。两本账、两个方向，措辞不同。
    enum Action: Equatable {
        /// agent 往**人类 Todo** 加一条（`add_human_todo`）。
        case added
        /// 人类回应一条 Todo（`.human` 那本的详细窗口）。
        case responded
    }

    /// 走到了哪一步。**顺序写死，一步都不许跳**：落账 → 发群 → 唤醒。
    ///
    /// 先落账再宣布，不许颠倒（#577）。发群失败**不回滚**已落的账（本地已落是事实），
    /// 但回执必须如实说「记下了、群里没吱声」—— 不是「成了」。
    enum Step: Int, Comparable, CaseIterable, Sendable {
        /// 连账都没落上 —— **到此为止**，不许往下走，也不许出现「已记下」字样。
        case nothing = 0
        /// 落账成了，群里还没吱声。
        case persisted = 1
        /// 群里那行也发出去了，还没叫醒该醒的人。
        case announced = 2
        /// 三步齐了。
        case woke = 3

        static func < (a: Step, b: Step) -> Bool { a.rawValue < b.rawValue }
    }

    /// 步骤顺序（单测钉住）。调用点按这个顺序做 I/O，做成一步才把 `reached` 往前推。
    static let order: [Step] = [.persisted, .announced, .woke]

    /// 这个动作的**终点**在哪一步。
    ///
    /// `.added` 只有两步 —— agent 加完一条人类 Todo，不用叫醒谁（人类在 app 里
    /// 看得到，那条群消息本来就标着 `@human`「别为它叫醒 agent」）。`.responded`
    /// 三步齐全：人类拍完板，得把当初提问的那个 session 叫回来。
    static func terminal(_ action: Action) -> Step {
        action == .added ? .announced : .woke
    }

    /// 群里那行该挂什么 mention。
    ///
    /// * `.added` → `[human]`：这条是讲给人听的，**别为它叫醒 agent**；human 不收窄
    ///   可见范围，队友照样看得见（2026-08-23 修过的那件）。
    /// * `.responded` → 由 `HumanTodoWakePlan` 给（`[broadcast, 提问者]` /
    ///   回落 `[broadcast, captain]`）：全组看得见 + 只叫醒该醒的那个。
    ///   拿不到计划（不该发生）→ 退回纯广播，宁可多让人看见，也不静默变私信。
    static func mentions(_ action: Action, wake: HumanTodoWakePlan.Plan? = nil) -> [CrewMention] {
        switch action {
        case .added:
            return [CrewMention(kind: "human", targetId: nil)]
        case .responded:
            return wake?.mentions ?? [.broadcast]
        }
    }

    /// 回执 —— **唯一出口**，调用点不许自己拼字符串。
    ///
    /// `reached` 必须是**真跑到**的那一步，别拿意图当结果：`.nothing` 拿不到号、
    /// 也拿不到任何「已记下」的措辞；`.persisted` / `.announced` 一定带警示。
    /// `detail` = 那一步的失败说明（`.woke` 时忽略）。
    static func receipt(ledger: TodoLedger, action: Action,
                        number: Int?, reached: Step, detail: String? = nil) -> String {
        let noun = ledger == .human ? "人类 Todo" : "Todo"
        guard reached > .nothing, let number else {
            return notPersistedReceipt(ledger: ledger, action: action, detail: detail)
        }
        let head: String
        switch action {
        case .added:
            head = "已记入\(noun) #\(number)。人类回应时群里会出「\(ledger.responseAnnouncement(number: number, text: "…"))」并叫醒你——现在接着干别的活，别守着等。"
        case .responded:
            head = "已回应\(noun) #\(number)。"
        }
        // 走到终点才说得出「成了」这一句 —— 没走到就必须带上那一步的警示。
        guard reached < terminal(action) else { return head }
        switch reached {
        case .persisted:
            return head + "\n⚠️ 但**群里那行没发出去**：\(detail ?? "原因不明")。"
                + "条目/回应已经落在账上了，群里没人看得见 —— 需要的话自己去群里补一句。"
        case .announced:
            return head + "\n⚠️ 但**没能叫醒该醒的那个**：\(detail ?? "原因不明")。"
                + "群里那行在，账也在 —— 对方下次自己看白板时才会知道。"
        case .nothing, .woke:
            return head   // .nothing 上面 guard 已挡住；.woke 不小于任何终点
        }
    }

    /// 第一步就没成 —— 这句话是**唯一**能说的。绝不出现「已记下」，也绝不给号
    /// （账上根本没有那条，给了号等于凭空造一个）。
    static func notPersistedReceipt(ledger: TodoLedger, action: Action,
                                    detail: String? = nil) -> String {
        let noun = ledger == .human ? "人类 Todo" : "Todo"
        let why = detail ?? "列表文件这次读不出来或漏读，原有内容没被动过（群聊白板上有一条系统警示）"
        switch action {
        case .added:
            return "ERROR: 没能记进\(noun) —— \(why)。**这条没有记下**，请重试；"
                + "急事改用 ask（阻塞等人答）。"
        case .responded:
            return "回应没能落上 —— \(why)。**这条回应没有记下**，群里也不会出现它，请重试。"
        }
    }
}
