#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

/// Todo #69 第 1 条：@-候选列表限高 + 内部滚动。
///
/// 人类看到的病：「输入框打@之后 出来好长一条列表 太长太长了 不行」。GUI 验不了，
/// 所以把「可用高度 + 行数 + 行高 → 实际 maxHeight」这段算成纯函数，用测试钉死。
///
/// 钉的是**四条不许再犯的事**：
/// 1. 候选多时不许超出可用空间（今天的 bug）；
/// 2. 候选少时不许撑出空盒子（别为了限高把小列表也拉大）；
/// 3. 窗口矮到连保底行数都放不下时，**可用空间赢**（否则保底会重犯第 1 条）；
/// 4. 裁剪时要留半行露头 —— macOS 滚动条默认隐藏，不留露头就看不出「还能滚」。
final class CrewMentionPickerLayoutTests: XCTestCase {

    private typealias L = CrewMentionPickerLayout

    // MARK: - 1) 顶穿：这条 Todo 的病本身

    /// 本机 crew 动辄四十几个成员 —— 那正是人类看到的那条「太长太长」。
    func testManyCandidatesNeverExceedAvailableHeight() {
        for available in stride(from: 80.0, through: 1200.0, by: 40.0) {
            let h = L.maxHeight(availableHeight: CGFloat(available), rowCount: 45)
            XCTAssertLessThanOrEqual(
                h, CGFloat(available),
                "45 个候选在可用高度 \(available) 下算出 \(h) —— 顶穿了，正是 #69 要修的那件事")
        }
    }

    /// 而且不许把聊天记录整个盖住：够放不下时只吃可用空间的一部分。
    func testTallListTakesOnlyAShareOfAvailableHeight() {
        let available: CGFloat = 600
        let h = L.maxHeight(availableHeight: available, rowCount: 45)
        XCTAssertLessThanOrEqual(h, available * L.availableShare)
        XCTAssertGreaterThan(h, L.height(showingRows: L.minVisibleRows))
    }

    // MARK: - 2) 候选少时视觉不变

    func testFewCandidatesKeepTheirNaturalHeight() {
        let available: CGFloat = 600
        for n in 1...8 {
            let content = L.contentHeight(rowCount: n)
            guard content <= available * L.availableShare else { continue }
            XCTAssertEqual(
                L.maxHeight(availableHeight: available, rowCount: n), content, accuracy: 0.001,
                "\(n) 个候选被撑成了盒子 —— 限高不该改变小列表的样子")
        }
    }

    /// 恒 ≤ 内容高度：任何组合下都不许算出一个比内容还高的框。
    func testNeverTallerThanItsOwnContent() {
        for n in [1, 2, 3, 5, 12, 45] {
            for available in [0.0, 60.0, 120.0, 400.0, 1400.0] {
                let h = L.maxHeight(availableHeight: CGFloat(available), rowCount: n)
                XCTAssertLessThanOrEqual(
                    h, L.contentHeight(rowCount: n), "n=\(n) available=\(available)")
            }
        }
    }

    func testZeroCandidatesIsJustChrome() {
        XCTAssertEqual(L.maxHeight(availableHeight: 600, rowCount: 0), L.chromeHeight, accuracy: 0.001)
    }

    // MARK: - 3) 矮窗口：保底行数不许反过来顶穿

    /// 这条是「钉死一个像素值」那种做法真正会犯的错：窗口被拖矮之后，那个下限
    /// 自己变成了新的顶穿源。可用空间必须赢。
    func testFloorNeverOverflowsAShortWindow() {
        let floorHeight = L.height(showingRows: L.minVisibleRows + 0.5)
        for available in stride(from: 20.0, through: floorHeight, by: 8.0) {
            let h = L.maxHeight(availableHeight: CGFloat(available), rowCount: 45)
            XCTAssertLessThanOrEqual(
                h, CGFloat(available),
                "可用高度 \(available) 小于保底 \(floorHeight) 时，保底把浮层顶穿了")
        }
    }

    /// 空间够时下界要真的生效 —— 比例算出来太小也得给到保底那么多。
    func testFloorAppliesWhenShareWouldBeTooSmall() {
        // 200 * 0.5 = 100，小于保底（3.5 行）。
        let available: CGFloat = 200
        XCTAssertLessThan(available * L.availableShare, L.height(showingRows: L.minVisibleRows + 0.5))
        let h = L.maxHeight(availableHeight: available, rowCount: 45)
        XCTAssertEqual(h, L.height(showingRows: L.minVisibleRows + 0.5), accuracy: 0.001)
        XCTAssertLessThanOrEqual(h, available)
    }

    // MARK: - 4) 裁剪时留半行露头（macOS 滚动条默认隐藏）

    func testClippedHeightLandsOnAHalfRow() {
        for available in [300.0, 420.0, 600.0, 900.0] {
            let h = L.maxHeight(availableHeight: CGFloat(available), rowCount: 45)
            let rows = (h - L.chromeHeight + L.dividerHeight) / (L.rowHeight + L.dividerHeight)
            XCTAssertEqual(
                rows - rows.rounded(.down), 0.5, accuracy: 0.001,
                "available=\(available) 裁成了整行 —— 没有半行露头，人看不出还能滚")
        }
    }

    // MARK: - 5) 没量到高度也不许回到「无上限」

    /// 首帧 / 没接测量的宿主：`availableHeight <= 0`。今天的 bug 就是「没有上限」，
    /// 兜底路径必须仍然有界。
    func testUnmeasuredHostStillGetsABound() {
        for available in [0.0, -1.0, -600.0] {
            let h = L.maxHeight(availableHeight: CGFloat(available), rowCount: 45)
            XCTAssertEqual(h, L.height(showingRows: L.fallbackRows), accuracy: 0.001)
            XCTAssertLessThan(h, L.contentHeight(rowCount: 45))
        }
        // 兜底路径同样不许撑空盒子。
        XCTAssertEqual(
            L.maxHeight(availableHeight: 0, rowCount: 2), L.contentHeight(rowCount: 2),
            accuracy: 0.001)
    }

    // MARK: - 6) 单调：窗口越高，能露出的越多（不许出现"拉高反而变矮"）

    func testHeightIsMonotonicInAvailableSpace() {
        var last: CGFloat = 0
        for available in stride(from: 60.0, through: 1400.0, by: 20.0) {
            let h = L.maxHeight(availableHeight: CGFloat(available), rowCount: 45)
            XCTAssertGreaterThanOrEqual(h, last, "可用高度涨到 \(available) 时浮层反而变矮了")
            last = h
        }
    }

    // MARK: - 7) 几何本身

    func testContentHeightCountsRowsDividersAndChrome() {
        XCTAssertEqual(L.contentHeight(rowCount: 1), L.rowHeight + L.chromeHeight, accuracy: 0.001)
        XCTAssertEqual(
            L.contentHeight(rowCount: 3),
            3 * L.rowHeight + 2 * L.dividerHeight + L.chromeHeight, accuracy: 0.001)
        // 45 个成员 ≈ 1400pt —— 这就是「太长太长」的量级，任何窗口都放不下。
        XCTAssertGreaterThan(L.contentHeight(rowCount: 45), 1400)
    }

    func testHeightShowingRowsMatchesContentHeightOnWholeRows() {
        for n in 1...10 {
            XCTAssertEqual(
                L.height(showingRows: CGFloat(n)), L.contentHeight(rowCount: n), accuracy: 0.001)
        }
    }
}
#endif
