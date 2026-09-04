#if os(macOS)
import XCTest

/// **同一条机长命令被执行两遍**（2026-09-04 真机实测：投 2 条 `start_session`
/// 起了 **3 个** session，重复的是排在前面的那条）。
///
/// 病根不在命令通道，在 `CrewStore` 那 9 条待办队列的形状：一个 `@Published`
/// 数组，生产方**在循环里逐条 append**，消费方 `.receive(on:)` 之后处理收到的那份
/// 快照、再清空。于是 `append cmd1` 发出 `[cmd1]`、`append cmd2` 发出 `[cmd1, cmd2]`，
/// 两份都排队投递 —— 第二次 sink 拿到的仍是**它被发出时的那份**，`cmd1` 被处理第二遍。
///
/// **这不是翻默认的回归**（GUI 模式下是同一份代码，应该一直都在），但它现在必须收：
/// `start_session` 要花订阅额度，机长连派两个活时第一个会被起两遍。
///
/// 修法不是「按 id 去重」——那是在下游擦屁股，上游仍然把同一条交出去两次，
/// 而下一个消费方不知道要挡。
final class PendingRequestQueueTests: XCTestCase {

    /// **主条**：消费方被叫醒几次不重要，**同一批不许被取到两次**。
    ///
    /// 复刻真实时序：生产方逐条 append（于是消费方被叫醒两次），两次都走 `take()`。
    func test_同一批不许被取两次() {
        let queue = PendingRequestQueue<String>()
        queue.enqueue("cmd1")
        queue.enqueue("cmd2")

        var processed: [String] = []
        processed += queue.take()      // 第一次被叫醒
        processed += queue.take()      // 第二次被叫醒（原形状里这一次会把 cmd1 再交一遍）

        XCTAssertEqual(
            processed, ["cmd1", "cmd2"],
            """
            同一批被取了两次 —— 真机上就是这样：投 2 条命令起了 3 个 session，
            重复的是排在前面的那条。而 start_session 要花订阅额度。
            """)
    }

    /// 取走之后队列就空了 —— 「取走」和「清空」是同一个动作，不是两步。
    /// 分成两步正是原来那个 bug 的形状：消费方拿到快照、再另外去清。
    func test_取走即清空() {
        let queue = PendingRequestQueue<String>()
        queue.enqueue("a")
        XCTAssertEqual(queue.take(), ["a"])
        XCTAssertTrue(queue.isEmpty)
        XCTAssertEqual(queue.take(), [], "空队列再取还是空")
    }

    /// **脉冲会丢，队列不会。**
    ///
    /// 消费方在脉冲发出那一刻还没订阅上（进程刚起、订阅还没接好），那一下脉冲就没了，
    /// 而队列里的东西还在。**所以消费方接上订阅的第一件事必须先取一次** ——
    /// 不这么做的症状是「派了活，什么都没发生」，正是这一整期在消灭的那种静默。
    func test_先入队后接订阅仍然要被处理() {
        let queue = PendingRequestQueue<String>()
        queue.enqueue("在订阅接上之前就到的那条")

        // 消费方这时才接上：它做的第一件事是 take()，而不是干等下一个脉冲。
        let firstThingAfterSubscribing = queue.take()

        XCTAssertEqual(
            firstThingAfterSubscribing, ["在订阅接上之前就到的那条"],
            "只等脉冲的话，这条命令会永远躺在队列里 —— 派了活、什么都没发生")
    }

    /// 取走之后新入队的照常拿得到（不会因为取过一次就再也收不到）。
    func test_取走之后新来的照常拿得到() {
        let queue = PendingRequestQueue<Int>()
        queue.enqueue(1)
        XCTAssertEqual(queue.take(), [1])
        queue.enqueue(2)
        queue.enqueue(3)
        XCTAssertEqual(queue.take(), [2, 3])
    }

    /// **消费首批期间新到的请求进下一批，且恰好一次。**
    ///
    /// 这条堵的是「取走语义」自己最容易写错的地方：如果 `take()` 是「先记下要清哪些、
    /// 处理完再清」，那么处理期间新到的那条会被一起清掉 —— **丢命令**，
    /// 而丢命令比重复执行更难发现（重复至少看得见）。
    func test_消费首批期间新到的请求进下一批且各一次() {
        let queue = PendingRequestQueue<String>()
        queue.enqueue("第一批 A")
        queue.enqueue("第一批 B")

        var processed: [String] = []
        // 消费第一批 —— 处理**期间**又来了一条（真实里就是排空线程还在往里塞）。
        for item in queue.take() {
            processed.append(item)
            if item == "第一批 A" { queue.enqueue("处理途中来的") }
        }
        // 下一次被叫醒。
        processed += queue.take()
        // 再被叫醒一次（多余的脉冲）。
        processed += queue.take()

        XCTAssertEqual(processed, ["第一批 A", "第一批 B", "处理途中来的"],
                       "处理途中来的那条要么被一起清掉（丢命令），要么被处理两遍")
    }

    /// 顺序就是入队顺序 —— 机长连派两个活，起来的顺序不该反。
    func test_保持入队顺序() {
        let queue = PendingRequestQueue<String>()
        queue.enqueue(contentsOf: ["先", "中", "后"])
        XCTAssertEqual(queue.take(), ["先", "中", "后"])
    }
}


/// **形状本身的护栏**：别退回「订阅 `@Published` 数组、处理快照、再清空」那种写法。
///
/// 这条不是洁癖。那种写法出过一次真事故（投 2 条 `start_session` 起了 3 个 session，
/// 而 `start_session` 要花订阅额度），而且**看代码看不出问题** —— 消费方明明「立刻
/// 捕获 + 清空」了，注释里还写着这么做是为了避免重复处理。错的是上游发布的语义，
/// 不是下游的小心程度。
///
/// 排查那次 bug 时，「同形状的队列有几条」按注释 grep 得到 1 条、按**形状**反推得到
/// **9 条** —— 又一次「名单会过期，形状不会」。所以这条测试盯的是形状。
final class PendingRequestQueueShapeTests: XCTestCase {

    private func source(_ relativePath: String) throws -> String {
        try String(
            contentsOf: URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent().deletingLastPathComponent()
                .deletingLastPathComponent().appendingPathComponent(relativePath),
            encoding: .utf8)
    }

    /// 消费方不许再用「把队列赋成 `[]`」来清空 —— 清空必须是 `take()` 的一部分。
    func test_消费方不许自己清空队列() throws {
        let host = try source("Sources/Mac/Services/SessionHost.swift")
        var offenders: [String] = []
        for (i, line) in host.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
            let text = String(line)
            guard text.contains("Requests = []") || text.contains("Wakes = []") else { continue }
            offenders.append("\(i + 1): \(text.trimmingCharacters(in: .whitespaces))")
        }
        XCTAssertTrue(
            offenders.isEmpty,
            """
            消费方又在自己清空队列了 —— 那意味着「拿到的那份」和「清掉的那份」是两个
            动作，中间就是同一批被处理两遍的窗口。清空必须由 `take()` 一并完成。
            \(offenders.joined(separator: "\n"))
            """)
    }

    /// 那九条待办队列不许再是 `@Published` 数组。
    func test_待办队列不许再是Published数组() throws {
        let store = try source("Sources/Stores/CrewStore.swift")
        var offenders: [String] = []
        for (i, line) in store.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
            let text = String(line).trimmingCharacters(in: .whitespaces)
            guard text.hasPrefix("@Published"), text.contains(": [") else { continue }
            // 待办队列的名字都以 Requests / Wakes 结尾；其余 `@Published` 数组
            // （crews / subjects / machines 这些**状态**）不在这条规矩里 ——
            // 它们本来就是「发布当前状态」，不是待办队列。
            guard text.contains("Requests") || text.contains("Wakes") else { continue }
            offenders.append("\(i + 1): \(text)")
        }
        XCTAssertTrue(
            offenders.isEmpty,
            """
            待办队列又变回 `@Published` 数组了。发布器发的是「变化」，而待办队列要的是
            「取走」——两者混用就是那次投 2 条起 3 个的病根。用 `PendingRequestQueue`。
            \(offenders.joined(separator: "\n"))
            """)
    }
}
#endif
