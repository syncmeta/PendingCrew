import XCTest

/// LocalTodoStore（task #478）：人类 Todo 列表数据层。
final class LocalTodoStoreTests: XCTestCase {
    private final class StepClock: @unchecked Sendable {
        private let dates: [Date]
        private(set) var calls = 0

        init(_ dates: [Date]) { self.dates = dates }

        func next() -> Date {
            defer { calls += 1 }
            return dates[calls]
        }
    }

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("todo-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func testAddAssignsIncreasingNumbersAndPendingStatus() throws {
        let s = LocalTodoStore(directory: tempDir())
        let a = try XCTUnwrap(s.add(crewId: "c", text: "修复登录"))
        let b = try XCTUnwrap(s.add(crewId: "c", text: "写文档"))
        XCTAssertEqual(a.number, 1)
        XCTAssertEqual(b.number, 2)
        XCTAssertEqual(a.status, "pending")
        XCTAssertTrue(a.responses.isEmpty)
        XCTAssertEqual(s.list(crewId: "c").map(\.text), ["修复登录", "写文档"])
    }

    func testNumbersAreScopedPerCrew() {
        let s = LocalTodoStore(directory: tempDir())
        XCTAssertEqual(s.add(crewId: "c1", text: "a")?.number, 1)
        XCTAssertEqual(s.add(crewId: "c2", text: "b")?.number, 1)
        XCTAssertEqual(s.add(crewId: "c1", text: "c")?.number, 2)
    }

    func testRespondAppendsAndAdvancesStatus() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "修复登录")
        let r1 = s.respond(crewId: "c", number: 1, sessionId: "sess-1",
                           senderName: "机长", text: "收到，安排中", newStatus: "in_progress")
        XCTAssertEqual(r1?.status, "in_progress")
        XCTAssertEqual(r1?.responses.count, 1)
        XCTAssertEqual(r1?.responses[0].senderName, "机长")
        XCTAssertEqual(r1?.responses[0].status, "in_progress")
        // 追加式：第二条回应不覆盖第一条。
        let r2 = s.respond(crewId: "c", number: 1, sessionId: "sess-2",
                           text: "已修好并验证", newStatus: "completed")
        XCTAssertEqual(r2?.status, "completed")
        XCTAssertEqual(r2?.responses.map(\.text), ["收到，安排中", "已修好并验证"])
    }

    func testRespondWithoutStatusKeepsStatus() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "调研方案")
        let r = s.respond(crewId: "c", number: 1, sessionId: "sess-1", text: "看了一圈，明天给结论")
        XCTAssertEqual(r?.status, "pending")
        XCTAssertNil(r?.responses[0].status)
    }

    // MARK: - #95 创建 / 更新时间

    func testEveryRealWriteAdvancesUpdatedAtIncludingSoftDelete() throws {
        let dir = tempDir()
        let dates = (0..<8).map { Date(timeIntervalSince1970: 1_800_000_000 + Double($0)) }
        let clock = StepClock(dates)
        let s = LocalTodoStore(directory: dir, now: { clock.next() })

        let added = try XCTUnwrap(s.add(crewId: "c", text: "原文"))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(added.updatedAt)), dates[0])
        XCTAssertEqual(added.createdAt, added.updatedAt)

        let responded = try XCTUnwrap(s.respond(
            crewId: "c", number: 1, sessionId: "s", text: "收到", newStatus: "in_progress"))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(responded.updatedAt)), dates[1])

        let edited = try XCTUnwrap(s.edit(crewId: "c", number: 1, text: "新正文"))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(edited.updatedAt)), dates[2])

        let followedUp = try XCTUnwrap(s.followUp(crewId: "c", number: 1, note: "再看看"))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(followedUp.updatedAt)), dates[3])

        let completed = try XCTUnwrap(s.respond(
            crewId: "c", number: 1, sessionId: "s", text: "完成", newStatus: "completed"))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(completed.updatedAt)), dates[4])

        let reopened = try XCTUnwrap(s.reopen(crewId: "c", number: 1, note: "重开"))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(reopened.updatedAt)), dates[5])

        XCTAssertTrue(s.setDismissed(crewId: "c", number: 1))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(
            s.item(crewId: "c", number: 1)?.updatedAt)), dates[6])

        XCTAssertTrue(s.delete(crewId: "c", number: 1))
        let stored = try JSONDecoder().decode(
            [LocalTodoItem].self, from: Data(contentsOf: rawFileURL(dir, "c")))
        XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(stored[0].updatedAt)), dates[7])
        XCTAssertEqual(stored[0].deletedAt, stored[0].updatedAt,
                       "软删虽不再可见，墓碑与更新时间仍应是同一次写入")
    }

    func testBothLedgersPersistUpdatedAt() throws {
        for ledger in TodoLedger.allCases {
            let date = Date(timeIntervalSince1970: 1_800_000_100)
            let store = LocalTodoStore(directory: tempDir(), ledger: ledger, now: { date })
            let item = try XCTUnwrap(store.add(crewId: "c", text: ledger.pillTitle))
            XCTAssertEqual(CrewTimestamp.parse(try XCTUnwrap(item.updatedAt)), date)
            XCTAssertEqual(
                CrewTimestamp.parse(try XCTUnwrap(store.respond(
                    crewId: "c", number: 1, sessionId: "s", text: "回应")?.updatedAt)),
                date)
        }
    }

    func testReadsAndNoOpWritesDoNotAdvanceUpdatedAt() throws {
        let dates = (0..<2).map { Date(timeIntervalSince1970: 1_800_000_200 + Double($0)) }
        let clock = StepClock(dates)
        let s = LocalTodoStore(directory: tempDir(), now: { clock.next() })
        let added = try XCTUnwrap(s.add(crewId: "c", text: "不变"))
        let original = added.updatedAt

        _ = s.list(crewId: "c")
        _ = s.item(crewId: "c", number: 1)
        _ = s.edit(crewId: "c", number: 1, text: "  不变  ")
        XCTAssertTrue(s.setDismissed(crewId: "c", number: 1, dismissed: false))
        XCTAssertNil(s.reopen(crewId: "c", number: 1, note: "状态不是完成"))

        XCTAssertEqual(s.item(crewId: "c", number: 1)?.updatedAt, original)
        XCTAssertEqual(clock.calls, 1, "纯读取和没有真实变化的写请求不应读取新时间")
    }

    func testRespondUnknownNumberReturnsNil() {
        let s = LocalTodoStore(directory: tempDir())
        XCTAssertNil(s.respond(crewId: "c", number: 7, sessionId: "sess-1", text: "?"))
    }

    // MARK: - reopen（Todo #12：人类把 completed 翻回 pending + 追问落时间线）

    func testReopenCompletedFlipsToPendingAndAppendsFollowUp() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "修复登录")
        _ = s.respond(crewId: "c", number: 1, sessionId: "sess-1",
                      senderName: "机长", text: "已修好", newStatus: "completed")
        let r = s.reopen(crewId: "c", number: 1, note: "真机上还是复现，再查一下")
        XCTAssertEqual(r?.status, "pending")
        XCTAssertEqual(r?.responses.count, 2)   // 追加式：机器人回应保留
        XCTAssertEqual(r?.responses.last?.sessionId, "human")
        XCTAssertEqual(r?.responses.last?.senderName, "人")
        XCTAssertEqual(r?.responses.last?.text, "真机上还是复现，再查一下")
        XCTAssertEqual(r?.responses.last?.status, "pending")
    }

    func testReopenNonCompletedReturnsNilAndKeepsState() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "调研方案")   // pending
        XCTAssertNil(s.reopen(crewId: "c", number: 1, note: "追问"))
        _ = s.respond(crewId: "c", number: 1, sessionId: "s", text: "做着", newStatus: "in_progress")
        XCTAssertNil(s.reopen(crewId: "c", number: 1, note: "追问"))
        let row = s.list(crewId: "c")[0]
        XCTAssertEqual(row.status, "in_progress")
        XCTAssertEqual(row.responses.count, 1)   // 两次 reopen 都没落记录
    }

    func testReopenUnknownNumberReturnsNil() {
        let s = LocalTodoStore(directory: tempDir())
        XCTAssertNil(s.reopen(crewId: "c", number: 7, note: "?"))
    }

    func testReopenEmptyNoteFallsBackToDefaultText() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "写文档")
        _ = s.respond(crewId: "c", number: 1, sessionId: "s", text: "好了", newStatus: "completed")
        let r = s.reopen(crewId: "c", number: 1, note: "   ")
        XCTAssertEqual(r?.responses.last?.text, "重开了这条 Todo")
    }

    // MARK: - edit / delete / followUp（Todo #21：随时改、删、追问）

    func testEditChangesTextAtAnyStatusWithoutTouchingStateOrResponses() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "修复登录")
        _ = s.respond(crewId: "c", number: 1, sessionId: "s",
                      senderName: "机长", text: "已修好", newStatus: "completed")
        // 已完成 + 已被回复过 —— 人类明确要求这种也能改。
        let r = s.edit(crewId: "c", number: 1, text: "修复登录（含 iPad）")
        XCTAssertEqual(r?.text, "修复登录（含 iPad）")
        XCTAssertEqual(r?.status, "completed")     // 改正文不动状态
        XCTAssertEqual(r?.responses.count, 1)      // 也不动时间线
        XCTAssertEqual(s.list(crewId: "c")[0].text, "修复登录（含 iPad）")
    }

    func testEditTrimsAndRejectsBlank() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "原文")
        XCTAssertEqual(s.edit(crewId: "c", number: 1, text: "  改过  ")?.text, "改过")
        XCTAssertNil(s.edit(crewId: "c", number: 1, text: "   "))
        XCTAssertEqual(s.list(crewId: "c")[0].text, "改过")   // 空白没把正文冲掉
    }

    func testEditUnknownNumberReturnsNil() {
        let s = LocalTodoStore(directory: tempDir())
        XCTAssertNil(s.edit(crewId: "c", number: 7, text: "x"))
    }

    func testDeleteHidesRowButKeepsNumberReserved() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "一")
        _ = s.add(crewId: "c", text: "二")
        XCTAssertTrue(s.delete(crewId: "c", number: 2))
        XCTAssertEqual(s.list(crewId: "c").map(\.text), ["一"])
        XCTAssertNil(s.item(crewId: "c", number: 2))
        // #2 已经在群里以「To do +1: #2」说过 —— 不能被下一条复用。
        XCTAssertEqual(s.add(crewId: "c", text: "三")?.number, 3)
    }

    func testDeletedRowIsInvisibleToAllWritePaths() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "删掉我")
        XCTAssertTrue(s.delete(crewId: "c", number: 1))
        XCTAssertFalse(s.delete(crewId: "c", number: 1))    // 二次删 = 找不到
        XCTAssertNil(s.respond(crewId: "c", number: 1, sessionId: "s", text: "回应"))
        XCTAssertNil(s.edit(crewId: "c", number: 1, text: "改"))
        XCTAssertNil(s.followUp(crewId: "c", number: 1, note: "追问"))
    }

    func testDeleteSurvivesReloadAndUnknownNumberIsFalse() {
        let dir = tempDir()
        _ = LocalTodoStore(directory: dir).add(crewId: "c", text: "一")
        XCTAssertTrue(LocalTodoStore(directory: dir).delete(crewId: "c", number: 1))
        XCTAssertTrue(LocalTodoStore(directory: dir).list(crewId: "c").isEmpty)
        XCTAssertFalse(LocalTodoStore(directory: dir).delete(crewId: "c", number: 9))
    }

    func testFollowUpOnCompletedFlipsToPending() {
        // 追问与重开是同一条通道：完成的被追问 = 事情没完。
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "修复登录")
        _ = s.respond(crewId: "c", number: 1, sessionId: "s", text: "已修好", newStatus: "completed")
        let r = s.followUp(crewId: "c", number: 1, note: "真机还复现")
        XCTAssertEqual(r?.status, "pending")
        XCTAssertEqual(r?.responses.last?.sessionId, "human")
        XCTAssertEqual(r?.responses.last?.senderName, "人")
        XCTAssertEqual(r?.responses.last?.text, "真机还复现")
        XCTAssertEqual(r?.responses.last?.status, "pending")
    }

    func testFollowUpOnUnfinishedKeepsStatus() {
        // 进行中的被追问不该打回待办 —— 已经在做了。
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "调研方案")
        let p = s.followUp(crewId: "c", number: 1, note: "什么时候有结论？")
        XCTAssertEqual(p?.status, "pending")
        XCTAssertNil(p?.responses.last?.status)      // 这条追问没动状态
        _ = s.respond(crewId: "c", number: 1, sessionId: "s", text: "在看", newStatus: "in_progress")
        let ip = s.followUp(crewId: "c", number: 1, note: "还要多久？")
        XCTAssertEqual(ip?.status, "in_progress")
        XCTAssertNil(ip?.responses.last?.status)
        XCTAssertEqual(ip?.responses.map(\.text),
                       ["什么时候有结论？", "在看", "还要多久？"])   // 时间序，追加式
    }

    func testFollowUpEmptyNoteFallsBackPerStatus() {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "写文档")
        XCTAssertEqual(s.followUp(crewId: "c", number: 1, note: " ")?.responses.last?.text,
                       "追问了这条 Todo")
        _ = s.respond(crewId: "c", number: 1, sessionId: "s", text: "好了", newStatus: "completed")
        XCTAssertEqual(s.followUp(crewId: "c", number: 1, note: "")?.responses.last?.text,
                       "重开了这条 Todo")
    }

    func testFollowUpUnknownNumberReturnsNil() {
        let s = LocalTodoStore(directory: tempDir())
        XCTAssertNil(s.followUp(crewId: "c", number: 7, note: "?"))
    }

    func testPersistsAcrossInstances() {
        let dir = tempDir()
        _ = LocalTodoStore(directory: dir).add(crewId: "c", text: "持久化")
        // 另一实例（模拟 helper 子进程）读同一目录、回应后 app 实例可见。
        _ = LocalTodoStore(directory: dir).respond(
            crewId: "c", number: 1, sessionId: "s", text: "ok", newStatus: "completed")
        let rows = LocalTodoStore(directory: dir).list(crewId: "c")
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].status, "completed")
        XCTAssertEqual(rows[0].responses.count, 1)
    }

    // MARK: - 附件（Todo #52：新建 / 追问都能附图）

    private func attachment(_ path: String, mime: String = "image/png") -> LocalWhiteboardAttachment {
        LocalWhiteboardAttachment(id: UUID().uuidString, mime: mime, size: 12, path: path)
    }

    func testAddKeepsAttachmentsAndRendersAgentHints() throws {
        let dir = tempDir()
        let img = attachment("/tmp/att/IMG-1.png")
        _ = LocalTodoStore(directory: dir).add(
            crewId: "c", text: "如图，这个叉按钮有阴影", attachments: [img])
        // 另一实例（模拟 helper 子进程）读同一目录 —— 附件跟着条目落盘。
        let row = try XCTUnwrap(LocalTodoStore(directory: dir).item(crewId: "c", number: 1))
        XCTAssertEqual(row.attachments?.map(\.path), ["/tmp/att/IMG-1.png"])
        // 给 agent 的措辞与群聊同一套（照抄 LocalWhiteboardAttachment.agentHint）。
        XCTAssertEqual(row.agentText,
                       "如图，这个叉按钮有阴影\n用户发来图片：/tmp/att/IMG-1.png（请 Read 查看）")
    }

    func testAddWithoutAttachmentsLeavesFieldNil() throws {
        let s = LocalTodoStore(directory: tempDir())
        let a = try XCTUnwrap(s.add(crewId: "c", text: "纯文字"))
        XCTAssertNil(a.attachments)
        // 空数组也归 nil —— 别在 JSON 里留一串空壳。
        let b = try XCTUnwrap(s.add(crewId: "c", text: "空数组", attachments: []))
        XCTAssertNil(b.attachments)
        XCTAssertEqual(b.agentText, "空数组")
    }

    func testFollowUpCarriesAttachmentsOnThatResponseOnly() throws {
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "改配色", attachments: [attachment("/tmp/att/before.png")])
        let r = try XCTUnwrap(s.followUp(crewId: "c", number: 1, note: "还是不对，见图",
                                         attachments: [attachment("/tmp/att/after.png")]))
        // 追问的图挂在追问那条上，条目本身的图不动。
        XCTAssertEqual(r.attachments?.map(\.path), ["/tmp/att/before.png"])
        XCTAssertEqual(r.responses.last?.attachments?.map(\.path), ["/tmp/att/after.png"])
        XCTAssertEqual(r.responses.last?.agentText,
                       "还是不对，见图\n用户发来图片：/tmp/att/after.png（请 Read 查看）")
    }

    func testFollowUpWithOnlyAttachmentsFallsBackToDefaultNote() throws {
        // 只贴图不打字也得留下一条读得懂的追问。
        let s = LocalTodoStore(directory: tempDir())
        _ = s.add(crewId: "c", text: "调布局")
        let r = try XCTUnwrap(s.followUp(crewId: "c", number: 1, note: "",
                                         attachments: [attachment("/tmp/att/x.png")]))
        XCTAssertEqual(r.responses.last?.text, "追问了这条 Todo")
        XCTAssertEqual(r.responses.last?.attachments?.count, 1)
    }

    func testOldJSONWithoutAttachmentsFieldStillDecodes() throws {
        // 向后兼容：#52 之前写的条目/回应没有 attachments 键，照样读得出来。
        let dir = tempDir()
        let json = """
        [{"id":"a","number":1,"text":"老条目","status":"pending","createdAt":"2026-08-01T00:00:00Z",
          "responses":[{"id":"r","sessionId":"s","text":"老回应","createdAt":"2026-08-01T00:00:01Z"}]}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir, "c"))
        let row = try XCTUnwrap(LocalTodoStore(directory: dir).item(crewId: "c", number: 1))
        XCTAssertNil(row.attachments)
        XCTAssertEqual(row.agentText, "老条目")
        XCTAssertNil(row.responses.first?.attachments)
    }

    // MARK: - #528 基座三件套（照 LocalWhiteboardStoreTests 的 #483 模式）

    private func rawFileURL(_ dir: URL, _ crewId: String) -> URL {
        dir.appendingPathComponent("\(crewId).todos.json")
    }

    private func corruptArchives(_ dir: URL, _ crewId: String) throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: dir.path)
            .filter { $0.hasPrefix("\(crewId).todos.json.corrupt-") }
    }

    func testCorruptFileArchivedAndNextAddDoesNotClobber() throws {
        // 曾经的致命路径：损坏 → load 当空 → 下一次写以空数组重写落盘、历史蒸发。
        // 现在：损坏字节归档可找回 + 白板落系统警示，新增从 #1 重新开始。
        let dir = tempDir()
        let garbage = Data("not json {{{".utf8)
        try garbage.write(to: rawFileURL(dir, "c"))
        let s = LocalTodoStore(directory: dir)
        let item = try XCTUnwrap(s.add(crewId: "c", text: "损坏后新增"))
        XCTAssertEqual(item.number, 1)
        XCTAssertEqual(s.list(crewId: "c").map(\.text), ["损坏后新增"])
        let archived = try corruptArchives(dir, "c")
        XCTAssertEqual(archived.count, 1)
        XCTAssertEqual(try Data(contentsOf: dir.appendingPathComponent(archived[0])), garbage)
        // fail-loud：白板上有系统警示，人在群里看得到出过事。
        let notices = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertEqual(notices.count, 1)
        XCTAssertEqual(notices[0].senderSessionId, "system")
        XCTAssertTrue(notices[0].text.contains("Todo"))
        XCTAssertTrue(notices[0].text.contains("已归档"), notices[0].text)
    }

    func testLenientDecodeDropsOnlyBadElements() throws {
        // 中间一条缺必填字段 → 只丢那条，好的不连坐，也不算整文件损坏（不归档）。
        let dir = tempDir()
        let json = """
        [{"id":"a","number":1,"text":"one","status":"pending","createdAt":"2026-07-25T00:00:00Z","responses":[]},
         {"id":"b","number":2,"status":"pending","createdAt":"2026-07-25T00:00:01Z","responses":[]},
         {"id":"c","number":3,"text":"three","status":"completed","createdAt":"2026-07-25T00:00:02Z","responses":[]}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir, "c"))
        let s = LocalTodoStore(directory: dir)
        XCTAssertEqual(s.list(crewId: "c").map(\.text), ["one", "three"])
        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty)
        // 后续写不清史：respond 落在幸存条目上，好消息仍在。
        _ = s.respond(crewId: "c", number: 1, sessionId: "s", text: "ok")
        XCTAssertEqual(s.list(crewId: "c").map(\.text), ["one", "three"])
    }

    func testConcurrentWritesAcrossInstancesLoseNothing() {
        // 模拟 app 与 helper 两个进程（两个实例）并发 add —— flock 后一条不丢，
        // 且 #N 分配串行化不重号。
        let dir = tempDir()
        let a = LocalTodoStore(directory: dir)
        let b = LocalTodoStore(directory: dir)
        DispatchQueue.concurrentPerform(iterations: 40) { i in
            _ = (i % 2 == 0 ? a : b).add(crewId: "c", text: "t\(i)")
        }
        let rows = a.list(crewId: "c")
        XCTAssertEqual(rows.count, 40)
        XCTAssertEqual(Set(rows.map(\.number)).count, 40)
    }

    // MARK: - #577 读不出来时不许谎报成功

    private func rawTodoURL(_ dir: URL) -> URL { dir.appendingPathComponent("c.todos.json") }

    func testAddReturnsNilWhenListFileIsUnreadable() throws {
        // 旧行为：照样返回条目 → 调用方拿着一个磁盘上不存在的 #N 去群里宣布。
        let dir = tempDir()
        let url = rawTodoURL(dir)
        let original = Data(#"[{"id":"a","number":1,"text":"人手输的","status":"pending","createdAt":"2026-08-01T00:00:00Z"}]"#.utf8)
        try original.write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer { _ = chmod(url.path, S_IRUSR | S_IWUSR) }

        XCTAssertNil(LocalTodoStore(directory: dir).add(crewId: "c", text: "新待办"))

        // ⚠️ 2026-08-12 失效批注：这里原先断言「原字节已归档为 .corrupt-*」。
        // 那个动作在 fd 打满的瞬时读失败下会把完好的 todos.json 整份搬走。
        // 现在读不出来 = 拒写 + 原件原地不动 + 零归档（fail-loud 由返回 nil 承担）。
        XCTAssertTrue(
            try FileManager.default.contentsOfDirectory(atPath: dir.path)
                .filter { $0.hasPrefix("c.todos.json.corrupt-") }.isEmpty,
            "读不出来不许归档")
        XCTAssertEqual(chmod(url.path, S_IRUSR | S_IWUSR), 0)
        XCTAssertEqual(try Data(contentsOf: url), original, "原件一个字节都不许动")
    }

    func testRespondOnUnreadableListIsFailLoudNotJustNotFound() throws {
        // 读不出来时 respond 只会「找不到 #N」—— 听着像人类删了。现在至少归档 +
        // 白板警示，群里看得见真正的原因。
        let dir = tempDir()
        let url = rawTodoURL(dir)
        try Data(#"[{"id":"a","number":1,"text":"x","status":"pending","createdAt":"2026-08-01T00:00:00Z"}]"#.utf8)
            .write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer { _ = chmod(url.path, S_IRUSR | S_IWUSR) }

        XCTAssertNil(LocalTodoStore(directory: dir).respond(
            crewId: "c", number: 1, sessionId: "s", text: "回应"))

        let board = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertEqual(board.count, 1)
        XCTAssertEqual(board[0].senderSessionId, "system")
        XCTAssertTrue(board[0].text.contains("Todo"))
    }

    // MARK: - 账读不动时，提上来的话先存着别丢（2026-09-12）

    /// 人类那条提醒每次都在问的，就是这件事：「账上可能挂着一堆，只是这一刻看不见。」
    /// 而看不见的那段时间里**提上来的**，以前一条都没留下 —— `add` 撞上拒写闸就
    /// `return nil`，那句话当场蒸发。
    ///
    /// 这里用 `chmod 000` 把账做成读不动来复现。判据落在**「话有没有留下来」**，
    /// 不落在 errno：真故障是 EPERM，这里是 EACCES，两者走同一条拒写闸。
    private func makeLedgerUnreadable(_ dir: URL, crew: String) throws -> URL {
        let s = LocalTodoStore(directory: dir)
        _ = s.add(crewId: crew, text: "开张第一条")
        let f = try XCTUnwrap(
            FileManager.default.contentsOfDirectory(atPath: dir.path)
                .first { $0.contains(crew) && $0.hasSuffix(".json") })
        let url = dir.appendingPathComponent(f)
        try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: url.path)
        return url
    }

    func testAddOnUnreadableLedgerSpoolsTheRequest() throws {
        let dir = tempDir(), crew = "c-spool"
        let url = try makeLedgerUnreadable(dir, crew: crew)
        let s = LocalTodoStore(directory: dir)

        var reported: Error?
        let item = s.add(crewId: crew, text: "这条必须活下来",
                         onWriteFailure: { reported = $0 })
        XCTAssertNil(item, "账读不动时不该硬落号")
        XCTAssertTrue(
            (reported?.localizedDescription ?? "").contains("会自动补成"),
            "没告诉提的人这条已经存下来了，他就会再提一遍，于是一件事两条账：\(reported?.localizedDescription ?? "nil")")

        let spooled = (try? FileManager.default.contentsOfDirectory(
            atPath: dir.appendingPathComponent("outbox-todos").path)) ?? []
        XCTAssertEqual(spooled.count, 1, "话没被存下来 —— 这正是这一单要治的病")

        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
    }

    /// 恢复之后补成正式条目，**号在那一刻才算**，而且按当初的顺序。
    func testSpooledRequestsBecomeRealItemsInOrderOnceReadable() throws {
        let dir = tempDir(), crew = "c-spool2"
        let url = try makeLedgerUnreadable(dir, crew: crew)
        let s = LocalTodoStore(directory: dir)

        for t in ["断网期第一条", "断网期第二条"] {
            _ = s.add(crewId: crew, text: t)
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)

        let fresh = try XCTUnwrap(s.add(crewId: crew, text: "恢复之后这条"))
        let rows = s.list(crewId: crew)
        XCTAssertEqual(rows.map(\.text),
                       ["开张第一条", "断网期第一条", "断网期第二条", "恢复之后这条"],
                       "补回来的顺序不对 —— 倒过来会让账上的因果乱掉")
        XCTAssertEqual(rows.map(\.number), [1, 2, 3, 4], "号没有连续现算")
        XCTAssertEqual(fresh.number, 4, "恢复后那条的号要排在补发的后面")

        let left = (try? FileManager.default.contentsOfDirectory(
            atPath: dir.appendingPathComponent("outbox-todos").path)) ?? []
        XCTAssertTrue(left.isEmpty, "补完没清掉，下次会再补一遍：\(left)")
    }

    /// 补过一次不再补，而且按 crew 分家。
    func testDrainIsIdempotentAndPerCrew() throws {
        let dir = tempDir(), crew = "c-spool3", other = "c-other"
        let url = try makeLedgerUnreadable(dir, crew: crew)
        let s = LocalTodoStore(directory: dir)
        _ = s.add(crewId: crew, text: "存下来的那条")

        _ = s.add(crewId: other, text: "别人家的")
        XCTAssertEqual(s.list(crewId: other).map(\.text), ["别人家的"],
                       "另一个 crew 的写把这个 crew 的积压捞过去了")

        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
        _ = s.add(crewId: crew, text: "第一次恢复")
        _ = s.add(crewId: crew, text: "第二次")
        XCTAssertEqual(s.list(crewId: crew).filter { $0.text == "存下来的那条" }.count, 1,
                       "补了两遍")
    }


    /// **补发不能只挂在写路径上。** 第一版就是那样，而那意味着一个 crew 只要之后不再
    /// 提新的 Todo，存下来的那几条就永远补不回来、也永远看不见 —— 它们不在 `list` 里，
    /// 人类面板上一条都不显示。「存下来了」于是变成另一种形式的丢，而且更难发现：
    /// 回执还说过会自动补。
    ///
    /// 所以这一条只做一件事：**只读，不写**，看它回不回得来。
    func testSpooledRequestsComeBackOnAPlainReadWithNoFurtherWrites() throws {
        let dir = tempDir(), crew = "c-readback"
        let url = try makeLedgerUnreadable(dir, crew: crew)
        let s = LocalTodoStore(directory: dir)
        _ = s.add(crewId: crew, text: "断网期提的")
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)

        // 从这里往下**一次写都没有** —— 只是看一眼账。
        let rows = s.list(crewId: crew)
        XCTAssertEqual(rows.map(\.text), ["开张第一条", "断网期提的"],
                       "只读不写就补不回来 —— 这个 crew 要是不再提新的，它就永远看不见")
        XCTAssertEqual(rows.map(\.number), [1, 2])

        let left = (try? FileManager.default.contentsOfDirectory(
            atPath: dir.appendingPathComponent("outbox-todos").path)) ?? []
        XCTAssertTrue(left.isEmpty, "补完没清掉：\(left)")
    }

    /// 账还没恢复的时候去读，**绝不能落号** —— 拿一张空表算出来的 #N 会跟盘上
    /// 真实的号撞车，而撞车之后两条 Todo 共用一个号，回应打在哪条都说不准。
    func testReadWhileStillUnreadableDoesNotAssignNumbers() throws {
        let dir = tempDir(), crew = "c-nonum"
        let url = try makeLedgerUnreadable(dir, crew: crew)
        let s = LocalTodoStore(directory: dir)
        _ = s.add(crewId: crew, text: "断网期提的")

        XCTAssertEqual(s.read(crewId: crew), .unreadable, "前置：这会儿就该是读不出来")
        let stillSpooled = (try? FileManager.default.contentsOfDirectory(
            atPath: dir.appendingPathComponent("outbox-todos").path)) ?? []
        XCTAssertEqual(stillSpooled.count, 1, "账还没好就把它落号了 —— 号会跟盘上的撞车")

        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
        XCTAssertEqual(s.list(crewId: crew).map(\.number), [1, 2], "恢复后才落，且接着盘上的号")
    }

}
