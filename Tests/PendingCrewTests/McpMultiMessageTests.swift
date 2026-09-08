import XCTest

/// 一次调用发**多条**，每条各自落自己的账（人类 Todo #115 后半）。
///
/// 人类原话：「一次汇报把各种东西都揉在一条消息里 这就不好 **这是个重要的改变**」。
@MainActor
final class McpMultiMessageTests: XCTestCase {

    private struct Fixture { let base: URL; let whiteboards: URL; let crewId: String }

    private func fixture() -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcpmulti-\(UUID().uuidString)")
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

    /// ⚠️ `todos` 必须显式注入 —— 默认值 `LocalTodoStore()` 指向**真的 app 数据目录**
    /// （这个坑在 `McpCategoryLandingTests` 里踩过一次，真往人的账本写进去过）。
    private func server(_ f: Fixture) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: f.whiteboards),
                  approvals: LocalApprovalStore(directory: f.whiteboards),
                  control: LocalCrewControlStore(directory: f.whiteboards),
                  crewId: f.crewId, sessionId: "sess-1",
                  isCaptain: true, sessionLabel: "机长",
                  quotaDirectory: f.whiteboards,
                  todos: LocalTodoStore(directory: f.whiteboards, ledger: .agent))
    }

    private func post(_ s: McpServer, _ arguments: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call",\
        "params":{"name":"post_to_crew","arguments":\(arguments)}}
        """) ?? ""
    }

    private func board(_ f: Fixture) -> [LocalWhiteboardMessage] {
        LocalWhiteboardStore(directory: f.whiteboards).list(crewId: f.crewId)
    }
    private func humanTodos(_ f: Fixture) -> [LocalTodoItem] {
        LocalTodoStore(directory: f.whiteboards, ledger: .human).list(crewId: f.crewId)
    }

    /// 正身：一次汇报 → 三个气泡 + 各自落账。
    func test_一次发三条各成一个气泡() {
        let f = fixture()
        let r = post(server(f), """
        {"messages":[\
        {"text":"闸门全绿，包可以发了","category":"note"},\
        {"text":"侧栏那个胶囊的像素没人看过，你瞄一眼","category":"human_todo"},\
        {"text":"迁移工作目录要不要一起拆？","category":"question"}]}
        """)
        let msgs = board(f)
        XCTAssertEqual(msgs.count, 3, "没分成三条：\(r)")
        XCTAssertEqual(msgs.map(\.text), [
            "闸门全绿，包可以发了",
            "侧栏那个胶囊的像素没人看过，你瞄一眼",
            "迁移工作目录要不要一起拆？",
        ], "顺序要跟给的一样 —— 一次汇报读起来是有先后的")
        XCTAssertEqual(msgs.map(\.category), ["note", "human_todo", "question"],
                       "分类是**逐条**的，不是整批一个")
        XCTAssertEqual(humanTodos(f).count, 1, "中间那条该建一条人类 Todo：\(r)")
        XCTAssertTrue(r.contains("已发出 3 条"), "回执没说发了几条：\(r)")
    }

    /// 校验全有或全无：第 3 条空白 ⇒ 前两条也不许出去。
    func test_有一条不合法时一条都不发() {
        let f = fixture()
        let r = post(server(f), #"{"messages":[{"text":"好的"},{"text":"也好"},{"text":"  "}]}"#)
        XCTAssertTrue(r.contains("ERROR:"), r)
        XCTAssertTrue(board(f).isEmpty,
                      "第三条写错，前两条却已经发出去了 —— 分条之后「一半成功」是新的失败形态")
    }

    func test_message和messages同时给要拒() {
        let f = fixture()
        let r = post(server(f), #"{"message":"一条","messages":[{"text":"另一条"}]}"#)
        XCTAssertTrue(r.contains("ERROR:"), r)
        XCTAssertTrue(board(f).isEmpty)
    }

    /// 老形态一个字都不许变。
    func test_单条老形态不受影响() {
        let f = fixture()
        let r = post(server(f), #"{"message":"就一条","category":"note"}"#)
        XCTAssertFalse(r.contains("ERROR:"), r)
        XCTAssertEqual(board(f).map(\.text), ["就一条"])
    }

    /// 逐条里某条的**落账参数**不合法（挂了 todo 号却没给状态）⇒ 整批拒。
    func test_某条落账参数不合法时整批拒() {
        let f = fixture()
        let r = post(server(f), #"{"messages":[{"text":"好的"},{"text":"挂错了","todo":1}]}"#)
        XCTAssertTrue(r.contains("ERROR:"), r)
        XCTAssertTrue(r.contains("第 2 条"), "没指出是第几条出的问题：\(r)")
        XCTAssertTrue(board(f).isEmpty, "整批该拒")
    }
}
