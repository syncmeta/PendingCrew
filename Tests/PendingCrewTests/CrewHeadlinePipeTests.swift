import XCTest

/// 「作者自己写的那一行结论」这根**管子**（人类 Todo #143）。
///
/// ## 这一单为什么是「接管子」而不是「加功能」
///
/// `CrewMessageFold.fold` 一直有 `explicitSummary` 这一级，注释写着「最可靠的一级」，
/// `guard` 也在。**但整条路不存在**：`post_to_crew` 没有这个参数、白板没有这个字段、
/// 全仓 0 个调用点传它。从上线到 2026-09-11，那一级**一次都没跑过** ——
/// 每条折叠消息的标题都是猜的（取正文前 3 段的第一个粗体）。
///
/// 所以这批用例钉的是**每一节管子**，而不是折叠规则本身（那个有 `CrewMessageFoldTests`）：
/// 工具参数 → 落盘 → 白板条目 → 气泡模型 → 折叠判定。**中间任何一节漏传，
/// 折叠层照样全绿，而标题仍然是猜的。**
@MainActor
final class CrewHeadlinePipeTests: XCTestCase {

    private struct Fixture {
        let whiteboards: URL
        let crewId: String
    }

    private func fixture() -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("headline-\(UUID().uuidString)")
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

    private func server(_ f: Fixture) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: f.whiteboards),
                  approvals: LocalApprovalStore(directory: f.whiteboards),
                  control: LocalCrewControlStore(directory: f.whiteboards),
                  crewId: f.crewId, sessionId: "sess-1",
                  isCaptain: true, sessionLabel: "机长",
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

    /// 一段**第一个粗体不是结论**的正文 —— 猜法会取到「顺手」，作者要的是「闸门全绿」。
    private let wall = """
    今天这一趟有点绕，**顺手**把几个不相干的东西也看了一遍，
    中间换过两次方向，具体过程不重要。
    第三段。
    第四段。
    第五段。
    第六段。
    第七段。
    第八段。
    第九段。
    """

    // MARK: - 一节一节量

    func test_一_工具参数落到盘上() {
        let f = fixture()
        _ = post(server(f), #"{"message":"随便写点","category":"note","headline":"闸门全绿"}"#)
        XCTAssertEqual(board(f).first?.headline, "闸门全绿",
                       "参数没落到盘上 —— 后面每一节都白搭")
    }

    func test_二_空白当没写() {
        let f = fixture()
        _ = post(server(f), #"{"message":"随便写点","category":"note","headline":"   "}"#)
        XCTAssertNil(board(f).first?.headline,
                     "写个空格就算写过，是最廉价的一种假账")
    }

    func test_三_盘上的值过得了本地映射这一层() throws {
        let f = fixture()
        _ = post(server(f), #"{"message":"随便写点","category":"note","headline":"闸门全绿"}"#)
        let entry = try XCTUnwrap(board(f).first.map(CrewLocalWhiteboardMapping.entry))
        XCTAssertEqual(entry.headline, "闸门全绿")
        XCTAssertNotEqual(entry.headline, entry.summary,
                          "跟 wire 层那个 summary（正文兜底）不是一回事，别撞上")
    }

    func test_四_过得了气泡模型这一层() throws {
        let f = fixture()
        _ = post(server(f), #"{"message":"随便写点","category":"note","headline":"闸门全绿"}"#)
        let entry = try XCTUnwrap(board(f).first.map(CrewLocalWhiteboardMapping.entry))
        let (msg, _) = CrewChatAdapter.adapt(
            entry, members: [], captainBotId: nil, localUserId: nil)
        XCTAssertEqual(msg.headline, "闸门全绿")
    }

    /// 最后一节：给了就**压过**猜出来的那个。
    func test_五_给了结论就不再猜第一个粗体() throws {
        let guessed = try XCTUnwrap(CrewMessageFold.fold(wall))
        XCTAssertEqual(guessed.summary, "顺手", "前置条件：猜法确实会取到那个粗体")

        let explicit = try XCTUnwrap(
            CrewMessageFold.fold(wall, explicitSummary: "闸门全绿"))
        XCTAssertEqual(explicit.summary, "闸门全绿",
                       "作者写了结论还去猜 —— 这一单就白做了")
    }

    func test_六_没写就照旧退回猜法_老消息不受影响() throws {
        let f = fixture()
        _ = post(server(f), #"{"message":"随便写点","category":"note"}"#)
        XCTAssertNil(board(f).first?.headline)
        let stillGuesses = try XCTUnwrap(CrewMessageFold.fold(wall, explicitSummary: nil))
        XCTAssertEqual(stillGuesses.summary, "顺手")
    }

    /// 管子的**最后一节**：渲染层真的把 `headline` 喂给了折叠判定。
    ///
    /// 前面五条钉的都是「值传到了下一层」，这一条钉的是「最后那一层真的用了它」——
    /// 少了它，谁把 `explicitSummary:` 从视图里删掉，上面全绿而标题又变回猜的。
    func test_五半_渲染层真的把结论喂给了折叠判定() throws {
        let folded = try XCTUnwrap(CrewMessageFold.decideForRender(
            text: wall, headline: "闸门全绿", isStreaming: false))
        XCTAssertEqual(folded.summary, "闸门全绿")

        let guessed = try XCTUnwrap(CrewMessageFold.decideForRender(
            text: wall, headline: nil, isStreaming: false))
        XCTAssertEqual(guessed.summary, "顺手", "没写结论时仍走猜法")

        XCTAssertNil(CrewMessageFold.decideForRender(
            text: wall, headline: "闸门全绿", isStreaming: true),
            "还在吐字的不折 —— 折一个还在长的东西，人会以为它写完了")
    }

    // MARK: - 分条发送

    /// `headline` 是**每条自己的**。顶层给了会被丢掉（`Entry.args` 只有那一项自己的
    /// 字典），所以必须整批拒并说清挪到哪 —— 静默丢 + 回执照回「已发到」是最坏的形态。
    func test_七_顶层给headline要整批拒() {
        let f = fixture()
        let r = post(server(f), #"""
        {"messages":[{"text":"一","category":"note"}],"headline":"顶层写错地方了"}
        """#)
        XCTAssertTrue(r.contains("ERROR") && r.contains("headline"), r)
        XCTAssertTrue(board(f).isEmpty, "整批该一条都不发")
    }

    func test_八_分条时每条带自己的结论() {
        let f = fixture()
        // ⚠️ 一行写完：`#"""…"""#` 是**原始**字符串，行尾的 `\` 不是续行符、是一个
        // 真的反斜杠，会把 JSON 弄坏 —— 而坏掉的 JSON 表现为「回执为空、一条没发」，
        // 跟「整批被拒」长得一模一样。
        let r = post(server(f), #"{"messages":[{"text":"一","category":"note","headline":"结论甲"},{"text":"二","category":"note","headline":"结论乙"}]}"#)
        XCTAssertEqual(board(f).map(\.headline), ["结论甲", "结论乙"], "回执：\(r)")
    }

    // MARK: - 回执在出错那一刻教人（#143 的「文案真的教得会」那半）

    /// JSON 里的正文：把换行转义掉。**不用 `#"""…"""#` 拼** —— 原始字符串里
    /// 行尾的 `\` 是字面反斜杠，会把 JSON 弄坏，而坏掉的 JSON 表现为
    /// 「回执为空、一条没发」，跟「被拒」长得一模一样。
    private var wallEscaped: String {
        wall.replacingOccurrences(of: "\n", with: "\\n")
    }

    /// **schema 里的说明只在写之前被读到一次，而且多半没读。**
    /// 真正教得会人的是出错那一刻的那句话 —— 所以长消息没给结论时，
    /// 回执把**猜出来的那一行原样摆给作者看**：他一眼看出那不是他的结论。
    func test_长消息没给结论时回执把猜出来的那行摆出来() {
        let f = fixture()
        let r = post(server(f), "{\"message\":\"\(wallEscaped)\",\"category\":\"note\"}")
        XCTAssertTrue(r.contains("没给 `headline`"), "回执没提醒：\(r)")
        XCTAssertTrue(r.contains("顺手"),
                      "没把猜出来的那一行摆出来，提醒就只是一句空话：\(r)")
    }

    func test_给了结论就不提醒() {
        let f = fixture()
        let r = post(server(f),
                     "{\"message\":\"\(wallEscaped)\",\"category\":\"note\","
                     + "\"headline\":\"闸门全绿\"}")
        XCTAssertFalse(r.contains("没给 `headline`"), r)
    }

    /// 短消息不折，就别拿这句去烦人 —— **一条永远都在提醒的提醒会被忽略**。
    func test_短消息不提醒() {
        let f = fixture()
        let r = post(server(f), #"{"message":"一句话","category":"note"}"#)
        XCTAssertFalse(r.contains("没给 `headline`"), r)
    }

    func test_长但猜不出结论时也不提醒() {
        // 没有粗体、没有标题 —— `fold` 返回 nil（这条根本折不起来）。
        let plain = (1...12).map { "第 \($0) 段。" }.joined(separator: "\n")
        XCTAssertNil(CrewMessageFold.receiptHintIfGuessed(text: plain, headline: nil),
                     "猜都猜不出来时提醒没有内容可摆，等于噪音")
    }

    func test_空白结论视同没给() {
        XCTAssertNotNil(CrewMessageFold.receiptHintIfGuessed(text: wall, headline: "   "))
    }
}
