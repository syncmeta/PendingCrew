import XCTest
// McpServer.swift + LocalWhiteboardStore.swift + LocalApprovalStore.swift +
// LocalCrewControlStore.swift 编进 test bundle（见 project.yml）。
//
// 覆盖机长专用 start_session / create_child_crew 两工具：happy path 入队 +
// isCaptain=false 拒绝 + 空 brief 拒绝，均不落队列（见 McpServer.swift handleToolCall）。

final class McpServerCaptainSpawnTests: XCTestCase {
    private func makeServer(isCaptain: Bool, dir: URL) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "local-x", sessionId: "cap", isCaptain: isCaptain, sessionLabel: "Captain")
    }
    private func tmp() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("mcpspawn-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true); return d
    }
    private func call(_ s: McpServer, _ name: String, _ args: [String: Any]) -> String? {
        let obj: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": ["name": name, "arguments": args]]
        let line = String(data: try! JSONSerialization.data(withJSONObject: obj), encoding: .utf8)!
        return s.handleLine(line)
    }

    func testStartSessionEnqueues() {
        let dir = tmp()
        let s = makeServer(isCaptain: true, dir: dir)
        _ = call(s, "start_session", ["brief": "修登录闪退", "runner": "claude", "isolation": true])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "start_session")
        XCTAssertEqual(cmds[0].crewId, "local-x")
        XCTAssertEqual(cmds[0].brief, "修登录闪退")
        XCTAssertEqual(cmds[0].runner, "claude")
        XCTAssertEqual(cmds[0].isolation, true)
    }

    func testStartSessionCarriesModelAndEffort() {
        let dir = tmp()
        let s = makeServer(isCaptain: true, dir: dir)
        _ = call(s, "start_session", [
            "brief": "重构鉴权", "isolation": false,
            "model": "haiku", "effort": "LOW",
        ])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].model, "haiku")
        XCTAssertEqual(cmds[0].effort, "low")   // effort 归一小写
    }

    func testStartSessionSanitizesInvalidModelEffort() {
        let dir = tmp()
        let s = makeServer(isCaptain: true, dir: dir)
        // 带空格的 model（像一句话）与空 effort → 双双丢弃为 nil，不进 argv。
        _ = call(s, "start_session", [
            "brief": "干活", "isolation": false,
            "model": "use the best model", "effort": "  ",
        ])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertNil(cmds[0].model)
        XCTAssertNil(cmds[0].effort)
    }

    func testOldQueueFileWithoutModelEffortStillDecodes() {
        // 旧版排队文件（无 model/effort 键）必须照常 decode —— 升级兼容。
        let dir = tmp()
        let json = """
        {"id":"i1","crewId":"local-x","kind":"start_session","brief":"旧命令","ts":"2026-07-01T00:00:00Z"}
        """
        try! json.data(using: .utf8)!.write(to: dir.appendingPathComponent("local-x.i1.crewcmd.json"))
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].brief, "旧命令")
        XCTAssertNil(cmds[0].model)
        XCTAssertNil(cmds[0].effort)
    }

    func testCreateChildCrewEnqueues() {
        let dir = tmp()
        _ = call(makeServer(isCaptain: true, dir: dir), "create_child_crew", ["brief": "做支付", "title": "支付"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "create_child_crew")
        XCTAssertEqual(cmds[0].title, "支付")
    }

    func testEmptyBriefRejectedNoEnqueue() {
        let dir = tmp()
        let out = call(makeServer(isCaptain: true, dir: dir), "start_session", ["brief": "   "])
        XCTAssertTrue((out ?? "").contains("ERROR"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }

    func testMissingIsolationRejectedNoEnqueue() {
        let dir = tmp()
        let out = call(makeServer(isCaptain: true, dir: dir), "start_session", ["brief": "干活"])
        XCTAssertTrue((out ?? "").contains("isolation 必填"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }

    func testNonCaptainRejectedNoEnqueue() {
        let dir = tmp()
        let out = call(makeServer(isCaptain: false, dir: dir), "start_session", ["brief": "干活"])
        XCTAssertTrue((out ?? "").contains("仅机长可用"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }
}
