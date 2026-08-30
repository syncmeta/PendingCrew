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

    private func listedTools(_ s: McpServer) -> [[String: Any]] {
        let raw = s.handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        let json = try! JSONSerialization.jsonObject(with: Data(raw.utf8)) as! [String: Any]
        let result = json["result"] as! [String: Any]
        return result["tools"] as! [[String: Any]]
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

    func testCaptainHandoffToolsAreCaptainOnlyAndHaveUnambiguousSchemas() {
        let dir = tmp()
        let workerNames = Set(listedTools(makeServer(isCaptain: false, dir: dir))
            .compactMap { $0["name"] as? String })
        XCTAssertFalse(workerNames.contains("handoff_captain_to_session"))
        XCTAssertFalse(workerNames.contains("create_and_handoff_captain"))

        let captainTools = listedTools(makeServer(isCaptain: true, dir: dir))
        let existing = captainTools.first { $0["name"] as? String == "handoff_captain_to_session" }
        let create = captainTools.first { $0["name"] as? String == "create_and_handoff_captain" }
        XCTAssertNotNil(existing)
        XCTAssertNotNil(create)

        let existingSchema = existing?["inputSchema"] as? [String: Any]
        XCTAssertEqual(existingSchema?["required"] as? [String], ["session_id"])
        let existingProperties = existingSchema?["properties"] as? [String: Any]
        XCTAssertNotNil(existingProperties?["target_crew_id"])
        let createSchema = create?["inputSchema"] as? [String: Any]
        XCTAssertEqual(createSchema?["required"] as? [String], ["runner"])
        let createProperties = createSchema?["properties"] as? [String: Any]
        XCTAssertNotNil(createProperties?["target_crew_id"])
        XCTAssertNotNil(createProperties?["model"])
        XCTAssertNotNil(createProperties?["effort"])
        XCTAssertNotNil(createProperties?["title"])
        XCTAssertNotNil(createProperties?["opening_brief"])
    }

    func testExistingMemberHandoffEnqueuesExactSessionWithoutClaimingSuccess() {
        let dir = tmp()
        let out = call(makeServer(isCaptain: true, dir: dir),
                       "handoff_captain_to_session", ["session_id": "worker-123"])
        let commands = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(commands.count, 1)
        XCTAssertEqual(commands[0].kind, "handoff_captain")
        XCTAssertNil(commands[0].targetCrewId, "省略目标必须继续操作本 crew")
        XCTAssertEqual(commands[0].sessionId, "worker-123")
        XCTAssertNil(commands[0].runner)
        XCTAssertTrue((out ?? "").contains("已受理"))
        XCTAssertTrue((out ?? "").contains("群聊最终回执"))
        XCTAssertFalse((out ?? "").contains("交接完成"), "helper 不能只写命令文件就宣称成功")
    }

    func testDirectChildExistingMemberHandoffCarriesExplicitTargetCrew() {
        let dir = tmp()
        _ = call(makeServer(isCaptain: true, dir: dir),
                 "handoff_captain_to_session", [
                    "target_crew_id": "child-crew", "session_id": "child-worker-123",
                 ])

        let commands = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(commands.count, 1)
        XCTAssertEqual(commands[0].crewId, "local-x", "发起 crew 不能被目标覆盖")
        XCTAssertEqual(commands[0].targetCrewId, "child-crew")
        XCTAssertEqual(commands[0].requesterSessionId, "cap")
        XCTAssertEqual(commands[0].sessionId, "child-worker-123")
    }

    func testCreateCaptainHandoffCarriesExplicitLaunchOptionsAndDefaults() {
        let dir = tmp()
        _ = call(makeServer(isCaptain: true, dir: dir), "create_and_handoff_captain", [
            "runner": "codex", "model": "gpt-5.6-sol", "effort": "HIGH",
            "title": "接班机长", "opening_brief": "从白板续接 #74",
        ])
        var commands = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(commands.count, 1)
        XCTAssertEqual(commands[0].kind, "handoff_captain")
        XCTAssertNil(commands[0].sessionId)
        XCTAssertEqual(commands[0].runner, "codex")
        XCTAssertEqual(commands[0].model, "gpt-5.6-sol")
        XCTAssertEqual(commands[0].effort, "high")
        XCTAssertEqual(commands[0].title, "接班机长")
        XCTAssertEqual(commands[0].note, "从白板续接 #74")

        _ = call(makeServer(isCaptain: true, dir: dir),
                 "create_and_handoff_captain", ["runner": "codex"])
        commands = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(commands[0].title, "机长")
        XCTAssertNil(commands[0].model)
        XCTAssertNil(commands[0].effort)
        XCTAssertNil(commands[0].note)
    }

    func testDirectChildCreateRequiresAndCarriesExplicitLaunchProfileAndBrief() {
        let dir = tmp()
        let captain = makeServer(isCaptain: true, dir: dir)

        for args: [String: Any] in [
            ["target_crew_id": "child", "runner": "codex", "effort": "high", "opening_brief": "救援"],
            ["target_crew_id": "child", "runner": "codex", "model": "gpt-5.6-sol", "opening_brief": "救援"],
            ["target_crew_id": "child", "runner": "codex", "model": "gpt-5.6-sol", "effort": "high"],
        ] {
            XCTAssertTrue((call(captain, "create_and_handoff_captain", args) ?? "").contains("ERROR"))
        }
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)

        _ = call(captain, "create_and_handoff_captain", [
            "target_crew_id": "child", "runner": "codex", "model": "gpt-5.6-sol",
            "effort": "HIGH", "opening_brief": "从子 crew 白板接管",
        ])
        let commands = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(commands.count, 1)
        XCTAssertEqual(commands[0].crewId, "local-x")
        XCTAssertEqual(commands[0].targetCrewId, "child")
        XCTAssertEqual(commands[0].runner, "codex")
        XCTAssertEqual(commands[0].model, "gpt-5.6-sol")
        XCTAssertEqual(commands[0].effort, "high")
        XCTAssertEqual(commands[0].note, "从子 crew 白板接管")
    }

    func testCaptainHandoffRejectsWorkerCallsAndInvalidModeArguments() {
        let dir = tmp()
        let worker = makeServer(isCaptain: false, dir: dir)
        XCTAssertTrue((call(worker, "handoff_captain_to_session", ["session_id": "s"]) ?? "")
            .contains("仅机长可用"))
        XCTAssertTrue((call(worker, "create_and_handoff_captain", ["runner": "codex"]) ?? "")
            .contains("仅机长可用"))
        XCTAssertTrue((call(worker, "handoff_captain_to_session", [
            "target_crew_id": "child", "session_id": "s",
        ]) ?? "").contains("仅机长可用"))

        let captain = makeServer(isCaptain: true, dir: dir)
        XCTAssertTrue((call(captain, "handoff_captain_to_session", [:]) ?? "").contains("ERROR"))
        XCTAssertTrue((call(captain, "create_and_handoff_captain", ["runner": "terminal"]) ?? "")
            .contains("runner"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }
}
