import XCTest
// LocalCrewStore + McpServer + CrewSessionsSnapshot + LocalSessionWorldModel
// 直接编进 PendingCrewTests target。

/// 机长组织能力单测（#463）：DAG helpers、汇报线工具入队、成员快照渲染、
/// 世界观组织位置节。
@MainActor
final class CaptainOrgToolsTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("org-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true); return d
    }
    private func req(title: String) -> CreateCrewRequest {
        CreateCrewRequest.make(
            responsibleSubjectId: "subj", title: title, machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "codex",
            captain: .systemGenerated(templateName: nil))
    }

    // MARK: - LocalCrewStore DAG helpers

    func testParentChildAndTitleHelpers() throws {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let root = s.createCrew(req(title: "总部")).crewId
        let a = s.createCrew(req(title: "鉴权重构")).crewId
        let b = s.createCrew(req(title: "深色模式")).crewId
        try s.attachParent(crewId: a, parentCrewId: root)
        try s.attachParent(crewId: b, parentCrewId: root)
        XCTAssertEqual(s.parentIds(of: a), [root])
        XCTAssertEqual(s.parentIds(of: root), [])
        XCTAssertEqual(Set(s.children(of: root).map(\.title)), ["鉴权重构", "深色模式"])
        XCTAssertEqual(s.title(of: a), "鉴权重构")
    }

    func testResolveChildByIdTitleAndUniquePrefix() throws {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let root = s.createCrew(req(title: "总部")).crewId
        let auth = s.createCrew(req(title: "鉴权重构")).crewId
        let dark = s.createCrew(req(title: "深色模式")).crewId
        let dark2 = s.createCrew(req(title: "深色文案")).crewId
        for c in [auth, dark, dark2] { try s.attachParent(crewId: c, parentCrewId: root) }
        XCTAssertEqual(s.resolveChild(of: root, hint: auth), auth)          // id 精确
        XCTAssertEqual(s.resolveChild(of: root, hint: "鉴权重构"), auth)     // title 精确
        XCTAssertEqual(s.resolveChild(of: root, hint: "鉴权"), auth)         // 唯一前缀
        XCTAssertNil(s.resolveChild(of: root, hint: "深色"))                 // 歧义前缀 → nil
        XCTAssertNil(s.resolveChild(of: root, hint: "语音"))                 // 无匹配
        XCTAssertNil(s.resolveChild(of: auth, hint: "深色模式"))             // 非直系子
    }

    // MARK: - 机长不许有「解散 crew」这种能力（人类 Todo #12）

    /// 人类 2026-09-12 定的一条硬边界：
    /// 「**不要让机长能够解散。解散必须人工解散。**另外，收编啊 真的搬走啊 还是多挂
    /// 一个父 这都由机长决定。」
    ///
    /// 也就是说组织**结构**归机长（adopt / release / create_parent / adopt_parent
    /// 一个都不减），但把一个 crew **消灭掉**只能是人在界面上做的动作。
    ///
    /// ## 为什么这条测试长成「扫注册表」而不是「调某个函数」
    ///
    /// 今天这条约束**已经成立** —— MCP 注册表里压根没有这样一个工具，侧栏的「藏起来」
    /// 是右键菜单、agent 够不着。一条「现在没有」的事实不需要测试，需要测试的是
    /// **以后别人顺手加一个**：那时候没有任何地方会报错，而它恰恰是最难挽回的一类动作。
    ///
    /// 所以判据落在**工具注册表本身**：凡是名字里带 dissolve / disband / delete_crew /
    /// remove_crew / destroy 这类词的工具一旦出现，这条当场红，加的人会在这里读到
    /// 人类的原话，然后决定是去掉它、还是拿着这段话去找人类改口径。
    func testNoMcpToolCanDestroyACrew() throws {
        let server = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // Tests/PendingCrewTests
            .deletingLastPathComponent()   // Tests
            .deletingLastPathComponent()   // 仓库根
            .appendingPathComponent("Sources/Mcp/McpServer.swift")
        guard let text = try? String(contentsOf: server, encoding: .utf8) else {
            throw XCTSkip("读不到 \(server.path)（不在开发机上跑）")
        }

        // 注册表里每个 `"name": "xxx"`。
        let names = text
            .components(separatedBy: "\"name\"")
            .dropFirst()
            .compactMap { chunk -> String? in
                guard let open = chunk.firstIndex(of: "\"") else { return nil }
                let rest = chunk[chunk.index(after: open)...]
                guard let close = rest.firstIndex(of: "\"") else { return nil }
                return String(rest[..<close])
            }
        // 先证明这把尺子确实读到了东西 —— 解析一坏，下面的「零命中」就是假绿。
        XCTAssertTrue(
            names.contains("adopt_crew") && names.contains("create_child_crew"),
            "没从注册表里解析出已知工具名，说明解析坏了，这条的绿不算数：\(names.prefix(10))")

        let destructive = ["dissolve", "disband", "destroy", "delete_crew",
                           "remove_crew", "drop_crew", "archive_crew", "purge"]
        let offenders = names.filter { name in
            destructive.contains { name.contains($0) }
        }
        XCTAssertTrue(
            offenders.isEmpty,
            """
            MCP 注册表里出现了能消灭 crew 的工具：\(offenders)
            人类 2026-09-12 的原话是「不要让机长能够解散。解散必须人工解散」——
            组织结构（收编 / 搬走 / 多挂一个父）归机长，消灭一个 crew 只能是人在界面上做。
            要改这条口径，先去问人，别在这里放行。
            """)
    }

    // MARK: - 汇报线工具入队（helper 侧）

    private func makeServer(isCaptain: Bool, dir: URL) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "local-org", sessionId: "cap-1", isCaptain: isCaptain,
                  sessionLabel: "机长", quotaDirectory: dir)
    }
    private func call(_ s: McpServer, _ name: String, _ args: [String: Any]) -> String {
        let obj: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": ["name": name, "arguments": args]]
        let line = String(data: try! JSONSerialization.data(withJSONObject: obj), encoding: .utf8)!
        return s.handleLine(line) ?? ""
    }

    func testReportToParentEnqueuesCrewMessage() {
        let dir = tempDir()
        _ = call(makeServer(isCaptain: true, dir: dir), "report_to_parent",
                 ["message": "鉴权重构完成 80%,被 CI 权限阻塞,需上级协调"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "crew_message")
        XCTAssertEqual(cmds[0].direction, "to_parent")
        XCTAssertNil(cmds[0].title)
        XCTAssertEqual(cmds[0].brief, "鉴权重构完成 80%,被 CI 权限阻塞,需上级协调")
        XCTAssertEqual(cmds[0].sessionId, "cap-1")
    }

    func testMessageChildCrewEnqueuesWithTargetHint() {
        let dir = tempDir()
        _ = call(makeServer(isCaptain: true, dir: dir), "message_child_crew",
                 ["crew": "鉴权重构", "message": "优先修登录闪退,今天要进展汇报"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].direction, "to_child")
        XCTAssertEqual(cmds[0].title, "鉴权重构")
    }

    func testOrgToolsCaptainGatedAndValidated() {
        let dir = tempDir()
        let worker = makeServer(isCaptain: false, dir: dir)
        XCTAssertTrue(call(worker, "report_to_parent", ["message": "x"]).contains("仅机长可用"))
        XCTAssertTrue(call(worker, "list_sessions", [:]).contains("仅机长可用"))
        let cap = makeServer(isCaptain: true, dir: dir)
        XCTAssertTrue(call(cap, "report_to_parent", ["message": "  "]).contains("ERROR"))
        XCTAssertTrue(call(cap, "message_child_crew", ["crew": "x"]).contains("ERROR"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }

    // MARK: - 组织架构调整工具入队（#22/#25：收编/摘出/建父/认父）

    func testAdoptCrewEnqueuesTargetHint() {
        let dir = tempDir()
        _ = call(makeServer(isCaptain: true, dir: dir), "adopt_crew", ["crew": "鉴权重构"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "adopt_crew")
        XCTAssertEqual(cmds[0].title, "鉴权重构")
        XCTAssertEqual(cmds[0].sessionId, "cap-1")
    }

    func testReleaseCrewEnqueuesChildAndOptionalDestination() {
        let dir = tempDir()
        let cap = makeServer(isCaptain: true, dir: dir)
        _ = call(cap, "release_crew", ["crew": "登录闪退"])
        _ = call(cap, "release_crew", ["crew": "登录闪退", "to": "鉴权重构"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 2)
        XCTAssertTrue(cmds.allSatisfy { $0.kind == "release_crew" && $0.title == "登录闪退" })
        XCTAssertEqual(Set(cmds.map(\.note)), [nil, "鉴权重构"])
    }

    func testCreateParentCrewEnqueuesOptionalTitle() {
        let dir = tempDir()
        let cap = makeServer(isCaptain: true, dir: dir)
        _ = call(cap, "create_parent_crew", [:])
        _ = call(cap, "create_parent_crew", ["title": "PendingCrew"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 2)
        XCTAssertTrue(cmds.allSatisfy { $0.kind == "create_parent_crew" })
        XCTAssertEqual(Set(cmds.map(\.title)), [nil, "PendingCrew"])
    }

    func testAdoptParentEnqueuesTargetHint() {
        let dir = tempDir()
        _ = call(makeServer(isCaptain: true, dir: dir), "adopt_parent", ["crew": "总部"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "adopt_parent")
        XCTAssertEqual(cmds[0].title, "总部")
    }

    func testOrgRestructureToolsCaptainGatedAndValidated() {
        let dir = tempDir()
        let worker = makeServer(isCaptain: false, dir: dir)
        for tool in ["adopt_crew", "release_crew", "adopt_parent"] {
            XCTAssertTrue(call(worker, tool, ["crew": "x"]).contains("仅机长可用"), tool)
        }
        XCTAssertTrue(call(worker, "create_parent_crew", [:]).contains("仅机长可用"))
        let cap = makeServer(isCaptain: true, dir: dir)
        XCTAssertTrue(call(cap, "adopt_crew", ["crew": "  "]).contains("ERROR"))
        XCTAssertTrue(call(cap, "release_crew", [:]).contains("ERROR"))
        XCTAssertTrue(call(cap, "adopt_parent", [:]).contains("ERROR"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }

    // MARK: - list_sessions 快照读取

    func testListSessionsReadsSnapshotFile() throws {
        let dir = tempDir()
        var snap = CrewSessionsSnapshot()
        snap.updatedAt = "2026-07-05T12:00:00Z"
        snap.crews["local-org"] = [
            .init(sessionId: "cap-1", name: "机长", role: "captain", brief: "", state: "idle"),
            .init(sessionId: "w-1", name: "Claude Code · ab12cd", role: "worker",
                  brief: "修登录闪退", state: "working"),
            .init(sessionId: "w-2", name: "Codex · ef34gh", role: "worker",
                  brief: "清死代码", state: "error", healthDetail: "额度受限"),
        ]
        try JSONEncoder().encode(snap).write(to: dir.appendingPathComponent(CrewSessionsSnapshot.fileName))
        let out = call(makeServer(isCaptain: true, dir: dir), "list_sessions", [:])
        XCTAssertTrue(out.contains("机长"))
        XCTAssertTrue(out.contains("修登录闪退"))
        XCTAssertTrue(out.contains("额度受限"))
        XCTAssertTrue(out.contains("w-1"))
    }

    // MARK: - 世界观组织位置节

    func testWorldModelLineageBlockRendersDAG() {
        let vars = LocalSessionWorldModel().buildVars(.init(
            sessionTaskBrief: "x", runnerKind: "codex", crewId: "c1", crewTitle: "鉴权重构",
            parentTitles: ["总部"], childTitles: ["登录闪退", "OAuth"]))
        let block = vars["lineageBlock"] ?? ""
        XCTAssertTrue(block.contains("「总部」"))
        XCTAssertTrue(block.contains("「登录闪退」"))
        XCTAssertTrue(block.contains("report_to_parent"))
        // 根 crew 无子的表述。
        let rootVars = LocalSessionWorldModel().buildVars(.init(
            sessionTaskBrief: "x", runnerKind: "codex", crewId: "c0", crewTitle: "总部"))
        XCTAssertTrue((rootVars["lineageBlock"] ?? "").contains("根 crew"))
    }
}
