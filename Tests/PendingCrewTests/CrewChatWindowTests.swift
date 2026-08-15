import XCTest

/// 渲染窗口的纯逻辑（#443 第三道闸）。**「不减功能」这条靠这里钉住** ——
/// 窗口只决定「一次往视图树里塞多少行」，往上翻必须真能翻到最早那一条。
final class CrewChatWindowTests: XCTestCase {

    private func msgs(_ n: Int) -> [Int] { Array(0 ..< n) }

    func test_只渲染最近一页且保持时间顺序() {
        let all = msgs(70)
        let w = CrewChatWindow.window(all, limit: CrewChatWindow.pageSize)
        XCTAssertEqual(w.count, CrewChatWindow.pageSize)
        XCTAssertEqual(w.first, all.count - CrewChatWindow.pageSize,
                       "取的是最近的那一段（末尾），不是开头")
        XCTAssertEqual(w.last, 69, "最新一条必须在窗口里 —— 打开就该看到最新")
        XCTAssertEqual(w, Array(w).sorted(), "顺序仍是旧→新")
    }

    func test_历史比一页短时全给不留占位() {
        let all = msgs(7)
        XCTAssertEqual(CrewChatWindow.window(all, limit: CrewChatWindow.pageSize), all)
        XCTAssertFalse(CrewChatWindow.hasMore(total: 7, limit: CrewChatWindow.pageSize))
        XCTAssertEqual(CrewChatWindow.remaining(total: 7, limit: CrewChatWindow.pageSize), 0)
    }

    func test_只有首屏eager_手点加载历史后恢复lazy() {
        XCTAssertTrue(CrewChatWindow.usesEagerInitialLayout(limit: CrewChatWindow.pageSize))
        XCTAssertFalse(CrewChatWindow.usesEagerInitialLayout(limit: CrewChatWindow.pageSize * 2))
    }

    func test_一页一页能翻到最早一条_不减功能() {
        let all = msgs(70)
        var limit = CrewChatWindow.pageSize
        var guardCount = 0
        while CrewChatWindow.hasMore(total: all.count, limit: limit) {
            limit = CrewChatWindow.expanded(limit, total: all.count)
            guardCount += 1
            XCTAssertLessThan(guardCount, 100, "翻页必须收敛，不许原地打转")
        }
        let w = CrewChatWindow.window(all, limit: limit)
        XCTAssertEqual(w, all, "翻到底必须是完整历史 —— 一条都不许丢")
        XCTAssertFalse(CrewChatWindow.hasMore(total: all.count, limit: limit),
                       "到顶后占位必须消失")
    }

    func test_占位上的数字是还没渲染的条数() {
        XCTAssertEqual(CrewChatWindow.remaining(total: 70, limit: 30), 40)
        XCTAssertEqual(CrewChatWindow.remaining(total: 70, limit: 65), 5)
        XCTAssertEqual(CrewChatWindow.remaining(total: 70, limit: 70), 0)
    }

    func test_越界的上限被夹回合法区间() {
        XCTAssertEqual(CrewChatWindow.clampedLimit(-5, total: 10), 0)
        XCTAssertEqual(CrewChatWindow.clampedLimit(999, total: 10), 10)
        XCTAssertEqual(CrewChatWindow.clampedLimit(30, total: 0), 0)
        XCTAssertEqual(CrewChatWindow.window(msgs(10), limit: 999), msgs(10))
        XCTAssertEqual(CrewChatWindow.expanded(999, total: 10), 10)
    }

    // MARK: - 新消息进来时，已经翻出来的不许消失

    func test_没翻过页时上限恒定_窗口跟着新消息往前滑() {
        // 默认状态：来 20 条新的，上限仍是一页 —— 封顶不许被聊天一路长回整表。
        XCTAssertEqual(
            CrewChatWindow.afterInsert(limit: CrewChatWindow.pageSize, added: 20),
            CrewChatWindow.pageSize)
    }

    func test_翻过页之后新消息把上限顶高_已露出的留在原地() throws {
        let expanded = CrewChatWindow.expanded(CrewChatWindow.pageSize, total: 200) // 60
        let after = CrewChatWindow.afterInsert(limit: expanded, added: 3)
        XCTAssertEqual(after, expanded + 3)

        // 真正要证的：翻出来的那条**还在**窗口里。
        let before = CrewChatWindow.window(msgs(200), limit: expanded)
        let oldest = try XCTUnwrap(before.first)
        // 追加 3 条新的（旧→新，所以接在末尾）。
        let grown = msgs(200) + [200, 201, 202]
        let now = CrewChatWindow.window(grown, limit: after)
        XCTAssertTrue(now.contains(oldest),
                      "刚翻出来的最老那条不许因为来了新消息就从上面消失")
        XCTAssertEqual(now.last, 202, "最新一条仍然在")
    }

    func test_没有新消息时上限不动() {
        XCTAssertEqual(CrewChatWindow.afterInsert(limit: 60, added: 0), 60)
        // 撤回/切 crew 让条目变少 → 负数，不许把上限改小（window 自己会夹）。
        XCTAssertEqual(CrewChatWindow.afterInsert(limit: 60, added: -5), 60)
    }

    func test_成本与历史长度脱钩() {
        // 同一个上限，crew 聊得再长，窗口取出来的行数不变 —— tech-debt #1621 那条
        // 「重排成本随消息数线性增长」就是被这一条掐掉的。
        for total in [30, 70, 338, 5_000] {
            XCTAssertEqual(
                CrewChatWindow.window(msgs(total), limit: CrewChatWindow.pageSize).count,
                min(total, CrewChatWindow.pageSize),
                "total=\(total)")
        }
    }
}
