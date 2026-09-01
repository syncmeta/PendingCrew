import XCTest

/// 两本 Todo 账的**隔离**（Todo #62 ①）：同一套 `LocalTodoStore` 基座、
/// 按 `TodoLedger` 参数化，落两个不同的文件、各自从 #1 编号、互相看不见。
///
/// 这一族测例是「不许 fork 出第二个 store」那条决定的守卫：如果哪天有人复制粘贴
/// 出第二份实现，隔离还能过，但下面 `testHumanLedgerKeepsTheCorruptArchiveBase`
/// 那条（走的是基座的 corrupt 归档）会告诉你新那份漏了基座的哪一件。
final class TodoLedgerIsolationTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("todo-ledger-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    // MARK: - 文件与编号的隔离

    func testTwoLedgersWriteDifferentFiles() {
        let dir = tempDir()
        _ = LocalTodoStore(directory: dir, ledger: .agent).add(crewId: "c", text: "派给 agent")
        _ = LocalTodoStore(directory: dir, ledger: .human).add(crewId: "c", text: "请人拍板")
        let fm = FileManager.default
        XCTAssertTrue(fm.fileExists(atPath: dir.appendingPathComponent("c.todos.json").path))
        XCTAssertTrue(fm.fileExists(atPath: dir.appendingPathComponent("c.human-todos.json").path))
    }

    /// 两本各自从 #1 起 —— 所以群里那行必须带账本前缀，不然 #1 指两件事。
    func testEachLedgerNumbersFromOne() {
        let dir = tempDir()
        let agent = LocalTodoStore(directory: dir, ledger: .agent)
        let human = LocalTodoStore(directory: dir, ledger: .human)
        XCTAssertEqual(agent.add(crewId: "c", text: "a1")?.number, 1)
        XCTAssertEqual(human.add(crewId: "c", text: "h1")?.number, 1)
        XCTAssertEqual(agent.add(crewId: "c", text: "a2")?.number, 2)
        XCTAssertEqual(human.add(crewId: "c", text: "h2")?.number, 2)
    }

    func testOneLedgerNeverSeesTheOthersRows() {
        let dir = tempDir()
        let agent = LocalTodoStore(directory: dir, ledger: .agent)
        let human = LocalTodoStore(directory: dir, ledger: .human)
        _ = agent.add(crewId: "c", text: "派给 agent")
        _ = human.add(crewId: "c", text: "请人拍板")
        XCTAssertEqual(agent.list(crewId: "c").map(\.text), ["派给 agent"])
        XCTAssertEqual(human.list(crewId: "c").map(\.text), ["请人拍板"])
    }

    /// 写一本不该动另一本：回应 / 改 / 删 / 重开打在 `.human` 的 #1 上，
    /// `.agent` 的 #1 一个字不变。
    func testWritesToOneLedgerLeaveTheOtherUntouched() {
        let dir = tempDir()
        let agent = LocalTodoStore(directory: dir, ledger: .agent)
        let human = LocalTodoStore(directory: dir, ledger: .human)
        _ = agent.add(crewId: "c", text: "派给 agent")
        _ = human.add(crewId: "c", text: "请人拍板")

        _ = human.respond(crewId: "c", number: 1, sessionId: "human",
                          senderName: "人", text: "我拍了：做 A", newStatus: "completed")
        _ = human.edit(crewId: "c", number: 1, text: "请人拍板（改过）")

        let a = agent.list(crewId: "c")
        XCTAssertEqual(a.count, 1)
        XCTAssertEqual(a[0].text, "派给 agent")
        XCTAssertEqual(a[0].status, "pending")
        XCTAssertTrue(a[0].responses.isEmpty)

        XCTAssertEqual(human.item(crewId: "c", number: 1)?.text, "请人拍板（改过）")
        XCTAssertEqual(human.item(crewId: "c", number: 1)?.status, "completed")
    }

    /// 删掉一本里的 #1，另一本的 #1 照常在。
    func testDeleteIsScopedToItsOwnLedger() {
        let dir = tempDir()
        let agent = LocalTodoStore(directory: dir, ledger: .agent)
        let human = LocalTodoStore(directory: dir, ledger: .human)
        _ = agent.add(crewId: "c", text: "派给 agent")
        _ = human.add(crewId: "c", text: "请人拍板")
        XCTAssertTrue(human.delete(crewId: "c", number: 1))
        XCTAssertTrue(human.list(crewId: "c").isEmpty)
        XCTAssertEqual(agent.list(crewId: "c").count, 1)
    }

    /// crew 隔离在两本账上各自成立（原有的 per-crew 隔离没被账本参数化弄坏）。
    func testCrewScopingStillHoldsInsideEachLedger() {
        let dir = tempDir()
        let human = LocalTodoStore(directory: dir, ledger: .human)
        XCTAssertEqual(human.add(crewId: "c1", text: "a")?.number, 1)
        XCTAssertEqual(human.add(crewId: "c2", text: "b")?.number, 1)
        XCTAssertEqual(human.add(crewId: "c1", text: "c")?.number, 2)
    }

    /// 默认参数 = `.agent`，且落的还是原来那个文件名 —— 已有机器上的账不能搬家。
    func testDefaultLedgerIsAgentAndKeepsLegacyFilename() {
        let dir = tempDir()
        let s = LocalTodoStore(directory: dir)
        XCTAssertEqual(s.ledger, .agent)
        _ = s.add(crewId: "c", text: "旧账")
        XCTAssertTrue(FileManager.default
            .fileExists(atPath: dir.appendingPathComponent("c.todos.json").path))
        // 读回来的还是同一份（新老实例互通）。
        XCTAssertEqual(LocalTodoStore(directory: dir, ledger: .agent)
            .list(crewId: "c").map(\.text), ["旧账"])
    }

    // MARK: - 基座三件套在新账本上同样在场（这就是「不许 fork」的理由）

    /// corrupt 归档 + 不清史：`.human` 那本走的是同一个 `MultiProcessJSONStore`
    /// 基座，所以整份文件坏掉时同样归档、同样不静默清空。
    func testHumanLedgerKeepsTheCorruptArchiveBase() throws {
        let dir = tempDir()
        let human = LocalTodoStore(directory: dir, ledger: .human)
        _ = human.add(crewId: "c", text: "先有一条")
        let file = dir.appendingPathComponent("c.human-todos.json")
        try "{ 这不是 JSON".write(to: file, atomically: true, encoding: .utf8)

        // 坏了之后再加一条：基座会先归档坏文件，再从空重来 —— 不是拿着空表覆盖。
        _ = human.add(crewId: "c", text: "坏掉之后")
        let archived = try FileManager.default.contentsOfDirectory(atPath: dir.path)
            .filter { $0.hasPrefix("c.human-todos.json") && $0 != "c.human-todos.json" }
        XCTAssertFalse(archived.isEmpty, "坏文件必须被归档，不许静默丢：\(archived)")
    }

    /// 逐条 lenient 解码：坏的那条丢掉，好的那条留着。
    func testHumanLedgerDropsOnlyBadRows() throws {
        let dir = tempDir()
        let file = dir.appendingPathComponent("c.human-todos.json")
        let json = """
            [{"id":"1","number":1,"text":"好的","status":"pending","createdAt":"2026-08-25T00:00:00Z","responses":[]},
             {"id":"2","text":"缺 number"},
             {"id":"3","number":3,"text":"也是好的","status":"pending","createdAt":"2026-08-25T00:00:01Z","responses":[]}]
            """
        try json.write(to: file, atomically: true, encoding: .utf8)
        let rows = LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c")
        XCTAssertEqual(rows.map(\.number), [1, 3])
    }

    // MARK: - 账本的方向与措辞

    func testLedgerDirectionsAreMirrored() {
        XCTAssertEqual(TodoLedger.agent.author, .human)
        XCTAssertEqual(TodoLedger.agent.responder, .agent)
        XCTAssertEqual(TodoLedger.human.author, .agent)
        XCTAssertEqual(TodoLedger.human.responder, .human)
    }

    /// 群里那行必须一眼分清是哪本账 —— 两本各自从 #1 编号，不带前缀就会打架。
    func testGroupChatLinesAreDistinguishable() {
        XCTAssertEqual(TodoLedger.agent.newItemAnnouncement(number: 7, text: "修登录"),
                       "To do +1: #7 修登录")
        XCTAssertEqual(TodoLedger.human.newItemAnnouncement(number: 7, text: "选 A 还是 B"),
                       "人类 To do +1: #7 选 A 还是 B")
        XCTAssertEqual(TodoLedger.human.responseAnnouncement(number: 7, text: "选 A"),
                       "回应 人类 To Do #7：选 A")
        XCTAssertNotEqual(TodoLedger.agent.newItemAnnouncement(number: 1, text: "x"),
                          TodoLedger.human.newItemAnnouncement(number: 1, text: "x"))
    }

    func testIncidentSubjectNamesWhichLedger() {
        XCTAssertNotEqual(TodoLedger.agent.incidentSubject, TodoLedger.human.incidentSubject)
        XCTAssertTrue(TodoLedger.human.incidentSubject.contains("人类"))
        XCTAssertTrue(TodoLedger.agent.incidentSubject.contains("Agent"))
    }

    // MARK: - createdBySessionId（Todo #62 ②）

    func testHumanLedgerRecordsWhoAsked() throws {
        let dir = tempDir()
        let human = LocalTodoStore(directory: dir, ledger: .human)
        let item = try XCTUnwrap(human.add(crewId: "c", text: "选 A 还是 B",
                                           bySessionId: "worker-42", bySenderName: "两本账"))
        XCTAssertEqual(item.createdBySessionId, "worker-42")
        XCTAssertEqual(item.createdBySenderName, "两本账")
        // 落盘后读回来还在（老实例也读得出 —— 字段是可选的，不破坏旧文件）。
        let reread = try XCTUnwrap(LocalTodoStore(directory: dir, ledger: .human)
            .item(crewId: "c", number: 1))
        XCTAssertEqual(reread.createdBySessionId, "worker-42")
    }

    /// 人类那本由人类新增（`.agent` 账本）时不记提问者 —— 那本没这个概念。
    func testAgentLedgerLeavesAskerNil() throws {
        let dir = tempDir()
        let item = try XCTUnwrap(LocalTodoStore(directory: dir).add(crewId: "c", text: "修登录"))
        XCTAssertNil(item.createdBySessionId)
        XCTAssertNil(item.createdBySenderName)
    }

    /// 老文件没有新字段照样解得开（不能因为加了字段就把已有的账读没了）。
    func testOldJSONWithoutNewFieldsStillDecodes() throws {
        let dir = tempDir()
        let file = dir.appendingPathComponent("c.human-todos.json")
        try #"[{"id":"1","number":1,"text":"老条目","status":"pending","createdAt":"2026-08-25T00:00:00Z","responses":[]}]"#
            .write(to: file, atomically: true, encoding: .utf8)
        let item = try XCTUnwrap(LocalTodoStore(directory: dir, ledger: .human)
            .item(crewId: "c", number: 1))
        XCTAssertEqual(item.text, "老条目")
        XCTAssertNil(item.createdBySessionId)
        XCTAssertNil(item.dismissedAt)
        XCTAssertTrue(item.isUnanswered)
    }

    func testOldJSONWithoutUpdatedAtFallsBackToCreationOrLatestResponse() throws {
        let dir = tempDir()
        let file = dir.appendingPathComponent("c.human-todos.json")
        let json = """
            [{"id":"1","number":1,"text":"未回应","status":"pending","createdAt":"2026-08-25T00:00:00Z","responses":[]},
             {"id":"2","number":2,"text":"有旧回应","status":"pending","createdAt":"2026-08-25T00:00:00Z",
              "responses":[{"id":"r","sessionId":"s","text":"旧回应","createdAt":"2026-08-26T03:04:05Z"}]}]
            """
        try json.write(to: file, atomically: true, encoding: .utf8)
        let rows = LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c")

        XCTAssertEqual(rows.count, 2, "缺 updatedAt 的旧条目不能解码失败")
        XCTAssertNil(rows[0].updatedAt)
        XCTAssertEqual(rows[0].effectiveUpdatedAt, rows[0].createdAt)
        XCTAssertNil(rows[1].updatedAt)
        XCTAssertEqual(rows[1].effectiveUpdatedAt, "2026-08-26T03:04:05Z")
    }

    // MARK: - 黄点判据（Todo #62 ⑥）：未回应就亮，全部有回应就灭

    func testUnansweredFlipsOnResponse() {
        let dir = tempDir()
        let human = LocalTodoStore(directory: dir, ledger: .human)
        _ = human.add(crewId: "c", text: "选 A 还是 B", bySessionId: "w1")
        XCTAssertTrue(human.list(crewId: "c").contains { $0.isUnanswered })
        _ = human.respond(crewId: "c", number: 1, sessionId: "human",
                          senderName: "人", text: "选 A")
        XCTAssertFalse(human.list(crewId: "c").contains { $0.isUnanswered })
    }

    /// 不回应也能按灭 —— 否则一条人类不打算处理的条目会把黄点永久钉死。
    func testDismissClearsUnansweredWithoutRespondingOrChangingStatus() throws {
        let dir = tempDir()
        let human = LocalTodoStore(directory: dir, ledger: .human)
        _ = human.add(crewId: "c", text: "要不要上这个功能", bySessionId: "w1")
        XCTAssertTrue(human.setDismissed(crewId: "c", number: 1))
        let item = try XCTUnwrap(human.item(crewId: "c", number: 1))
        XCTAssertFalse(item.isUnanswered)
        XCTAssertTrue(item.responses.isEmpty, "按灭不该伪造一条回应")
        XCTAssertEqual(item.status, "pending", "按灭不该动状态")
        XCTAssertNotNil(item.dismissedAt)
        // 反悔：重新算作未回应。
        XCTAssertTrue(human.setDismissed(crewId: "c", number: 1, dismissed: false))
        XCTAssertTrue(try XCTUnwrap(human.item(crewId: "c", number: 1)).isUnanswered)
    }

    func testDismissUnknownNumberIsFalse() {
        let dir = tempDir()
        XCTAssertFalse(LocalTodoStore(directory: dir, ledger: .human)
            .setDismissed(crewId: "c", number: 9))
    }

    /// 新条目会把黄点重新点亮 —— 按灭的是那一条，不是这本账。
    func testNewItemRelightsAfterDismiss() {
        let dir = tempDir()
        let human = LocalTodoStore(directory: dir, ledger: .human)
        _ = human.add(crewId: "c", text: "第一条", bySessionId: "w1")
        _ = human.setDismissed(crewId: "c", number: 1)
        XCTAssertFalse(human.list(crewId: "c").contains { $0.isUnanswered })
        _ = human.add(crewId: "c", text: "第二条", bySessionId: "w2")
        XCTAssertTrue(human.list(crewId: "c").contains { $0.isUnanswered })
    }
}
