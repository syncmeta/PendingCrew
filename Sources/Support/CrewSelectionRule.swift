import Foundation

/// 列表刷新之后，**当前选中的那个还该不该选中**。
///
/// ## 为什么这条要单独拿出来
/// 原来的规则一句话：「选中项不在刚拉回来的列表里 → 清掉」。它对了很久，因为
/// 那时「列表」和「所有能被选中的东西」是同一份。
///
/// **总机组那一层把这两者拆开了**：它按设计**永远不在** crew 列表里
/// （它是一层不是一个机组，见 `LocalCrewStore.listCrews`），但它**是可以被选中的**
/// —— 侧栏第三视图顶上那个固定入口点的就是它。旧规则会在下一次刷新时
/// 精准地把它弹下来：人点进去、群聊闪一下、又跳回空白。
///
/// 拿出来单独放，是因为「清掉」和「不清掉」这两种都必须**有测试压着**：
/// 只放行总机组、不许顺手把「真的没了就清掉」那一半也关掉。
enum CrewSelectionRule {

    /// - Parameters:
    ///   - selected: 当前选中的 crew id（nil = 没选中）。
    ///   - listedIds: 刚拉回来的那份 crew 列表（**不含**内建那一层）。
    /// - Returns: 该不该把选中清掉。
    static func shouldClearSelection(selected: String?, listedIds: [String]) -> Bool {
        guard let selected else { return false }
        // 内建那一层不在列表里是**常态**，不是「它没了」。
        if selected == LocalCrew.chiefCrewId { return false }
        return !listedIds.contains(selected)
    }
}
