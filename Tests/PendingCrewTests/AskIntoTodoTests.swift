import XCTest

/// `ask` 不再阻塞：问题并进人类 Todo，问完接着干别的（驾驶舱计划 #75 ①）。
///
/// ## 人类原话
///
/// > 这个东西是不是很早以前就有了？是不是过时了？我觉得这个应该是 todo 才对。
/// > 至少 todo 现在已经承担了决策类了。
///
/// ## 硬证据（机长查的，我核过）
///
/// 待审批那套的 spec 是 **2026-06-08**；人类 Todo 那本账 **2026-08-25** 才引入。
/// 差两个半月 —— 待审批诞生时「agent 请人类拍板」无处可去，所以它自造了一套；
/// Todo 出现之后那套的一半职能就重复了，没人回头拆。
///
/// **而且它就是人类反复问的「怎么又停了」的一个主要来源**：`ask` 走
/// `McpServer.awaitReply(pollInterval: 0.5, maxWaits: 3600)` —— **正好 30 分钟**
/// 阻塞轮询，人不在就真的停在那儿。
///
/// ## 承重点：放下再捡起来，上下文不能丢
///
/// 不阻塞之后有个新风险，**比原来更糟**：agent 提完问题去干别的，就再也不回来做那件事了。
/// 原来至少它停在那儿等。所以「人答了之后能接回原来那件事」这条不是可选项 ——
/// 条目要记下 agent 自己写的 `resumeNote`，人回应时**原样念回去**。
///
/// ## 量得到什么、量不到什么
///
/// **量得到**：ask 会不会阻塞（真跑，不靠读代码）、问题有没有落进那本账、
/// resume 有没有被记下并原样带回。
///
/// **量不到**：agent 拿到那段 resume 之后会不会**真的**接着做。这机制只保证
/// 上下文送到它手上，不保证它用。别声称它解决了这个。
final class AskIntoTodoTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("ask-todo-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL, isCaptain: Bool = false) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1", isCaptain: isCaptain,
                  sessionLabel: "小工", quotaDirectory: dir,
                  todos: LocalTodoStore(directory: dir),
                  plans: CockpitPlanStore(directory: dir),
                  wakeups: LocalWakeupStore(directory: dir))
    }

    private func call(_ s: McpServer, _ name: String, _ args: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"\(name)","arguments":\(args)}}
        """) ?? ""
    }

    // MARK: - ① 不许再停在那儿

    /// **这条直接对着人类那句「怎么又停了」。**
    ///
    /// 故意不去读代码判断它阻不阻塞 —— 真调一次、看它多久回来。旧实现会挂 30 分钟，
    /// 所以这里放到后台跑、只等 3 秒：超时就是红，而且测试**自己不会挂死**。
    func testAskReturnsImmediatelyInsteadOfBlockingForHalfAnHour() {
        let dir = tempDir()
        let s = server(dir)
        let done = expectation(description: "ask 回来了")
        DispatchQueue.global().async {
            _ = self.call(s, "ask", #"{"question":"A 还是 B？我倾向 A"}"#)
            done.fulfill()
        }
        let outcome = XCTWaiter().wait(for: [done], timeout: 3)
        XCTAssertEqual(outcome, .completed, """
            `ask` 3 秒没回来 —— 它还在阻塞等人。人不在的时候 agent 就真的停在那儿，\
            那正是人类反复问的「怎么又停了」。问题该进人类 Todo，然后立刻返回。
            """)
    }

    // MARK: - ② 问题要落进那本账，不是另起一套列表

    func testAskFilesTheQuestionIntoTheHumanTodoLedger() {
        let dir = tempDir()
        let s = server(dir)
        let done = expectation(description: "ask 回来了")
        DispatchQueue.global().async {
            _ = self.call(s, "ask", #"{"question":"要不要先发版？"}"#)
            done.fulfill()
        }
        _ = XCTWaiter().wait(for: [done], timeout: 3)

        let todos = LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c")
        XCTAssertEqual(todos.count, 1, "问题没进人类 Todo —— 那本账才是人会去翻的地方")
        XCTAssertTrue(todos.first?.text.contains("要不要先发版？") ?? false,
                      "条目里没有原问题")
        XCTAssertEqual(todos.first?.createdBySessionId, "sess-1",
                       "没记下是谁提的 —— 人回应时就不知道该叫醒谁（HumanTodoWakePlan 靠它）")
    }

    /// 回执要告诉 agent 两件事：**记在哪了**、**接下来该干嘛**。
    /// 少了后半句，它拿到一个「已记录」就可能真的停在那儿等 —— 白改。
    func testAskReceiptTellsTheAgentToGoDoSomethingElse() {
        let dir = tempDir()
        let s = server(dir)
        let done = expectation(description: "ask 回来了")
        var reply = ""
        DispatchQueue.global().async {
            reply = self.call(s, "ask", #"{"question":"X 还是 Y？"}"#)
            done.fulfill()
        }
        _ = XCTWaiter().wait(for: [done], timeout: 3)
        XCTAssertTrue(reply.contains("#1"), "回执没说记成了哪一条，agent 没法引用它")
        XCTAssertFalse(reply.contains("暂无人响应"),
                       "还在走那条 30 分钟超时的话术 —— 那说明阻塞路径没拆")
    }

    // MARK: - ③ 承重点：放下再捡起来

    func testAskRecordsWhatToResumeAfterTheAnswer() {
        let dir = tempDir()
        let s = server(dir)
        let done = expectation(description: "ask 回来了")
        DispatchQueue.global().async {
            _ = self.call(s, "ask",
                #"{"question":"用 A 还是 B？","resume_note":"我正在改 Foo.swift 第 3 步，答复回来后接着把 B 分支删掉"}"#)
            done.fulfill()
        }
        _ = XCTWaiter().wait(for: [done], timeout: 3)

        let todos = LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c")
        XCTAssertTrue(todos.first?.isMidFlowAsk ?? false,
                      "ask 提的条目没标成「半路上问的」—— 那 resume 为空时就没人会说出来")
        XCTAssertEqual(todos.first?.resumeNote,
                       "我正在改 Foo.swift 第 3 步，答复回来后接着把 B 分支删掉",
                       """
                       条目没记下「答复回来后接着干什么」。不阻塞之后这一条是承重点：\
                       没有它，agent 提完问题去干别的就再也不回来了 —— 那比原来停在那儿还糟。
                       """)
    }

    /// **人回应时那段 resume 必须原样回到 agent 手上。** 记下来却不送回去等于没记。
    func testTheWakeTextCarriesTheResumeNoteVerbatim() {
        let text = TodoLandingFlow.wakeText(
            announce: "回应 人类 To Do #3：按 A 办",
            fallbackNote: nil,
            resumeNote: "我正在改 Foo.swift 第 3 步，接着把 B 分支删掉", expectsResume: true)
        XCTAssertTrue(text.contains("回应 人类 To Do #3：按 A 办"), "答复本身丢了")
        XCTAssertTrue(text.contains("我正在改 Foo.swift 第 3 步，接着把 B 分支删掉"),
                      "agent 自己写的 resume 没被念回去 —— 它被叫醒了却不知道要接着做什么")
    }

    /// 绝大多数条目走 `add_human_todo`（agent 本来就没在半路上），**不该被加料**。
    func testWakeTextForAPlainTodoIsUnchanged() {
        let text = TodoLandingFlow.wakeText(
            announce: "回应 人类 To Do #3：按 A 办", fallbackNote: nil,
            resumeNote: nil, expectsResume: false)
        XCTAssertEqual(text, "回应 人类 To Do #3：按 A 办",
                       "普通 Todo 的答复被加了料 —— 那是绝大多数条目，不该变样")
    }

    /// 但 `ask` 那条**按定义就是在半路上问的**。它没写 resume 时不能装作没这回事：
    /// 叫醒了、却不知道从哪儿接，跟没叫醒差不多，而且**没有任何人会发现**。
    func testWakeTextSaysSoWhenAMidFlowAskLeftNoResume() {
        let text = TodoLandingFlow.wakeText(
            announce: "回应 人类 To Do #3：按 A 办", fallbackNote: nil,
            resumeNote: nil, expectsResume: true)
        XCTAssertNotEqual(text, "回应 人类 To Do #3：按 A 办",
                          """
                          半路上问的问题被答复了，却没有任何一句说明「它当时没写要接着做什么」。\
                          空值静默通过 = agent 被叫醒后不知道从哪儿接，而没有人会发现。
                          """)
        XCTAssertTrue(text.contains("回应 人类 To Do #3：按 A 办"), "答复本身丢了")
    }

    func testWakeTextKeepsTheFallbackNote() {
        let text = TodoLandingFlow.wakeText(
            announce: "回应 人类 To Do #3：按 A 办",
            fallbackNote: "（提问的 session 已退出，请机长转达）",
            resumeNote: "接着做第 3 步", expectsResume: true)
        XCTAssertTrue(text.contains("请机长转达"),
                      "回落原因被 resume 挤掉了 —— 那条丢了机长不知道该自己办还是转达")
    }

    // MARK: - ④ 旧的阻塞路径必须真的拆掉，不是并存

    /// ⚠️ 扫的是**代码**，注释剥掉。拆掉一条老路时**正该在原地留注释说明它去哪了**，
    /// 而那段注释里必然写着老路的名字。不剥注释的话，尺子会逼着人删掉最该留的那段话
    /// —— 今天这是第三次撞同一个形状了。
    func testAskNoLongerGoesThroughTheBlockingApprovalPath() throws {
        let source = Self.codeOnly(try Self.text(of: "McpServer.swift"))
        guard let ask = source.range(of: "case \"ask\":") else {
            return XCTFail("找不到 ask 的 handler —— 先修测试")
        }
        // 只切 ask 自己那一块（到下一个顶层 case 为止）—— 第一版切了固定 2600 字，
        // 一路切进了隔壁 case，红的是切法不是代码。
        let after = source[ask.upperBound...]
        let body = after.range(of: "\n        case \"").map { String(after[..<$0.lowerBound]) }
            ?? String(after)
        XCTAssertFalse(body.contains("awaitReply("),
                       "ask 还在调 awaitReply —— 那就是那条 30 分钟阻塞，没拆")
        XCTAssertFalse(body.contains("kind: \"decision\""),
                       "ask 还在往待决策列表 raise —— 决策类该并进 Todo，不是两套并存")
        XCTAssertFalse(source.contains("\"name\": \"answer_decision\""),
                       """
                       `answer_decision` 还在。ask 不再产生待决策了，它从此**永远找不到目标**——\
                       留着等于在机长的世界观里继续教它用一个死工具。
                       """)
    }

    /// 新字段**不许写成非可选带默认值**：Swift 合成的 Decodable 不用属性默认值，
    /// 那样会让这次改动之前落盘的每一条 Todo 当场解不开、账看起来是空的。
    /// 加这一条之前既有的 `TodoLedgerIsolationTests` 已经抓到过一次。
    func testOldRowsWithoutTheNewFieldsStillDecode() throws {
        let dir = tempDir()
        let old = """
        [{"id":"a","number":1,"text":"老条目","status":"pending","createdAt":"2026-08-01T00:00:00Z","responses":[]}]
        """
        try Data(old.utf8).write(
            to: dir.appendingPathComponent("c\(TodoLedger.human.fileSuffix)"))
        let rows = LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c")
        XCTAssertEqual(rows.count, 1, "加了新字段之后，旧条目解不动了 —— 整本账会看起来是空的")
        XCTAssertEqual(rows.first?.isMidFlowAsk, false, "老数据该按「不是半路上问的」处理")
        XCTAssertNil(rows.first?.resumeNote)
    }

    /// 记下来却不送回去 = 等于没记。这条钉住那段 resume 真的接到了人回应那条路上。
    func testTheRespondPathActuallyUsesTheWakeTextComposer() throws {
        let respond = Self.codeOnly(try Self.text(of: "CrewHumanTodoRespond.swift"))
        XCTAssertTrue(respond.contains("TodoLandingFlow.wakeText("),
                      "人回应那条路没走 wakeText —— resume 记下了但永远送不到 agent 手上")
        XCTAssertTrue(respond.contains("item.resumeNote"),
                      "没把条目上的 resume 喂进去")
        XCTAssertTrue(respond.contains("item.isMidFlowAsk"),
                      "没把「是不是半路上问的」喂进去 —— 那 resume 为空时就没人会说出来")
    }

    private static func codeOnly(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    private static func text(of fileName: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录") }
        for case let url as URL in walker where url.lastPathComponent == fileName {
            return try String(contentsOf: url, encoding: .utf8)
        }
        throw XCTSkip("找不到 \(fileName)")
    }
}
