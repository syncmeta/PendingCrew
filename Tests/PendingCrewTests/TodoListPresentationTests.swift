import XCTest

/// TodoListPresentation（Todo #4/#5/#11）：列表从新到旧 + 状态圆圈图标。
final class TodoListPresentationTests: XCTestCase {
    private func item(_ number: Int, status: String = "pending",
                      createdAt: String = "2026-07-26T00:00:00Z") -> LocalTodoItem {
        LocalTodoItem(id: "id-\(number)", number: number, text: "t\(number)",
                      status: status, createdAt: createdAt)
    }

    // MARK: - #4/#5 排序：新建的在最上面

    func testNewestFirstPutsHighestNumberOnTop() {
        let sorted = TodoListPresentation.newestFirst([item(1), item(2), item(3)])
        XCTAssertEqual(sorted.map(\.number), [3, 2, 1])
    }

    func testNewestFirstIsIndependentOfInputOrder() {
        let sorted = TodoListPresentation.newestFirst([item(2), item(11), item(1), item(7)])
        XCTAssertEqual(sorted.map(\.number), [11, 7, 2, 1])
    }

    func testNewestFirstIgnoresStatus() {
        // 已完成的旧条目不该因为状态被顶到前面（排序只看新旧）。
        let sorted = TodoListPresentation.newestFirst([
            item(1, status: "completed"), item(2, status: "pending"),
            item(3, status: "in_progress"),
        ])
        XCTAssertEqual(sorted.map(\.number), [3, 2, 1])
    }

    func testNewestFirstEmptyAndSingle() {
        XCTAssertTrue(TodoListPresentation.newestFirst([]).isEmpty)
        XCTAssertEqual(TodoListPresentation.newestFirst([item(5)]).map(\.number), [5])
    }

    func testNewestFirstFallsBackToCreatedAtForDuplicateNumbers() {
        // #N 理论上 crew 内唯一；真出现重号（历史文件被人工编辑过）也要稳定序。
        let older = item(1, createdAt: "2026-07-26T00:00:00Z")
        let newer = item(1, createdAt: "2026-07-26T09:00:00Z")
        XCTAssertEqual(TodoListPresentation.newestFirst([older, newer]).map(\.createdAt),
                       [newer.createdAt, older.createdAt])
    }

    // MARK: - #11 状态 → 圆圈图标（提醒事项逻辑）

    func testPendingIsHollowCircleNotBreathing() {
        let icon = TodoListPresentation.statusIcon("pending")
        XCTAssertEqual(icon.symbol, "circle")
        XCTAssertFalse(icon.isFilled)
        XCTAssertFalse(icon.isBreathing)
        XCTAssertFalse(icon.dimsText)
    }

    func testInProgressIsFilledAndBreathing() {
        let icon = TodoListPresentation.statusIcon("in_progress")
        XCTAssertTrue(icon.isFilled)
        XCTAssertTrue(icon.isBreathing)
        XCTAssertFalse(icon.dimsText)
    }

    func testCompletedIsFilledNotBreathingAndDimsText() {
        let icon = TodoListPresentation.statusIcon("completed")
        XCTAssertTrue(icon.isFilled)
        XCTAssertFalse(icon.isBreathing)
        XCTAssertTrue(icon.dimsText)
    }

    func testOnlyInProgressBreathes() {
        for status in ["pending", "completed", "weird"] {
            XCTAssertFalse(TodoListPresentation.statusIcon(status).isBreathing,
                           "\(status) 不该呼吸")
        }
    }

    func testUnknownStatusFallsBackToPendingAppearance() {
        XCTAssertEqual(TodoListPresentation.statusIcon("garbage"),
                       TodoListPresentation.statusIcon("pending"))
    }

    func testStatusAccessibilityLabelIsChinese() {
        XCTAssertEqual(TodoListPresentation.statusAccessibilityLabel("in_progress"), "进行中")
        XCTAssertEqual(TodoListPresentation.statusAccessibilityLabel("completed"), "完成")
        XCTAssertEqual(TodoListPresentation.statusAccessibilityLabel("pending"), "待办")
    }

    // MARK: - #52 建 Todo 的正文口径（能附图之后「只贴图不打字」是合法输入）

    func testNewTodoTextUsesTypedTextTrimmed() {
        XCTAssertEqual(
            TodoListPresentation.newTodoText(draft: "  修一下这个  ", attachmentCount: 0,
                                             allImages: true),
            "修一下这个")
        // 有字就用字，附件不影响正文。
        XCTAssertEqual(
            TodoListPresentation.newTodoText(draft: "如图", attachmentCount: 2, allImages: true),
            "如图")
    }

    func testNewTodoTextFallsBackToPlaceholderWhenOnlyAttachments() {
        XCTAssertEqual(
            TodoListPresentation.newTodoText(draft: "", attachmentCount: 1, allImages: true),
            "（见附图）")
        XCTAssertEqual(
            TodoListPresentation.newTodoText(draft: "  ", attachmentCount: 2, allImages: false),
            "（见附件）")
    }

    func testNewTodoTextNilWhenNothingToRecord() {
        XCTAssertNil(TodoListPresentation.newTodoText(draft: "   ", attachmentCount: 0,
                                                      allImages: true))
    }
}
