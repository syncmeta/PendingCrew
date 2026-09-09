import XCTest

/// 「在等你回复」这一档（人类 Todo #139）—— **一条、一个归属、两个视图**。
///
/// 人类原话：「我希望 todo 应该多几个状态，比如在等我回复的应该是黄色，并挂到给人类的
/// todo 上。」
///
/// ## 这一单不是「避免引入第二份」，是「消灭已经存在的第二份」
///
/// 「在等人类回复」今天有两类，只有一类是缺口：
///
/// - **agent 问人类**（`ask` / `add_human_todo`）—— #75 之后本来就写进人类那本，
///   他已经看得见。**不是缺口。**
/// - **人类派给 agent 的活（agent 那本），agent 卡在他身上推不动** —— 今天想让他
///   看见，**唯一的办法就是 `add_human_todo` 再开一条**。
///
/// 也就是说：**复制已经在发生**（机长自述这一天里干过好几次）。两份会各自被回应、
/// 各自翻牌，从此对不上 —— **两个事实源就是没有事实源。**
///
/// 所以做法是：给**原条目**加一档状态，人类那本的**视图**把它显示出来。
/// **一条、一个归属、一个号、一份回应列表。**
///
/// ## 防重复这件事，量的是那件事本身
///
/// `testFlippingToBlockedWritesNothingToTheHumanLedger` 断言的是**人类那本的条数
/// 一个字不变** —— 不是「代码里没有调 `add_human_todo`」那种代理量。
/// 代理量平时都对，只在你真正需要它的时候错。
final class TodoBlockedOnHumanTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("todo139-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    // MARK: - ① 这一档要真的存在，agent 标得上去

    func testBlockedOnHumanIsAValidStatus() {
        XCTAssertTrue(LocalTodoStore.validStatuses.contains(LocalTodoItem.blockedOnHumanStatus),
                      "agent 标不上这一档 —— 那它只是个没人写得进去的枚举值")
    }

    func testAgentCanFlipAnItemToBlockedOnHuman() throws {
        let dir = tempDir()
        let agent = LocalTodoStore(directory: dir, ledger: .agent)
        XCTAssertNotNil(agent.add(crewId: "c", text: "把发版脚本改了"))
        XCTAssertNotNil(agent.respond(crewId: "c", number: 1, sessionId: "s", senderName: "小工",
                                      text: "要你先拍一下用哪个方案",
                                      newStatus: LocalTodoItem.blockedOnHumanStatus))
        XCTAssertEqual(agent.list(crewId: "c").first?.status,
                       LocalTodoItem.blockedOnHumanStatus)
    }

    // MARK: - ② 承重点：**不许多出第二条**

    /// 量的是那件事本身：翻牌前后**人类那本的条数一个字不变**。
    func testFlippingToBlockedWritesNothingToTheHumanLedger() throws {
        let dir = tempDir()
        let agent = LocalTodoStore(directory: dir, ledger: .agent)
        let human = LocalTodoStore(directory: dir, ledger: .human)
        XCTAssertNotNil(human.add(crewId: "c", text: "本来就有的一条"))
        let before = human.list(crewId: "c").count

        XCTAssertNotNil(agent.add(crewId: "c", text: "卡在你身上的活"))
        _ = agent.respond(crewId: "c", number: 1, sessionId: "s", senderName: "小工",
                          text: "等你拍", newStatus: LocalTodoItem.blockedOnHumanStatus)

        XCTAssertEqual(
            human.list(crewId: "c").count, before,
            """
            翻到「等你回复」之后，人类那本多出了条目 —— **那就是第二份**。
            两份会各自被回应、各自翻牌，从此对不上；两个事实源就是没有事实源。
            这一单要消灭的正是这个，不是引入它。
            """)
        XCTAssertEqual(agent.list(crewId: "c").count, 1, "agent 那本也不该被复制")
    }

    // MARK: - ③ 人类那本的**视图**要看得见它（一条、带着它的归属）

    func testHumanViewShowsAgentItemsThatAreWaitingOnTheHuman() {
        let rows = TodoListPresentation.humanFacingRows(
            human: [Self.item(1, "agent 早先问你的事", status: "pending"),
                    Self.item(3, "agent 新问你的事", status: "pending")],
            agent: [Self.item(7, "卡在你身上的活",
                              status: LocalTodoItem.blockedOnHumanStatus),
                    Self.item(8, "正常在跑的活", status: "in_progress")])
        // 顺序是**故意的**：等你回复的那几条置顶（它们各自卡着一条真在跑的活，
        // 拖一分钟就多堵一分钟），其余按这本账原本的「新的在上面」。
        // 两本账的 #N 各自从 1 起，混在一起按号排没有意义，所以分组排而不是全局排。
        XCTAssertEqual(rows.map(\.item.number), [7, 3, 1],
                       """
                       人类那本的视图没显示 agent 那本里等他回复的那条（或者没置顶）。
                       他说的「挂到给人类的 todo 上」就是这个 —— 看不见等于没做。
                       """)
        XCTAssertEqual(rows.map(\.ledger), [.agent, .human, .human],
                       "每条必须带着它**自己那本账**的归属 —— 回应要写回原处，不能写串本")
    }

    /// 反面：**没在等他的 agent 条目不许挤进他的视图**。
    func testHumanViewDoesNotPullInOtherAgentWork() {
        let rows = TodoListPresentation.humanFacingRows(
            human: [],
            agent: [Self.item(1, "在跑", status: "in_progress"),
                    Self.item(2, "待办", status: "pending"),
                    Self.item(3, "完成了", status: "completed")])
        XCTAssertTrue(rows.isEmpty,
                      "把 agent 那本的普通活也塞进人类视图 —— 那本账会当场变成噪音源")
    }

    /// 归属不同、号可以撞（两本各自从 #1 起）。视图得分得开。
    func testSameNumberInBothLedgersStaysDistinct() {
        let rows = TodoListPresentation.humanFacingRows(
            human: [Self.item(1, "人类那本的 #1", status: "pending")],
            agent: [Self.item(1, "agent 那本的 #1",
                              status: LocalTodoItem.blockedOnHumanStatus)])
        XCTAssertEqual(Set(rows.map(\.id)).count, 2,
                       "两本账的 #1 被当成同一条了 —— 回应会打在错的那本上")
    }

    // MARK: - ④ 黄色（他点名的）

    func testBlockedOnHumanRendersDistinctlyFromTheOtherStates() {
        let blocked = TodoListPresentation.statusIcon(LocalTodoItem.blockedOnHumanStatus)
        for other in ["pending", "in_progress", "completed"] {
            XCTAssertNotEqual(blocked, TodoListPresentation.statusIcon(other),
                              "「等你回复」跟「\(other)」画得一模一样 —— 那他一眼分不出来")
        }
        XCTAssertEqual(LocalTodoItem.statusLabel(LocalTodoItem.blockedOnHumanStatus), "等你回复",
                       "状态名没有人话，tooltip/无障碍会念出裸标识符")
    }

    // MARK: - ⑤ 只能有一个真值（加状态必然踩到的那处）

    /// ⚠️ **交给我时说的是「两份」，实际是四份。** 机长点名了
    /// `LocalTodoStore.swift` 和 `CrewMessageCategory.swift`；照着那份名单改完，
    /// `McpServer` 里两处 JSON schema 的 `enum` 还留着旧三档 —— 结果是
    /// **状态在数据层合法、但模型根本填不出这个值**，而且不报任何错。
    ///
    /// 所以这条尺子**不认名单，机械扫全仓**：给它一份名单，它就只会跟名单一样对。
    func testValidStatusesHasExactlyOneTruthSource() throws {
        let truth = "Sources/Stores/LocalTodoStore.swift"
        let offenders = try Self.sourceFiles().filter { url in
            guard !url.path.hasSuffix(truth) else { return false }
            let code = Self.codeOnly((try? String(contentsOf: url, encoding: .utf8)) ?? "")
            return code.contains(#""pending", "in_progress", "completed""#)
        }
        XCTAssertEqual(
            offenders.map { $0.lastPathComponent }.sorted(), [],
            """
            这些文件各自抄了一份状态字面量。加一个状态要改 N 处，漏一处**不报错**：
            漏数据层 = 群里能标工具标不了；漏 JSON schema 的 enum = 模型根本填不出
            这个值。引用 `LocalTodoStore.statusOrder`，别另立一份。
            """)
    }

    /// 加了一档状态却没在 `respond_todo` 的说明里提它 —— agent 不会知道它存在。
    /// 数据层收得下、schema 也放行，但没有人会去填 = 这一单等于没做。
    ///
    /// 只能钉「提到了」，钉不了「说清楚了」——**这是已知盲区，不是期望的产品行为**。
    func testEveryStatusIsDescribedToTheAgent() throws {
        let mcp = try Self.text(of: "McpServer.swift")
        guard let range = mcp.range(of: "\"name\": \"respond_todo\""),
              let end = mcp.range(of: "\"required\"", range: range.upperBound..<mcp.endIndex)
        else { throw XCTSkip("找不到 respond_todo 的工具定义") }
        let block = String(mcp[range.lowerBound..<end.upperBound])
        for status in LocalTodoStore.statusOrder {
            XCTAssertTrue(block.contains(status),
                          "`respond_todo` 的说明里没有 `\(status)` —— agent 不会知道它存在")
        }
    }

    // MARK: - ⑤b 借显过来的号不许裸写

    func testBorrowedRowCarriesItsLedgerNameInTheNumber() {
        let borrowed = TodoListPresentation.Row(
            ledger: .agent, item: Self.item(7, "卡在你身上", status: "pending"))
        let own = TodoListPresentation.Row(
            ledger: .human, item: Self.item(7, "你自己那本的 7", status: "pending"))
        XCTAssertNotEqual(
            TodoListPresentation.rowNumberLabel(borrowed, shownIn: .human), "7",
            """
            借显过来的行裸写了个「7」—— 他会去人类那本找 #7，那是另一件事。
            两本账的 #N 各自从 1 起，恒定都存在，不会有任何报错提示他找错了。
            """)
        XCTAssertEqual(TodoListPresentation.rowNumberLabel(own, shownIn: .human), "7",
                       "自己那本的行不该被加噪音")
    }

    // MARK: - ⑤c 接线（视觉我验不了，只能钉住「产品真的调了它」）

    /// 纯函数绿 ≠ 界面上看得见。这几条钉的是**视图真的调了新那条路** ——
    /// 少了它，上面那些可以全绿而屏幕上一点没变。
    ///
    /// ⚠️ 一律走 `codeOnly`：**注释里提到函数名会让这条尺子自己变哑**。
    /// 变异自证抓到过一次 —— 把详细窗口的调用整个换掉，测试照样绿，
    /// 因为同一个文件的一句 doc comment 里写着「理由见 `blockedOnHumanHint`」。
    func testTheViewsActuallyUseTheNewPaths() throws {
        let panel = Self.codeOnly(try Self.text(of: "CrewTodoPanel.swift"))
        XCTAssertTrue(panel.contains("TodoListPresentation.rows(for:"),
                      "概览面板没走合并那条路 —— 纯函数再绿，人类那本屏幕上也不会多出一行")
        XCTAssertTrue(panel.contains("rowNumberLabel"),
                      "概览面板还在裸写 #N —— 借显过来的号会指错本账")
        XCTAssertTrue(panel.contains("openDetail(ledger: row.ledger"),
                      "点进去用的还是药丸那本，不是这一行自己那本 —— 会打开另一件事")

        let detail = Self.codeOnly(try Self.text(of: "CrewTodoDetailWindow.swift"))
        XCTAssertTrue(detail.contains("TodoListPresentation.blockedOnHumanHint("),
                      "详细窗口的人类那本既不借显也不指路 —— 那张面上他就是看不见")
    }

    /// **面板得真去读 agent 那本**，否则借显那半边恒定是空的。
    ///
    /// 变异自证抓到的第二个缺口：把喂进去的 agent 数组换成 `[]`，
    /// 上面那条「调了 rows(for:)」照样绿 —— 它只证明了调用存在，
    /// 没证明**喂进去的东西不是空的**。函数调对了、参数是空的，
    /// 屏幕上的结果跟没做完全一样。
    func testThePanelActuallyReadsTheAgentLedger() throws {
        let panel = Self.codeOnly(try Self.text(of: "CrewTodoPanel.swift"))
        XCTAssertTrue(panel.contains("rows(for: .human, human: todos, agent: waitingOnHuman)"),
                      "人类那屏喂给合并函数的不是真读来的 agent 条目 —— 借显恒为空")
        XCTAssertTrue(panel.contains("LocalTodoStore.shared(.agent)"),
                      "面板压根没读 agent 那本")
        XCTAssertTrue(panel.contains("agentStore.todoChanges(crewId: crewId)"),
                      """
                      没订 agent 那本的变更流 —— agent 翻成「等你回复」时这一屏不会动，
                      他得关掉驾驶舱重开才看得见，那跟没做差不多。
                      """)
    }

    func testTheHintOnlyShowsWhenThereIsSomethingToShow() {
        XCTAssertNil(TodoListPresentation.blockedOnHumanHint(count: 0),
                     "没有可看的还挂一句指路 —— 点过去是空的，比不给更糟")
        XCTAssertNotNil(TodoListPresentation.blockedOnHumanHint(count: 2))
    }

    // MARK: - ⑥ 旧数据（这一单**不加字段**，所以这条该是结构性地绿）

    func testOldRowsWithoutTheNewStatusStillDecode() throws {
        let dir = tempDir()
        let old = """
        [{"id":"a","number":1,"text":"老条目","status":"pending","createdAt":"2026-08-01T00:00:00Z","responses":[]}]
        """
        try Data(old.utf8).write(
            to: dir.appendingPathComponent("c\(TodoLedger.agent.fileSuffix)"))
        XCTAssertEqual(LocalTodoStore(directory: dir, ledger: .agent).list(crewId: "c").count, 1,
                       "旧行解不开了 —— 但这一单只加了 status 的一个新取值、没加字段，本不该发生")
    }

    /// 反方向：**旧版 app 读到新状态**要安全降级（`statusIcon` 的 `default` 兜底）。
    /// 两个方向都验，才叫验过兼容性。
    func testAnUnknownStatusStillRendersSomething() {
        let icon = TodoListPresentation.statusIcon("something_from_the_future")
        XCTAssertEqual(icon.symbol, "circle", "未知状态没有兜底渲染 —— 旧版 app 会画不出来")
    }

    // MARK: - 小工具

    private static func item(_ n: Int, _ text: String, status: String) -> LocalTodoItem {
        LocalTodoItem(id: "id-\(status)-\(n)", number: n, text: text, status: status,
                      createdAt: "2026-09-09T00:00:00Z")
    }

    private static func codeOnly(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    private static func sourceFiles() throws -> [URL] {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录") }
        return (walker.allObjects as? [URL] ?? []).filter { $0.pathExtension == "swift" }
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
