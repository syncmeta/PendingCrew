import XCTest
// McpServer.swift + AgentModelCatalog.swift + LocalWhiteboardStore.swift 等编进 test bundle。

/// 模型表注入（Todo #37）与参数 fail-loud（Todo #36）在 MCP 面的行为。
///
/// 两条不可动摇的性质，都在这里钉死：
/// 1. **注入的清单一定带新鲜度**——手工兜底表必须说出「X 天没核实过」，不许当事实呈现。
/// 2. **提醒从不拦截**——填了表里没有的值照常入队，只是把话说到工具回执 + 白板上。
final class McpServerModelCatalogTests: XCTestCase {

    private func tmp() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcpcatalog-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func makeServer(dir: URL, isCaptain: Bool = true, agentKey: String? = nil) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "local-x", sessionId: "s1", isCaptain: isCaptain,
                  sessionLabel: "测试 session", quotaDirectory: dir,
                  todos: LocalTodoStore(directory: dir), agentKey: agentKey)
    }

    private func call(_ s: McpServer, _ name: String, _ args: [String: Any]) -> String {
        let obj: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": ["name": name, "arguments": args]]
        let line = String(data: try! JSONSerialization.data(withJSONObject: obj), encoding: .utf8)!
        return s.handleLine(line) ?? ""
    }

    private func toolDescription(_ s: McpServer, _ name: String) throws -> String {
        let raw = try XCTUnwrap(s.handleLine(#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#))
        let obj = try XCTUnwrap(
            (try JSONSerialization.jsonObject(with: Data(raw.utf8))) as? [String: Any])
        let tools = try XCTUnwrap(
            (obj["result"] as? [String: Any])?["tools"] as? [[String: Any]])
        let tool = try XCTUnwrap(tools.first { $0["name"] as? String == name })
        return try XCTUnwrap(tool["description"] as? String)
    }

    /// 往目录里写一份「现探且新鲜」的 models.json。
    private func writeProbedCatalog(dir: URL, probedAt: Date = Date()) {
        let iso = ISO8601DateFormatter().string(from: probedAt)
        let file = AgentModelCatalogFile(
            claude: AgentModelTable(
                agent: "claude", source: .probe, probedAt: iso,
                models: [AgentModel(id: "opus"), AgentModel(id: "sonnet")],
                efforts: ["low", "high", "max"],
                resolvedDefault: "sonnet", resolvedDefaultSource: "~/.claude/settings.json 的 model"),
            codex: AgentModelTable(
                agent: "codex", source: .probe, probedAt: iso,
                models: [AgentModel(id: "gpt-5.6-sol", efforts: ["low", "high", "ultra"])],
                efforts: ["low", "high", "ultra"],
                resolvedDefault: "gpt-5.6-sol",
                resolvedDefaultSource: "~/.codex/config.toml 顶层 model"))
        try? JSONEncoder().encode(file)
            .write(to: dir.appendingPathComponent(AgentModelCatalog.fileName))
    }

    // MARK: - 清单注入

    func testStartSessionDescriptionCarriesLiveCatalog() throws {
        let dir = tmp()
        writeProbedCatalog(dir: dir)
        let text = try toolDescription(makeServer(dir: dir), "start_session")
        XCTAssertTrue(text.contains("【可用模型表】"), text)
        XCTAssertTrue(text.contains("opus"), "claude 那张表要出现在描述里")
        XCTAssertTrue(text.contains("gpt-5.6-sol"), "codex 那张表也要 —— 机长是跨家派活的")
        XCTAssertTrue(text.contains("不显式选 model 时实际跑 sonnet"),
                      "默认那条腿也要说清，否则机长不知道不填会跑什么：\(text)")
        XCTAssertFalse(text.contains("⚠️"), "现探且新鲜就不该挂过期警示")
    }

    func testDescriptionDisclosesStaleFallbackTable() throws {
        let dir = tmp()   // 不写 models.json → 回落手工兜底表
        let text = try toolDescription(makeServer(dir: dir), "start_session")
        XCTAssertTrue(text.contains("【可用模型表】"), text)
        XCTAssertTrue(text.contains("手工兜底表"),
                      "回落到手工表时必须自报家门，不许静默当现探的表呈现：\(text)")
    }

    func testSetProfileDescriptionScopesToOwnRunner() throws {
        let dir = tmp()
        writeProbedCatalog(dir: dir)
        let text = try toolDescription(makeServer(dir: dir, agentKey: "codex"),
                                       "set_session_profile")
        XCTAssertTrue(text.contains("gpt-5.6-sol"), text)
        XCTAssertFalse(text.contains("opus"),
                       "codex session 不该看到 claude 的清单（照着切只会白切）：\(text)")
    }

    // MARK: - 参数提醒：说话，但**不拦**

    func testStartSessionWithUnlistedModelStillEnqueues() {
        let dir = tmp()
        writeProbedCatalog(dir: dir)
        let s = makeServer(dir: dir)
        let out = call(s, "start_session", ["brief": "干活", "isolation": false,
                                            "runner": "codex", "model": "gpt-5-codex"])
        // ① 照常入队 —— 旧别名后端往往仍解析得了，实测过，绝不能拦。
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].model, "gpt-5-codex", "值要原样透传，不许改写")
        // ② 工具回执里要有提醒。
        XCTAssertTrue(out.contains("参数提醒"), out)
        XCTAssertTrue(out.contains("gpt-5-codex"), out)
        // ③ 白板上也要有（fail-loud 的真正落点 —— 回执只有调用方看得到）。
        let board = LocalWhiteboardStore(directory: dir).list(crewId: "local-x")
        XCTAssertTrue(board.contains { $0.text.contains("gpt-5-codex") && $0.text.contains("⚠️") },
                      "提醒必须进白板：\(board.map(\.text))")
    }

    func testSetProfileWithUnlistedEffortStillEnqueuesAndWarns() {
        let dir = tmp()
        writeProbedCatalog(dir: dir)
        let s = makeServer(dir: dir, agentKey: "codex")
        // gpt-5.6-sol 支持 low/high/ultra，没有 xhigh。
        let out = call(s, "set_session_profile", ["model": "gpt-5.6-sol", "effort": "xhigh"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].effort, "xhigh", "照常排队，不拦")
        XCTAssertTrue(out.contains("参数提醒"), out)
        let board = LocalWhiteboardStore(directory: dir).list(crewId: "local-x")
        XCTAssertTrue(board.contains { $0.text.contains("effort") && $0.text.contains("⚠️") },
                      "白板要留下这条：\(board.map(\.text))")
    }

    func testKnownValuesProduceNoNoise() {
        let dir = tmp()
        writeProbedCatalog(dir: dir)
        let s = makeServer(dir: dir, agentKey: "claude")
        let out = call(s, "set_session_profile", ["model": "opus", "effort": "high"])
        XCTAssertFalse(out.contains("参数提醒"), "对的值别刷屏：\(out)")
        XCTAssertTrue(LocalWhiteboardStore(directory: dir).list(crewId: "local-x").isEmpty,
                      "对的值不该往白板写东西")
    }

    // MARK: - 起 session 与切配置走的是两套 effort（Todo #36 的坑）

    /// 往目录写一份带「启动/运行两套 effort」的 claude 现探表。
    private func writeSplitEffortCatalog(dir: URL) {
        let file = AgentModelCatalogFile(claude: AgentModelTable(
            agent: "claude", source: .probe,
            probedAt: ISO8601DateFormatter().string(from: Date()),
            models: [AgentModel(id: "opus")],
            efforts: ["low", "medium", "high", "xhigh", "max", "ultracode", "auto"],
            launchEfforts: ["low", "medium", "high", "xhigh", "max"],
            undocumentedLaunchEfforts: ["ultracode"]))
        try? JSONEncoder().encode(file)
            .write(to: dir.appendingPathComponent(AgentModelCatalog.fileName))
    }

    func testStartSessionWarnsOnRuntimeOnlyEffort() {
        let dir = tmp()
        writeSplitEffortCatalog(dir: dir)
        let s = makeServer(dir: dir)
        let out = call(s, "start_session", ["brief": "干活", "isolation": false,
                                            "runner": "claude", "effort": "auto"])
        // 照常入队（不拦），但必须把「会被静默降级」说出来。
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].effort, "auto")
        XCTAssertTrue(out.contains("静默降级"), out)
        XCTAssertTrue(out.contains("set_session_profile"), "要指出替代路径：\(out)")
        let board = LocalWhiteboardStore(directory: dir).list(crewId: "local-x")
        XCTAssertTrue(board.contains { $0.text.contains("静默降级") },
                      "白板要留下这条：\(board.map(\.text))")
    }

    func testSetProfileAcceptsTheSameValueSilently() {
        let dir = tmp()
        writeSplitEffortCatalog(dir: dir)
        let s = makeServer(dir: dir, agentKey: "claude")
        // 同一个 auto，走运行时那条腿是完全合法的 —— 一个字都不该说。
        let out = call(s, "set_session_profile", ["effort": "auto"])
        XCTAssertFalse(out.contains("参数提醒"), out)
        XCTAssertTrue(LocalWhiteboardStore(directory: dir).list(crewId: "local-x").isEmpty)
    }

    func testStartSessionAcceptsUndocumentedLaunchEffort() {
        let dir = tmp()
        writeSplitEffortCatalog(dir: dir)
        let s = makeServer(dir: dir)
        // ultracode：帮助里没写但实测起 session 也收 —— 不该报成不可用。
        let out = call(s, "start_session", ["brief": "干活", "isolation": false,
                                            "runner": "claude", "effort": "ultracode"])
        XCTAssertFalse(out.contains("参数提醒"), out)
    }

    func testDescriptionSeparatesLaunchAndRuntimeEfforts() throws {
        let dir = tmp()
        writeSplitEffortCatalog(dir: dir)
        let text = try toolDescription(makeServer(dir: dir), "start_session")
        XCTAssertTrue(text.contains("起 session 时的 effort"), text)
        XCTAssertTrue(text.contains("只能跑起来之后用 set_session_profile 切"), text)
    }

    func testUnknownRunnerChecksBothFamiliesBeforeSpeaking() {
        let dir = tmp()
        writeProbedCatalog(dir: dir)
        let s = makeServer(dir: dir)
        // runner 没填 → 判不出是哪家。opus 是 claude 那张表里的 → 不该报。
        _ = call(s, "start_session", ["brief": "干活", "isolation": false, "model": "opus"])
        XCTAssertTrue(LocalWhiteboardStore(directory: dir).list(crewId: "local-x").isEmpty,
                      "任一家认得就该闭嘴，别对着错的表瞎报")
    }
}
