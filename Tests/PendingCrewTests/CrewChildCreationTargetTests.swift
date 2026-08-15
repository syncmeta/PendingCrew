import XCTest

/// 侧栏右键「在这下面建子 crew」的目标解析单测（Todo #35）。
/// 钉的是那处最容易写错的地方：作用对象必须是**被右键的那一行**，与当前选中态、
/// 与这一行渲染时所处的父边都无关。
final class CrewChildCreationTargetTests: XCTestCase {
    private func crew(_ id: String, parents: [String] = []) -> CrewSummary {
        CrewSummary(
            id: id, title: id, responsibleSubjectId: "s", runtimeLocation: "local_host",
            captainBotId: nil, status: nil, createdAt: "2026-08-08T00:00:00Z",
            updatedAt: "2026-08-08T00:00:00Z", parentCrewIds: parents)
    }

    /// 目标就是被右键那一行的 crew。
    func testTargetIsTheRightClickedRow() {
        XCTAssertEqual(CrewChildCreationTarget.forRow(crew("b")).parentCrewId, "b")
    }

    /// 选中的是别的 crew（右键不改变选中）时，目标仍是被右键的行 —— 选中态压根
    /// 进不来（`forRow` 只接一行的 crew），这里把这条不变量写死。
    func testSelectionDoesNotAffectTarget() {
        let selected = crew("a")
        let rightClicked = crew("b", parents: ["a"])
        let target = CrewChildCreationTarget.forRow(rightClicked)
        XCTAssertEqual(target.parentCrewId, rightClicked.id)
        XCTAssertNotEqual(target.parentCrewId, selected.id)
    }

    /// 多父 crew 在每个父下各画一次；不管从哪条父边下右键，目标都是这个 crew
    /// 自己，不是它的父。
    func testMultiParentRowTargetsItselfNotItsParent() {
        let multi = crew("c", parents: ["a", "b"])
        XCTAssertEqual(CrewChildCreationTarget.forRow(multi).parentCrewId, "c")
    }

    /// sheet(item:) 的身份 = 父 crew id，同一行重复右键不会开出两份表单。
    func testIdentityIsParentCrewId() {
        let target = CrewChildCreationTarget.forRow(crew("b"))
        XCTAssertEqual(target.id, "b")
        XCTAssertEqual(target, CrewChildCreationTarget.forRow(crew("b")))
    }
}
