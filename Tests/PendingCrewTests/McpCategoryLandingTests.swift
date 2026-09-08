import XCTest

/// 分类**真的驱动落账**（人类 Todo #115 / #120）—— 这一层量的是
/// 「发一条消息之后，那本账上到底有没有多出/改掉一条」。
///
/// 人类原话的根：「不然第一 todo、cockpit等等不能及时更新」。
/// **分类做成一个标签颜色 = 这一单白做**，所以判据不是「消息里带了 category」，
/// 是**账变了没有**。
///
/// 这一笔只接不碰驾驶舱那道门的两类（`human_todo` / `todo_response`）——
/// `plan_add` / `plan_update` 现在是 `guard isCaptain`，worker 标 `progress` 会被
/// 直接拒，那部分等权限拆分（(D) 方案）再接。
@MainActor
final class McpCategoryLandingTests: XCTestCase {

    private struct Fixture {
        let base: URL
        let whiteboards: URL
        let crewId: String
    }

    private func fixture() -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcpcat-\(UUID().uuidString)")
        let wb = base.appendingPathComponent("whiteboards", isDirectory: true)
        try? FileManager.default.createDirectory(at: wb, withIntermediateDirectories: true)
        let store = LocalCrewStore(baseDirectory: base)
        let crew = store.createCrew(.make(
            responsibleSubjectId: "s", title: "机组群聊体验", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        store.recordSessionMember(crewId: crew, sessionId: "sess-1", displayName: "机长")
        return Fixture(base: base, whiteboards: wb, crewId: crew)
    }

    private func server(_ f: Fixture, captain: Bool = true) -> McpServer {
        // ⚠️ `todos` 必须显式注入：`McpServer` 的默认值是 `LocalTodoStore()`，
        // 它指向**真的 app 数据目录**。不注入的话这些用例会往真账本里写东西
        // （第一版就写进去了三条，事后删掉的）—— 而且用例自己读 fixture、
        // 读到 0 条，看起来像「没落账」，实际是**落到别人家去了**。
        McpServer(store: LocalWhiteboardStore(directory: f.whiteboards),
                  approvals: LocalApprovalStore(directory: f.whiteboards),
                  control: LocalCrewControlStore(directory: f.whiteboards),
                  crewId: f.crewId, sessionId: "sess-1",
                  isCaptain: captain, sessionLabel: "机长",
                  quotaDirectory: f.whiteboards,
                  todos: LocalTodoStore(directory: f.whiteboards, ledger: .agent))
    }

    private func post(_ s: McpServer, _ arguments: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call",\
        "params":{"name":"post_to_crew","arguments":\(arguments)}}
        """) ?? ""
    }

    private func humanTodos(_ f: Fixture) -> [LocalTodoItem] {
        LocalTodoStore(directory: f.whiteboards, ledger: .human).list(crewId: f.crewId)
    }

    private func board(_ f: Fixture) -> [LocalWhiteboardMessage] {
        LocalWhiteboardStore(directory: f.whiteboards).list(crewId: f.crewId)
    }

    // MARK: - human_todo：发这条 = 人类 Todo 面板多一条

    func test_标human_todo的消息会真的建一条人类todo() {
        let f = fixture()
        XCTAssertTrue(humanTodos(f).isEmpty, "前置条件：这本账本来是空的")

        let r = post(server(f), #"{"message":"侧栏那个胶囊的像素没人看过，你瞄一眼","category":"human_todo"}"#)

        let items = humanTodos(f)
        XCTAssertEqual(items.count, 1, "账上没多出那一条 —— 分类没有驱动落账，等于白做：\(r)")
        XCTAssertTrue(items.first?.text.contains("像素") ?? false, items.first?.text ?? "nil")
        XCTAssertEqual(board(f).count, 1, "消息本身也要照常发出去")
    }

    /// **回执必须说清它动了哪本账、第几条** —— 否则「落账可撤」无从谈起：
    /// 人得先知道它建了 #N，才点得进去、撤得掉。
    func test_回执要说清建了第几条() {
        let f = fixture()
        let r = post(server(f), #"{"message":"要你拍板","category":"human_todo"}"#)
        XCTAssertTrue(r.contains("人类 Todo"), "回执没说动了哪本账：\(r)")
        XCTAssertTrue(r.contains("#1"), "回执没给号，撤不掉也点不进去：\(r)")
    }

    // MARK: - 不落账的分类：一条账都不许动

    func test_不落账的分类不许动任何账() {
        let f = fixture()
        for c in ["ack", "question", "finding", "note", "handoff"] {
            _ = post(server(f), "{\"message\":\"随便说一句\",\"category\":\"\(c)\"}")
        }
        XCTAssertTrue(humanTodos(f).isEmpty,
                      "有分类把不该落账的也落了 —— `handoff` 尤其危险，它一旦驱动动作就是起进程")
        XCTAssertEqual(board(f).count, 5, "但消息本身都该照常发出去")
    }

    // MARK: - 第一步不许炸任何人

    func test_不给分类照常发只是不落账() {
        let f = fixture()
        let r = post(server(f), #"{"message":"老调用方没带 category"}"#)
        XCTAssertFalse(r.contains("ERROR:"), "第一步把分类翻成必填了 —— 在跑的 session 会开始失败：\(r)")
        XCTAssertEqual(board(f).count, 1)
        XCTAssertTrue(humanTodos(f).isEmpty)
    }

    func test_旧值milestone不许炸() {
        let f = fixture()
        let r = post(server(f), #"{"message":"老值","category":"milestone"}"#)
        XCTAssertFalse(r.contains("ERROR:"), r)
        XCTAssertEqual(board(f).count, 1)
    }

    // MARK: - fail-loud：账没落上就**不许**把消息发出去

    /// 这一单最坏的结果不是「落账失败」，是**消息发出去了、账没落上** ——
    /// 那比现在还糟，因为人会**以为**账更新了。
    /// 顺序照 `TodoLandingFlow` 写死的那条：**落账 → 发群，一步都不许跳。**
    func test_落账失败时消息不许发出去() throws {
        let f = fixture()
        // 把人类那本账做成写不进去：占住它的位置，让写入必失败。
        let todoFile = f.whiteboards.appendingPathComponent("\(f.crewId).human-todos.json")
        try FileManager.default.createDirectory(at: todoFile, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: todoFile) }

        let r = post(server(f), #"{"message":"要你拍板","category":"human_todo"}"#)

        XCTAssertTrue(r.contains("ERROR:"), "落账失败却回了成功 —— 人会以为账更新了：\(r)")
        // 白板上**可以**有一条系统警示（读失败要 fail-loud，那是对的）；
        // 不许出现的是**我这条消息本身**。
        XCTAssertFalse(board(f).contains { $0.text.contains("要你拍板") },
                       "账没落上，消息却发出去了 —— 这正是这一单最坏的结果")
    }

    // MARK: - #120：挂 Todo 号 + 强制更新状态

    /// 分类与 todo 号是**两根轴**：这里用 `note`（不落账）配一个 todo 号，
    /// 正是为了证明翻牌不依赖分类。（第一版这条用了 `progress`，结果被
    /// 「progress 要计划号」挡住 —— 用例自己把两根轴混成了一根。）
    func test_挂todo号会真的翻状态() {
        let f = fixture()
        let agent = LocalTodoStore(directory: f.whiteboards, ledger: .agent)
        _ = agent.add(crewId: f.crewId, text: "人类派下来的活",
                      bySessionId: "human", bySenderName: "人")
        let r = post(server(f),
                     #"{"message":"开工了","category":"note","todo":1,"todo_status":"in_progress"}"#)
        XCTAssertEqual(agent.list(crewId: f.crewId).first?.status, "in_progress",
                       "挂了号却没翻状态 —— 人类要的「强制更新」没发生：\(r)")
        XCTAssertTrue(r.contains("#1"), "回执没说动了哪条 Todo：\(r)")
    }

    /// 挂号不给状态 ⇒ 拒，**而且消息不发**（跟落账失败同一条规矩）。
    func test_挂了号不给状态要拒而且不发消息() {
        let f = fixture()
        let r = post(server(f), #"{"message":"随便","category":"note","todo":1}"#)
        XCTAssertTrue(r.contains("ERROR:"), r)
        XCTAssertFalse(board(f).contains { $0.text.contains("随便") },
                       "参数不合法却把消息发出去了")
    }
}
