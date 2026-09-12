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

    // MARK: - Todo #68 第 2 件：机长最近一条记录

    private func stamp(_ iso: String) -> Date { CrewTimestamp.parse(iso)! }

    func testLatestCaptainRecordPicksTheNewestByParsedDate() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "captain-aaa", kind: "claude_code",
                 agentSessionId: "old", now: stamp("2026-08-20T10:00:00Z"))
        s.record(crewId: "c", sessionId: "captain-bbb", kind: "claude_code",
                 agentSessionId: "new", now: stamp("2026-08-24T09:00:00Z"))
        s.record(crewId: "c", sessionId: "captain-ccc", kind: "claude_code",
                 agentSessionId: "mid", now: stamp("2026-08-22T23:00:00Z"))
        XCTAssertEqual(
            s.latestCaptainRecord(crewId: "c", kind: "claude_code")?.agentSessionId, "new")
    }

    /// worker 的记录不算机长的 —— 判据是 `captain-` 前缀（全仓只有 `startCaptain` 铸这种 id）。
    func testLatestCaptainRecordIgnoresWorkers() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "captain-aaa", kind: "claude_code",
                 agentSessionId: "cap", now: stamp("2026-08-20T10:00:00Z"))
        s.record(crewId: "c", sessionId: "worker-zzz", kind: "claude_code",
                 agentSessionId: "wrk", now: stamp("2026-08-25T10:00:00Z"))
        XCTAssertEqual(
            s.latestCaptainRecord(crewId: "c", kind: "claude_code")?.agentSessionId, "cap")
    }

    /// **换过 runner 的 crew 不许串**：拿 codex 的 threadId 去喂 claude 的 `--resume`
    /// 是纯粹的错，所以 kind 严格过滤。
    func testLatestCaptainRecordFiltersByKind() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "captain-aaa", kind: "claude_code",
                 agentSessionId: "claude-uuid", now: stamp("2026-08-20T10:00:00Z"))
        s.record(crewId: "c", sessionId: "captain-bbb", kind: "codex",
                 agentSessionId: "codex-thread", now: stamp("2026-08-25T10:00:00Z"))
        XCTAssertEqual(
            s.latestCaptainRecord(crewId: "c", kind: "claude_code")?.agentSessionId, "claude-uuid")
        XCTAssertEqual(
            s.latestCaptainRecord(crewId: "c", kind: "codex")?.agentSessionId, "codex-thread")
    }

    func testLatestCaptainRecordDoesNotCrossCrews() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "other", sessionId: "captain-aaa", kind: "claude_code",
                 agentSessionId: "theirs", now: stamp("2026-08-25T10:00:00Z"))
        XCTAssertNil(s.latestCaptainRecord(crewId: "c", kind: "claude_code"))
    }

    // MARK: - 档位落盘（Todo #146）

    /// 切模型只改内存、不落盘，是「切了、回执说成功、一重启又变回去」的全部机制。
    func test_切换的档位会落盘并且能查回来() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "w1", kind: "codex", agentSessionId: "thread-1")
        XCTAssertNil(s.record(crewId: "c", sessionId: "w1")?.model, "起始不该有档位")

        s.recordProfile(crewId: "c", sessionId: "w1", model: "gpt-5.6-sol")
        XCTAssertEqual(s.record(crewId: "c", sessionId: "w1")?.model, "gpt-5.6-sol")
        XCTAssertNil(s.record(crewId: "c", sessionId: "w1")?.effort,
                     "只切了模型，effort 不该被写上")

        s.recordProfile(crewId: "c", sessionId: "w1", effort: "high")
        XCTAssertEqual(s.record(crewId: "c", sessionId: "w1")?.model, "gpt-5.6-sol",
                       "再切 effort 不该把模型冲掉")
        XCTAssertEqual(s.record(crewId: "c", sessionId: "w1")?.effort, "high")
    }

    /// 会话号那一路（重启接回原对话）不能被档位写坏 —— 它们同住一行。
    func test_写档位不动会话号和工作目录() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "w1", kind: "codex", agentSessionId: "thread-1",
                 workingDirectory: "/tmp/wd")
        s.recordProfile(crewId: "c", sessionId: "w1", model: "gpt-5.5")
        let r = s.record(crewId: "c", sessionId: "w1")
        XCTAssertEqual(r?.agentSessionId, "thread-1")
        XCTAssertEqual(r?.workingDirectory, "/tmp/wd")
        XCTAssertEqual(r?.kind, "codex")
    }

    /// 反过来：后续的 `record`（握手又写一次会话号）不能把人切过的档位清掉 ——
    /// 它传的 model 是 nil，而 nil 的意思是「这次不知道」，不是「没有」。
    func test_后续写会话号不清掉已记的档位() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.record(crewId: "c", sessionId: "w1", kind: "codex", agentSessionId: "thread-1")
        s.recordProfile(crewId: "c", sessionId: "w1", model: "gpt-5.6-sol", effort: "high")
        s.record(crewId: "c", sessionId: "w1", kind: "codex", agentSessionId: "thread-2")
        let r = s.record(crewId: "c", sessionId: "w1")
        XCTAssertEqual(r?.agentSessionId, "thread-2")
        XCTAssertEqual(r?.model, "gpt-5.6-sol", "换了 thread 不等于换了模型")
        XCTAssertEqual(r?.effort, "high")
    }

    /// 没有那一行时不凭空造一条 —— 一条会话号为空的记录会把「续跑哪一轮」带偏。
    func test_查无此行时不造记录() {
        let s = LocalAgentSessionStore(directory: tempDir())
        s.recordProfile(crewId: "c", sessionId: "w1", model: "gpt-5.5")
        XCTAssertEqual(s.list().count, 0)
    }

    /// 旧账本（没有这两个字段）照样解得开，解成 nil。
    func test_旧记录解得开且档位为nil() throws {
        let dir = tempDir()
        try Data("""
            [{"crewId":"c","sessionId":"w1","kind":"claude",\
            "agentSessionId":"uuid-1","updatedAt":"2026-09-01T00:00:00Z"}]
            """.utf8).write(to: rawFileURL(dir))
        let s = LocalAgentSessionStore(directory: dir)
        let r = s.record(crewId: "c", sessionId: "w1")
        XCTAssertEqual(r?.agentSessionId, "uuid-1")
        XCTAssertNil(r?.model)
        XCTAssertNil(r?.effort)
    }

    // MARK: - 接线（Todo #146）

    /// 上面那些只证明 store 会存会取。**「存了但没人用」跟「没存」对用户是同一件事**，
    /// 而且本仓一天之内撞过三次（规则有测试、接线没有）。所以这里直接扫源码，
    /// 断言三个用它的地方都真的接上了。
    func test_档位落盘真的被接上了三处() throws {
        let runner = try Self.source("Sources/Mac/Services/CrewSessionRunner.swift")

        XCTAssertTrue(runner.contains("LocalAgentSessionStore.shared.recordProfile("),
                      "切换成功后没有落盘 —— 切了还是会在下次重启丢掉")
        XCTAssertTrue(runner.contains("model: recorded?.model, effort: recorded?.effort"),
                      "restartMember 没有用记下来的档位（用 nil 就是走默认解析，等于撤销）")
        XCTAssertTrue(runner.contains("if model == nil { model = previousCaptain?.model }"),
                      "机长续跑没有沿用上一任的档位 —— 每次 @ 唤醒都会把它打回默认")
    }

    private static func source(_ relative: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // .../Tests/PendingCrewTests
            .deletingLastPathComponent()   // .../Tests
            .deletingLastPathComponent()   // 仓库根
        let url = root.appendingPathComponent(relative)
        guard let text = try? String(contentsOf: url, encoding: .utf8) else {
            throw XCTSkip("读不到 \(url.path)（不在开发机上跑）")
        }
        return text
    }
}
