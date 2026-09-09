import XCTest

/// 撤回一条人类 Todo（Todo #102；2026-09-08 机长那道闸）。
///
/// 这扇门存在的理由：一条人类 Todo 最常见的死法是**世界变了**（版本发出去了、站
/// 上线了、那条线被别的决定取代了），而能判断世界变没变的只有 agent 这一侧。
/// 门开之前，明知道那条已作废，也只能看着它挂在人的账上亮灯。
///
/// 所以这里钉的不是「能不能撤」，是几件撤错了就会伤人的事：
/// 撤得动的只有提出者本人和**本 crew 的机长**、原因必填、
/// **撤完不许静默消失**（条目留着 + 群里那行）、撤不动时要说得出**为什么**撤不动。
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
        XCTAssertTrue(r.contains("群里那行"))
        XCTAssertTrue(r.contains(WriteReceipt.notWrittenMarker), "实得：\(r)")
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
                        label: String? = "机长",
                        isCaptain: Bool = false) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: sessionId, isCaptain: isCaptain,
                  sessionLabel: label,
                  quotaDirectory: dir,
                  todos: LocalTodoStore(directory: dir))
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

    // MARK: - supersede（Todo #102 第二刀）

    private func add(_ s: McpServer, _ args: [String: Any]) -> String {
        let json = String(data: try! JSONSerialization.data(withJSONObject: args),
                          encoding: .utf8)!
        return s.handleLine("""
            {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"add_human_todo","arguments":\(json)}}
            """) ?? ""
    }

    func testSupersedeAddsTheNewOneAndWithdrawsTheOld() {
        let dir = tempDir()
        let s = server(dir)
        let old = seed(store(dir), text: "0.1.24 要不要发？")
        let receipt = add(s, ["text": "0.1.25 要不要发？", "supersedes": old.number])
        XCTAssertTrue(receipt.contains("同时撤回了旧的 #\(old.number)"), receipt)

        let ledger = LocalTodoStore(directory: dir, ledger: .human)
        let open = ledger.list(crewId: "c").filter(\.isUnanswered)
        XCTAssertEqual(open.count, 1, "人只该看到一条在等他")
        XCTAssertEqual(open.first?.text, "0.1.25 要不要发？")
        XCTAssertNotNil(ledger.item(crewId: "c", number: old.number)?.withdrawnAt)
    }

    func testSupersedeReasonNamesTheReplacement() {
        // 「被 #M 取代」必须写出 M —— 人回头看群聊/时间线要能顺着号找到新的那条。
        let dir = tempDir()
        let s = server(dir)
        let old = seed(store(dir))
        _ = add(s, ["text": "新的问法", "supersedes": old.number])
        let posted = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertTrue(posted.contains { $0.text.hasPrefix("撤回 人类 To Do #\(old.number)：被 #") },
                      posted.map(\.text).joined(separator: " | "))
    }

    func testSupersedeIsRefusedWholesaleWhenTargetIsUnknown() {
        // 验不过就**整件事都不做** —— 不留「新的加了、旧的还挂着」这种半截状态。
        let dir = tempDir()
        let s = server(dir)
        let receipt = add(s, ["text": "新的问法", "supersedes": 42])
        XCTAssertTrue(receipt.contains("新条目也没有加"), receipt)
        XCTAssertTrue(LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c").isEmpty)
    }

    func testSupersedeIsRefusedWhenTargetIsSomeoneElses() {
        let dir = tempDir()
        let old = seed(store(dir), by: "sess-other", name: "别人")
        let receipt = add(server(dir, sessionId: "sess-1"),
                          ["text": "我来重提", "supersedes": old.number])
        XCTAssertTrue(receipt.contains("只能取代自己提的"), receipt)
        let ledger = LocalTodoStore(directory: dir, ledger: .human)
        XCTAssertEqual(ledger.list(crewId: "c").count, 1, "新条目不许落下")
        XCTAssertTrue(ledger.item(crewId: "c", number: old.number)!.isUnanswered)
    }

    func testSupersedeIsRefusedWhenTargetAlreadyWithdrawn() {
        let dir = tempDir()
        let ledger = store(dir)
        let old = seed(ledger)
        _ = ledger.withdraw(crewId: "c", number: old.number, sessionId: "sess-1", reason: "过期")
        let receipt = add(server(dir), ["text": "再提一次", "supersedes": old.number])
        XCTAssertTrue(receipt.contains("已经撤回过了"), receipt)
        XCTAssertEqual(LocalTodoStore(directory: dir, ledger: .human).list(crewId: "c").count, 1)
    }

    func testPlainAddStillWorksWithoutSupersedes() {
        // supersede 是可选的，别把普通新增带坏。
        let dir = tempDir()
        let receipt = add(server(dir), ["text": "一件新事"])
        XCTAssertTrue(receipt.contains("已记入人类 Todo #1"), receipt)
        XCTAssertFalse(receipt.contains("撤回"), receipt)
    }

    func testWithdrawObstacleIsTheSingleJudgeForBothDoors() {
        // 撤回和 supersede 的目标校验共用这一份纯判据 —— 各写一套迟早分叉。
        let item = LocalTodoItem(id: "i", number: 1, text: "t", status: "pending",
                                 createdAt: "2026-09-07T00:00:00Z",
                                 createdBySessionId: "sess-1")
        XCTAssertNil(LocalTodoStore.withdrawObstacle(item: item, sessionId: "sess-1"))
        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: item, sessionId: "sess-2"),
                       .notYours(owner: nil))
        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: nil, sessionId: "sess-1"), .notFound)

        var gone = item
        gone.deletedAt = "2026-09-07T01:00:00Z"
        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: gone, sessionId: "sess-1"), .notFound)
    }

    func testSupersedesDescriptionTellsAgentsNotToWriteItInProse() {
        // 这句是这把刀的全部意义：写在正文里没有任何东西会去执行它。
        let r = (try? XCTUnwrap(server(tempDir())
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#))) ?? ""
        XCTAssertTrue(r.contains("别在正文里写"))
    }

    func testMcpDescriptionKeepsTheTwoSafetyBelts() throws {
        // 这两句是这扇门的安全带：一句挡住「拿它清理我不想答的事」，
        // 一句挡住「以为撤了就没人看得见了」。**权限放宽之后它们更要紧** ——
        // 撤得动的东西变多了，「这不是删除、人有权追问」才是那道兜底。
        let d = try toolDescription("withdraw_human_todo")
        XCTAssertTrue(d.contains("不是删除"), d)
        XCTAssertTrue(d.contains("那是人的账不是你的"), d)
        XCTAssertTrue(d.contains("只能撤本 crew 的"), "跨 crew 那条边界不许在改文案时丢掉")
    }

    // MARK: - 机长撤得动本 crew 的任何一条（人类原话：「我希望机长能处理所有的 todo，
    //         不要出现这种撤不掉的情况」）
    //
    // ## 这几条钉的是什么
    // 原来的判据是「你是不是提出者」，判等的对象是 `createdBySessionId`。
    // **session 是会消失的实体**（后台重启、正常收工、被停掉都会带走它），
    // 而权限被挂在了它上面 —— 于是至少三类条目永远撤不掉：提出者已经没了的、
    // 老得根本没记提出者的、别人代提的。今天真撞上了：一个子 crew 有两条已经
    // 被人当面拍板作废的人类 Todo，谁也撤不掉，只能一直在人的待办里亮着灯，
    // 催他答一个他已经答过的问题。
    //
    // 新判据是「你是不是这个 crew 的机长」—— **机长是常驻角色**。
    // 把永久性的权限挂在会消失的东西上，就是这个 bug 的形状本身。
    //
    // 下面两条红是分开写的，因为它们在旧代码里走的是**不同分支**：
    // 一条死在 `owner == sessionId` 的判等上，一条死在 `guard let owner` 的解包上。
    // 一条测试盖不住两条分支。

    func testCaptainCanWithdrawWhenTheAuthorSessionIsGone() {
        // 类型①：提出者 session 已经不存在了。**store 看不见 session 的死活，
        // 也不需要看见** —— 判据是「我是不是机长」，不是「那个 session 还在不在」。
        // 这里用一个此刻绝不会再出现的 sessionId 表示「它已经没了」。
        let s = store(tempDir())
        let item = seed(s, by: "sess-已经没了", name: "某个已退出的 session")

        guard case .withdrawn(let after) = s.withdraw(
            crewId: "c", number: item.number, sessionId: "captain-新的一轮",
            senderName: "机长", reason: "人类今天当面答过了，这条作废", isCaptain: true) else {
            return XCTFail("机长该撤得动本 crew 的任何一条")
        }
        XCTAssertNotNil(after.withdrawnAt)
        XCTAssertFalse(after.isUnanswered, "撤完不该再算等人回应，否则灯还亮着")
    }

    func testCaptainCanWithdrawALegacyItemWithNoRecordedAuthor() {
        // 类型②：`createdBySessionId` 是后来加的字段，早期条目上根本没有 ——
        // 判等永远不成立，这类条目在旧代码里**任何人**都撤不掉。
        let s = store(tempDir())
        let item = seed(s, by: nil, name: nil)

        guard case .withdrawn = s.withdraw(
            crewId: "c", number: item.number, sessionId: "captain-新的一轮",
            senderName: "机长", reason: "老条目，那件事早就没了", isCaptain: true) else {
            return XCTFail("没记提出者的老条目，机长也该撤得动")
        }
        XCTAssertFalse(s.item(crewId: "c", number: item.number)!.isUnanswered)
    }

    func testCaptainWithdrawLeavesATrailSayingWhoDidIt() {
        // 权限放宽了就必须留痕：撤的是别人提的一条，人回头要看得出**是谁撤的**、
        // 为什么撤 —— 不然「撤回不是删除、人有权追问」这句话就落不了地。
        let s = store(tempDir())
        let item = seed(s, by: "sess-已经没了", name: "某个已退出的 session")
        _ = s.withdraw(crewId: "c", number: item.number, sessionId: "captain-9",
                       senderName: "机长", reason: "人类今天当面答过了", isCaptain: true)

        let after = s.item(crewId: "c", number: item.number)
        XCTAssertEqual(after?.withdrawnBySessionId, "captain-9")
        XCTAssertEqual(after?.responses.first?.senderName, "机长")
        XCTAssertTrue(after?.responses.first?.text.contains("人类今天当面答过了") == true)
        XCTAssertEqual(after?.createdBySenderName, "某个已退出的 session",
                       "提出者是谁不许被撤回抹掉")
    }

    // MARK: 机长身份只解开「谁能撤」这一道闸，别的一道都不许顺手放开

    func testCaptainStillNeedsAReason() {
        let s = store(tempDir())
        let item = seed(s, by: "sess-other")
        XCTAssertEqual(s.withdraw(crewId: "c", number: item.number, sessionId: "captain-9",
                                  reason: "   ", isCaptain: true), .reasonRequired)
        XCTAssertTrue(s.item(crewId: "c", number: item.number)!.isUnanswered)
    }

    func testCaptainCannotWithdrawWhatTheHumanDeleted() {
        // `delete` 是人类自己的动作。机长身份不该让一条人类删掉的条目复活成可撤。
        //
        // ⚠️ **这一条挡住的不是判据**。变异测试当场证伪：把判据里的 `!item.isDeleted`
        // 对机长放开，这条照样全绿 —— 因为 `withdraw` 里 `liveIndexLocked` 更早一步
        // 就找不到已删的行了。它守的是**这条路的行为**（有用，但只有这么多）；
        // 判据本身由下面那条直接钉。
        let s = store(tempDir())
        let item = seed(s, by: "sess-other")
        XCTAssertTrue(s.delete(crewId: "c", number: item.number))
        XCTAssertEqual(s.withdraw(crewId: "c", number: item.number, sessionId: "captain-9",
                                  reason: "过期", isCaptain: true), .notFound)
    }

    func testCaptaincyDoesNotResurrectAHumanDeletedItemInTheJudgeItself() {
        // 上面那条测不到判据（见它的注释），所以这里绕开 `withdraw` 那条链，
        // 直接把已删的条目递给判据 —— **判据是两扇门共用的那一份**，
        // 哪天有人从别的地方调它，挡住机长的就只剩这一句。
        var gone = LocalTodoItem(id: "i", number: 1, text: "t", status: "pending",
                                 createdAt: "2026-09-07T00:00:00Z",
                                 createdBySessionId: "sess-other")
        gone.deletedAt = "2026-09-07T01:00:00Z"
        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: gone, sessionId: "captain-9",
                                                       isCaptain: true), .notFound)
        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: gone, sessionId: "sess-other",
                                                       isCaptain: false), .notFound,
                       "不是机长时也一样 —— 这句证明上一句不是被别的判据顺手挡住的")
    }

    func testCaptainWithdrawIsStillIdempotent() {
        let s = store(tempDir())
        let item = seed(s, by: "sess-other")
        _ = s.withdraw(crewId: "c", number: item.number, sessionId: "captain-9",
                       reason: "过期了", isCaptain: true)
        guard case .alreadyWithdrawn = s.withdraw(
            crewId: "c", number: item.number, sessionId: "captain-9",
            reason: "又过期了", isCaptain: true) else {
            return XCTFail("撤过的再撤该说撤过了，机长也不例外")
        }
        XCTAssertEqual(s.item(crewId: "c", number: item.number)?.responses.count, 1)
    }

    func testNonCaptainGateIsUnchanged() {
        // 反面：不是机长的照旧只能撤自己提的。这条守的是「别顺手放宽给所有人」。
        let s = store(tempDir())
        let mine = seed(s, text: "我提的", by: "sess-1")
        let theirs = seed(s, text: "别人提的", by: "sess-other", name: "别人")
        let legacy = seed(s, text: "老条目", by: nil, name: nil)

        guard case .withdrawn = s.withdraw(crewId: "c", number: mine.number,
                                           sessionId: "sess-1", reason: "过期") else {
            return XCTFail("自己提的照旧撤得动")
        }
        XCTAssertEqual(s.withdraw(crewId: "c", number: theirs.number,
                                  sessionId: "sess-1", reason: "过期"),
                       .notYours(owner: "别人"))
        XCTAssertEqual(s.withdraw(crewId: "c", number: legacy.number,
                                  sessionId: "sess-1", reason: "过期"),
                       .notYours(owner: nil))
    }

    func testWithdrawObstacleJudgesByCaptaincyNotByAuthorship() {
        // 两扇门（`withdraw_human_todo` 和 `add_human_todo(supersedes:)`）共用的
        // 那一份纯判据，也必须认机长 —— 各写一套迟早分叉。
        let theirs = LocalTodoItem(id: "i", number: 1, text: "t", status: "pending",
                                   createdAt: "2026-09-07T00:00:00Z",
                                   createdBySessionId: "sess-已经没了",
                                   createdBySenderName: "别人")
        let legacy = LocalTodoItem(id: "j", number: 2, text: "t", status: "pending",
                                   createdAt: "2026-09-07T00:00:00Z")

        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: theirs, sessionId: "captain-9"),
                       .notYours(owner: "别人"), "不是机长时判据一个字没变")
        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: legacy, sessionId: "captain-9"),
                       .notYours(owner: nil))

        XCTAssertNil(LocalTodoStore.withdrawObstacle(item: theirs, sessionId: "captain-9",
                                                     isCaptain: true))
        XCTAssertNil(LocalTodoStore.withdrawObstacle(item: legacy, sessionId: "captain-9",
                                                     isCaptain: true))
        XCTAssertEqual(LocalTodoStore.withdrawObstacle(item: nil, sessionId: "captain-9",
                                                       isCaptain: true), .notFound,
                       "机长身份不该把「没这条」变成撤得动")
    }

    // MARK: MCP 那一层：机长的 helper 带着 --captain 起来，这道闸要真的接上去

    func testMcpCaptainWithdrawsATodoLeftBehindByAGoneSession() {
        let dir = tempDir()
        let item = seed(store(dir), by: "sess-已经没了", name: "某个已退出的 session")
        let receipt = call(server(dir, sessionId: "captain-9", isCaptain: true),
                           ["number": item.number, "reason": "人类今天当面答过了"])
        XCTAssertTrue(receipt.contains("已撤回"), receipt)

        let posted = LocalWhiteboardStore(directory: dir).list(crewId: "c")
        XCTAssertTrue(posted.contains { $0.text.contains("撤回 人类 To Do #\(item.number)：人类今天当面答过了") },
                      "机长撤的照样要在群里留下带原因的那一行")
    }

    func testMcpRefusalTellsWorkersToAskTheCaptain() {
        // 撤不动时给的那句建议**本身就是这次要修的东西**：旧文案让人「在群里说明、
        // 让人类自己决定删不删」—— 那正是今天真的发生的、把一条死条目永远挂在
        // 人账上的路。现在有确定的出口：找本 crew 机长。
        let dir = tempDir()
        let item = seed(store(dir), by: "sess-other", name: "别人")
        let receipt = call(server(dir, sessionId: "sess-1"),
                           ["number": item.number, "reason": "我看它没用了"])
        XCTAssertTrue(receipt.contains("机长"), receipt)
    }

    func testMcpCaptainCanSupersedeATodoLeftBehindByAGoneSession() {
        // supersede 是撤回的第二扇门，走的是同一份判据 —— 机长在这扇门上也该撤得动，
        // 否则两扇门对同一个人给出相反的答案。
        let dir = tempDir()
        let old = seed(store(dir), text: "0.1.24 要不要发？", by: "sess-已经没了", name: "别人")
        let receipt = add(server(dir, sessionId: "captain-9", isCaptain: true),
                          ["text": "0.1.25 要不要发？", "supersedes": old.number])
        XCTAssertTrue(receipt.contains("同时撤回了旧的 #\(old.number)"), receipt)

        let ledger = LocalTodoStore(directory: dir, ledger: .human)
        XCTAssertEqual(ledger.list(crewId: "c").filter(\.isUnanswered).count, 1,
                       "人只该看到一条在等他")
    }

    /// `tools/list` 里那个工具自己的 description。**必须只取这一个工具的那一段** ——
    /// 拿整份 JSON 去 `contains("机长")` 会被别的工具（`plan_add` 等）里的「机长」
    /// 二字喂饱，那种绿跟这次改动一点关系都没有。
    private func toolDescription(_ name: String) throws -> String {
        let raw = try XCTUnwrap(server(tempDir())
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#))
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(
            with: Data(raw.utf8)) as? [String: Any])
        let result = try XCTUnwrap(json["result"] as? [String: Any])
        let tools = try XCTUnwrap(result["tools"] as? [[String: Any]])
        let tool = try XCTUnwrap(tools.first { $0["name"] as? String == name },
                                 "tools/list 里没有 \(name)")
        return try XCTUnwrap(tool["description"] as? String)
    }

    func testToolDescriptionOnlyReadsTheWithdrawToolsOwnText() throws {
        // 先证明这把尺子会红：别的工具里的「机长」不许算进来。
        let d = try toolDescription("withdraw_human_todo")
        XCTAssertFalse(d.contains("督办"), "取错工具了 —— 这是 plan_add 那边的词")
    }

    func testToolDescriptionSaysTheCaptainCanWithdrawAnyOfThisCrews() throws {
        // 描述必须跟着判据改：agent 只读得到描述。描述还写着「只能撤自己提的」，
        // 机长就根本不会去试 —— 一条修好了却没人知道的权限等于没修。
        let d = try toolDescription("withdraw_human_todo")
        XCTAssertTrue(d.contains("机长"), d)
        XCTAssertTrue(d.contains("不是删除"), "这条安全带不许在改文案时被顺手拆掉")
    }
}
