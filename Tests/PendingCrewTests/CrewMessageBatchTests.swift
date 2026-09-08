import XCTest

/// 一次调用发多条（人类 Todo #115 后半 —— 他原话里「**这是个重要的改变**」指的是这半）。
final class CrewMessageBatchTests: XCTestCase {

    private func parse(_ args: [String: Any]) -> CrewMessageBatch.Decision {
        CrewMessageBatch.parse(args: args)
    }

    func test_没给messages就是老形态() {
        guard case .single = parse(["message": "一条"]) else { return XCTFail("老调用方被改了行为") }
    }

    func test_三条各带自己的分类() {
        guard case let .batch(entries) = parse(["messages": [
            ["text": "闸门全绿", "category": "progress", "plan": 3],
            ["text": "这条要你拍板", "category": "human_todo"],
            ["text": "顺带一问", "category": "question"],
        ]]) else { return XCTFail("没解析成三条") }
        XCTAssertEqual(entries.map(\.text), ["闸门全绿", "这条要你拍板", "顺带一问"])
        XCTAssertEqual(entries.map(\.index), [0, 1, 2])
        XCTAssertEqual(entries[0].args["category"] as? String, "progress",
                       "每条要带住自己的参数 —— 分类是逐条的，不是整批一个")
    }

    /// **两个都给 = 没人知道你想发几条。**
    func test_message和messages不许同时给() {
        guard case let .refuse(msg) = parse(["message": "一条", "messages": [["text": "另一条"]]])
        else { return XCTFail("两个都给却放行了") }
        XCTAssertTrue(msg.contains("只能给一个"), msg)
    }

    func test_空数组要拒() {
        guard case .refuse = parse(["messages": [[String: Any]]()]) else {
            return XCTFail("空数组被当成发送成功")
        }
    }

    /// 校验是**全有或全无**：第 3 条写错，前两条也不许发出去 ——
    /// 分条之后「一半成功」是新的失败形态，而它最容易被读成「全成了」。
    func test_任何一条不合法整批都不发() {
        guard case let .refuse(msg) = parse(["messages": [
            ["text": "好的一条"],
            ["text": "也好的一条"],
            ["text": "   "],
        ]]) else { return XCTFail("第三条空白却让前两条发出去了") }
        XCTAssertTrue(msg.contains("第 3 条"), "没指名道姓是第几条：\(msg)")
        XCTAssertTrue(msg.contains("整批都没有发出去"), "没说清前两条也没发：\(msg)")
    }

    /// 上限是**产品判断**不是性能限制：一次十几个气泡只是把一堵墙拆成一排墙。
    func test_超过上限要拒并说清这是产品判断() {
        let many = (1...(CrewMessageBatch.maxEntries + 1)).map { ["text": "第\($0)条"] }
        guard case let .refuse(msg) = parse(["messages": many]) else {
            return XCTFail("超上限却放行")
        }
        XCTAssertTrue(msg.contains("\(CrewMessageBatch.maxEntries)"), msg)
        XCTAssertTrue(msg.contains("不是性能限制"), "没说清为什么有这个上限：\(msg)")
    }

    // MARK: - 回执：一半成功是常态，不许回一句「已发送」

    func test_全成功的回执() {
        let r = CrewMessageBatch.batchReceipt(sent: [0, 1, 2], failed: [])
        XCTAssertTrue(r.contains("已发出 3 条"), r)
        XCTAssertFalse(r.contains("⚠️"), r)
    }

    func test_一半成功时必须逐条说清哪几条没发() {
        let r = CrewMessageBatch.batchReceipt(
            sent: [0], failed: [(index: 1, why: "人类 Todo 那本账写不进去")])
        XCTAssertTrue(r.contains("已发出 1 条"), r)
        XCTAssertTrue(r.contains("第 2 条"), "没指出是哪一条没发：\(r)")
        XCTAssertTrue(r.contains("人类 Todo 那本账写不进去"), "没带上失败原因：\(r)")
        XCTAssertTrue(r.contains("重发"), "没告诉人那几条已经没了、要重发：\(r)")
    }
}
