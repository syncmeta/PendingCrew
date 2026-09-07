import XCTest
import Foundation

/// 侧栏「总机长视图」（Todo #102）分段推导的守卫。
///
/// 这个视图存在的理由是**收敛**（人类原话「现在消息太多太乱了」），所以这里钉的
/// 不是「排得好看」，而是几件错了就会让收敛失效的事：等人回应的不许被折进安静里、
/// 后代的 Todo 不许算到祖先头上、空段不许画标题、同一份输入两次渲染顺序必须一致。
final class CrewChiefOverviewTests: XCTestCase {

    // MARK: - helpers

    private func crew(_ id: String, title: String? = nil, parents: [String] = [],
                      updatedAt: String = "2020-01-01T00:00:00Z") -> CrewSummary {
        CrewSummary(id: id, title: title ?? id, responsibleSubjectId: "s",
                    runtimeLocation: "local_host", captainBotId: nil, status: nil,
                    createdAt: "", updatedAt: updatedAt, parentCrewIds: parents,
                    captainAgentKind: nil, machineId: nil)
    }

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func ago(_ seconds: TimeInterval) -> Date { now.addingTimeInterval(-seconds) }

    private func sections(
        _ crews: [CrewSummary],
        unanswered: [String: Int] = [:],
        activity: [String: Date] = [:],
        quietAfter: TimeInterval = CrewChiefOverview.defaultQuietAfter
    ) -> [CrewChiefOverview.Group] {
        CrewChiefOverview.sections(
            crews: crews,
            unanswered: { unanswered[$0.id] ?? 0 },
            activity: { activity[$0.id] },
            now: now,
            quietAfter: quietAfter)
    }

    // MARK: - 分段判据

    func testAwaitingHumanWinsOverStaleness() {
        // 这条是整个视图的意义所在：等人回应的哪怕十天没动静，也**不能**被划进
        // 「安静」折起来 —— 它没动静正是因为在等人。
        let groups = sections(
            [crew("a")],
            unanswered: ["a": 1],
            activity: ["a": ago(10 * 24 * 60 * 60)])
        XCTAssertEqual(groups.map(\.section), [.awaitingHuman])
    }

    func testRunningAndQuietSplitOnTheWindow() {
        let groups = sections(
            [crew("fresh"), crew("stale")],
            activity: ["fresh": ago(60 * 60), "stale": ago(5 * 60 * 60)])
        XCTAssertEqual(groups.map(\.section), [.running, .quiet])
        XCTAssertEqual(groups[0].entries.map(\.id), ["fresh"])
        XCTAssertEqual(groups[1].entries.map(\.id), ["stale"])
    }

    func testWindowBoundaryIsInclusive() {
        // 正好卡在窗口边界上算「还在跑」—— 边界两侧各钉一发，免得改窗口时悄悄翻边。
        let groups = sections(
            [crew("edge")],
            activity: ["edge": ago(CrewChiefOverview.defaultQuietAfter)])
        XCTAssertEqual(groups.map(\.section), [.running])

        let past = sections(
            [crew("edge")],
            activity: ["edge": ago(CrewChiefOverview.defaultQuietAfter + 1)])
        XCTAssertEqual(past.map(\.section), [.quiet])
    }

    func testNeverActiveGoesQuietNotRunning() {
        // 活动时间解析不出来（脏时间戳 / 从来没动静）→ 安静，不能混进「还在跑」。
        XCTAssertEqual(sections([crew("a")]).map(\.section), [.quiet])
    }

    func testFutureActivityCountsAsRunning() {
        // 时钟漂移 / 手改数据造出的未来时间戳，不该掉进安静里被折起来。
        let groups = sections([crew("a")], activity: ["a": now.addingTimeInterval(3600)])
        XCTAssertEqual(groups.map(\.section), [.running])
    }

    // MARK: - 收敛本身

    func testEmptySectionsAreNotEmitted() {
        // 空段不返回：画一个「在等你回应 0」的标题，就是在给一个本该收敛的界面
        // 加噪音。
        let groups = sections([crew("a")], activity: ["a": ago(60)])
        XCTAssertEqual(groups.map(\.section), [.running])
    }

    func testNoCrewsYieldsNoGroups() {
        XCTAssertTrue(sections([]).isEmpty)
    }

    func testSectionOrderIsFixedRegardlessOfInputOrder() {
        let groups = sections(
            [crew("quiet"), crew("await"), crew("run")],
            unanswered: ["await": 1],
            activity: ["run": ago(60), "quiet": ago(99 * 60 * 60)])
        XCTAssertEqual(groups.map(\.section), [.awaitingHuman, .running, .quiet])
    }

    func testDescendantTodosAreNotChargedToAncestor() {
        // 扁平列表里后代自己占一行，把它的条数再算进祖先，同一条 Todo 会出现两次。
        // 调用方喂的是 `ownUnanswered`，这里钉的是「只按喂进来的数分段」。
        let parent = crew("p")
        let child = crew("c", parents: ["p"])
        let groups = sections([parent, child], unanswered: ["c": 2], activity: ["p": ago(60)])
        XCTAssertEqual(groups.map(\.section), [.awaitingHuman, .running])
        XCTAssertEqual(groups[0].entries.map(\.id), ["c"])
        XCTAssertEqual(groups[1].entries.map(\.id), ["p"])
    }

    // MARK: - 段内排序

    func testAwaitingSortsByDebtThenActivity() {
        let groups = sections(
            [crew("one"), crew("three"), crew("two")],
            unanswered: ["one": 1, "three": 3, "two": 2],
            activity: ["one": ago(60), "three": ago(600), "two": ago(300)])
        XCTAssertEqual(groups[0].entries.map(\.id), ["three", "two", "one"])
    }

    func testRunningSortsByActivityDescending() {
        let groups = sections(
            [crew("old"), crew("new")],
            activity: ["old": ago(600), "new": ago(60)])
        XCTAssertEqual(groups[0].entries.map(\.id), ["new", "old"])
    }

    func testOrderIsTotalSoRowsDoNotJump() {
        // 全序：条数、时间、标题全打平时仍按 id 定序。少了这一层，同一份数据每次
        // 渲染顺序都可能不同 —— 侧栏最不能忍的就是行自己跳。
        let same = ago(120)
        let groups = sections(
            [crew("b", title: "同名"), crew("a", title: "同名")],
            activity: ["a": same, "b": same])
        XCTAssertEqual(groups[0].entries.map(\.id), ["a", "b"])
    }

    func testTitleBreaksTieBeforeId() {
        // 标题先于 id 生效：id 倒着排也要按标题出。
        // **刻意用 ASCII 标题**：第一版这里写的是两个汉字，测试红了 —— 汉字的
        // `localizedCompare` 顺序不是我以为的那样。这条测的是「标题压过 id」这个
        // 规则，不是中文排序规则，所以不该把一个我说不准的口径混进判据里。
        let same = ago(120)
        let groups = sections(
            [crew("z", title: "Alpha"), crew("a", title: "Beta")],
            activity: ["a": same, "z": same])
        XCTAssertEqual(groups[0].entries.map(\.id), ["z", "a"])
    }

    // MARK: - 视图模式

    func testChiefIsAThirdModeAndNotTheDefault() {
        // 新视图是增量：没切过仍然停在层级视图，不动任何人的肌肉记忆。
        XCTAssertEqual(CrewSidebarViewMode.default, .hierarchy)
        XCTAssertEqual(CrewSidebarViewMode.allCases, [.hierarchy, .timeline, .chief])
        XCTAssertEqual(CrewSidebarViewMode.resolve(rawValue: "chief"), .chief)
        XCTAssertEqual(CrewSidebarViewMode.chief.label, "总机长")
    }

    func testQuietIsTheOnlySectionCollapsedByDefault() {
        let collapsed = CrewChiefOverview.Section.allCases.filter(\.collapsedByDefault)
        XCTAssertEqual(collapsed, [.quiet])
    }
}
