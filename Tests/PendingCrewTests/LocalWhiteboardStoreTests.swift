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
}
