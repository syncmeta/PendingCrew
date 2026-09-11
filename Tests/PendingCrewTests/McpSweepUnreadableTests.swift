import XCTest

/// **账读不出来时，不许把那个提醒关掉**（人类 Todo 侧的空闲核账，驾驶舱计划 #71）。
///
/// ## 为什么这一处比别处重
///
/// `confirm_todo_sweep` 的收尾动作就是**熄掉提醒**（「空闲时不会再提醒你」）。
/// 它原来从 `todos.list` 取真账，而 `list` 把读失败压成空表 —— 于是在账本坏掉时：
///
/// 1. `sweepOpen` 是空的 → 校验通过 →
/// 2. 回执说「这本账一条未完成都没有」→
/// 3. **那个唯一还在报信的通道被永久关掉，依据是一个假前提。**
///
/// `LocalTodoStore.LedgerRead` 的注释里点名的就是这条路
/// （「病根 2026-09-08 由『机长空闲核账』那条路暴露」）——
/// **三态读是为它建的，却一直没接到它身上**。2026-09-12 的 EPERM 断线里才发现。
@MainActor
final class McpSweepUnreadableTests: XCTestCase {

    private var base: URL!
    private var wb: URL!
    private var crewId: String!

    override func setUpWithError() throws {
        base = FileManager.default.temporaryDirectory
            .appendingPathComponent("sweep-\(UUID().uuidString)")
        wb = base.appendingPathComponent("whiteboards", isDirectory: true)
        try FileManager.default.createDirectory(at: wb, withIntermediateDirectories: true)
        let store = LocalCrewStore(baseDirectory: base)
        crewId = store.createCrew(.make(
            responsibleSubjectId: "s", title: "机组群聊体验", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
    }

    override func tearDownWithError() throws {
        for f in (try? FileManager.default.contentsOfDirectory(atPath: wb.path)) ?? [] {
            try? FileManager.default.setAttributes(
                [.posixPermissions: 0o644], ofItemAtPath: wb.appendingPathComponent(f).path)
        }
        try? FileManager.default.removeItem(at: base)
    }

    private func server() -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: wb),
                  approvals: LocalApprovalStore(directory: wb),
                  control: LocalCrewControlStore(directory: wb),
                  crewId: crewId, sessionId: "captain-x",
                  isCaptain: true, sessionLabel: "机长",
                  quotaDirectory: wb,
                  todos: LocalTodoStore(directory: wb, ledger: .agent),
                  plans: CockpitPlanStore(directory: wb))
    }

    private func sweep(_ s: McpServer) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":\
        {"name":"confirm_todo_sweep","arguments":{"running":[],"blocked_on_human":[],"queued":[]}}}
        """) ?? ""
    }

    /// 账本文件不可读 —— `chmod 000` 是 EPERM 的可移植替身。
    private func makeLedgerUnreadable() throws {
        let todos = LocalTodoStore(directory: wb, ledger: .agent)
        XCTAssertNotNil(todos.add(crewId: crewId, text: "账上本来有一条没完成"))
        let f = wb.appendingPathComponent("\(crewId!).todos.json")
        try FileManager.default.setAttributes([.posixPermissions: 0], ofItemAtPath: f.path)
        try XCTSkipIf(FileManager.default.isReadableFile(atPath: f.path),
                      "这个身份读得动 000 的文件（多半是 root），换个身份跑")
    }

    func test_账读不出来时核账要被拒而不是宣布没事() throws {
        try makeLedgerUnreadable()
        let r = sweep(server())
        XCTAssertTrue(r.contains("ERROR"), r)
        XCTAssertTrue(r.contains("读不出来"), r)
        // 判据是**「有没有记下」**，不是某个词出现没出现 ——
        // 正确的拒绝文案里本来就要引用「一条未完成都没有」来否定它，
        // 拿那个词当判据会把对的实现判成错的（第一版就是这么红的）。
        XCTAssertFalse(r.contains("记下了"),
                       "它把这次核账记下了 —— 而记下的后果就是把提醒关掉：\(r)")
        XCTAssertTrue(r.contains("提醒会继续来") || r.contains("没有记下"),
                      "必须说清提醒不会被关掉，否则机长以为自己已经收尾了：\(r)")
    }

    /// 真的一条都没有时**照旧**说没事 —— 否则一个永远拒绝的实现也能全绿。
    func test_真的空账时照旧记下并说没事() {
        let r = sweep(server())
        XCTAssertFalse(r.contains("ERROR"), r)
        XCTAssertTrue(r.contains("一条未完成都没有"), r)
    }

    /// 账上有条目、也如实报上来时照常记下 —— 这条钉住「没把正常路径一起拒掉」。
    func test_有未完成条目且报对了时照常记下() {
        let todos = LocalTodoStore(directory: wb, ledger: .agent)
        let one = todos.add(crewId: crewId, text: "一件在跑的活")
        XCTAssertNotNil(one)
        let s = server()
        let r = s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":\
        {"name":"confirm_todo_sweep","arguments":{"running":[\(one!.number)],\
        "blocked_on_human":[],"queued":[]}}}
        """) ?? ""
        XCTAssertFalse(r.contains("ERROR"), r)
        XCTAssertTrue(r.contains("已逐条归桶"), r)
    }
}
