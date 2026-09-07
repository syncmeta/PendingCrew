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
                  sessionLabel: "机长", quotaDirectory: dir,
                  todos: LocalTodoStore(directory: dir),
                  plans: CockpitPlanStore(directory: dir),
                  wakeups: LocalWakeupStore(directory: dir))
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

    // MARK: - 督办租约（人类 Todo #107：别人停了我不知道）

    private func leaseCommands(_ dir: URL) -> [CrewCommand] {
        LocalCrewControlStore(directory: dir).drainCommands()
            .filter { $0.planNumber != nil }
    }

    func testPlanAddCanAttachASupervisionLease() {
        let dir = tempDir()
        let s = server(dir)
        let out = call(s, "plan_add", #"{"title":"派 codex 修丢 brief","supervise_after_minutes":40}"#)
        XCTAssertTrue(out.contains("#1"))
        XCTAssertTrue(out.contains("督办"), "回执要说清挂上了：\(out)")
        let cmds = leaseCommands(dir)
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds.first?.planNumber, 1)
        XCTAssertEqual(cmds.first?.sessionId, "sess-1", "只叫醒持有这笔委托的那一个 session")
        XCTAssertEqual(cmds.first?.leaseBaseSeconds ?? 0, 40 * 60, accuracy: 1)
    }

    /// 默认不挂 —— 不给参数就是普通排一条活，一个唤醒都不该产生。
    func testPlanAddWithoutTheParameterAttachesNoLease() {
        let dir = tempDir()
        _ = call(server(dir), "plan_add", #"{"title":"随手排一条"}"#)
        XCTAssertTrue(leaseCommands(dir).isEmpty)
    }

    func testPlanUpdateCanAttachALeaseWhenHandingWorkOff() {
        let dir = tempDir()
        let s = server(dir)
        _ = call(s, "plan_add", #"{"title":"某条活"}"#)
        _ = leaseCommands(dir)  // 清掉 plan_add 那一批（这里应为空）
        let out = call(s, "plan_update",
                       #"{"number":1,"status":"in_progress","progress":"交给 worker 了","supervise_after_minutes":40}"#)
        XCTAssertTrue(out.contains("督办"), out)
        XCTAssertEqual(leaseCommands(dir).first?.planNumber, 1)
    }

    /// **命门**：不许有「我知道了 / 已查看 / 顺延」任何一种动作。
    /// 已经挂着督办的计划再挂一次 = 顺延，必须拒；解除只有翻状态一条路。
    func testAnExistingLeaseCannotBeRenewedOrSnoozed() {
        let dir = tempDir()
        let s = server(dir)
        _ = call(s, "plan_add", #"{"title":"某条活","supervise_after_minutes":40}"#)
        // app 侧登记后账本上就有这一条了（这里直接摆上，等价于命令已被排空执行）。
        LocalWakeupStore(directory: dir).register(LocalWakeupStore.PendingWakeup(
            id: SupervisionLease.id(crewId: "c", planNumber: 1), crewId: "c", sessionId: "sess-1",
            fireAt: "2099-01-01T00:00:00Z", note: "-", planNumber: 1,
            leaseSince: "2026-09-07T00:00:00Z", leaseBaseSeconds: 2400, leaseStep: 0))
        let out = call(s, "plan_update", #"{"number":1,"progress":"还在看","supervise_after_minutes":90}"#)
        XCTAssertTrue(out.contains("ERROR"), out)
        XCTAssertTrue(out.contains("顺延"), "要明说这条路不存在：\(out)")
        XCTAssertTrue(out.contains("done") || out.contains("完成"), "要指出唯一的解除路径：\(out)")
    }

    /// 工具面上根本不该出现「消音 / 已读 / 顺延」这类**参数或工具** —— 一旦有，
    /// 督办必然退化成「看一眼就算办完」的仪式。
    ///
    /// ⚠️ 量的是**工具面上的动作**（参数名 / 工具名），不是文案里的词：说明文字
    /// 里恰恰要写「没有『我知道了』这种动作」，按词禁会把这句话也禁掉。
    func testPlanToolsExposeNoAcknowledgeOrSnoozeAction() {
        let r = server(tempDir(), isCaptain: true)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        for forbidden in ["supervise_ack", "acknowledge", "snooze",
                          "dismiss_supervision", "supervise_cancel", "supervise_extend"] {
            XCTAssertFalse(r.contains(forbidden), "不许给「我知道了」这个动作：\(forbidden)")
        }
        XCTAssertTrue(r.contains("supervise_after_minutes"), "挂得上")
        XCTAssertTrue(r.contains("解除只有一条路"), "而且解除条件要写在参数说明里")
    }

    /// 参数不合法 → 在**动账本之前**就拒，不许留下一条排好了却没挂上督办的计划。
    func testInvalidLeaseParameterIsRefusedBeforeTheBoardIsTouched() {
        let dir = tempDir()
        let s = server(dir)
        let out = call(s, "plan_add", #"{"title":"这条不该被排上","supervise_after_minutes":1}"#)
        XCTAssertTrue(out.contains("ERROR"), out)
        XCTAssertTrue(s.plans.list(crewId: "c").isEmpty, "拒了就不该留下半截状态")
        XCTAssertTrue(leaseCommands(dir).isEmpty)
    }

    /// 督办**不写白板** —— 白板是给人看的，不是闹钟。挂上的那一刻群里也不该多一条。
    func testAttachingALeaseWritesNothingToTheWhiteboard() {
        let dir = tempDir()
        _ = call(server(dir), "plan_add", #"{"title":"某条活","supervise_after_minutes":40}"#)
        XCTAssertTrue(whiteboardTexts(dir).isEmpty, "\(whiteboardTexts(dir))")
    }
}
