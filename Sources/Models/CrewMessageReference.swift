import Foundation

/// 一条群消息**指向**的东西（人类 Todo #132 / #133）。
///
/// 人类要的是：群里出现的 Todo #N、某条消息、某个 session、某个机组**点得进去**。
///
/// ## 为什么不是从正文里正则认 `#123`
///
/// 正文是人和 agent 随手写的。正则认出来的 `#78` 可能是一句话里的编号、可能是
/// **别的 crew** 的号、也可能是贴进来的代码里的行号 —— **认错了会跳到一个无关的
/// 地方，而且看起来像功能正常**。这是最难发现的一类坏：它不报错，它带你去了错的
/// 地方，而你以为那就是对的地方。
///
/// 而**发消息的那一刻我们本来就知道它指的是哪条账、哪个 session** ——
/// `post_to_crew(todo:)` / `(plan:)` / `reply_to` / `mentions`、Todo 落账拿到的 `#N`、
/// `contact` 的目标号码，全都是结构化参数。这一层做的事只有一件：
/// **把那一刻已经是结构的东西存下来**，而不是事后拿正则去猜。
///
/// ## 渲染端怎么用
///
/// 引用**不做成正文里的内联链接**（那又要回去在文本里找位置，等于绕回正则）。
/// 它是气泡下面单独一排小胶囊 —— 老消息没有这个字段，就一颗胶囊都不长，
/// 正文一个字不变。
struct CrewMessageReference: Codable, Equatable {

    enum Kind: String, Codable, CaseIterable {
        /// 「人类的」那本 Todo 的 #N（我们请人类拍板的）。
        case humanTodo = "human_todo"
        /// 「Agent 的」那本 Todo 的 #N（人类派给我们的）。
        case agentTodo = "agent_todo"
        /// 驾驶舱里那条计划的 #N。
        case plan
        /// 本群另一条消息的白板 id。
        case message
        /// 某个 session 的 id。
        case session
        /// 某个机组的短号码（如 `7` 或 `7-1`）。
        case crew
    }

    /// `Kind` 的 rawValue。**存字符串不存 enum**：老数据里冒出一个我们还不认识的
    /// 种类时，解码不该整条炸掉 —— 渲染端认不出来就不长那颗胶囊。
    let kind: String
    /// 指向谁：Todo / 计划是 `#N` 的 N（十进制字符串）；消息是白板消息 id；
    /// session 是 sessionId；机组是短号码。
    let targetId: String

    init(_ kind: Kind, _ targetId: String) {
        self.kind = kind.rawValue
        self.targetId = targetId
    }

    var resolvedKind: Kind? { Kind(rawValue: kind) }

    enum CodingKeys: String, CodingKey {
        case kind
        case targetId = "target_id"
    }
}

/// 从**一次发言的结构化字段**收集引用。**纯函数，不看正文一个字。**
enum CrewMessageReferences {

    /// 一次发言手里已经有的那些结构化事实。
    ///
    /// 每一项都对应一个**调用方明确给过的参数**或**落账刚拿到的号**——
    /// 没有一项是从正文推出来的。
    struct Input: Equatable {
        /// `post_to_crew(todo:)` —— Agent 那本 Todo 的 #N。
        var agentTodoNumber: Int?
        /// 落账**刚建出来**的人类 Todo #N（`category: human_todo`），
        /// 或人类自己在面板里新建时那条「人类 To do +1: #N」的 N。
        var humanTodoNumber: Int?
        /// `post_to_crew(plan:)`，或 `category: plan` 落账刚排出来的 #N。
        var planNumber: Int?
        /// `reply_to` —— 被回复的那条消息的白板 id。
        var inReplyTo: String?
        /// 定向 @ 里的 session（`captain` / `human` / `broadcast` 不是引用：
        /// 前者点不到一个具体对象，后两者不指向任何可跳转的东西）。
        var mentionedSessionIds: [String] = []
        /// 跨 crew 来电的来源号码，或 `contact` 回执里那个目标号码。
        var crewNumber: String?

        init(agentTodoNumber: Int? = nil, humanTodoNumber: Int? = nil, planNumber: Int? = nil,
             inReplyTo: String? = nil, mentionedSessionIds: [String] = [],
             crewNumber: String? = nil) {
            self.agentTodoNumber = agentTodoNumber
            self.humanTodoNumber = humanTodoNumber
            self.planNumber = planNumber
            self.inReplyTo = inReplyTo
            self.mentionedSessionIds = mentionedSessionIds
            self.crewNumber = crewNumber
        }
    }

    /// 顺序固定：账（人类 Todo → Agent Todo → 计划）→ 被回复的消息 → session → 机组。
    /// **固定顺序是为了让胶囊排布稳定** —— 同一条消息重渲染时胶囊跳来跳去，
    /// 看起来就像界面在闪。
    static func build(_ input: Input) -> [CrewMessageReference] {
        var out: [CrewMessageReference] = []
        func add(_ kind: CrewMessageReference.Kind, _ target: String?) {
            guard let target, !target.trimmingCharacters(in: .whitespaces).isEmpty else { return }
            let ref = CrewMessageReference(kind, target)
            guard !out.contains(ref) else { return }   // 同一个东西被两条路指到，只留一颗
            out.append(ref)
        }
        add(.humanTodo, input.humanTodoNumber.flatMap(positive))
        add(.agentTodo, input.agentTodoNumber.flatMap(positive))
        add(.plan, input.planNumber.flatMap(positive))
        add(.message, input.inReplyTo)
        for sessionId in input.mentionedSessionIds { add(.session, sessionId) }
        add(.crew, input.crewNumber)
        return out
    }

    /// 号码必须为正。**0 和负数不是「没给」，是给错了** —— 让它长出一颗点进去
    /// 什么也没有的胶囊，比不长更糟。
    private static func positive(_ n: Int) -> String? { n > 0 ? String(n) : nil }
}
