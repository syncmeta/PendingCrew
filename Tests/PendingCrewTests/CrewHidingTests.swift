import XCTest

/// 侧栏「手动藏起来一个 crew」的判定层单测。
///
/// 钉的都是**判错了人就看不出来**的地方：藏了父之后子该跟着消失而不是被提到顶层、
/// 多父 crew 还有一条露着的入口时不该消失、底下有 session 在跑时不许藏、
/// 以及未读的参照点是「藏的时刻」和「上次看过」里较晚的那个。
final class CrewHidingTests: XCTestCase {
    private func crew(_ id: String, parents: [String] = [], hiddenAt: String? = nil,
                      title: String? = nil) -> CrewSummary {
        CrewSummary(
            id: id, title: title ?? id, responsibleSubjectId: "s", runtimeLocation: "local_host",
            captainBotId: nil, status: nil, createdAt: "2026-08-26T00:00:00Z",
            updatedAt: "2026-08-26T00:00:00Z", parentCrewIds: parents,
            manuallyHiddenAt: hiddenAt)
    }

    private let t0 = "2026-08-26T10:00:00Z"

    // MARK: - 可见性

    func testNothingHiddenShowsEverything() {
        let crews = [crew("a"), crew("b", parents: ["a"])]
        XCTAssertEqual(CrewHiding.visible(crews).map(\.id), ["a", "b"])
    }

    func testHiddenCrewDisappears() {
        let crews = [crew("a", hiddenAt: t0), crew("b")]
        XCTAssertEqual(CrewHiding.visible(crews).map(\.id), ["b"])
    }

    /// **本次最容易翻的车**：藏了父，子不许被提到顶层去。侧栏显示的组织架构一旦
    /// 和实际汇报线对不上，人看到的树就是假的 —— 宁可留一个空父，也别让组织图撒谎。
    /// 这里的正确行为是整棵子树跟着消失（而它们的入口由「已隐藏的群」那行保住）。
    func testHidingAParentTakesTheWholeSubtreeWithIt() {
        let crews = [
            crew("a", hiddenAt: t0),
            crew("b", parents: ["a"]),
            crew("c", parents: ["b"]),
            crew("z"),
        ]
        XCTAssertEqual(CrewHiding.visible(crews).map(\.id), ["z"])
    }

    /// 多父 crew：只要还有**一个**可见的父，它就仍然进得去 —— 不该消失。
    func testMultiParentCrewStaysVisibleThroughItsOtherParent() {
        let crews = [
            crew("a", hiddenAt: t0),
            crew("b"),
            crew("c", parents: ["a", "b"]),
        ]
        XCTAssertEqual(Set(CrewHiding.visible(crews).map(\.id)), ["b", "c"])
    }

    /// 父边指向的 crew 不在这份名单里（跨机器 / 脏数据）→ 当根看待，
    /// 与 `CrewDAGTreeView` 的口径一致，不能因为查不到父就整个消失。
    func testCrewWithParentOutsideTheListIsTreatedAsRoot() {
        let crews = [crew("b", parents: ["missing"])]
        XCTAssertEqual(CrewHiding.visible(crews).map(\.id), ["b"])
    }

    /// 脏数据成环：既不许死循环，也**不许让环上那几个 crew 消失** —— 没人藏过
    /// 它们。少了这条自保，机器上一旦有一个环，人随便藏掉任何一个别的群，那个环
    /// 就整个从侧栏不见了（自根走不到 = 不可达）。
    func testCyclicDataStaysVisible() {
        let crews = [
            crew("a", parents: ["b"]), crew("b", parents: ["a"]),
            crew("z"), crew("hidden", hiddenAt: t0),
        ]
        XCTAssertEqual(Set(CrewHiding.visible(crews).map(\.id)), ["a", "b", "z"])
    }

    /// 环上的 crew 自己被藏了，仍然照常从侧栏消失、照常进「已隐藏的群」列表。
    func testHiddenCrewOnACycleIsStillHiddenAndListed() {
        let crews = [crew("a", parents: ["b"], hiddenAt: t0), crew("b", parents: ["a"])]
        XCTAssertEqual(CrewHiding.visible(crews).map(\.id), ["b"])
        XCTAssertEqual(
            CrewHiding.hiddenEntries(in: crews, lastActivity: { _ in nil }, lastViewed: [:])
                .map(\.id),
            ["a"])
    }

    // MARK: - 已隐藏列表列谁

    func testHiddenListListsTheCrewsWhoseEntryIsExposed() {
        let crews = [
            crew("a", hiddenAt: t0),
            crew("b", parents: ["a"]),
            crew("z"),
        ]
        let entries = CrewHiding.hiddenEntries(in: crews, lastActivity: { _ in nil }, lastViewed: [:])
        XCTAssertEqual(entries.map(\.id), ["a"])
        XCTAssertEqual(entries.first?.alsoHiddenCount, 1, "b 跟着 a 一起消失了，要数进去")
    }

    /// 藏在另一个被藏的群底下的那个**不单列** —— 单独取回它什么也不会发生，
    /// 列出来就是撒谎。等外层取回了它自会露出来。
    func testHiddenCrewNestedUnderAnotherHiddenCrewIsNotListed() {
        let crews = [
            crew("a", hiddenAt: t0),
            crew("b", parents: ["a"], hiddenAt: t0),
        ]
        let entries = CrewHiding.hiddenEntries(in: crews, lastActivity: { _ in nil }, lastViewed: [:])
        XCTAssertEqual(entries.map(\.id), ["a"])

        // a 取回之后，b 就该出现在列表里了（自愈，不需要额外的迁移动作）。
        let afterUnhidingA = [crew("a"), crew("b", parents: ["a"], hiddenAt: t0)]
        XCTAssertEqual(
            CrewHiding.hiddenEntries(in: afterUnhidingA, lastActivity: { _ in nil }, lastViewed: [:])
                .map(\.id),
            ["b"])
    }

    /// 最近藏的排最上；**不按有没有未读排** —— 来条消息就让某一行窜到顶，
    /// 是「藏起来」这个决定被推翻的另一种形式。
    func testHiddenListSortsByHiddenAtDescending() {
        let crews = [
            crew("old", hiddenAt: "2026-08-01T00:00:00Z"),
            crew("new", hiddenAt: "2026-08-25T00:00:00Z"),
        ]
        let entries = CrewHiding.hiddenEntries(
            in: crews,
            // 老的那个有未读，新的没有 —— 顺序仍按藏的时间来。
            lastActivity: { $0 == "old" ? Date(timeIntervalSince1970: 4_000_000_000) : nil },
            lastViewed: [:])
        XCTAssertEqual(entries.map(\.id), ["new", "old"])
        XCTAssertTrue(entries.first(where: { $0.id == "old" })?.hasUnread == true)
    }

    // MARK: - 未读（⑤）

    private func date(_ iso: String) -> Date { CrewTimestamp.parse(iso)! }

    func testUnreadWhenMessageArrivedAfterHiding() {
        let crews = [crew("a", hiddenAt: t0)]
        let entries = CrewHiding.hiddenEntries(
            in: crews, lastActivity: { _ in self.date("2026-08-26T11:00:00Z") }, lastViewed: [:])
        XCTAssertTrue(entries.first?.hasUnread == true)
    }

    func testNoUnreadWhenLastMessagePredatesHiding() {
        let crews = [crew("a", hiddenAt: t0)]
        let entries = CrewHiding.hiddenEntries(
            in: crews, lastActivity: { _ in self.date("2026-08-26T09:00:00Z") }, lastViewed: [:])
        XCTAssertTrue(entries.first?.hasUnread == false)
    }

    /// 点进去看过一眼，未读就该清掉 —— 群**不取回**。参照点取两者中较晚的。
    func testViewingClearsUnreadWithoutUnhiding() {
        let crews = [crew("a", hiddenAt: t0)]
        let entries = CrewHiding.hiddenEntries(
            in: crews,
            lastActivity: { _ in self.date("2026-08-26T11:00:00Z") },
            lastViewed: ["a": date("2026-08-26T11:30:00Z")])
        XCTAssertTrue(entries.first?.hasUnread == false)
        XCTAssertNotNil(entries.first?.crew.manuallyHiddenAt, "看过≠取回：还得是藏着的")
    }

    /// 藏之前看过、藏之后又来了消息 —— 仍算未读（参照点取较晚的那个 = 藏的时刻）。
    func testViewedBeforeHidingDoesNotSuppressLaterMessages() {
        let crews = [crew("a", hiddenAt: t0)]
        let entries = CrewHiding.hiddenEntries(
            in: crews,
            lastActivity: { _ in self.date("2026-08-26T11:00:00Z") },
            lastViewed: ["a": date("2026-08-26T08:00:00Z")])
        XCTAssertTrue(entries.first?.hasUnread == true)
    }

    /// 藏了父之后，**子**里来的消息一样是人看不见的动静 —— 要冒到父那一行上。
    func testUnreadLooksAtTheWholeVanishedSubtree() {
        let crews = [crew("a", hiddenAt: t0), crew("b", parents: ["a"])]
        let entries = CrewHiding.hiddenEntries(
            in: crews,
            lastActivity: { $0 == "b" ? self.date("2026-08-26T11:00:00Z") : nil },
            lastViewed: [:])
        XCTAssertTrue(entries.first?.hasUnread == true)
    }

    // MARK: - 能不能藏（④）

    func testHidingALeafIsAllowed() {
        let crews = [crew("a"), crew("z")]
        XCTAssertEqual(
            CrewHiding.decide(hiding: "a", in: crews, activeSessionCrewIds: []),
            .allowed(alsoHiddenCount: 0))
    }

    /// 子 crew 全闲着 → 放行，并告诉调用方会跟着消失几个（菜单上要说清楚）。
    func testHidingAParentWithIdleChildrenIsAllowedAndCounted() {
        let crews = [crew("a"), crew("b", parents: ["a"]), crew("c", parents: ["b"])]
        XCTAssertEqual(
            CrewHiding.decide(hiding: "a", in: crews, activeSessionCrewIds: []),
            .allowed(alsoHiddenCount: 2))
    }

    /// **④ 的正题**：子 crew 还有 session 在跑 → 拦住，并点名是哪几个。
    /// 藏了父它们就没有入口进去了，那跟删了没区别。
    func testHidingAParentWithActiveChildIsBlocked() {
        let crews = [crew("a"), crew("b", parents: ["a"], title: "在跑的子")]
        XCTAssertEqual(
            CrewHiding.decide(hiding: "a", in: crews, activeSessionCrewIds: ["b"]),
            .blocked(activeDescendantTitles: ["在跑的子"]))
    }

    /// crew **自己**有 session 在跑不拦 —— ④ 管的是「子 crew 没有入口了」，
    /// 藏自己不会让自己失去入口（「已隐藏的群」那行进得去），session 也照常干活。
    func testHidingACrewWithItsOwnRunningSessionIsAllowed() {
        let crews = [crew("a")]
        XCTAssertEqual(
            CrewHiding.decide(hiding: "a", in: crews, activeSessionCrewIds: ["a"]),
            .allowed(alsoHiddenCount: 0))
    }

    /// 活跃的子**还有另一条露着的入口**（多父）→ 藏这个父它并不会消失，不该拦。
    func testActiveChildReachableThroughAnotherParentDoesNotBlock() {
        let crews = [crew("a"), crew("other"), crew("b", parents: ["a", "other"])]
        XCTAssertEqual(
            CrewHiding.decide(hiding: "a", in: crews, activeSessionCrewIds: ["b"]),
            .allowed(alsoHiddenCount: 0))
    }
}
