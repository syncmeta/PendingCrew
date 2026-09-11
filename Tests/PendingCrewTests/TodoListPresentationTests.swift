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

    // MARK: - `rows(for:)` 真的把排序用上了

    /// **面板调的是 `rows(for:)`，不是 `newestFirst`。** 上面那五条钉住了
    /// `newestFirst` 自己排得对，但没有一条钉住 `rows` 会去调它 ——
    /// 把 `case .agent` 那一行的 `newestFirst(` 拿掉，**全量 2695 条一条不红**
    /// （2026-09-12 变异量的）。于是「agent 那本从新到旧」这条保证当时没人守。
    ///
    /// 接线尺子也挡不住这一刀：`newestFirst` 在 `humanFacingRows` 里仍有调用点，
    /// `ViewWiringTests` 照样绿。**那把尺子问的是「有没有人用」，
    /// 不是「这一条路上有没有用」。**
    func test_agent那本经由rows也从新到旧() {
        let rows = TodoListPresentation.rows(
            for: .agent, human: [], agent: [item(1), item(3), item(2)])
        XCTAssertEqual(rows.map(\.item.number), [3, 2, 1],
                       "agent 那本没按从新到旧排 —— 新建的条目不在最上面")
        XCTAssertEqual(rows.map(\.ledger), [.agent, .agent, .agent])
    }

    /// 人类那本同理：`humanFacingRows` 自己排得对是一回事，`rows(for: .human)`
    /// 有没有走它是另一回事。
    func test_人类那本经由rows也是置顶加从新到旧() {
        let rows = TodoListPresentation.rows(
            for: .human,
            human: [item(1), item(2)],
            agent: [item(9, status: LocalTodoItem.blockedOnHumanStatus),
                    item(8, status: "in_progress")])
        XCTAssertEqual(rows.map(\.item.number), [9, 2, 1],
                       "等他回复的那条没置顶，或人类那本没从新到旧")
        XCTAssertEqual(rows.map(\.ledger), [.agent, .human, .human],
                       "每条必须带着自己那本账的归属")
    }

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

    // MARK: - #86 概览卡片契约

    func testOverviewLayoutPinsThreeLineBodyOneLineResponseAndHeaderAboveCard() {
        let layout = TodoListPresentation.overviewLayout
        XCTAssertEqual(layout.bodyLineLimit, 3)
        XCTAssertEqual(layout.responseLineLimit, 1)
        XCTAssertEqual(layout.statusNumberPlacement, .aboveCard)
        XCTAssertEqual(layout.responsePlacement, .insideCard)
        XCTAssertEqual(layout.detailButtonTitle, "放大看")
    }

    func testOverviewCardKeepsOnlyTopLeadingCornerSquare() {
        XCTAssertEqual(
            TodoListPresentation.overviewLayout.cardCorners,
            .init(topLeading: 0, bottomLeading: 8, bottomTrailing: 8, topTrailing: 8))
    }

    func testOverviewResponseUsesOnlyLatestResponseAsCompactSingleLineSummary() {
        var todo = item(84)
        todo.responses = [
            response(id: "old", sessionId: "worker-old", senderName: "成员", text: "先前回应"),
            response(id: "new", sessionId: "worker-new", senderName: " 机长 ",
                     text: " 已完成，\n 这是机长答复的示例。 "),
        ]

        XCTAssertEqual(TodoListPresentation.overviewResponse(for: todo),
                       "机长：已完成， 这是机长答复的示例。")
    }

    func testOverviewResponseFallsBackToShortSessionIdAndNilWhenUnanswered() {
        var todo = item(84)
        XCTAssertNil(TodoListPresentation.overviewResponse(for: todo))

        todo.responses = [
            response(id: "r", sessionId: "worker-123456789", senderName: nil, text: "收到"),
        ]
        XCTAssertEqual(TodoListPresentation.overviewResponse(for: todo),
                       "session:worker：收到")
    }

    // MARK: - #95 条目时间元信息

    func testMetadataTextLabelsLocalizedCreationAndUpdateTimes() {
        var todo = item(95, createdAt: "2026-09-01T02:03:04Z")
        todo.updatedAt = "2026-09-02T05:06:07Z"
        let text = TodoListPresentation.metadataText(
            for: todo,
            locale: Locale(identifier: "zh_CN"),
            timeZone: TimeZone(secondsFromGMT: 8 * 3600)!)

        XCTAssertTrue(text.hasPrefix("创建 "), text)
        XCTAssertTrue(text.contains(" · 更新 "), text)
        XCTAssertFalse(text.contains("T02:03:04Z"), "时间应本地化，不该直接铺 ISO 原文：\(text)")
        let values = String(text.dropFirst("创建 ".count))
            .components(separatedBy: " · 更新 ")
        XCTAssertEqual(values.count, 2, text)
        XCTAssertNotEqual(values[0], values[1],
                          "创建与更新时间不同，展示不应把两者误写成同一个值")
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

    private func response(id: String, sessionId: String, senderName: String?,
                          text: String) -> LocalTodoResponse {
        LocalTodoResponse(id: id, sessionId: sessionId, senderName: senderName, text: text,
                          createdAt: "2026-08-28T00:00:00Z")
    }
}
