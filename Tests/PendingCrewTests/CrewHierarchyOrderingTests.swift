import XCTest
import Foundation

/// 侧栏「层级视图」按最新活动排序（人类 Todo #67）的纯逻辑守卫。
///
/// GUI 观感（列表在眼皮底下重排难不难受）验不到 —— 那部分要人实际看，见群里那条。
final class CrewHierarchyOrderingTests: XCTestCase {

    private func crew(_ id: String, title: String? = nil, parents: [String] = [],
                      createdAt: String = "2020-01-01T00:00:00Z") -> CrewSummary {
        CrewSummary(id: id, title: title ?? id, responsibleSubjectId: "s",
                    runtimeLocation: "local_host", captainBotId: nil, status: nil,
                    createdAt: createdAt, updatedAt: createdAt, parentCrewIds: parents,
                    captainAgentKind: nil, machineId: nil)
    }

    private func date(_ iso: String) -> Date {
        guard let d = CrewTimestamp.parse(iso) else {
            XCTFail("测试用时间串解析不了: \(iso)"); return Date(timeIntervalSince1970: 0)
        }
        return d
    }

    /// 按 id 喂活动时间；没列到的 = 从没说过话。
    private func activity(_ table: [String: String]) -> (CrewSummary) -> Date? {
        { crew in table[crew.id].map(self.date) }
    }

    // MARK: - 同级：从新到旧

    func testSiblingsSortNewestFirst() {
        let crews = [crew("a"), crew("b"), crew("c")]
        let keys = CrewHierarchyOrdering.activityKeys(crews: crews, activity: activity([
            "a": "2026-08-01T00:00:00Z",
            "b": "2026-08-25T00:00:00Z",
            "c": "2026-08-10T00:00:00Z",
        ]))
        XCTAssertEqual(CrewHierarchyOrdering.sortedSiblings(crews, keys: keys).map(\.id), ["b", "c", "a"])
    }

    // MARK: - 父的键 = max(自己, 全部后代)

    func testQuietParentRidesOnNoisyChild() {
        // 这条是整个 #67 里最容易做错、也最要紧的一条：安静的父部门底下有个正在
        // 刷屏的子部门，父要是按自己的时间沉到底，那个正在动的子部门就找不到了。
        let quietParent = crew("p")
        let noisyChild = crew("c", parents: ["p"])
        let busyOther = crew("o")
        let all = [quietParent, noisyChild, busyOther]
        let keys = CrewHierarchyOrdering.activityKeys(crews: all, activity: activity([
            "p": "2026-01-01T00:00:00Z",
            "c": "2026-08-25T12:00:00Z",
            "o": "2026-08-20T00:00:00Z",
        ]))
        XCTAssertEqual(keys["p"], self.date("2026-08-25T12:00:00Z"), "父要冒泡到后代的时间")
        XCTAssertEqual(
            CrewHierarchyOrdering.sortedSiblings([quietParent, busyOther], keys: keys).map(\.id),
            ["p", "o"], "父靠子的动静排在前面")
    }

    func testActivityBubblesUpThroughGrandchild() {
        let all = [crew("p"), crew("m", parents: ["p"]), crew("g", parents: ["m"])]
        let keys = CrewHierarchyOrdering.activityKeys(crews: all, activity: activity([
            "p": "2026-01-01T00:00:00Z",
            "m": "2026-02-01T00:00:00Z",
            "g": "2026-08-25T00:00:00Z",
        ]))
        // 隔一层也要冒上来 —— 组织树不止两层。
        XCTAssertEqual(keys["p"], self.date("2026-08-25T00:00:00Z"))
        XCTAssertEqual(keys["m"], self.date("2026-08-25T00:00:00Z"))
    }

    func testParentKeepsOwnTimeWhenNewerThanChildren() {
        let all = [crew("p"), crew("c", parents: ["p"])]
        let keys = CrewHierarchyOrdering.activityKeys(crews: all, activity: activity([
            "p": "2026-08-25T00:00:00Z",
            "c": "2026-01-01T00:00:00Z",
        ]))
        XCTAssertEqual(keys["p"], self.date("2026-08-25T00:00:00Z"))
    }

    // MARK: - 从没说过话的

    func testSilentCrewFallsBackToCreatedAtNotEpoch() {
        // 回落由调用方注入（视图侧用 createdAt）。这里钉的是：**回落值真的参与排序**，
        // 不是被当成 nil 沉到 1970 那一档。
        let young = crew("young", createdAt: "2026-08-24T00:00:00Z")
        let old = crew("old", createdAt: "2024-01-01T00:00:00Z")
        let keys = CrewHierarchyOrdering.activityKeys(crews: [young, old]) { c in
            CrewActivityTime.resolve(lastMessageCreatedAt: nil, crewUpdatedAt: c.createdAt)
        }
        XCTAssertEqual(CrewHierarchyOrdering.sortedSiblings([old, young], keys: keys).map(\.id),
                       ["young", "old"])
    }

    func testCrewWithNoKeyAtAllSinksToBottomStably() {
        // 时间串脏到解不出来 → 没有键 → 沉底，且同为无键时按标题稳定排（不跳）。
        // 标题用 ASCII：中文的本地化排序取决于 locale 的排序规则，那不是这条要钉的东西。
        let a = crew("a", title: "Alpha", createdAt: "坏时间")
        let b = crew("b", title: "Beta", createdAt: "坏时间")
        let live = crew("z", title: "在动的")
        let keys = CrewHierarchyOrdering.activityKeys(crews: [a, b, live]) { c in
            c.id == "z" ? self.date("2026-08-25T00:00:00Z") : CrewTimestamp.parse(c.createdAt)
        }
        XCTAssertEqual(CrewHierarchyOrdering.sortedSiblings([b, a, live], keys: keys).map(\.id),
                       ["z", "a", "b"])
    }

    // MARK: - 多父 / 环

    func testMultiParentChildIsSortedUnderEachParent() {
        // 多父 crew 在每个父下各出现一次 —— 排序必须逐个父分别成立。
        let shared = crew("shared", parents: ["p1", "p2"])
        let onlyP1 = crew("k1", parents: ["p1"])
        let onlyP2 = crew("k2", parents: ["p2"])
        let all = [crew("p1"), crew("p2"), shared, onlyP1, onlyP2]
        let keys = CrewHierarchyOrdering.activityKeys(crews: all, activity: activity([
            "shared": "2026-08-25T00:00:00Z",
            "k1": "2026-08-01T00:00:00Z",
            "k2": "2026-08-26T00:00:00Z",
        ]))
        let map = CrewHierarchyOrdering.sortedChildMap(crews: all, keys: keys)
        XCTAssertEqual(map["p1"]?.map(\.id), ["shared", "k1"])
        XCTAssertEqual(map["p2"]?.map(\.id), ["k2", "shared"])
    }

    func testCycleInParentEdgesTerminates() {
        // 父边理论上无环（attachParent 禁了），但脏数据不该让侧栏转不出来。
        let a = crew("a", parents: ["b"])
        let b = crew("b", parents: ["a"])
        let keys = CrewHierarchyOrdering.activityKeys(crews: [a, b], activity: activity([
            "a": "2026-08-25T00:00:00Z",
        ]))
        XCTAssertEqual(keys["a"], self.date("2026-08-25T00:00:00Z"))
        XCTAssertEqual(keys["b"], self.date("2026-08-25T00:00:00Z"))
    }

    func testSelfLoopTerminates() {
        let a = crew("a", parents: ["a"])
        let keys = CrewHierarchyOrdering.activityKeys(crews: [a], activity: activity([
            "a": "2026-08-25T00:00:00Z",
        ]))
        XCTAssertEqual(keys["a"], self.date("2026-08-25T00:00:00Z"))
    }

    // MARK: - 与时间流视图同一套口径

    func testTieBreakMatchesTimelineView() {
        // 两个视图对「谁更新」的说法必须一致，否则同一个人在两个 tab 里看到两种事实。
        let sameTime = "2026-08-25T00:00:00Z"
        let a = crew("a", title: "Bravo")
        let b = crew("b", title: "Alfa")
        let keys = CrewHierarchyOrdering.activityKeys(crews: [a, b], activity: activity([
            "a": sameTime, "b": sameTime,
        ]))
        let hierarchy = CrewHierarchyOrdering.sortedSiblings([a, b], keys: keys).map(\.id)
        let timeline = CrewTimelineOrdering.ordered(crews: [a, b]) { _ in self.date(sameTime) }
            .map(\.crew.id)
        XCTAssertEqual(hierarchy, timeline)
    }
}
