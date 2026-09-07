import XCTest

/// 提出者撤回自己那条人类 Todo（Todo #102）。
///
/// 这扇门存在的理由：一条人类 Todo 最常见的死法是**世界变了**（版本发出去了、站
/// 上线了、那条线被别的决定取代了），而能判断世界变没变的只有当初提的那一方。
/// 门开之前，提出者明知道自己那条已作废，也只能看着它挂在人的账上亮灯。
///
/// 所以这里钉的不是「能不能撤」，是几件撤错了就会伤人的事：
/// 只能撤自己提的、原因必填、**撤完不许静默消失**（条目留着 + 群里那行）、
/// 撤不动时要说得出**为什么**撤不动。
final class HumanTodoWithdrawTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("todo-withdraw-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func store(_ dir: URL) -> LocalTodoStore {
        LocalTodoStore(directory: dir, ledger: .human)
    }

    @discardableResult
    private func seed(_ s: LocalTodoStore, text: String = "要不要发布？",
                      by sessionId: String? = "sess-1",
                      name: String? = "机长") -> LocalTodoItem {
        s.add(crewId: "c", text: text, bySessionId: sessionId, bySenderName: name)!
    }

    // MARK: - 撤得动的那条路

    func testWithdrawStopsCountingAsUnansweredButKeepsTheItem() {
        let s = store(tempDir())
        let item = seed(s)
        XCTAssertTrue(item.isUnanswered)

        guard case .withdrawn(let after) = s.withdraw(
            crewId: "c", number: item.number, sessionId: "sess-1",
            senderName: "机长", reason: "0.1.25 已经发出去了，这条问的就是要不要发") else {
            return XCTFail("该撤得动")
        }
        XCTAssertNotNil(after.withdrawnAt)
        XCTAssertFalse(after.isUnanswered, "撤回后不该再算等人回应")

        // **留在列表里** —— 撤回不是删除。人有权看见你撤了什么。
        let live = s.list(crewId: "c")
        XCTAssertEqual(live.map(\.number), [item.number])
        XCTAssertFalse(live[0].isDeleted)
    }

    func testReasonLandsOnTheItemTimeline() {
        // 原因不能只活在群聊里 —— 群消息会被刷走，条目不会。
        let s = store(tempDir())
        let item = seed(s)
        _ = s.withdraw(crewId: "c", number: item.number, sessionId: "sess-1",
                       senderName: "机长", reason: "站已经上线了")
        let after = s.item(crewId: "c", number: item.number)
        XCTAssertEqual(after?.responses.count, 1)
        XCTAssertTrue(after?.responses.first?.text.contains("站已经上线了") == true)
        XCTAssertEqual(after?.withdrawnBySessionId, "sess-1")
    }

    // MARK: - 撤不动的那几条路，每条要说得出不同的话

    func testCannotWithdrawSomeoneElsesTodo() {
        let s = store(tempDir())
        let item = seed(s, by: "sess-1", name: "机长")
        guard case .notYours(let owner) = s.withdraw(
            crewId: "c", number: item.number, sessionId: "sess-2",
            reason: "我觉得没用了") else {
            return XCTFail("别人提的不许撤")
        }
        XCTAssertEqual(owner, "机长", "要说得出这条是谁提的，不然对方不知道该找谁")
        XCTAssertTrue(s.item(crewId: "c", number: item.number)!.isUnanswered, "账不许被动过")
    }

    func testLegacyItemWithoutOwnerCannotBeWithdrawn() {
        // 老条目没记提出者 —— **不许撤**。撤不掉只是不方便；撤错了是把人正在等的
        // 一件事从他眼前拿走。拿不准归属时一律不动。
        let s = store(tempDir())
        let item = seed(s, by: nil, name: nil)
        guard case .notYours(let owner) = s.withdraw(
            crewId: "c", number: item.number, sessionId: "sess-1", reason: "过期了") else {
            return XCTFail("记不到提出者就不该撤得动")
        }
        XCTAssertNil(owner)
        XCTAssertTrue(s.item(crewId: "c", number: item.number)!.isUnanswered)
    }

    func testEmptyReasonIsRefusedAndChangesNothing() {
        let s = store(tempDir())
        let item = seed(s)
        XCTAssertEqual(s.withdraw(crewId: "c", number: item.number,
                                  sessionId: "sess-1", reason: "   "), .reasonRequired)
        XCTAssertTrue(s.item(crewId: "c", number: item.number)!.isUnanswered)
    }

    func testUnknownNumberIsNotFound() {
        let s = store(tempDir())
        seed(s)
        XCTAssertEqual(s.withdraw(crewId: "c", number: 99,
                                  sessionId: "sess-1", reason: "过期"), .notFound)
    }

    func testSecondWithdrawIsIdempotentAndSaysSo() {
        let s = store(tempDir())
        let item = seed(s)
        _ = s.withdraw(crewId: "c", number: item.number, sessionId: "sess-1", reason: "过期了")
        guard case .alreadyWithdrawn = s.withdraw(
            crewId: "c", number: item.number, sessionId: "sess-1", reason: "又过期了") else {
            return XCTFail("撤过的再撤该说撤过了，不该重复动账、也不该再发一行群消息")
        }
        XCTAssertEqual(s.item(crewId: "c", number: item.number)?.responses.count, 1)
    }

    func testHumanDeletedItemCannotBeWithdrawn() {
        let s = store(tempDir())
        let item = seed(s)
        XCTAssertTrue(s.delete(crewId: "c", number: item.number))
        XCTAssertEqual(s.withdraw(crewId: "c", number: item.number,
                                  sessionId: "sess-1", reason: "过期"), .notFound)
    }

    func testWithdrawnMarkerAloneExtinguishesUnanswered() {
        // **这条是补出来的，而且是被证伪补出来的。**
        //
        // 上面那些测试全绿的时候，我把 `isUnanswered` 里的 `withdrawnAt == nil`
        // 整条删掉重跑 —— **21 条一条没红**。原因：`withdraw` 顺手落了一条写着
        // 原因的回应，`responses.isEmpty` 已经把 `isUnanswered` 压成 false 了，
        // 那个判据被另一个判据挡在后面，测不着。
        //
        // 所以这里直接钉最裸的那一面：**只有撤回标记、没有任何回应**时也必须熄灭。
        // 它守的是「哪天有人改成不落那条回应了，撤过的条目会悄悄重新亮灯」。
        let item = LocalTodoItem(
            id: "i", number: 1, text: "要不要发布？", status: "pending",
            createdAt: "2026-09-07T00:00:00Z", responses: [],
            withdrawnAt: "2026-09-07T01:00:00Z")
        XCTAssertFalse(item.isUnanswered)

        var notWithdrawn = item
        notWithdrawn.withdrawnAt = nil
        XCTAssertTrue(notWithdrawn.isUnanswered, "没撤的该照常算未回应，否则上一句证明不了什么")
    }

    // MARK: - 黄点/收敛那一面

    func testWithdrawnItemDropsOutOfTheAttentionCount() {
        // 侧栏黄点和总机长视图第①段都数 `isUnanswered` —— 撤回要真的让那个数掉下去，
        // 否则这扇门开了也不解决「太多太乱」。
        let s = store(tempDir())
        let a = seed(s, text: "第一条")
        let b = seed(s, text: "第二条")
        XCTAssertEqual(s.list(crewId: "c").filter(\.isUnanswered).count, 2)
        _ = s.withdraw(crewId: "c", number: a.number, sessionId: "sess-1", reason: "过期了")
        let rest = s.list(crewId: "c").filter(\.isUnanswered)
        XCTAssertEqual(rest.map(\.number), [b.number])
    }

    // MARK: - 落地剧本

    func testWithdrawTerminatesAtAnnouncedNotWoke() {
        // 撤回不叫醒任何人：它是**减少**一件待办，为此把人叫过来看一句「你不用管了」
        // 本身就是新的打扰。
        XCTAssertEqual(TodoLandingFlow.terminal(.withdrawn), .announced)
        XCTAssertEqual(TodoLandingFlow.terminal(.added), .announced)
        XCTAssertEqual(TodoLandingFlow.terminal(.responded), .woke)
    }

    func testWithdrawMentionsHumanOnly() {
        XCTAssertEqual(TodoLandingFlow.mentions(.withdrawn).map(\.kind), ["human"])
    }

    func testReceiptRefusesToClaimSuccessWhenTheGroupLineFailed() {
        // 落了账、群里没吱声 —— 回执必须带警示。没有那行群消息，撤回就是静默消失。
        let r = TodoLandingFlow.receipt(ledger: .human, action: .withdrawn,
                                        number: 7, reached: .persisted, detail: "写失败")
        XCTAssertTrue(r.contains("群里那行没发出去"))
    }

    func testNotPersistedReceiptSaysTheItemIsStillWaiting() {
        // 撤回没落上 = 那条还在人的账上等他 —— 这句必须说出来，不然 agent 会以为
        // 自己清干净了。
        let r = TodoLandingFlow.notPersistedReceipt(ledger: .human, action: .withdrawn)
        XCTAssertTrue(r.contains("仍然挂在人的账上"))
    }

    func testAnnouncementCarriesTheReason() {
        let line = TodoLedger.human.withdrawAnnouncement(number: 3, reason: "0.1.25 已发")
        XCTAssertEqual(line, "撤回 人类 To Do #3：0.1.25 已发")
    }

    // MARK: - MCP 那一层

    private func server(_ dir: URL, sessionId: String = "sess-1",
                        label: String? = "机长") -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: sessionId, isCaptain: false,
                  sessionLabel: label, todos: LocalTodoStore(directory: dir))
    }

    private func call(_ s: McpServer, _ args: [String: Any]) -> String {
        let json = String(data: try! JSONSerialization.data(withJSONObject: args),
                          encoding: .utf8)!
        return s.handleLine("""
            {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"withdraw_human_todo","arguments":\(json)}}
            """) ?? ""
    }

    func testToolIsAvailableToWorkersNotJustCaptain() throws {
        // 谁提的谁撤 —— worker 提的条目只有 worker 撤得动，所以这个工具不能是机长专用。
        let r = try XCTUnwrap(server(tempDir())
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#))
        XCTAssertTrue(r.contains("withdraw_human_todo"))
    }

    func testMcpWithdrawPostsTheGroupLineWithTheReason() {
        let dir = tempDir()
        let s = server(dir)
        let item = seed(store(dir))
        let receipt = call(s, ["number": item.number, "reason": "0.1.25 已发"])
        XCTAssertTrue(receipt.contains("已撤回"), receipt)

        let posted = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertTrue(posted.contains { $0.text.contains("撤回 人类 To Do #\(item.number)：0.1.25 已发") },
                      "群里必须留下带原因的那一行 —— 没有它，撤回就是静默消失")
    }

    func testMcpRefusesSomeoneElsesTodoAndNamesTheOwner() {
        let dir = tempDir()
        let item = seed(store(dir), by: "sess-other", name: "别人")
        let receipt = call(server(dir, sessionId: "sess-1"),
                           ["number": item.number, "reason": "我看它没用了"])
        XCTAssertTrue(receipt.contains("不是你提的"), receipt)
        XCTAssertTrue(receipt.contains("别人"), "得说出提出者是谁，否则对方不知道该找谁")
        XCTAssertTrue(LocalTodoStore(directory: dir, ledger: .human)
            .item(crewId: "c", number: item.number)!.isUnanswered)
    }

    func testMcpMissingNumberIsRejectedBeforeTouchingTheLedger() {
        let dir = tempDir()
        let item = seed(store(dir))
        XCTAssertTrue(call(server(dir), ["reason": "过期"]).contains("number 必填"))
        XCTAssertTrue(LocalTodoStore(directory: dir, ledger: .human)
            .item(crewId: "c", number: item.number)!.isUnanswered)
    }

    func testMcpEmptyReasonIsRejected() {
        let dir = tempDir()
        let item = seed(store(dir))
        let receipt = call(server(dir), ["number": item.number, "reason": "  "])
        XCTAssertTrue(receipt.contains("reason 不能为空"), receipt)
    }

    func testMcpUnknownNumberListsWhatIsStillOpen() {
        // 撤不动时把还开着的条目列出来 —— agent 号写错时能自己改对，不用瞎试。
        let dir = tempDir()
        let item = seed(store(dir), text: "要不要发布？")
        let receipt = call(server(dir), ["number": item.number + 5, "reason": "过期"])
        XCTAssertTrue(receipt.contains("什么都没改"), receipt)
        XCTAssertTrue(receipt.contains("要不要发布？"), receipt)
    }

    func testMcpDescriptionSaysItIsNotDeletionAndOwnerOnly() {
        // 这两句是这扇门的安全带：一句挡住「拿它清理我不想答的事」，
        // 一句挡住「以为撤了就没人看得见了」。
        let r = (try? XCTUnwrap(server(tempDir())
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#))) ?? ""
        XCTAssertTrue(r.contains("只能撤自己提的"))
        XCTAssertTrue(r.contains("不是删除"))
    }
}
