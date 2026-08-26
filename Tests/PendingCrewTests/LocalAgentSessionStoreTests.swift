import XCTest

/// LocalAgentSessionStore（Todo #28）：agent 侧会话号账本 —— 「重启接回原对话」的持久化半边。
/// 基座三件套断言照 LocalWakeupStoreTests 的模式。
final class LocalAgentSessionStoreTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("agentsess-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func rawFileURL(_ dir: URL) -> URL { dir.appendingPathComponent("agent-sessions.json") }

    func testRecordAndLookupRoundtrip() {
        let s = LocalAgentSessionStore(directory: tempDir())
        XCTAssertNil(s.agentSessionId(crewId: "c", sessionId: "w1"))
        s.record(crewId: "c", sessionId: "w1", kind: "claude", agentSessionId: "uuid-1")
        XCTAssertEqual(s.agentSessionId(crewId: "c", sessionId: "w1"), "uuid-1")
        // 不串 crew / 不串 session
        XCTAssertNil(s.agentSessionId(crewId: "other", sessionId: "w1"))
        XCTAssertNil(s.agentSessionId(crewId: "c", sessionId: "w2"))
    }

    func testRecordOverwritesSameSession() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "w1", kind: "codex", agentSessionId: "thread-1")
        s.record(crewId: "c", sessionId: "w1", kind: "codex", agentSessionId: "thread-2")
        XCTAssertEqual(s.list().count, 1)
        XCTAssertEqual(s.agentSessionId(crewId: "c", sessionId: "w1"), "thread-2")
    }

    func testBlankIdIsIgnored() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "w1", kind: "claude", agentSessionId: "  ")
        XCTAssertTrue(s.list().isEmpty)
    }

    func testSurvivesProcessRestart() {
        let dir = tempDir()
        LocalAgentSessionStore(directory: dir).record(
            crewId: "c", sessionId: "w1", kind: "claude", agentSessionId: "uuid-1")
        XCTAssertEqual(
            LocalAgentSessionStore(directory: dir).agentSessionId(crewId: "c", sessionId: "w1"),
            "uuid-1")
    }

    func testCorruptFileIsArchivedAndReported() throws {
        let dir = tempDir()
        try "{ 半截".data(using: .utf8)!.write(to: rawFileURL(dir))
        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        let s = LocalAgentSessionStore(directory: dir)
        XCTAssertNil(s.agentSessionId(crewId: "c", sessionId: "w1",
                                      onIncident: { incidents.append($0) }))
        XCTAssertEqual(incidents.count, 1, "损坏文件必须 fail-loud 回调，不能静默当空")
        // 2026-08-12：事故分两种，这条是**真解不开**那种，才谈得上归档。
        XCTAssertFalse(incidents[0].isDataIntact)
        let names = try FileManager.default.contentsOfDirectory(atPath: dir.path)
            .filter { $0.hasPrefix("agent-sessions.json.corrupt-") }
        XCTAssertEqual(names.count, 1)
    }

    // MARK: - Todo #68：账本还要记「当初在哪儿跑」

    func testRecordsWorkingDirectory() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "w1", kind: "claude_code", agentSessionId: "uuid-1",
                 workingDirectory: "/wt/a")
        XCTAssertEqual(s.record(crewId: "c", sessionId: "w1")?.workingDirectory, "/wt/a")
    }

    /// **旧记录（没有 `workingDirectory` 字段）必须照常读出来**，不许炸、不许把整份
    /// 账本判成 corrupt —— 这条正是本机盘上那 383 条记录今天的样子。喂的是真实格式的
    /// JSON，不是构造出来的对象。
    func testLegacyRowsWithoutWorkingDirectoryStillLoad() throws {
        let dir = tempDir()
        let legacy = """
        [{"crewId":"local-abc","sessionId":"captain-61949935","kind":"claude_code",        "updatedAt":"2026-08-08T13:57:25Z","agentSessionId":"d2641172-e1ce-4ca1-92fb-6117f6580ab0"}]
        """
        try legacy.data(using: .utf8)!.write(to: rawFileURL(dir))
        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        let s = LocalAgentSessionStore(directory: dir)
        let rows = s.list(onIncident: { incidents.append($0) })
        XCTAssertTrue(incidents.isEmpty, "旧格式不是事故")
        XCTAssertEqual(rows.count, 1)
        XCTAssertNil(rows[0].workingDirectory)
        // 行为与今天一致：会话号照样查得到。
        XCTAssertEqual(s.agentSessionId(crewId: "local-abc", sessionId: "captain-61949935"),
                       "d2641172-e1ce-4ca1-92fb-6117f6580ab0")
    }

    /// 「这次不知道」不等于「没有」——传 nil 不许把已知的工作目录抹掉。
    func testUpdateWithoutWorkingDirectoryKeepsTheOldOne() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "w1", kind: "claude_code", agentSessionId: "uuid-1",
                 workingDirectory: "/wt/a")
        s.record(crewId: "c", sessionId: "w1", kind: "claude_code", agentSessionId: "uuid-2")
        XCTAssertEqual(s.record(crewId: "c", sessionId: "w1")?.workingDirectory, "/wt/a")
        XCTAssertEqual(s.record(crewId: "c", sessionId: "w1")?.agentSessionId, "uuid-2")
    }
}
