import XCTest

/// 「被叫停」这一档 + 侧栏黄点收成一个判据（人类 Todo #139 第二批）。
///
/// 人类原话是「1 加。2 亮」——① 作废/被叫停那一档：加；② 侧栏黄点为「在等你回复」
/// 也亮。第三档「卡在前置」他没要，不加。
///
/// ## `dropped` 不是 `completed` 的近义词
///
/// 两者都表示「这条不用再推了」，但**记的是完全不同的事**：
/// `completed` 说「做完了，凭据在这儿」；`dropped` 说「不做了，是谁决定的、为什么」。
/// 混在一起的代价是**统计当场变假**：2026-09-09 报出去的「138 条完成 111」里，
/// 有三条（#123 / #18 / #102）是被人类叫停的，被记成了完成。
/// 那个数当时就是错的，只是没有任何人看得出来 —— 这正是本档存在的理由。
final class TodoDroppedAndAttentionTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("todo139b-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1", isCaptain: true,
                  sessionLabel: "机长",
                  quotaDirectory: dir,
                  todos: LocalTodoStore(directory: dir))
    }

    private func respond(_ s: McpServer, _ args: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"respond_todo","arguments":\(args)}}
        """) ?? ""
    }

    // MARK: - ① 这一档存在，而且是「结了」而不是「完成了」

    func testDroppedIsAValidStatus() {
        XCTAssertTrue(LocalTodoStore.validStatuses.contains(LocalTodoItem.droppedStatus))
        XCTAssertEqual(LocalTodoItem.statusLabel(LocalTodoItem.droppedStatus), "已叫停")
    }

    /// **不许混进「完成」。** 这条钉的就是那个错掉的数。
    func testDroppedIsSettledButNotCompleted() {
        let dropped = Self.item(1, status: LocalTodoItem.droppedStatus)
        let done = Self.item(2, status: "completed")
        let doing = Self.item(3, status: "in_progress")

        XCTAssertTrue(dropped.isSettled, "被叫停的还被当成「未结」—— 督办会一直响")
        XCTAssertTrue(done.isSettled)
        XCTAssertFalse(doing.isSettled)

        let tally = LocalTodoStore.tally([dropped, done, doing])
        XCTAssertEqual(tally.completed, 1,
                       """
                       把被叫停的算进「完成 N 条」了 —— 这正是 2026-09-09 那个
                       「138 条完成 111」错掉的地方，而且报出去时没有任何人看得出来。
                       """)
        XCTAssertEqual(tally.dropped, 1)
        XCTAssertEqual(tally.open, 1)
    }

    /// 全仓不许再有人用「不等于 completed」来表达「还没结」——
    /// `dropped` 一加，那种写法就当场变成一条会骗人的判据，而且不报错。
    func testNobodySpellsUnfinishedAsNotCompleted() throws {
        let offenders = try Self.sourceFiles().filter { url in
            let code = Self.codeOnly((try? String(contentsOf: url, encoding: .utf8)) ?? "")
            return code.contains(#"status != "completed""#)
        }
        XCTAssertEqual(offenders.map { $0.lastPathComponent }.sorted(), [],
                       """
                       这些文件用「status != completed」当「还没结」。加了 `dropped`
                       之后它就是错的：被叫停的条目会被永远当成「还欠着」——
                       机长的 Todo 督办会为一件人类已经喊停的事一直响。
                       改用 `LocalTodoItem.isSettled`。
                       """)
    }

    // MARK: - ② 凭据闸：完成要凭据，叫停要理由

    func testDroppingNeedsNoEvidenceButCompletingStillDoes() {
        let dir = tempDir()
        let s = server(dir)
        _ = s.todos.add(crewId: "c", text: "人类后来喊停的活")
        _ = s.todos.add(crewId: "c", text: "真做完的活")

        let drop = respond(s, #"{"number":1,"response":"人类当面喊停，不做了","status":"dropped"}"#)
        XCTAssertFalse(drop.contains("ERROR"),
                       """
                       翻「已叫停」被凭据闸拦下了。叫停没有产出，拿不出 commit ——
                       逼它给凭据的结果是 agent 只好去翻 `completed`，
                       那道闸就把账变假了，正好跟它的目的相反。
                       """)
        XCTAssertEqual(s.todos.item(crewId: "c", number: 1)?.status, LocalTodoItem.droppedStatus)

        let done = respond(s, #"{"number":2,"response":"做完了","status":"completed"}"#)
        XCTAssertTrue(done.contains("ERROR"),
                      "凭据闸被这一单放宽了 —— 那道闸是用 191 小时假账换来的，不许顺手拆")
        XCTAssertEqual(s.todos.item(crewId: "c", number: 2)?.status, "pending")
    }

    /// 叫停要的是**理由**，理由就是那句回应 —— 空回应照旧拒绝。
    func testDroppingStillNeedsAReason() {
        let s = server(tempDir())
        _ = s.todos.add(crewId: "c", text: "x")
        XCTAssertTrue(respond(s, #"{"number":1,"response":"","status":"dropped"}"#).contains("ERROR"),
                      "没写理由就叫停了 —— 这一档记的就是「谁决定不做的、为什么」")
    }

    // MARK: - ③ 侧栏黄点：一个判据，两个来源

    func testWaitingOnHumanIsOneCriterionFedByTwoSources() {
        // 人类那本：agent 问了、人还没答。
        XCTAssertTrue(Self.item(1, status: "pending").isWaitingOnHuman(in: .human))
        XCTAssertFalse(Self.item(1, status: "pending", responses: 1).isWaitingOnHuman(in: .human))
        // agent 那本：只有翻成「等你回复」的才算。
        XCTAssertTrue(Self.item(1, status: LocalTodoItem.blockedOnHumanStatus)
            .isWaitingOnHuman(in: .agent))
        XCTAssertFalse(Self.item(1, status: "in_progress").isWaitingOnHuman(in: .agent))
        XCTAssertFalse(Self.item(1, status: "pending").isWaitingOnHuman(in: .agent),
                       "agent 那本的普通待办不该点亮黄点 —— 那盏灯会常年亮着，变成背景")
        // 结了的两档，两本账都不亮。
        for status in ["completed", LocalTodoItem.droppedStatus] {
            XCTAssertFalse(Self.item(1, status: status).isWaitingOnHuman(in: .human), status)
            XCTAssertFalse(Self.item(1, status: status).isWaitingOnHuman(in: .agent), status)
        }
    }

    /// 旧那个名字必须只是**转发**，不许自己再判一遍。
    ///
    /// 机长原话：「别在旁边并排加一个第二判据 —— 那样以后改一个忘一个，
    /// 黄点会开始骗人。」这条尺子钉的就是那件事本身。
    func testTheOldNameIsOnlyAForwarder() throws {
        let code = Self.codeOnly(try Self.text(of: "LocalTodoStore.swift"))
        guard let r = code.range(of: "var isUnanswered: Bool") else {
            return XCTFail("isUnanswered 没了 —— 它还有读者，别静默删掉")
        }
        let body = String(code[r.lowerBound...].prefix(200))
        XCTAssertTrue(body.contains("isWaitingOnHuman"),
                      """
                      `isUnanswered` 自己又判了一遍，而不是转发到那一个判据上。
                      两处判据以后一定会分叉，而分叉的表现是「黄点亮着但点进去没事」——
                      没有人会因此报 bug，只会慢慢学会忽略这盏灯。
                      """)
    }

    /// 黄点的计数真的把两本账都喂进去了（不是只读人类那本）。
    func testTheAttentionCountReadsBothLedgers() {
        let dir = tempDir()
        let human = LocalTodoStore(directory: dir, ledger: .human)
        let agent = LocalTodoStore(directory: dir, ledger: .agent)
        _ = human.add(crewId: "c", text: "我问你的事")
        _ = agent.add(crewId: "c", text: "卡在你身上的活")
        _ = agent.respond(crewId: "c", number: 1, sessionId: "s", text: "等你拍",
                          newStatus: LocalTodoItem.blockedOnHumanStatus)

        let cache = CrewHumanTodoAttentionCache(human: human, agent: agent)
        XCTAssertEqual(cache.refresh(crewIds: ["c"])["c"], 2,
                       """
                       黄点只数了人类那本 —— agent 那本里卡在他身上的活不点灯。
                       他会以为没事，而那条活正堵着。
                       """)
    }

    // MARK: - 小工具

    private static func item(_ n: Int, status: String, responses: Int = 0) -> LocalTodoItem {
        LocalTodoItem(
            id: "id-\(status)-\(n)", number: n, text: "t", status: status,
            createdAt: "2026-09-09T00:00:00Z",
            responses: (0..<responses).map {
                LocalTodoResponse(id: "r\($0)", sessionId: "s", senderName: nil,
                                  text: "回", status: nil, createdAt: "2026-09-09T00:00:00Z")
            })
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
        for url in try sourceFiles() where url.lastPathComponent == fileName {
            return try String(contentsOf: url, encoding: .utf8)
        }
        throw XCTSkip("找不到 \(fileName)")
    }
}
