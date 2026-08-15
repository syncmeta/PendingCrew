import XCTest
// CockpitTaskLedger.swift 在 Sources/Models（已整目录编进 test bundle），无需 @testable import。

/// 驾驶舱任务段的**账本来源判定** + 三段式归并（#542）。
///
/// 人类反馈「任务 tab 只显示一个任务且陈旧」的根因是数据源失养：仓库 `docs/tasks/*.md`
/// 那本账早停更了，真活在 coding agent 的活跃 task 账里。这组测试钉住「读哪本」这个判定，
/// 免得日后又悄悄退回死账（那种回归肉眼看不出来 —— 界面照样有内容，只是内容是旧的）。
final class CockpitTaskLedgerTests: XCTestCase {

    // MARK: - 从 .claude/settings.json 抽活跃账 id

    private let settingsWithId = """
    {
      "env": { "CLAUDE_CODE_TASK_LIST_ID": "pendingbot" },
      "permissions": { "allow": [] }
    }
    """

    func testTaskListIdFromSettings() {
        XCTAssertEqual(
            CockpitTaskLedger.taskListId(settings: settingsWithId, localSettings: nil),
            "pendingbot")
    }

    func testLocalSettingsOverridesBase() {
        let local = #"{"env":{"CLAUDE_CODE_TASK_LIST_ID":"local-scope"}}"#
        XCTAssertEqual(
            CockpitTaskLedger.taskListId(settings: settingsWithId, localSettings: local),
            "local-scope")
    }

    func testTaskListIdMissingOrMalformed() {
        // 没有 env 段
        XCTAssertNil(CockpitTaskLedger.taskListId(
            settings: #"{"permissions":{"allow":[]}}"#, localSettings: nil))
        // env 里没这个 key
        XCTAssertNil(CockpitTaskLedger.taskListId(
            settings: #"{"env":{"OTHER":"x"}}"#, localSettings: nil))
        // 空串不算配了
        XCTAssertNil(CockpitTaskLedger.taskListId(
            settings: #"{"env":{"CLAUDE_CODE_TASK_LIST_ID":""}}"#, localSettings: nil))
        // 坏 JSON 不该炸，当没配
        XCTAssertNil(CockpitTaskLedger.taskListId(settings: "{ not json", localSettings: nil))
        // 两处都没有
        XCTAssertNil(CockpitTaskLedger.taskListId(settings: nil, localSettings: nil))
    }

    // MARK: - 读哪本账

    private let home = URL(fileURLWithPath: "/Users/x/.claude/tasks", isDirectory: true)

    func testResolvePrefersLiveLedgerWhenItHasTasks() {
        let src = CockpitTaskLedger.resolve(taskListId: "pendingbot", tasksHome: home) { _ in true }
        XCTAssertEqual(src, .live(id: "pendingbot",
                                  dir: home.appendingPathComponent("pendingbot", isDirectory: true)))
    }

    func testResolveFallsBackWhenNoTaskListIdConfigured() {
        let src = CockpitTaskLedger.resolve(taskListId: nil, tasksHome: home) { _ in true }
        guard case let .repoLedger(reason) = src else { return XCTFail("应回落仓库账") }
        XCTAssertTrue(reason.contains("CLAUDE_CODE_TASK_LIST_ID"), reason)
    }

    func testResolveFallsBackWhenLiveDirEmpty() {
        // 配了 id 但目录里没有 task —— 也得回落，否则任务段会渲成空白且不说原因。
        let src = CockpitTaskLedger.resolve(taskListId: "pendingbot", tasksHome: home) { _ in false }
        guard case let .repoLedger(reason) = src else { return XCTFail("应回落仓库账") }
        XCTAssertTrue(reason.contains("pendingbot"), reason)
    }

    func testResolveProbesTheIdScopedDirectory() {
        var probed: [URL] = []
        _ = CockpitTaskLedger.resolve(taskListId: "pendingbot", tasksHome: home) {
            probed.append($0); return true
        }
        XCTAssertEqual(probed.map(\.lastPathComponent), ["pendingbot"])
    }

    // MARK: - 活跃账条目解析

    func testParseLiveTask() {
        let json = """
        {"id":"542","subject":"驾驶舱体验返工","description":"很长的一段…",
         "status":"in_progress","blocks":[],"blockedBy":[]}
        """.data(using: .utf8)!
        let when = Date(timeIntervalSince1970: 1_000)
        let item = CockpitTaskLedger.parseLiveTask(json, updated: when)
        XCTAssertEqual(item?.id, "542")
        XCTAssertEqual(item?.title, "驾驶舱体验返工")
        XCTAssertEqual(item?.statusRaw, "in_progress")
        XCTAssertEqual(item?.origin, .live)
        XCTAssertEqual(item?.badge, "#542")
        XCTAssertEqual(item?.updated, when)
    }

    func testParseLiveTaskTolerance() {
        // 缺 subject → 退回 #id 当标题（有总比空行好）
        let noSubject = #"{"id":"7","status":"pending"}"#.data(using: .utf8)!
        XCTAssertEqual(CockpitTaskLedger.parseLiveTask(noSubject, updated: nil)?.title, "#7")
        // 没有 id / 坏 JSON → 丢弃，不崩
        XCTAssertNil(CockpitTaskLedger.parseLiveTask(#"{"subject":"x"}"#.data(using: .utf8)!, updated: nil))
        XCTAssertNil(CockpitTaskLedger.parseLiveTask(Data("{ broken".utf8), updated: nil))
    }

    // MARK: - 三段式归并

    func testBandMapsBothLedgersStatusVocabularies() {
        // 活跃账
        XCTAssertEqual(CockpitTaskLedger.band("in_progress"), .doing)
        XCTAssertEqual(CockpitTaskLedger.band("pending"), .next)
        XCTAssertEqual(CockpitTaskLedger.band("completed"), .done)
        // 仓库 markdown 账（含复合值）
        XCTAssertEqual(CockpitTaskLedger.band("doing"), .doing)
        XCTAssertEqual(CockpitTaskLedger.band("partial, pending-qa"), .doing)
        XCTAssertEqual(CockpitTaskLedger.band("pending-qa"), .doing)
        XCTAssertEqual(CockpitTaskLedger.band("todo"), .next)
        XCTAssertEqual(CockpitTaskLedger.band("planned"), .next)
        XCTAssertEqual(CockpitTaskLedger.band("done"), .done)
        // 作废不占版面
        XCTAssertNil(CockpitTaskLedger.band("dropped"))
        // 空状态当没开工
        XCTAssertEqual(CockpitTaskLedger.band(""), .next)
    }

    func testBandsOrderNewestFirstAndCapDoneBand() {
        func item(_ id: String, _ status: String, _ t: TimeInterval?) -> CockpitTaskItem {
            CockpitTaskItem(id: id, title: "t\(id)", statusRaw: status, origin: .live,
                            updated: t.map { Date(timeIntervalSince1970: $0) }, badge: "#\(id)")
        }
        var items = [item("1", "in_progress", 10), item("2", "in_progress", 30),
                     item("3", "pending", 20), item("9", "dropped", 99)]
        items += (1...10).map { item("d\($0)", "completed", TimeInterval($0)) }

        let groups = CockpitTaskLedger.bands(items, doneLimit: 3)
        XCTAssertEqual(groups.map(\.band), [.doing, .next, .done])

        // 在做:新的在前
        XCTAssertEqual(groups[0].items.map(\.id), ["2", "1"])
        XCTAssertEqual(groups[1].items.map(\.id), ["3"])
        // 做了什么:按最近截断,且如实报被折走多少（不假装只有这几条）
        XCTAssertEqual(groups[2].items.map(\.id), ["d10", "d9", "d8"])
        XCTAssertEqual(groups[2].hiddenCount, 7)
        // dropped 一条都不出现
        XCTAssertFalse(groups.flatMap(\.items).contains { $0.id == "9" })
        // 未截断的段不报隐藏数
        XCTAssertEqual(groups[0].hiddenCount, 0)
    }

    func testBandsPutsUndatedItemsLast() {
        func item(_ id: String, _ t: TimeInterval?) -> CockpitTaskItem {
            CockpitTaskItem(id: id, title: id, statusRaw: "in_progress", origin: .repoLedger,
                            updated: t.map { Date(timeIntervalSince1970: $0) }, badge: id)
        }
        let groups = CockpitTaskLedger.bands([item("a", nil), item("b", 5), item("c", nil)])
        XCTAssertEqual(groups[0].items.map(\.id), ["b", "c", "a"])
    }

    func testEmptyInputStillReturnsThreeBands() {
        let groups = CockpitTaskLedger.bands([])
        XCTAssertEqual(groups.map(\.band), [.doing, .next, .done])
        XCTAssertTrue(groups.allSatisfy { $0.items.isEmpty && $0.hiddenCount == 0 })
    }
}
