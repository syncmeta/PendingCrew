#if os(macOS)
import Foundation
import XCTest

/// 菜单栏数字：全机两本账的未解决 Todo 去重数；其它等待事项单列。
final class HumanAttentionTallyTests: XCTestCase {
    private func item(_ id: String, number: Int, status: String = "pending",
                      withdrawn: Bool = false) -> LocalTodoItem {
        var row = LocalTodoItem(id: id, number: number, text: "todo", status: status,
                                createdAt: "2026-01-01T00:00:00Z")
        if withdrawn { row.withdrawnAt = "2026-01-02T00:00:00Z" }
        return row
    }

    func testBadgeCountsOpenTodosAcrossBothLedgersAndCrewsByIdentity() {
        let rows: [HumanAttentionTally.TodoRecord] = [
            .init(crewId: "a", ledger: .agent, item: item("same", number: 1)),
            .init(crewId: "a", ledger: .human, item: item("same", number: 1)),
            .init(crewId: "b", ledger: .agent, item: item("same", number: 1)),
            .init(crewId: "a", ledger: .agent, item: item("done", number: 2, status: "completed")),
            .init(crewId: "a", ledger: .human, item: item("stopped", number: 3, status: LocalTodoItem.droppedStatus)),
            .init(crewId: "a", ledger: .human, item: item("withdrawn", number: 4, withdrawn: true)),
        ]
        let count = HumanAttentionTally.tally(todos: rows, pendingApprovalSessionIds: ["s"],
                                               sessionStates: ["m": CrewSessionStateDerivation.awaitingDecision])
        XCTAssertEqual(count.badge, "2")
        XCTAssertEqual(count.todos, 2)
        XCTAssertEqual(count.approvals, 1)
        XCTAssertEqual(count.screenMenus, 1)
        XCTAssertTrue(count.lines.contains("2 条未解决 Todo"))
    }

    func testBadgeHidesAtZeroEvenIfOtherAttentionRemains() {
        let count = HumanAttentionTally.tally(todos: [], pendingApprovalSessionIds: ["s"],
                                               sessionStates: [:])
        XCTAssertNil(count.badge)
        XCTAssertTrue(count.lines.contains("1 件待审批"))
    }

    func testStateChangeImmediatelyChangesComputedBadge() {
        var row = item("one", number: 1)
        func count() -> String? {
            HumanAttentionTally.tally(todos: [.init(crewId: "a", ledger: .agent, item: row)],
                                      pendingApprovalSessionIds: [], sessionStates: [:]).badge
        }
        XCTAssertEqual(count(), "1")
        row.status = "completed"
        XCTAssertNil(count())
        row.status = "in_progress"
        XCTAssertEqual(count(), "1")
    }

    func testUnreadableEitherLedgerNeverBecomesZero() {
        for failedLedger in TodoLedger.allCases {
            XCTAssertThrowsError(try HumanAttentionTally.readRecords(crewIds: ["a"]) { _, ledger in
                ledger == failedLedger ? .unreadable : .rows([self.item("one", number: 1)])
            })
        }
    }

    @MainActor
    func testNewCrewFromAnotherStoreAppearsInFreshRosterAndReadFailureThrows() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("menu-roster-\(UUID().uuidString)")
        let whiteboards = root.appendingPathComponent("whiteboards")
        try FileManager.default.createDirectory(at: whiteboards, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let cached = LocalCrewStore(baseDirectory: root)
        let before = try HumanAttentionTally.loadCrewIds(whiteboardDirectory: whiteboards)
        let other = LocalCrewStore(baseDirectory: root)
        let request = CreateCrewRequest.make(
            responsibleSubjectId: "local-byok", title: "new crew", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "codex",
            captain: .systemGenerated(templateName: nil))
        let created = other.createCrew(request)
        XCTAssertFalse(cached.listCrews().contains { $0.id == created.crewId })
        XCTAssertFalse(before.contains(created.crewId))
        let currentIds = try HumanAttentionTally.loadCrewIds(whiteboardDirectory: whiteboards)
        XCTAssertTrue(currentIds.contains(created.crewId))
        let agent = LocalTodoStore(directory: whiteboards)
        let human = LocalTodoStore(directory: whiteboards, ledger: .human)
        XCTAssertNotNil(agent.add(crewId: created.crewId, text: "new work"))
        let records = try HumanAttentionTally.readRecords(crewIds: currentIds) { crewId, ledger in
            (ledger == .agent ? agent : human).read(crewId: crewId)
        }
        XCTAssertEqual(HumanAttentionTally.tally(todos: records,
            pendingApprovalSessionIds: [], sessionStates: [:]).badge, "1")
        try Data("invalid".utf8).write(to: root.appendingPathComponent("local-crews.json"))
        XCTAssertThrowsError(try HumanAttentionTally.loadCrewIds(whiteboardDirectory: whiteboards))
    }

    func testQuietWhenNothingIsWaiting() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [], sessionStates: ["s1": "working", "s2": "idle"],
            unansweredTodos: 0)
        XCTAssertTrue(count.isQuiet)
        XCTAssertNil(count.badge, "没事的时候还挂个数字，人就学会了无视它")
        XCTAssertEqual(count.lines, [], "常年显示「待审批 0」会训练人忽略这一栏")
    }

    func testCountsTheThreeKinds() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: ["s1", "s2"],
            sessionStates: ["s3": "awaitingDecision", "s4": "working"],
            unansweredTodos: 4)
        XCTAssertEqual(count, HumanAttentionCount(approvals: 2, screenMenus: 1, todos: 4))
        XCTAssertEqual(count.total, 4)
        XCTAssertEqual(count.badge, "4")
    }

    /// 同一个 session 上的两个待审批**是两件事**（按条目数，不按 session 去重）。
    func testTwoApprovalsOnOneSessionAreTwoThings() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: ["s1", "s1"], sessionStates: [:], unansweredTodos: 0)
        XCTAssertEqual(count.approvals, 2)
    }

    /// 一个 session 既有待审批、又卡在屏幕的框上：**按人的角度是一件事**
    /// （他打开那个 session 就都看见了），别数两遍。
    func testASessionWithBothDoesNotGetCountedTwice() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: ["s1"],
            sessionStates: ["s1": "awaitingDecision"],
            unansweredTodos: 0)
        XCTAssertEqual(count.total, 0, "审批不进入 Todo 数字")
        XCTAssertEqual(count.screenMenus, 0)
    }

    /// **`awaitingReply` 不计。** 它的判定输入之一就是审批台账里本 session 的
    /// pending 条目 —— 一起加进来就是把同一件事数两遍。
    func testAwaitingReplyIsDeliberatelyNotCounted() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [],
            sessionStates: ["s1": "awaitingReply", "s2": "awaitingReply"],
            unansweredTodos: 0)
        XCTAssertTrue(count.isQuiet,
                      "awaitingReply 被计进来了 —— 它和待审批是同一件事的两个出口")
    }

    /// 别的状态一律不算「在等人」。**`idle` 尤其**：它的语义是「起来了、在等活」，
    /// 不是在等人。
    func testOtherStatesAreNotWaitingOnAHuman() {
        for state in ["working", "idle", "rateLimited", "error", "launchFailed", "exited"] {
            let count = HumanAttentionTally.tally(
                pendingApprovalSessionIds: [], sessionStates: ["s": state], unansweredTodos: 0)
            XCTAssertTrue(count.isQuiet, "\(state) 被当成了在等人拍板")
        }
    }

    /// 状态字面量必须是点名快照那一份，不是这里另抄一个。
    func testUsesTheRosterStateVocabulary() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [],
            sessionStates: ["s": CrewSessionStateDerivation.awaitingDecision],
            unansweredTodos: 0)
        XCTAssertEqual(count.screenMenus, 1)
    }

    func testSummaryOnlyMentionsWhatIsActuallyWaiting() {
        let count = HumanAttentionTally.tally(
            pendingApprovalSessionIds: [], sessionStates: [:], unansweredTodos: 2)
        XCTAssertEqual(count.summary, "2 条未解决 Todo")
        XCTAssertFalse(count.summary.contains("待审批"))
    }
}
#endif
