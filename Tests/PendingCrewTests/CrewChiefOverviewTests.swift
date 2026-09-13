import XCTest
import Foundation

/// 侧栏「总机长视图」的排序守卫（Todo #102；口径按人类 #113 改过 —— **不分类，只排序**）。
///
/// 这里钉的是几件错了就会让这个视图变得不可信的事：
/// agent 的排布只能**叠在**确定性基础序上、一份过期的排布不许让任何一行消失、
/// 没人排过时顺序必须仍然算得出来、同一份输入两次渲染顺序必须一致。
final class CrewChiefOverviewTests: XCTestCase {

    private func crew(_ id: String, title: String? = nil, parents: [String] = [],
                      updatedAt: String = "2020-01-01T00:00:00Z") -> CrewSummary {
        CrewSummary(id: id, title: title ?? id, responsibleSubjectId: "s",
                    runtimeLocation: "local_host", captainBotId: nil, status: nil,
                    createdAt: "", updatedAt: updatedAt, parentCrewIds: parents,
                    captainAgentKind: nil, machineId: nil)
    }

    private let now = Date(timeIntervalSince1970: 1_800_000_000)
    private func ago(_ seconds: TimeInterval) -> Date { now.addingTimeInterval(-seconds) }

    private func ordered(_ crews: [CrewSummary], activity: [String: Date] = [:],
                         arrangement: [String] = []) -> [CrewChiefOverview.Entry] {
        CrewChiefOverview.ordered(crews: crews, activity: { activity[$0.id] },
                                  arrangement: arrangement)
    }

    // MARK: - 基础序

    func testBaseOrderIsMostRecentFirst() {
        let out = ordered([crew("old"), crew("new"), crew("mid")],
                          activity: ["old": ago(900), "mid": ago(300), "new": ago(60)])
        XCTAssertEqual(out.map(\.id), ["new", "mid", "old"])
    }

    func testNeverActiveSinksToTheBottom() {
        let out = ordered([crew("quiet"), crew("live")], activity: ["live": ago(60)])
        XCTAssertEqual(out.map(\.id), ["live", "quiet"])
    }

    func testOrderIsTotalSoRowsDoNotJump() {
        // 全序：时间、标题全打平时仍按 id 定序。少了这一层，同一份数据每次渲染
        // 顺序都可能不同 —— 侧栏最不能忍的就是行自己跳。
        let same = ago(120)
        let out = ordered([crew("b", title: "同名"), crew("a", title: "同名")],
                          activity: ["a": same, "b": same])
        XCTAssertEqual(out.map(\.id), ["a", "b"])
    }

    // MARK: - agent 排布是覆盖层

    func testArrangementPinsToTheFrontAndKeepsTheRestInBaseOrder() {
        let out = ordered([crew("a"), crew("b"), crew("c")],
                          activity: ["a": ago(60), "b": ago(600), "c": ago(6000)],
                          arrangement: ["c"])
        XCTAssertEqual(out.map(\.id), ["c", "a", "b"])
        XCTAssertEqual(out.map(\.pinnedByArrangement), [true, false, false])
    }

    func testArrangementKeepsItsOwnOrderNotTheBaseOne() {
        let out = ordered([crew("a"), crew("b")],
                          activity: ["a": ago(60), "b": ago(600)],
                          arrangement: ["b", "a"])
        XCTAssertEqual(out.map(\.id), ["b", "a"])
    }

    func testEmptyArrangementFallsBackToBaseOrderNotToNothing() {
        // 这条是覆盖层的地基：agent 没跑、排布是空的 —— 侧栏照常有序，不空白。
        let out = ordered([crew("a"), crew("b")],
                          activity: ["a": ago(600), "b": ago(60)], arrangement: [])
        XCTAssertEqual(out.map(\.id), ["b", "a"])
        XCTAssertFalse(out.contains { $0.pinnedByArrangement })
    }

    func testStaleArrangementIdsAreIgnoredAndNoRowDisappears() {
        // 一份过期的排布（里面的 crew 已经删了/藏了）**顶多是没顶上来**，
        // 绝不能让任何一行消失 —— 少一行比排错序严重得多。
        let out = ordered([crew("a"), crew("b")],
                          activity: ["a": ago(60), "b": ago(600)],
                          arrangement: ["ghost", "b", "also-gone"])
        XCTAssertEqual(out.map(\.id), ["b", "a"])
        XCTAssertEqual(out.count, 2)
    }

    func testDuplicateIdsInArrangementAreNotDoubled() {
        let out = ordered([crew("a"), crew("b")],
                          activity: ["a": ago(60), "b": ago(600)],
                          arrangement: ["b", "b", "a"])
        XCTAssertEqual(out.map(\.id), ["b", "a"])
    }

    func testArrangementCoveringEverythingStillListsEverything() {
        let out = ordered([crew("a"), crew("b"), crew("c")],
                          activity: ["a": ago(60), "b": ago(600), "c": ago(6000)],
                          arrangement: ["c", "b", "a"])
        XCTAssertEqual(out.map(\.id), ["c", "b", "a"])
        XCTAssertTrue(out.allSatisfy(\.pinnedByArrangement))
    }

    // MARK: - 视图模式

    func testChiefIsAThirdModeAndNotTheDefault() {
        XCTAssertEqual(CrewSidebarViewMode.default, .hierarchy)
        XCTAssertEqual(CrewSidebarViewMode.allCases, [.hierarchy, .timeline, .chief])
        XCTAssertEqual(CrewSidebarViewMode.resolve(rawValue: "chief"), .chief)
        XCTAssertEqual(CrewSidebarViewMode.chief.label, "总机长")
    }

    // MARK: - 排布落盘

    func testArrangementRoundTripsThroughDisk() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("arrangement-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let url = CrewArrangementStore.fileURL(dataRoot: dir)

        XCTAssertNil(CrewArrangementStore.load(at: url), "还没写过就该是「没有排布」")

        let arrangement = CrewArrangement(
            crewIds: ["c1", "c2"], reason: "这两个他今天在改",
            bySessionId: "sess-1", bySenderName: "机长",
            createdAt: "2026-09-08T09:00:00Z")
        XCTAssertTrue(CrewArrangementStore.save(arrangement, to: url))
        XCTAssertEqual(CrewArrangementStore.load(at: url), arrangement)

        XCTAssertTrue(CrewArrangementStore.clear(at: url))
        XCTAssertNil(CrewArrangementStore.load(at: url))
        XCTAssertTrue(CrewArrangementStore.clear(at: url), "本来就不在也算撤成功")
    }

    func testCorruptArrangementReadsAsNoArrangementNotAsCrash() {
        // 文件坏了 = 没有排布 = 退回基础序。**不许因此让侧栏空掉或炸掉。**
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("arrangement-bad-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let url = CrewArrangementStore.fileURL(dataRoot: dir)
        try? Data("{ 这不是 json".utf8).write(to: url)
        XCTAssertNil(CrewArrangementStore.load(at: url))
    }

    func testHelperDerivesDataRootFromTheWhiteboardDirectory() {
        // helper 只拿得到白板目录，数据根是它的上一级 —— 与 orgTreeLines 同一条推导。
        let whiteboards = URL(fileURLWithPath: "/tmp/pc/whiteboards")
        XCTAssertEqual(CrewArrangementStore.fileURL(whiteboardDirectory: whiteboards).path,
                       "/tmp/pc/crew-arrangement.json")
        XCTAssertEqual(CrewChiefSummaryStore.fileURL(whiteboardDirectory: whiteboards).path,
                       "/tmp/pc/crew-chief-summaries.json")
    }

    // MARK: - 总机长给每个机组写的摘要（人类 Todo #145）

    private func tempRoot() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("chief-summaries-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        return dir
    }

    func testSummariesRoundTripAndMergeKeepsTheOnesNotRewritten() throws {
        let url = CrewChiefSummaryStore.fileURL(dataRoot: try tempRoot())
        XCTAssertEqual(CrewChiefSummaryStore.load(at: url), [:])
        XCTAssertEqual(try CrewChiefSummaryStore.loadReportingFailure(at: url), [:],
                       "文件还不在 = 空表，不是读失败")

        let first = CrewChiefSummaryStore.merging(
            [:], incoming: ["a": "在改侧栏", "b": "等 CI"],
            writtenAt: "2026-09-13T01:00:00Z", bySessionId: "chief", bySenderName: "总机长")
        XCTAssertTrue(CrewChiefSummaryStore.save(first, to: url))
        XCTAssertEqual(CrewChiefSummaryStore.load(at: url), first)

        let second = CrewChiefSummaryStore.merging(
            try CrewChiefSummaryStore.loadReportingFailure(at: url), incoming: ["a": "侧栏改完了"],
            writtenAt: "2026-09-13T02:00:00Z", bySessionId: "chief", bySenderName: "总机长")
        XCTAssertEqual(second["a"]?.text, "侧栏改完了")
        XCTAssertEqual(second["a"]?.writtenAt, "2026-09-13T02:00:00Z")
        // 没重写的那句**连同它自己的写入时刻**原样留着 —— 共用一个时刻就会把它说成刚写的。
        XCTAssertEqual(second["b"], first["b"])
    }

    func testUnreadableSummariesThrowOnTheWritePathButReadAsEmptyInTheUI() throws {
        let url = CrewChiefSummaryStore.fileURL(dataRoot: try tempRoot())
        try Data("{ 这不是 json".utf8).write(to: url)
        // 写路径：抛。当成空表去合并，会拿这次这几句盖掉其它机组的旧摘要。
        XCTAssertThrowsError(try CrewChiefSummaryStore.loadReportingFailure(at: url))
        // 界面：没有摘要 = 每行退回机长自报 / 还没有，不炸。
        XCTAssertEqual(CrewChiefSummaryStore.load(at: url), [:])
    }

    func testWithdrawingTheArrangementDoesNotWipeTheSummaries() throws {
        // `arrange_crews(crew_ids: [])` 是删排布文件。摘要若跟它住一起，撤一次顺序就全抹了。
        let root = try tempRoot()
        let arrangementURL = CrewArrangementStore.fileURL(dataRoot: root)
        let summariesURL = CrewChiefSummaryStore.fileURL(dataRoot: root)
        let summaries = ["a": CrewChiefSummary(text: "在改侧栏", writtenAt: "2026-09-13T01:00:00Z",
                                               bySessionId: nil, bySenderName: nil)]
        XCTAssertTrue(CrewArrangementStore.save(CrewArrangement(
            crewIds: ["a"], reason: "r", bySessionId: nil, bySenderName: nil,
            createdAt: "2026-09-13T01:00:00Z"), to: arrangementURL))
        XCTAssertTrue(CrewChiefSummaryStore.save(summaries, to: summariesURL))
        XCTAssertTrue(CrewArrangementStore.clear(at: arrangementURL))
        XCTAssertEqual(CrewChiefSummaryStore.load(at: summariesURL), summaries)
    }

    func testSummaryIntakeShapes() {
        XCTAssertEqual(CrewChiefSummaryIntake.decide(nil), .none)
        XCTAssertEqual(CrewChiefSummaryIntake.decide(NSNull()), .none)
        // 形状不对：整次什么都不写。
        guard case .refused = CrewChiefSummaryIntake.decide("一句话") else {
            return XCTFail("不是对象也收下了")
        }
        guard case .refused = CrewChiefSummaryIntake.decide(["a": 3]) else {
            return XCTFail("值不是字符串也收下了")
        }

        // 长度只提醒、不拦（机长 2026-09-13 定的默认值，人类 Todo #7 还没拍）。
        let long = String(repeating: "字", count: CrewStatusIntake.hintLength + 1)
        let veryLong = String(repeating: "字", count: 500)
        guard case let .accepted(got, notes) = CrewChiefSummaryIntake.decide(
            ["a": "  在改侧栏 ", "b": "   ", "c": long, "d": veryLong]) else {
            return XCTFail("形状对的竟然没收")
        }
        XCTAssertEqual(got, ["a": "在改侧栏", "c": long, "d": veryLong])
        XCTAssertTrue(notes.contains { $0.contains("b") && $0.contains("空") }, "\(notes)")
        XCTAssertTrue(notes.contains { $0.hasPrefix("c ") && $0.contains("照原样写进去了") }, "\(notes)")
    }
}
