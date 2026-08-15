import XCTest

final class LocalApprovalStoreTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("appr-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func testRaiseCreatesPending() throws {
        let s = LocalApprovalStore(directory: tempDir())
        let id = try XCTUnwrap(s.raise(crewId: "c", kind: "decision", sessionId: "sess", summary: "选 A 还是 B?"))
        let it = s.item(crewId: "c", id: id)
        XCTAssertEqual(it?.status, "pending")
        XCTAssertEqual(it?.kind, "decision")
        XCTAssertEqual(it?.summary, "选 A 还是 B?")
        XCTAssertEqual(s.pending(crewId: "c").count, 1)
    }

    func testAnswerMarksAnsweredWithReply() throws {
        let s = LocalApprovalStore(directory: tempDir())
        let id = try XCTUnwrap(s.raise(crewId: "c", kind: "decision", sessionId: "sess", summary: "q"))
        s.answer(crewId: "c", id: id, reply: "选 A")
        let it = s.item(crewId: "c", id: id)
        XCTAssertEqual(it?.status, "answered")
        XCTAssertEqual(it?.reply, "选 A")
        XCTAssertTrue(s.pending(crewId: "c").isEmpty)
    }

    func testDecideSetsAllowDeny() throws {
        let s = LocalApprovalStore(directory: tempDir())
        let id = try XCTUnwrap(s.raise(crewId: "c", kind: "permission", sessionId: "sess", summary: "允许 computer-use?"))
        s.decide(crewId: "c", id: id, decision: "deny")
        XCTAssertEqual(s.item(crewId: "c", id: id)?.decision, "deny")
        XCTAssertEqual(s.item(crewId: "c", id: id)?.status, "answered")
    }

    func testPersistsAcrossInstances() throws {
        let dir = tempDir()
        let id = try XCTUnwrap(LocalApprovalStore(directory: dir).raise(crewId: "c", kind: "decision", sessionId: "s", summary: "q"))
        XCTAssertEqual(LocalApprovalStore(directory: dir).item(crewId: "c", id: id)?.status, "pending")
    }

    func testCrewsIsolated() {
        let s = LocalApprovalStore(directory: tempDir())
        _ = s.raise(crewId: "a", kind: "decision", sessionId: "s", summary: "qa")
        _ = s.raise(crewId: "b", kind: "permission", sessionId: "s", summary: "qb")
        XCTAssertEqual(s.list(crewId: "a").count, 1)
        XCTAssertEqual(s.list(crewId: "b").count, 1)
        XCTAssertEqual(s.list(crewId: "a")[0].kind, "decision")
    }

    // MARK: - #528 基座三件套（照 LocalWhiteboardStoreTests 的 #483 模式）

    private func rawFileURL(_ dir: URL, _ crewId: String) -> URL {
        dir.appendingPathComponent("\(crewId).approvals.json")
    }

    private func corruptArchives(_ dir: URL, _ crewId: String) throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: dir.path)
            .filter { $0.hasPrefix("\(crewId).approvals.json.corrupt-") }
    }

    func testCorruptFileArchivedAndNextRaiseDoesNotClobber() throws {
        // 曾经的致命路径：损坏 → load 当空 → 下一次 raise 以空数组重写、在途审批蒸发。
        // 现在：损坏字节归档可找回 + 白板落系统警示，新 raise 从干净状态开始。
        let dir = tempDir()
        let garbage = Data("not json {{{".utf8)
        try garbage.write(to: rawFileURL(dir, "c"))
        let s = LocalApprovalStore(directory: dir)
        let id = try XCTUnwrap(s.raise(crewId: "c", kind: "decision", sessionId: "s", summary: "q"))
        XCTAssertEqual(s.item(crewId: "c", id: id)?.status, "pending")
        XCTAssertEqual(s.list(crewId: "c").count, 1)
        let archived = try corruptArchives(dir, "c")
        XCTAssertEqual(archived.count, 1)
        XCTAssertEqual(try Data(contentsOf: dir.appendingPathComponent(archived[0])), garbage)
        let notices = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertEqual(notices.count, 1)
        XCTAssertEqual(notices[0].senderSessionId, "system")
        XCTAssertTrue(notices[0].text.contains("已归档"), notices[0].text)
    }

    func testLenientDecodeDropsOnlyBadElements() throws {
        // 中间一条缺必填字段 → 只丢那条，好的不连坐，不算整文件损坏（不归档）。
        let dir = tempDir()
        let json = """
        [{"id":"a","kind":"decision","sessionId":"s","summary":"one","status":"pending","createdAt":"2026-07-25T00:00:00Z"},
         {"id":"b","kind":"decision","sessionId":"s","status":"pending","createdAt":"2026-07-25T00:00:01Z"},
         {"id":"c","kind":"permission","sessionId":"s","summary":"three","status":"answered","createdAt":"2026-07-25T00:00:02Z"}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir, "c"))
        let s = LocalApprovalStore(directory: dir)
        XCTAssertEqual(s.list(crewId: "c").map(\.summary), ["one", "three"])
        XCTAssertTrue(try corruptArchives(dir, "c").isEmpty)
        // 后续写不清史：answer 落盘后幸存条目仍在。
        s.answer(crewId: "c", id: "a", reply: "ok")
        XCTAssertEqual(s.list(crewId: "c").map(\.summary), ["one", "three"])
        XCTAssertEqual(s.item(crewId: "c", id: "a")?.reply, "ok")
    }

    func testConcurrentRaisesAcrossInstancesLoseNothing() {
        // 模拟 app 与 helper 两个进程（两个实例）并发 raise —— flock 后一条不丢。
        let dir = tempDir()
        let a = LocalApprovalStore(directory: dir)
        let b = LocalApprovalStore(directory: dir)
        DispatchQueue.concurrentPerform(iterations: 40) { i in
            _ = (i % 2 == 0 ? a : b).raise(crewId: "c", kind: "decision", sessionId: "s", summary: "q\(i)")
        }
        XCTAssertEqual(a.list(crewId: "c").count, 40)
    }

    // MARK: - #577 读不出来时不许发出幽灵 reqId

    func testRaiseReturnsNilWhenLedgerIsUnreadable() throws {
        // 旧行为：照样返回 id → 调用方对着磁盘上不存在的待决策 long-poll 干等 30 分钟。
        let dir = tempDir()
        let url = dir.appendingPathComponent("c.approvals.json")
        try Data(#"[{"id":"a","kind":"decision","sessionId":"s","summary":"q","status":"pending","createdAt":"2026-08-01T00:00:00Z"}]"#.utf8)
            .write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer { _ = chmod(url.path, S_IRUSR | S_IWUSR) }

        XCTAssertNil(LocalApprovalStore(directory: dir).raise(
            crewId: "c", kind: "decision", sessionId: "s", summary: "要不要发版?"))

        let board = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertEqual(board.count, 1)
        XCTAssertEqual(board[0].senderSessionId, "system")
    }
}
