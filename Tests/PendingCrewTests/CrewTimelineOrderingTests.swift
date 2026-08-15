import XCTest
import Foundation

/// 侧栏「时间流视图」（Todo #50）的纯逻辑守卫：活动时间取值口径、倒序、血缘次要行、
/// 视图模式持久化解析。GUI 观感（切换手感 / 色条 / 行高）验不到，那部分挂 QA #443。
final class CrewTimelineOrderingTests: XCTestCase {

    // MARK: - helpers

    private func crew(_ id: String, title: String? = nil, parents: [String] = [],
                      updatedAt: String = "2020-01-01T00:00:00Z") -> CrewSummary {
        CrewSummary(id: id, title: title ?? id, responsibleSubjectId: "s",
                    runtimeLocation: "local_host", captainBotId: nil, status: nil,
                    createdAt: "", updatedAt: updatedAt, parentCrewIds: parents,
                    captainAgentKind: nil, machineId: nil)
    }

    private func date(_ iso: String) -> Date {
        guard let d = CrewTimestamp.parse(iso) else {
            XCTFail("测试用时间串解析不了: \(iso)")
            return Date(timeIntervalSince1970: 0)
        }
        return d
    }

    // MARK: - 活动时间口径（两种视图的单一真值）

    func testActivityPrefersLastMessageOverCrewUpdatedAt() {
        // 白板有消息就用消息时间 —— crew.updatedAt 只在创建/改名时写，拿它当
        // 活动时间会一直显示成"创建时间"。
        let resolved = CrewActivityTime.resolve(
            lastMessageCreatedAt: "2026-08-12T10:00:00Z",
            crewUpdatedAt: "2020-01-01T00:00:00Z")
        XCTAssertEqual(resolved, date("2026-08-12T10:00:00Z"))
    }

    func testActivityFallsBackToUpdatedAtWhenNoMessage() {
        let resolved = CrewActivityTime.resolve(
            lastMessageCreatedAt: nil, crewUpdatedAt: "2026-08-01T09:00:00Z")
        XCTAssertEqual(resolved, date("2026-08-01T09:00:00Z"))
    }

    func testActivityParsesBothFractionalAndPlainSeconds() {
        // 本机写不带小数秒，relay 从 edge 搬进来的带 —— 两种都得认。
        XCTAssertNotNil(CrewActivityTime.resolve(
            lastMessageCreatedAt: "2026-08-12T10:00:00.123Z", crewUpdatedAt: nil))
        XCTAssertNotNil(CrewActivityTime.resolve(
            lastMessageCreatedAt: "2026-08-12T10:00:00Z", crewUpdatedAt: nil))
    }

    func testActivityIsNilWhenBothUnparsable() {
        XCTAssertNil(CrewActivityTime.resolve(lastMessageCreatedAt: "", crewUpdatedAt: ""))
        XCTAssertNil(CrewActivityTime.resolve(lastMessageCreatedAt: nil, crewUpdatedAt: nil))
    }

    func testTimelineActivityIsTheSameFieldTheRowShows() {
        // 时间流的排序键与行尾时间 pill 必须同源：都从末条消息的 createdAt 走
        // `CrewActivityTime.resolve`。两处对不上就是新 bug（Todo #50 明确点名）。
        let c = crew("c1", updatedAt: "2020-01-01T00:00:00Z")
        let lastMessageISO = "2026-08-12T12:34:56Z"
        let rowPillDate = CrewActivityTime.resolve(
            lastMessageCreatedAt: lastMessageISO, crewUpdatedAt: c.updatedAt)
        let entries = CrewTimelineOrdering.ordered(crews: [c]) { crew in
            CrewActivityTime.resolve(
                lastMessageCreatedAt: lastMessageISO, crewUpdatedAt: crew.updatedAt)
        }
        XCTAssertEqual(entries.first?.activity, rowPillDate)
        XCTAssertEqual(entries.first?.activity, date(lastMessageISO))
    }

    // MARK: - 排序

    func testOrderedIsNewestFirst() {
        let a = crew("a"), b = crew("b"), c = crew("c")
        let times = [
            "a": "2026-08-10T10:00:00Z",
            "b": "2026-08-12T10:00:00Z",
            "c": "2026-08-11T10:00:00Z",
        ]
        let entries = CrewTimelineOrdering.ordered(crews: [a, b, c]) {
            CrewActivityTime.resolve(lastMessageCreatedAt: times[$0.id], crewUpdatedAt: nil)
        }
        XCTAssertEqual(entries.map(\.crew.id), ["b", "c", "a"])
    }

    func testCrewsWithoutActivitySinkToBottom() {
        let a = crew("a"), b = crew("b"), c = crew("c")
        let times: [String: String] = ["b": "2026-08-01T10:00:00Z"]
        let entries = CrewTimelineOrdering.ordered(crews: [a, b, c]) {
            CrewActivityTime.resolve(lastMessageCreatedAt: times[$0.id], crewUpdatedAt: nil)
        }
        XCTAssertEqual(entries.map(\.crew.id), ["b", "a", "c"]) // 有时间的在前，其余按标题
        XCTAssertNil(entries[1].activity)
        XCTAssertNil(entries[2].activity)
    }

    func testTiesAreStableByTitleThenId() {
        // 同一时刻不能因为字典/输入顺序每次渲染跳来跳去。标题用 ASCII —— 中日韩
        // 标题之间谁先谁后由**当前 locale 的排序规则**决定（同一次运行内恒定，
        // 但不该在单测里把某个 locale 的结果写死）。
        let same = "2026-08-12T10:00:00Z"
        let x = crew("id-2", title: "Alpha"), y = crew("id-1", title: "Alpha"), z = crew("id-3", title: "Beta")
        let order = { (input: [CrewSummary]) -> [String] in
            CrewTimelineOrdering.ordered(crews: input) { _ in
                CrewActivityTime.resolve(lastMessageCreatedAt: same, crewUpdatedAt: nil)
            }.map(\.crew.id)
        }
        // 同标题按 id；不同标题按标题。
        XCTAssertEqual(order([z, x, y]), ["id-1", "id-2", "id-3"])
        // 输入顺序不影响结果 —— 换个进场顺序仍是同一串。
        XCTAssertEqual(order([y, z, x]), ["id-1", "id-2", "id-3"])
    }

    func testOrderedKeepsEveryCrew() {
        let crews = (1...20).map { crew("c\($0)") }
        let entries = CrewTimelineOrdering.ordered(crews: crews) { _ in nil }
        XCTAssertEqual(Set(entries.map(\.crew.id)), Set(crews.map(\.id)))
        XCTAssertEqual(entries.count, 20)
    }

    // MARK: - 血缘次要行

    func testLineageLineHiddenWhenDirectParentIsTheRootBadge() {
        // 直接父就是黄字已经标出来的根 → 不重复画。
        let parent = crew("p", title: "PendingCrew")
        let child = crew("c", title: "驾驶舱改造", parents: ["p"])
        let line = CrewTimelineOrdering.lineageLine(
            for: child, crewsById: ["p": parent, "c": child], rootTitles: ["PendingCrew"])
        XCTAssertNil(line)
    }

    func testLineageLineShowsDirectParentWhenDeeperThanRoot() {
        // 三层：黄字标的是根 A，中间那层 B 只有这一行说得出来。
        let root = crew("a", title: "A")
        let mid = crew("b", title: "B", parents: ["a"])
        let leaf = crew("c", title: "C", parents: ["b"])
        let line = CrewTimelineOrdering.lineageLine(
            for: leaf, crewsById: ["a": root, "b": mid, "c": leaf], rootTitles: ["A"])
        XCTAssertEqual(line, "B")
    }

    func testLineageLineJoinsMultipleParents() {
        let p1 = crew("p1", title: "甲"), p2 = crew("p2", title: "乙")
        let child = crew("c", title: "丙", parents: ["p1", "p2"])
        let line = CrewTimelineOrdering.lineageLine(
            for: child, crewsById: ["p1": p1, "p2": p2, "c": child], rootTitles: ["根"])
        XCTAssertEqual(line, "甲、乙")
    }

    func testLineageLineNilForRootCrewAndDirtyParentRefs() {
        let root = crew("r", title: "根")
        XCTAssertNil(CrewTimelineOrdering.lineageLine(
            for: root, crewsById: ["r": root], rootTitles: []))
        // 父 id 在表里查不到（脏数据/半同步）→ 不画裸 uuid。
        let orphan = crew("o", title: "孤", parents: ["missing"])
        XCTAssertNil(CrewTimelineOrdering.lineageLine(
            for: orphan, crewsById: ["o": orphan], rootTitles: []))
    }

    // MARK: - 视图模式持久化

    func testViewModeDefaultsToHierarchy() {
        XCTAssertEqual(CrewSidebarViewMode.default, .hierarchy)
        XCTAssertEqual(CrewSidebarViewMode.resolve(rawValue: nil), .hierarchy)
        XCTAssertEqual(CrewSidebarViewMode.resolve(rawValue: ""), .hierarchy)
        XCTAssertEqual(CrewSidebarViewMode.resolve(rawValue: "garbage"), .hierarchy)
    }

    func testViewModeRoundTripsThroughRawValue() {
        for mode in CrewSidebarViewMode.allCases {
            XCTAssertEqual(CrewSidebarViewMode.resolve(rawValue: mode.rawValue), mode)
        }
        XCTAssertEqual(CrewSidebarViewMode.timeline.rawValue, "timeline")
    }
}
