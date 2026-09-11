import XCTest

/// 引用**真的落在消息上**（人类 Todo #132/#133 的接线层）。
///
/// 纯层量的是「给了字段会不会算出引用」，这一层量的是
/// **「发一条消息之后，磁盘上那条消息带没带引用」** —— 中间任何一处漏传，
/// 纯层照样全绿，而胶囊一颗都不会长出来。
@MainActor
final class CrewMessageReferenceLandingTests: XCTestCase {

    private struct Fixture {
        let whiteboards: URL
        let crewId: String
    }

    private func fixture() -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcpref-\(UUID().uuidString)")
        let wb = base.appendingPathComponent("whiteboards", isDirectory: true)
        try? FileManager.default.createDirectory(at: wb, withIntermediateDirectories: true)
        let store = LocalCrewStore(baseDirectory: base)
        let crew = store.createCrew(.make(
            responsibleSubjectId: "s", title: "机组群聊体验", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        store.recordSessionMember(crewId: crew, sessionId: "sess-1", displayName: "机长")
        return Fixture(whiteboards: wb, crewId: crew)
    }

    private func server(_ f: Fixture, captain: Bool = true) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: f.whiteboards),
                  approvals: LocalApprovalStore(directory: f.whiteboards),
                  control: LocalCrewControlStore(directory: f.whiteboards),
                  crewId: f.crewId, sessionId: "sess-1",
                  isCaptain: captain, sessionLabel: "机长",
                  quotaDirectory: f.whiteboards,
                  todos: LocalTodoStore(directory: f.whiteboards, ledger: .agent),
                  plans: CockpitPlanStore(directory: f.whiteboards))
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

    // MARK: - agent 发言

    func test_标human_todo的消息带着刚建出来的那条的引用() {
        let f = fixture()
        _ = post(server(f), #"{"message":"这条要你拍板","category":"human_todo"}"#)
        XCTAssertEqual(board(f).first?.references,
                       [CrewMessageReference(.humanTodo, "1")],
                       "群里那条没挂上刚建的 #1 —— 人看到「已建 #1」却点不进去")
    }

    func test_标plan的消息带着刚排上那条计划的引用() {
        let f = fixture()
        _ = post(server(f), #"{"message":"接上引用可点","category":"plan"}"#)
        XCTAssertEqual(board(f).first?.references, [CrewMessageReference(.plan, "1")])
    }

    func test_回复和定向at会派生出消息与session两颗引用() {
        let f = fixture()
        let s = server(f)
        _ = post(s, #"{"message":"第一条","category":"note"}"#)
        guard let first = board(f).first else { return XCTFail("前置条件：先有一条") }

        _ = post(s, """
        {"message":"回你","category":"note","reply_to":"\(first.id)",\
        "mentions":[{"kind":"broadcast"},{"kind":"session","target_id":"sess-9"}]}
        """)

        XCTAssertEqual(board(f).last?.references,
                       [CrewMessageReference(.message, first.id),
                        CrewMessageReference(.session, "sess-9")],
                       "回复的那条和被 @ 的那个 session 都该点得进去")
    }

    /// **@broadcast / @captain / @human 不是引用** —— 它们指不到一个具体对象，
    /// 长一颗点了没反应的胶囊比不长更糟。
    func test_广播和at人类不产生引用() {
        let f = fixture()
        _ = post(server(f), #"{"message":"广播","category":"note","mentions":[{"kind":"broadcast"},{"kind":"human"},{"kind":"captain"}]}"#)
        XCTAssertNil(board(f).first?.references)
    }

    func test_不带任何结构化字段的消息不长引用() {
        let f = fixture()
        _ = post(server(f), #"{"message":"顺手提一句 Todo #78 和 #12","category":"note"}"#)
        XCTAssertNil(board(f).first?.references,
                     "正文里的 #78 / #12 被认出来了 —— 这正是这一单禁止的那条路")
    }

    // MARK: - 系统身份正规化不许把引用弄丢

    /// `LocalWhiteboardStore.normalizingSystemIdentity` **逐字段重建**消息。
    /// 每加一个字段都得在那儿补一行，漏了不报错、只静默丢 —— 所以这里钉一颗钉子。
    func test_系统消息经过身份正规化后引用还在() {
        let f = fixture()
        let store = LocalWhiteboardStore(directory: f.whiteboards)
        store.appendSessionMessage(
            crewId: f.crewId, sessionId: PendingCrewSystemMessage.sessionId,
            text: "系统通告", senderKind: PendingCrewSystemMessage.senderKind,
            references: [CrewMessageReference(.agentTodo, "5")])
        XCTAssertEqual(store.list(crewId: f.crewId).first?.references,
                       [CrewMessageReference(.agentTodo, "5")],
                       "系统消息过了一遍身份正规化，引用被丢掉了")
    }

    // MARK: - #136：整组状态跟着消息落盘

    func test_机长填的状态落在消息上() {
        let f = fixture()
        _ = post(server(f), #"{"message":"报一句","category":"note","crew_status":"在接引用可点"}"#)
        XCTAssertEqual(board(f).first?.crewStatus, "在接引用可点")
    }

    func test_worker填状态被拒而且消息一个字都不发() {
        let f = fixture()
        let r = post(server(f, captain: false),
                     #"{"message":"报一句","category":"note","crew_status":"我在跑全量"}"#)
        XCTAssertTrue(r.contains("ERROR") && r.contains("机长"), r)
        XCTAssertTrue(board(f).isEmpty, "被拒了却把消息发出去了")
    }

    /// **分条发送时状态每条都带**（不是只挂最后一条）。
    ///
    /// 只挂最后一条的话，执行期一半成功时它正好挂在最可能没发出去的那条上，
    /// 于是侧栏显示着上一次的旧状态、看起来像「他没报」。
    func test_分条发送时每条都带着同一句状态() {
        let f = fixture()
        _ = post(server(f), #"""
        {"messages":[{"text":"一","category":"note"},{"text":"二","category":"note"}],        "crew_status":"在接引用可点"}
        """#)
        let msgs = board(f)
        XCTAssertEqual(msgs.count, 2)
        XCTAssertEqual(msgs.map(\.crewStatus), ["在接引用可点", "在接引用可点"])
    }

    /// 顶层给 `mentions` / `reply_to` / `attachments` **会被丢掉** —— 所以要明说，
    /// 不许静默丢：回执照回「已发到」而 @ 谁都没 @ 到，是这一路最坏的形态。
    func test_顶层给每条自己的参数要整批拒并说清挪到哪() {
        let f = fixture()
        let r = post(server(f), #"""
        {"messages":[{"text":"一","category":"note"}],        "mentions":[{"kind":"session","target_id":"sess-9"}]}
        """#)
        XCTAssertTrue(r.contains("ERROR") && r.contains("mentions"), r)
        XCTAssertTrue(r.contains("挪进") || r.contains("每条"), "没说清该挪到哪：\(r)")
        XCTAssertTrue(board(f).isEmpty, "整批该一条都不发")
    }

    /// #142 接线：回执真的带上了那句提醒（纯层算得对 ≠ 它被调了）。
    func test_question没指定问谁时回执真的提醒了() {
        let f = fixture()
        let r = post(server(f), #"{"message":"这个怎么办？","category":"question"}"#)
        XCTAssertTrue(r.contains("没指定问谁"), r)
    }

    func test_question指定了人就不提醒() {
        let f = fixture()
        let r = post(server(f),
                     #"{"message":"这个怎么办？","category":"question","mentions":[{"kind":"human"}]}"#)
        XCTAssertFalse(r.contains("没指定问谁"), r)
    }
}
