import XCTest
import Combine
// 不 `` —— PendingCrewTests 是 standalone bundle
// （TEST_HOST=""），不链 app module；待测源码直接编进 bundle（见 project.yml
// 把 Sources/Stores/LocalWhiteboardStore.swift 列进 test target sources）。

@MainActor
final class LocalWhiteboardStoreTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("wb-test-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func testListMissingCrewReturnsEmpty() {
        let store = LocalWhiteboardStore(directory: tempDir())
        XCTAssertTrue(store.list(crewId: "local-x").isEmpty)
    }

    func testAppendThenListReturnsEntry() {
        let store = LocalWhiteboardStore(directory: tempDir())
        store.appendUserMessage(crewId: "local-x", text: "hello")
        let entries = store.list(crewId: "local-x")
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(entries[0].text, "hello")
        XCTAssertEqual(entries[0].senderKind, "user")
        XCTAssertEqual(entries[0].senderUserId, LocalWhiteboardStore.localUserId)
    }

    func testOrderedByInsertion() {
        let store = LocalWhiteboardStore(directory: tempDir())
        store.appendUserMessage(crewId: "c", text: "1")
        store.appendUserMessage(crewId: "c", text: "2")
        store.appendUserMessage(crewId: "c", text: "3")
        XCTAssertEqual(store.list(crewId: "c").map(\.text), ["1", "2", "3"])
    }

    func testPersistsAcrossInstances() {
        let dir = tempDir()
        LocalWhiteboardStore(directory: dir).appendUserMessage(crewId: "c", text: "persisted")
        let reloaded = LocalWhiteboardStore(directory: dir)
        XCTAssertEqual(reloaded.list(crewId: "c").map(\.text), ["persisted"])
    }

    func testCrewsAreIsolated() {
        let store = LocalWhiteboardStore(directory: tempDir())
        store.appendUserMessage(crewId: "a", text: "in-a")
        store.appendUserMessage(crewId: "b", text: "in-b")
        XCTAssertEqual(store.list(crewId: "a").map(\.text), ["in-a"])
        XCTAssertEqual(store.list(crewId: "b").map(\.text), ["in-b"])
    }

    func testAppendSessionMessage() {
        let s = LocalWhiteboardStore(directory: tempDir())
        s.appendSessionMessage(crewId: "c", sessionId: "sess-1", text: "started", category: "progress")
        let m = s.list(crewId: "c")
        XCTAssertEqual(m.count, 1)
        XCTAssertEqual(m[0].senderKind, "session")
        XCTAssertEqual(m[0].senderSessionId, "sess-1")
        XCTAssertEqual(m[0].category, "progress")
        XCTAssertEqual(m[0].text, "started")
    }

    /// Todo #43：历史上的系统写入口全都复用 `appendSessionMessage`，调用方可以传
    /// `senderName: "系统"`，所以只修某几个调用点必然继续漏。旧入口本身必须在
    /// 单一语义层把 system 哨兵正规化成 PendingCrew 身份。
    func testLegacySystemSessionAppendNormalizesToPendingCrewIdentity() {
        let s = LocalWhiteboardStore(directory: tempDir())
        s.appendSessionMessage(
            crewId: "c", sessionId: "system", text: "后台生成的通知",
            category: "progress", senderName: "系统")

        let row = s.list(crewId: "c").first
        XCTAssertEqual(row?.senderKind, "pendingcrew")
        XCTAssertEqual(row?.senderSessionId, "system")
        XCTAssertEqual(row?.senderName, "PendingCrew")
    }

    func testSessionSelfEndMessageUsesTheExactTemplateAndFallback() {
        XCTAssertEqual(
            PendingCrewSystemMessage.sessionEnded(
                sessionName: "整理设置", lastAgentText: "已完成。"),
            "Session「整理设置」自己结束了。它最后一句话：已完成。")
        XCTAssertEqual(
            PendingCrewSystemMessage.sessionEnded(
                sessionName: "整理设置", lastAgentText: "  \n "),
            "Session「整理设置」自己结束了。它最后一句话：（没有留下最后一句话）")
    }

    func testEntriesAfterCursor() {
        let s = LocalWhiteboardStore(directory: tempDir())
        s.appendUserMessage(crewId: "c", text: "1")
        s.appendUserMessage(crewId: "c", text: "2")
        let all = s.list(crewId: "c")
        let pos = WhiteboardCursorPosition(id: all[0].id, createdAt: all[0].createdAt)
        XCTAssertEqual(s.entries(crewId: "c", after: pos).map(\.text), ["2"])
    }

    func testEntriesAfterNilReturnsAll() {
        let s = LocalWhiteboardStore(directory: tempDir())
        s.appendUserMessage(crewId: "c", text: "x")
        XCTAssertEqual(s.entries(crewId: "c", after: nil).map(\.text), ["x"])
    }

    func testEntriesAfterUnknownAnchorDoesNotReplayEverything() {
        // #595：曾经是「找不到 → 返回全部」，2026-08-12 全机重放的病根。
        // 完整的 fail-closed 语义见 `WhiteboardCursorFailClosedTests`。
        let s = LocalWhiteboardStore(directory: tempDir())
        s.appendUserMessage(crewId: "c", text: "x")
        let gone = WhiteboardCursorPosition(id: "no-such-id", createdAt: nil)
        XCTAssertTrue(s.entries(crewId: "c", after: gone).isEmpty)
    }

    // MARK: - changes publisher（Phase 5：去轮询 —— append 即发 tick）

    func testAppendEmitsChangeWithCrewId() {
        let s = LocalWhiteboardStore(directory: tempDir())
        var received: [String] = []
        let c = s.changes.sink { received.append($0) }
        defer { c.cancel() }
        s.appendUserMessage(crewId: "crew-a", text: "hi")
        XCTAssertEqual(received, ["crew-a"])
    }

    func testEachAppendVariantEmitsOnce() {
        let s = LocalWhiteboardStore(directory: tempDir())
        var count = 0
        let c = s.changes.sink { _ in count += 1 }
        defer { c.cancel() }
        s.appendUserMessage(crewId: "c", text: "u")
        s.appendSessionMessage(crewId: "c", sessionId: "sess", text: "s")
        XCTAssertEqual(count, 2)
    }

    func testReportingFailureAppendThrowsWhenWhiteboardCannotBeWritten() throws {
        let path = tempDir().appendingPathComponent("not-a-directory")
        try Data("occupied by a file".utf8).write(to: path)
        let store = LocalWhiteboardStore(directory: path)

        XCTAssertThrowsError(try store.appendSessionMessageReportingFailure(
            crewId: "child", sessionId: "captain-parent", text: "开场任务"))
    }

    // MARK: - #483 解码失败 fail-loud + 逐条 lenient 解码 + 并发写防护

    private func rawFileURL(_ dir: URL, _ crewId: String) -> URL {
        dir.appendingPathComponent("\(crewId).json")
    }

    private func corruptArchives(_ dir: URL, _ crewId: String) throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: dir.path)
            .filter { $0.hasPrefix("\(crewId).json.corrupt-") }
    }

    func testCorruptFileArchivedAndWarnsInsteadOfSilentEmpty() throws {
        let dir = tempDir()
        let garbage = Data("not json at all {{{".utf8)
        try garbage.write(to: rawFileURL(dir, "c"))
        let store = LocalWhiteboardStore(directory: dir)
        let rows = store.list(crewId: "c")
        // fail-loud：白板上留一条系统警示（复用 postSystemNotice 形态），不再静默当空
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].senderKind, "pendingcrew")
        XCTAssertEqual(rows[0].senderSessionId, "system")
        XCTAssertEqual(rows[0].senderName, "PendingCrew")
        XCTAssertTrue(rows[0].text.contains("损坏"))
        // 原始损坏字节归档为 .corrupt-<ts>，可人工找回
        let archived = try corruptArchives(dir, "c")
        XCTAssertEqual(archived.count, 1)
        XCTAssertEqual(try Data(contentsOf: dir.appendingPathComponent(archived[0])), garbage)
    }

    func testHalfWrittenFileRecoveredAndAppendKeepsWarning() throws {
        // 半截写入（进程被杀在 write 中途）→ 归档 + 警示；后续 append 不清掉警示
        let dir = tempDir()
        try Data(#"[{"id":"a","senderKind":"user","te"#.utf8).write(to: rawFileURL(dir, "c"))
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "after")
        let rows = store.list(crewId: "c")
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0].senderSessionId, "system")
        XCTAssertEqual(rows[1].text, "after")
        XCTAssertEqual(try corruptArchives(dir, "c").count, 1)
    }

    func testAppendToUnreadableBoardPreservesOriginalBytes() throws {
        // 复现 2026-08-11：磁盘文件存在且非空，但 Data(contentsOf:) 读失败。
        //
        // ⚠️ 2026-08-12 失效批注：本测试原先断言的处置是「归档为 .corrupt-* +
        // 从系统警示重建 + 新消息照落」。**那个处置动作已被推翻** —— 当晚 fd 打满
        // （launchd 软上限 256）让 open() 抛 EPERM/EMFILE，这条路径把全机 19–24 份
        // 完好白板搬走重建，2000+ 条历史从 live 文件消失。8-11 的判断在当时是对的
        // （fail-loud 优于静默清空），是被后来的事实推翻的。
        //
        // 现在的契约：读不出来 → **原件一个字节不动、零归档**，写路径拒写。
        let dir = tempDir()
        let url = rawFileURL(dir, "c")
        let original = Data("existing whiteboard bytes".utf8)
        try original.write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer { _ = chmod(url.path, S_IRUSR | S_IWUSR) }

        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "不该拿历史陪葬")

        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty, "读不出来不许归档")
        XCTAssertEqual(chmod(url.path, S_IRUSR | S_IWUSR), 0)
        XCTAssertEqual(try Data(contentsOf: url), original, "原件一个字节都不许动")
    }

    func testReportingAppendToUnreadableBoardRefusesAndKeepsHistory() throws {
        // #577 立的规矩仍然成立：读不出来时不许回一句「已发到」。
        //
        // ⚠️ 2026-08-12 失效批注：这条测试原先断言的是「归档 + 从警示行重建 +
        // 本条照落 + 回执带 .corrupt- 路径」。**那半条结论已被推翻** —— 当晚
        // fd 打满（launchd 软上限 256）让 open() 抛 EPERM/EMFILE，这条路径把
        // 全机 19–24 份完好白板搬走重建，约 2000+ 条历史从 live 文件消失。
        // 现在的正确契约：读不出来 = **拒写 + 原件一字不动 + 如实报错**。
        // fail-loud 是对的，销毁性处置是错的。
        let dir = tempDir()
        let url = rawFileURL(dir, "c")
        let original = Data("do not lose me".utf8)
        try original.write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer { _ = chmod(url.path, S_IRUSR | S_IWUSR) }

        let store = LocalWhiteboardStore(directory: dir)
        XCTAssertThrowsError(try store.appendSessionMessageReportingFailure(
            crewId: "c", sessionId: "s", text: "进展"),
            "读不出来必须如实说没发出去")

        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty, "读不出来不许产生任何归档")
        XCTAssertEqual(chmod(url.path, S_IRUSR | S_IWUSR), 0)
        XCTAssertEqual(try Data(contentsOf: url), original, "原件一个字节都不许动")
    }

    func testReportingAppendThrowsWhenUnreadableFileCannotBeArchived() throws {
        // 归档也做不到（目录不可写）→ 原文件必须原地不动，且一个字都不许写；
        // 回执要如实说没发出去，绝不能吞成「已发送」。
        let dir = tempDir()
        let url = rawFileURL(dir, "c")
        let original = Data("still here".utf8)
        try original.write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        XCTAssertEqual(chmod(dir.path, S_IRUSR | S_IXUSR), 0)
        defer {
            _ = chmod(dir.path, S_IRUSR | S_IWUSR | S_IXUSR)
            _ = chmod(url.path, S_IRUSR | S_IWUSR)
        }

        let store = LocalWhiteboardStore(directory: dir)
        XCTAssertThrowsError(try store.appendSessionMessageReportingFailure(
            crewId: "c", sessionId: "s", text: "must fail"))

        XCTAssertEqual(chmod(dir.path, S_IRUSR | S_IWUSR | S_IXUSR), 0)
        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty)
        XCTAssertEqual(chmod(url.path, S_IRUSR | S_IWUSR), 0)
        XCTAssertEqual(try Data(contentsOf: url), original)
    }

    func testUnreadableFileReportsWarningWithoutQuarantine() throws {
        let dir = tempDir()
        let url = rawFileURL(dir, "c")
        let original = Data("still intact".utf8)
        try original.write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer { _ = chmod(url.path, S_IRUSR | S_IWUSR) }

        let rows = LocalWhiteboardStore(directory: dir).list(crewId: "c")

        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].senderSessionId, "system")
        XCTAssertTrue(rows[0].text.contains("无法读取"))
        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty)
    }

    func testCleanAppendReportsNoIncident() throws {
        // 对照组：白板好端端的时候不许报事故 —— 否则「有事故」这个信号会被稀释。
        let store = LocalWhiteboardStore(directory: tempDir())
        XCTAssertNil(try store.appendSessionMessageReportingFailure(
            crewId: "c", sessionId: "s", text: "一切正常"))
    }

    func testLenientDecodeDropsOnlyBadElements() throws {
        // 外层数组合法、中间一条缺必填 text（旧二进制读新 schema 之类）→ 只丢那条，
        // 好的不连坐，也不算整文件损坏（不归档不警示）
        let dir = tempDir()
        let json = """
        [{"id":"a","senderKind":"user","text":"one","createdAt":"2026-07-17T00:00:00Z"},
         {"id":"b","senderKind":"user","createdAt":"2026-07-17T00:00:01Z"},
         {"id":"c","senderKind":"user","text":"three","createdAt":"2026-07-17T00:00:02Z"}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir, "c"))
        let store = LocalWhiteboardStore(directory: dir)
        XCTAssertEqual(store.list(crewId: "c").map(\.text), ["one", "three"])
        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty)
    }

    func testAllRowsFailDecodeIsCorruptAndArchived() throws {
        let dir = tempDir()
        let original = Data("""
        [{"id":"a","senderKind":"user","createdAt":"2026-07-17T00:00:00Z"},
         {"id":"b","senderKind":"session","createdAt":"2026-07-17T00:00:01Z"}]
        """.utf8)
        try original.write(to: rawFileURL(dir, "c"))

        let rows = LocalWhiteboardStore(directory: dir).list(crewId: "c")

        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].senderSessionId, "system")
        XCTAssertTrue(rows[0].text.contains("损坏"))
        let archived = try corruptArchives(dir, "c")
        XCTAssertEqual(archived.count, 1)
        XCTAssertEqual(try Data(contentsOf: dir.appendingPathComponent(archived[0])), original)
    }

    func testQuarantineFailureKeepsOriginalBytesAndReportsWarning() throws {
        let dir = tempDir()
        let url = rawFileURL(dir, "c")
        let original = Data("not json".utf8)
        try original.write(to: url)
        XCTAssertEqual(chmod(dir.path, S_IRUSR | S_IXUSR), 0)
        defer { _ = chmod(dir.path, S_IRUSR | S_IWUSR | S_IXUSR) }

        let rows = LocalWhiteboardStore(directory: dir).list(crewId: "c")

        XCTAssertEqual(rows.count, 1)
        XCTAssertTrue(rows[0].text.contains("归档失败"))
        XCTAssertEqual(chmod(dir.path, S_IRUSR | S_IWUSR | S_IXUSR), 0)
        XCTAssertEqual(try Data(contentsOf: url), original)
        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty)
    }

    func testUnknownExtraFieldsTolerated() throws {
        // 新 schema 加字段、旧二进制混跑：未知键忽略，消息保留
        let dir = tempDir()
        let json = """
        [{"id":"a","senderKind":"user","text":"keep","createdAt":"2026-07-17T00:00:00Z","futureField":{"x":1}}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir, "c"))
        XCTAssertEqual(LocalWhiteboardStore(directory: dir).list(crewId: "c").map(\.text), ["keep"])
    }

    func testAppendAfterBadElementPreservesGoodOnes() throws {
        // 曾经的致命路径：一条坏 → load 视整板为空 → append 用「空+新」重写清史。
        // 现在：append 后好消息仍在，只有坏那条被丢
        let dir = tempDir()
        let json = """
        [{"id":"a","senderKind":"user","text":"one","createdAt":"2026-07-17T00:00:00Z"},
         {"id":"b","senderKind":"user","createdAt":"2026-07-17T00:00:01Z"}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir, "c"))
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "new")
        XCTAssertEqual(store.list(crewId: "c").map(\.text), ["one", "new"])
    }

    func testConcurrentAppendsAcrossInstancesLoseNothing() {
        // 模拟 app 与 helper 两个进程（两个实例）并发 append —— 文件锁后一条不丢
        let dir = tempDir()
        let a = LocalWhiteboardStore(directory: dir)
        let b = LocalWhiteboardStore(directory: dir)
        DispatchQueue.concurrentPerform(iterations: 40) { i in
            (i % 2 == 0 ? a : b).appendUserMessage(crewId: "c", text: "m\(i)")
        }
        XCTAssertEqual(a.list(crewId: "c").count, 40)
    }

    func testAttachmentsRoundtripAndAgentText() {
        // Todo #3：附件字段持久化 + agent 渲染带路径提示行。
        let store = LocalWhiteboardStore(directory: tempDir())
        let att = LocalWhiteboardAttachment(
            id: "att1", mime: "image/png", size: 3, path: "/tmp/a.png")
        store.appendUserMessage(crewId: "c", text: "看这个", attachments: [att])
        let m = store.list(crewId: "c")
        XCTAssertEqual(m.count, 1)
        XCTAssertEqual(m[0].attachments, [att])
        XCTAssertEqual(m[0].agentText, "看这个\n用户发来图片：/tmp/a.png（请 Read 查看）")
    }

    func testAttachmentOnlyMessageAgentTextIsHintOnly() {
        let att = LocalWhiteboardAttachment(
            id: "att1", mime: "application/pdf", size: nil, path: "/tmp/f.pdf", filename: "f.pdf")
        let m = LocalWhiteboardMessage(
            id: "m", senderKind: "user", senderUserId: nil, senderSessionId: nil,
            category: nil, text: "", createdAt: "2026-07-19T00:00:00Z", attachments: [att])
        XCTAssertEqual(m.agentText, "用户发来文件：/tmp/f.pdf（请 Read 查看）")
    }

    func testOldJsonWithoutAttachmentsStillDecodes() throws {
        // #483 lenient 先例：新增可选字段不能让老数据解不出。
        let dir = tempDir()
        let json = """
        [{"id":"a","senderKind":"user","text":"old","createdAt":"2026-07-17T00:00:00Z"}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir, "c"))
        let m = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertEqual(m.map(\.text), ["old"])
        XCTAssertNil(m[0].attachments)
        XCTAssertEqual(m[0].agentText, "old")
    }

    // MARK: - 白板读不动时，话先存着别丢（2026-09-12）

    /// 病历：那天这个故障连着四窗，每一窗里 agent 组织好的消息都当场蒸发，
    /// 回执还明写着「没有留在任何地方 —— 要它们的话得重发」。
    ///
    /// 而这类故障的形状是 **`open()` 一个已存在的 inode 被拒、建新文件照样成**
    /// （逐系统调用量过）。所以「存不下来」从来不是事实，只是没人去存。
    ///
    /// 这里用 `chmod 000` 把白板文件做成读不动来复现 —— 判据落在
    /// **「话有没有留下来」**，不落在「哪种 errno」：真故障是 EPERM，这里是 EACCES，
    /// 而两者走的是同一条 `catch`。换句话说这条测的是那条 catch 的行为，不是那个故障。
    private func makeUnreadable(_ dir: URL, crew: String) throws -> URL {
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: crew, text: "开张第一条")
        let f = try XCTUnwrap(
            FileManager.default.contentsOfDirectory(atPath: dir.path)
                .first { $0.contains(crew) && $0.hasSuffix(".json") })
        let url = dir.appendingPathComponent(f)
        try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: url.path)
        return url
    }

    func testUnreadableBoardSpoolsTheMessageInsteadOfDroppingIt() throws {
        let dir = tempDir(), crew = "local-spool"
        let url = try makeUnreadable(dir, crew: crew)
        let store = LocalWhiteboardStore(directory: dir)

        XCTAssertThrowsError(
            try store.appendSessionMessageReportingFailure(
                crewId: crew, sessionId: "s1", text: "这条必须活下来")
        ) { err in
            XCTAssertTrue(
                err.localizedDescription.contains("待发件箱"),
                "回执没告诉作者这条被存下来了，他就会去重发，于是同一条出现两次：\(err.localizedDescription)")
        }

        let spooled = (try? FileManager.default.contentsOfDirectory(
            atPath: dir.appendingPathComponent("outbox").path)) ?? []
        XCTAssertEqual(spooled.count, 1, "话没被存下来 —— 这正是这一单要治的病")

        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
    }

    /// 恢复之后自己补回去，而且**排在恢复后那条的前面** —— 它本来就发生得更早。
    func testSpooledMessagesComeBackInOrderOnceTheBoardIsReadableAgain() throws {
        let dir = tempDir(), crew = "local-spool2"
        let url = try makeUnreadable(dir, crew: crew)
        let store = LocalWhiteboardStore(directory: dir)

        for t in ["断网期第一条", "断网期第二条"] {
            _ = try? store.appendSessionMessageReportingFailure(
                crewId: crew, sessionId: "s1", text: t)
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)

        let note = try store.appendSessionMessageReportingFailure(
            crewId: crew, sessionId: "s1", text: "恢复之后这条")
        XCTAssertTrue(
            (note ?? "").contains("补发了之前存下的 2 条"),
            "补发了却不在回执里说 —— 白板上凭空多出两条旧消息，两边账对不上：\(note ?? "nil")")

        let texts = store.list(crewId: crew).map(\.text)
        XCTAssertEqual(texts, ["开张第一条", "断网期第一条", "断网期第二条", "恢复之后这条"],
                       "补发的顺序不对 —— 倒过来会让白板上的因果乱掉")

        let left = (try? FileManager.default.contentsOfDirectory(
            atPath: dir.appendingPathComponent("outbox").path)) ?? []
        XCTAssertTrue(left.isEmpty, "补发完没清掉，下次会再补一遍：\(left)")
    }

    /// 补过一次就不该再补 —— 顺手也证明 outbox 是按 crew 分的，别的 crew 恢复
    /// 不会把这个 crew 的话拖进去。
    func testDrainDoesNotDuplicateAndIsPerCrew() throws {
        let dir = tempDir(), crew = "local-spool3", other = "local-other"
        let url = try makeUnreadable(dir, crew: crew)
        let store = LocalWhiteboardStore(directory: dir)
        _ = try? store.appendSessionMessageReportingFailure(
            crewId: crew, sessionId: "s1", text: "存下来的那条")

        // 另一个 crew 照常写：它不该把上面那条捞走。
        store.appendUserMessage(crewId: other, text: "别人家的")
        XCTAssertEqual(store.list(crewId: other).map(\.text), ["别人家的"],
                       "另一个 crew 的恢复把这个 crew 的积压捞过去了")

        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: url.path)
        _ = try store.appendSessionMessageReportingFailure(
            crewId: crew, sessionId: "s1", text: "第一次恢复")
        let after = try store.appendSessionMessageReportingFailure(
            crewId: crew, sessionId: "s1", text: "第二次")
        XCTAssertFalse((after ?? "").contains("补发"), "补了两遍：\(after ?? "nil")")
        XCTAssertEqual(store.list(crewId: crew).filter { $0.text == "存下来的那条" }.count, 1)
    }


    /// **到顶之后拒绝再存，而且如实说。**
    ///
    /// 这不是容量估算，是跑飞保护：故障窗口可能持续几小时，期间自动重试的东西
    /// （定时提醒、轮询、循环里的 agent）会一直往里存。没有上限的话，一个卡住的循环
    /// 能在**系统已经出着毛病**的时候把盘刷满。
    ///
    /// 同时钉住**不丢旧的换新的**：旧的那些是先发生的，丢它们等于按时间倒序丢数据，
    /// 而且没有任何人看得见。
    func testSpoolRefusesPastItsCapAndKeepsTheOldOnes() throws {
        let dir = tempDir()
        let spool = LedgerSpool<LocalWhiteboardMessage>(
            directory: dir.appendingPathComponent("outbox", isDirectory: true))
        func msg(_ t: String) -> LocalWhiteboardMessage {
            LocalWhiteboardMessage(
                id: UUID().uuidString, senderKind: "session", senderUserId: nil,
                senderSessionId: "s", category: nil, text: t,
                createdAt: ISO8601DateFormatter().string(from: Date()), senderName: nil)
        }
        let cap = LedgerSpoolLimits.capacityPerKey
        for i in 0..<cap {
            XCTAssertTrue(spool.spool(msg("第\(i)条"), key: "c"), "第 \(i) 条就存不下了")
        }
        XCTAssertFalse(spool.spool(msg("超出上限的"), key: "c"), "到顶了还在存 —— 跑飞时会把盘刷满")

        // 旧的一条都不许少，而且第一条还得是最早那条。
        var seen: [String] = []
        _ = spool.drain(key: "c") { seen.append($0.text); return true }
        XCTAssertEqual(seen.count, cap, "到顶时丢了旧的换新的")
        XCTAssertEqual(seen.first, "第0条", "顺序都乱了")
        XCTAssertFalse(seen.contains("超出上限的"), "被拒的那条居然进去了")
    }

}
