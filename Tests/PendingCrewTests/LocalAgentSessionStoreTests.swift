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
}
