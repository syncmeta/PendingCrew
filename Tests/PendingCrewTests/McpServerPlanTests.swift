import XCTest

/// 机长作战板的 MCP 三个工具（人类 Todo #66）：`plan_add` / `plan_update` / `plan_list`。
///
/// 这里钉的是**门禁**和**进群纪律**：只有机长写得动；除了「卡住」那一档，进度更新
/// 一律不进群 —— 这块板存在的意义就是让进度不必靠刷屏传达。
final class McpServerPlanTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("mcp-plan-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL, isCaptain: Bool = true) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1", isCaptain: isCaptain,
                  sessionLabel: "机长", todos: LocalTodoStore(directory: dir),
                  plans: CockpitPlanStore(directory: dir))
    }

    private func call(_ s: McpServer, _ name: String, _ args: String = "{}") -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"\(name)","arguments":\(args)}}
        """) ?? ""
    }

    private func whiteboardTexts(_ dir: URL) -> [String] {
        LocalWhiteboardStore(directory: dir).list(crewId: "c").map(\.text)
    }

    // MARK: - 门禁

    func testWorkerCannotSeePlanTools() {
        let r = server(tempDir(), isCaptain: false)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertFalse(r.contains("plan_add"))
        XCTAssertFalse(r.contains("plan_update"))
    }

    func testCaptainSeesPlanTools() {
        let r = server(tempDir(), isCaptain: true)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertTrue(r.contains("plan_add"))
        XCTAssertTrue(r.contains("plan_list"))
    }

    /// 看不见 ≠ 调不动 —— 门禁得在**执行处**也站一道。
    func testWorkerCallIsRefusedAtExecution() {
        let dir = tempDir()
        let s = server(dir, isCaptain: false)
        XCTAssertTrue(call(s, "plan_add", #"{"title":"偷偷排一条"}"#).contains("仅机长可用"))
        XCTAssertTrue(s.plans.list(crewId: "c").isEmpty)
    }

    // MARK: - 排 / 推进

    func testAddThenListShowsNumberAndStaleness() {
        let s = server(tempDir())
        XCTAssertTrue(call(s, "plan_add", #"{"title":"把 A 段做完"}"#).contains("#1"))
        let list = call(s, "plan_list")
        XCTAssertTrue(list.contains("把 A 段做完"))
        XCTAssertTrue(list.contains("没做"))
        XCTAssertTrue(list.contains("最后更新"), "照妖镜也要照到机长自己读的这一面")
    }

    func testProgressUpdateDoesNotReachTheGroupChat() {
        let dir = tempDir()
        let s = server(dir)
        _ = call(s, "plan_add", #"{"title":"接线"}"#)
        _ = call(s, "plan_update", #"{"number":1,"progress":"读完了那笔提交","status":"in_progress"}"#)
        XCTAssertTrue(whiteboardTexts(dir).isEmpty, "进度更新不该进群 —— 每推一步发一条就等于把板搬回群里")
    }

    // MARK: - 卡住

    func testBlockedWithoutReferenceIsRefused() {
        let dir = tempDir()
        let s = server(dir)
        _ = call(s, "plan_add", #"{"title":"等人拍板"}"#)
        let r = call(s, "plan_update", #"{"number":1,"status":"blocked"}"#)
        XCTAssertTrue(r.contains("ERROR"))
        XCTAssertTrue(r.contains("卡在人身上"))
        XCTAssertEqual(s.plans.item(crewId: "c", number: 1)?.status, "not_started")
        XCTAssertTrue(whiteboardTexts(dir).isEmpty)
    }

    func testBlockedAnnouncesOnceAndSaysWhere() {
        let dir = tempDir()
        let s = server(dir)
        _ = call(s, "plan_add", #"{"title":"等人拍板"}"#)
        let r = call(s, "plan_update", #"{"number":1,"status":"blocked","blocked_by_number":7}"#)
        XCTAssertFalse(r.contains("ERROR"))
        XCTAssertTrue(r.contains("人类 Todo #7"))
        let posts = whiteboardTexts(dir)
        XCTAssertEqual(posts.count, 1, "卡住是唯一进群的那一档，而且只发这一次")
        XCTAssertTrue(posts[0].contains("卡住"))
        XCTAssertTrue(posts[0].contains("人类 Todo #7"))
        // 已经卡着了，再追加一句进度不该再发一遍。
        _ = call(s, "plan_update", #"{"number":1,"progress":"还在等"}"#)
        XCTAssertEqual(whiteboardTexts(dir).count, 1)
    }

    /// `.human` 那本账已接线（Todo #62）—— 那本里没有 #7 就**如实说找不到了**，
    /// 不再回「未核实」（那句现在会是假话），也不静默当成「在」。
    func testDanglingHumanReferenceIsSaidOutLoud() {
        let s = server(tempDir())
        _ = call(s, "plan_add", #"{"title":"等人拍板"}"#)
        let r = call(s, "plan_update", #"{"number":1,"status":"blocked","blocked_by_number":7}"#)
        XCTAssertTrue(r.contains("人类 Todo #7"))
        XCTAssertTrue(r.contains("找不到了"))
        XCTAssertFalse(r.contains("未核实"), "已经接线了，不许再说没核实")
    }

    /// 接线接的是**注入的那本**（跟着 `--dir` 走），不是 `LocalTodoStore.shared(.human)`。
    /// 这条就是那个陷阱的守卫：写成共享实例的话，它会跳过 temp 目录去读开发机上真实
    /// 的账 —— 那时这条会红在「找不到 / 找得到」上，而不是悄悄绿着。
    func testHumanReferenceReadsTheInjectedLedger() {
        let dir = tempDir()
        let s = server(dir)
        guard let added = s.humanTodos.add(crewId: "c", text: "请人类拍板") else {
            return XCTFail("写不进注入的那本人类 Todo")
        }
        _ = call(s, "plan_add", #"{"title":"等人拍板"}"#)
        let r = call(s, "plan_update",
                     "{\"number\":1,\"status\":\"blocked\",\"blocked_by_number\":\(added.number)}")
        XCTAssertTrue(r.contains("人类 Todo #\(added.number)"))
        XCTAssertFalse(r.contains("找不到了"), "这条明明写进了注入的那本，却没被读到 —— 多半读错了目录")
        XCTAssertFalse(r.contains("未核实"))
    }

    /// 指向 `.agent` 那本（现在就查得了）：人类把那条删了，作战板要说出来，
    /// 不静默把状态改回「进行中」。
    func testDanglingAgentReferenceIsSaidOutLoud() {
        let dir = tempDir()
        let s = server(dir)
        _ = s.todos.add(crewId: "c", text: "人类派的活")
        _ = call(s, "plan_add", #"{"title":"卡在那条上"}"#)
        let ok = call(s, "plan_update",
                      #"{"number":1,"status":"blocked","blocked_by_number":1,"blocked_by_ledger":"agent"}"#)
        XCTAssertTrue(ok.contains("卡在 Todo #1"))
        XCTAssertFalse(ok.contains("找不到了"))
        _ = s.todos.delete(crewId: "c", number: 1)
        let after = call(s, "plan_list")
        XCTAssertTrue(after.contains("找不到了"))
        XCTAssertEqual(s.plans.item(crewId: "c", number: 1)?.status, "blocked", "不许替机长把状态改回去")
    }

    func testUnknownLedgerIsRejected() {
        let s = server(tempDir())
        _ = call(s, "plan_add", #"{"title":"x"}"#)
        let r = call(s, "plan_update",
                     #"{"number":1,"status":"blocked","blocked_by_number":1,"blocked_by_ledger":"roadmap"}"#)
        XCTAssertTrue(r.contains("ERROR"))
    }

    // MARK: - 撤下

    func testDropRemovesFromListButKeepsNumber() {
        let s = server(tempDir())
        _ = call(s, "plan_add", #"{"title":"排错了"}"#)
        XCTAssertTrue(call(s, "plan_update", #"{"number":1,"drop":true}"#).contains("已撤下"))
        XCTAssertTrue(call(s, "plan_list").contains("空的"))
        XCTAssertTrue(call(s, "plan_add", #"{"title":"重排"}"#).contains("#2"))
    }
}
