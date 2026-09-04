import Foundation

/// **待办队列，取走语义**（2026-09-04）。
///
/// ## 它替掉的那种错法
///
/// 原来的形状是：一个 `@Published` 数组，生产方**在循环里逐条 append**，
/// 消费方 `.receive(on:)` 之后在 `sink` 里处理收到的那份快照、再把数组清空。
/// 看起来没问题，实际上是**发布的是「变化」、消费的当成「待办队列」**：
///
/// ```
/// append cmd1 → 发出 [cmd1]
/// append cmd2 → 发出 [cmd1, cmd2]      ← 两份都排队投递到主线程
/// sink([cmd1])        → 处理 cmd1，清空
/// sink([cmd1, cmd2])  → 拿到的仍是它被发出时的那份 → **cmd1 被处理第二遍**
/// ```
///
/// 真机实测对得上：投 1 条命令起 1 个 session；投 2 条起**3 个**，重复的正是排在
/// 前面的那条。而 `start_session` 是**要花订阅额度**的动作 —— 重复执行的代价不只是
/// 多一个进程。
///
/// ## 修法不是「按 id 去重」
///
/// 那是在下游擦屁股：上游仍然会把同一条交出去两次，只是被下游挡掉了；下一个消费方
/// 不知道要挡。这里把它改成**取走语义**：队列自己持有待办，`take()` **取走即清空**，
/// 于是「同一批被取两次」在结构上不可能 —— 不是靠订阅者小心。
///
/// ## ⚠️ 脉冲会丢，队列不会
///
/// 队列不 `@Published`，`@Published` 的只是一个「有新东西」的脉冲。**脉冲是会丢的**：
/// 消费方在脉冲发出那一刻还没订阅上（进程刚起、订阅还没接好），那一下就没了，
/// 而队列里的东西还在 —— 症状是「派了活，什么都没发生」，正是这一整期在消灭的静默。
///
/// 所以**消费方接上订阅的第一件事必须先 `take()` 一次**，不能只等脉冲。
/// （`@Published` 会把当前值立刻发给新订阅者，所以在本仓的用法里这一下是白送的；
/// 但**别把它当理所当然** —— 换成 `PassthroughSubject` 那种不重发当前值的发布器时，
/// 这条就得自己补，有一条测试盯着它。）
///
/// 线程：调用方全在 `@MainActor` 上（`CrewStore` / `SessionHost`），所以这里不另上锁；
/// 上锁反而会给人「它能跨线程用」的错觉。
final class PendingRequestQueue<Element> {
    private var items: [Element] = []

    init() {}

    var isEmpty: Bool { items.isEmpty }
    var count: Int { items.count }

    func enqueue(_ item: Element) { items.append(item) }
    func enqueue(contentsOf batch: [Element]) { items.append(contentsOf: batch) }

    /// 取走全部待办。**取走即清空** —— 这是这个类型存在的全部意义。
    func take() -> [Element] {
        let batch = items
        items.removeAll()
        return batch
    }
}
