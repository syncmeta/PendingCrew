import Foundation

/// 机长工具 `create_child_crew` 的落点。
///
/// 总机组是派生的协调层，不是一条可持久化的父边；它缺执行组时创建的是新的
/// 顶层 crew（该 crew 的汇报父级仍会由 `reportingParentIds` 派生为总机组）。
/// 普通 crew 保持原语义，创建并挂接直属子 crew。
enum CrewChildCreationPlacement: Equatable {
    case topLevelExecutionCrew
    case attachedChild(parentCrewId: String)

    static func resolve(parentCrewId: String) -> CrewChildCreationPlacement {
        if parentCrewId == LocalCrew.chiefCrewId {
            return .topLevelExecutionCrew
        }
        return .attachedChild(parentCrewId: parentCrewId)
    }
}

/// 侧栏 crew 行右键「在这下面建子 crew」的目标（Todo #35）。
///
/// 存在的意义只有一个：把「新 crew 挂到谁下面」这个 id 钉死成**被右键的那一行的
/// crew**。两个最容易顺手拿错的来源，签名上就不给传：
/// - **不是** `crewStore.selectedCrewId` —— macOS 右键不改变选中态，拿选中会把子
///   crew 建到当前选中的那个 crew 下面（右键别的行时就错）。
/// - **不是**节点这次渲染所处的那条父边（`CrewDAGNode.parentId`）—— 那是「这一行
///   的父」；多父 crew 在每个父下各画一次，父边会变，被右键的 crew 不变。
///
/// `Identifiable` 是给 `sheet(item:)` 用的：sheet 挂在侧栏顶层（不是行上），行只
/// 负责把目标写进这个可选值，行被 List 回收也不会把打开中的表单顶掉。
struct CrewChildCreationTarget: Identifiable, Equatable {
    /// 新 crew 要挂到的父 crew id（`CreateCrewSheet(parentCrewId:)` 的入参）。
    let parentCrewId: String

    var id: String { parentCrewId }

    /// 从被右键的那一行解析目标 —— 只吃这一行自己的 crew。
    static func forRow(_ crew: CrewSummary) -> CrewChildCreationTarget {
        CrewChildCreationTarget(parentCrewId: crew.id)
    }
}
