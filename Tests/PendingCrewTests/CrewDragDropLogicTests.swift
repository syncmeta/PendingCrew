import XCTest

/// 侧栏拖拽改父子关系的判定层单测（Todo #27）。护栏（禁环/禁自己）、移动语义
/// （只摘当前这条父边）、摘到顶层、负载编解码。
final class CrewDragDropLogicTests: XCTestCase {
    private func crew(_ id: String, parents: [String] = []) -> CrewSummary {
        CrewSummary(
            id: id, title: id, responsibleSubjectId: "s", runtimeLocation: "local_host",
            captainBotId: nil, status: nil, createdAt: "2026-08-08T00:00:00Z",
            updatedAt: "2026-08-08T00:00:00Z", parentCrewIds: parents)
    }

    /// a ← b ← c 一条链，外加无父的 solo。
    private var chain: [CrewSummary] {
        [crew("a"), crew("b", parents: ["a"]), crew("c", parents: ["b"]), crew("solo")]
    }

    // MARK: - descendants

    func testDescendantsWalksWholeSubtree() {
        XCTAssertEqual(CrewDragDropLogic.descendants(of: "a", in: chain), ["b", "c"])
        XCTAssertEqual(CrewDragDropLogic.descendants(of: "c", in: chain), [])
        XCTAssertEqual(CrewDragDropLogic.descendants(of: "solo", in: chain), [])
    }

    /// 脏数据成环（a→b→a）也必须收敛，不能死循环。
    func testDescendantsSurvivesDirtyCycle() {
        let dirty = [crew("a", parents: ["b"]), crew("b", parents: ["a"])]
        XCTAssertEqual(CrewDragDropLogic.descendants(of: "a", in: dirty), ["b"])
    }

    // MARK: - 护栏

    func testCannotDropOntoSelf() {
        XCTAssertFalse(CrewDragDropLogic.canDrop(
            draggedId: "a", sourceParentId: nil, targetId: "a", crews: chain))
    }

    func testCannotDropIntoOwnSubtree() {
        XCTAssertFalse(CrewDragDropLogic.canDrop(
            draggedId: "a", sourceParentId: nil, targetId: "b", crews: chain))
        XCTAssertFalse(CrewDragDropLogic.canDrop(
            draggedId: "a", sourceParentId: nil, targetId: "c", crews: chain))
    }

    func testUnknownDraggedOrTargetRejected() {
        XCTAssertFalse(CrewDragDropLogic.canDrop(
            draggedId: "nope", sourceParentId: nil, targetId: "a", crews: chain))
        XCTAssertFalse(CrewDragDropLogic.canDrop(
            draggedId: "solo", sourceParentId: nil, targetId: "nope", crews: chain))
    }

    /// payload 里的源父边已经不存在（列表刷新过）→ 不接受，别拿旧边乱摘。
    func testStaleSourceEdgeRejected() {
        XCTAssertFalse(CrewDragDropLogic.canDrop(
            draggedId: "c", sourceParentId: "a", targetId: "solo", crews: chain))
    }

    // MARK: - 移动语义

    func testMoveDetachesOnlyCurrentEdge() {
        // b 同时挂在 a 和 solo 下；拖的是「a 下的 b」→ 只摘 a 这条边。
        let crews = [crew("a"), crew("solo"), crew("t"),
                     crew("b", parents: ["a", "solo"])]
        XCTAssertEqual(
            CrewDragDropLogic.plan(draggedId: "b", sourceParentId: "a", targetId: "t", crews: crews),
            .init(detach: ["a"], attach: "t"))
        // 拖的是「solo 下的 b」→ 只摘 solo 这条边。
        XCTAssertEqual(
            CrewDragDropLogic.plan(draggedId: "b", sourceParentId: "solo", targetId: "t", crews: crews),
            .init(detach: ["solo"], attach: "t"))
    }

    func testTopLevelCrewDroppedOntoParentJustAttaches() {
        XCTAssertEqual(
            CrewDragDropLogic.plan(draggedId: "solo", sourceParentId: nil, targetId: "a", crews: chain),
            .init(detach: [], attach: "a"))
    }

    func testDropOntoCurrentParentIsNoop() {
        XCTAssertNil(CrewDragDropLogic.plan(
            draggedId: "b", sourceParentId: "a", targetId: "a", crews: chain))
    }

    /// 已经挂在目标下、但拖的是**另一条**边 → 等于把那条边并过来，允许。
    func testDropOntoExistingOtherParentMergesEdge() {
        let crews = [crew("a"), crew("solo"), crew("b", parents: ["a", "solo"])]
        XCTAssertEqual(
            CrewDragDropLogic.plan(draggedId: "b", sourceParentId: "solo", targetId: "a", crews: crews),
            .init(detach: ["solo"], attach: "a"))
    }

    // MARK: - 摘到顶层

    func testDropOnRootDetachesAllParents() {
        let crews = [crew("a"), crew("solo"), crew("b", parents: ["a", "solo"])]
        XCTAssertEqual(
            CrewDragDropLogic.plan(draggedId: "b", sourceParentId: "a", targetId: nil, crews: crews),
            .init(detach: ["a", "solo"], attach: nil))
    }

    func testDropOnRootWhenAlreadyTopLevelIsNoop() {
        XCTAssertNil(CrewDragDropLogic.plan(
            draggedId: "solo", sourceParentId: nil, targetId: nil, crews: chain))
    }

    // MARK: - 负载编解码

    func testPayloadRoundTrip() {
        let withParent = CrewDragDropLogic.encode(crewId: "b", parentId: "a")
        XCTAssertEqual(CrewDragDropLogic.decode(withParent)?.crewId, "b")
        XCTAssertEqual(CrewDragDropLogic.decode(withParent)?.parentId, "a")

        let root = CrewDragDropLogic.encode(crewId: "solo", parentId: nil)
        XCTAssertEqual(CrewDragDropLogic.decode(root)?.crewId, "solo")
        XCTAssertNil(CrewDragDropLogic.decode(root)?.parentId)
    }

    /// 外部拖进来的普通文本不该被当成 crew 边。
    func testDecodeRejectsForeignPayload() {
        XCTAssertNil(CrewDragDropLogic.decode("hello"))
        XCTAssertNil(CrewDragDropLogic.decode("crew-edge:|a"))
    }
}
