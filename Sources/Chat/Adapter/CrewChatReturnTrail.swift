import Foundation

/// 从群聊里点一颗引用胶囊跳走之后，**回去的那条路**（人类 Todo #132/#133）。
///
/// 人类点名的判据是「跳过去要能回来，别做成单程票」。原来那条跳消息的路
/// （`searchTargetMessageId` / `locateSearchTarget`）是搜索用的，跳完就没了下文：
/// 跨群点进一条结果之后，**侧栏选中项已经换了群**，想回去只能自己记得刚才在哪儿。
///
/// 这一层就是那份「刚才在哪儿」。做成栈而不是单个落点，是因为连着点两颗
/// （A 的消息 → B 的消息 → C）时，只留一个落点会把 A 直接丢掉 —— 而人对
/// 「返回」的预期是一层一层退回去。
struct CrewChatReturnTrail: Equatable {

    /// 一个落脚点：那一刻人在哪个群、看的是哪条消息。
    struct Stop: Equatable {
        var crewId: String
        var crewTitle: String
        var messageId: String

        init(crewId: String, crewTitle: String, messageId: String) {
            self.crewId = crewId
            self.crewTitle = crewTitle
            self.messageId = messageId
        }
    }

    /// 最多记这么多层。**不是怕内存** —— 是一条二十几层深的返回路人已经不知道
    /// 自己会退到哪儿去了，那时候「返回」和「随便跳一下」没有区别。
    static let limit = 20

    private(set) var stops: [Stop] = []

    init(stops: [Stop] = []) { self.stops = stops }

    var top: Stop? { stops.last }
    var isEmpty: Bool { stops.isEmpty }

    /// 跳走之前把当前落脚点压上。
    ///
    /// 跟栈顶完全相同就不压 —— 同一颗胶囊被点两下（或界面重入）不该多出一层，
    /// 否则人得按两次返回才退得回去，看起来就像第一次没生效。
    mutating func push(_ stop: Stop) {
        if stops.last == stop { return }
        stops.append(stop)
        if stops.count > Self.limit { stops.removeFirst(stops.count - Self.limit) }
    }

    /// 退一层。空栈 → nil（调用方据此决定还显不显示那颗返回件）。
    mutating func pop() -> Stop? {
        stops.popLast()
    }

    /// 整条路作废。**换群、换搜索这类「人自己走开了」的动作要调它** ——
    /// 留着一条通往人早就离开的地方的返回路，比没有返回路更让人困惑。
    mutating func clear() { stops.removeAll() }
}
