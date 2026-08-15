import Foundation

/// 侧栏 crew 树**拖拽改父子关系**的判定层（Todo #27）。
///
/// 视图（`CrewDAGTreeView`）只负责画和触发；「这个 drop 合不合法」「落下去要
/// 摘哪条边、挂哪条边」全在这里，纯函数、可单测。
///
/// 语义（与 spec 一致）：
/// - 把 A 拖到 B 上 = **移动**：摘掉 A 当前渲染路径上的那条父边（`sourceParentId`），
///   再把 A 挂到 B 名下。多父 crew 在多个父下重复出现，拖的是**当前这条边**，
///   别的父边不动。
/// - 把 A 拖到分组的根区域 = 摘到顶层：摘掉 A 的**所有**父边。
/// - 禁环：不能把 A 挂到 A 自己或 A 的后代之下。
enum CrewDragDropLogic {
    /// 一次拖拽落地要执行的边操作。执行顺序由调用方保证：**先 attach 再 detach**
    /// （挂新边失败时旧边原样保留，同 `LocalCrewStore.release`）。
    struct MovePlan: Equatable {
        /// 要摘掉的父边（`crewId` 的父 id 列表）。
        var detach: [String]
        /// 要挂上的新父；nil = 摘到顶层。
        var attach: String?
    }

    /// `crewId` 的全部后代 id（不含自己）。children 由「谁的 parentCrewIds 含本 crew」
    /// 反推；带 visited 自保，脏数据成环也不会死循环。
    static func descendants(of crewId: String, in crews: [CrewSummary]) -> Set<String> {
        var childMap: [String: [String]] = [:]
        for crew in crews {
            for parent in crew.parentCrewIds { childMap[parent, default: []].append(crew.id) }
        }
        var out: Set<String> = []
        var stack = childMap[crewId] ?? []
        while let next = stack.popLast() {
            guard !out.contains(next), next != crewId else { continue }
            out.insert(next)
            stack.append(contentsOf: childMap[next] ?? [])
        }
        return out
    }

    /// 这个目标接不接受 drop。`targetId == nil` 表示分组的根区域（摘到顶层）。
    ///
    /// 拒绝的情形：拖的 crew 不在本组、拖到自己身上、拖进自己的子树（成环）、
    /// 目标不存在、以及**落下去什么都不会变**的空拖（已经在该父下 / 已经在顶层）。
    static func canDrop(draggedId: String, sourceParentId: String?, targetId: String?,
                        crews: [CrewSummary]) -> Bool {
        plan(draggedId: draggedId, sourceParentId: sourceParentId,
             targetId: targetId, crews: crews) != nil
    }

    /// 落地方案；nil = 这个 drop 不该被接受（非法或纯空操作）。
    static func plan(draggedId: String, sourceParentId: String?, targetId: String?,
                     crews: [CrewSummary]) -> MovePlan? {
        guard let dragged = crews.first(where: { $0.id == draggedId }) else { return nil }
        // 源父边必须真的存在，否则说明 payload 与当前数据已经对不上（列表刷新过）。
        if let source = sourceParentId, !dragged.parentCrewIds.contains(source) { return nil }

        guard let targetId else {
            // 摘到顶层：已经没有父边就没什么可做的。
            return dragged.parentCrewIds.isEmpty ? nil : MovePlan(detach: dragged.parentCrewIds, attach: nil)
        }

        guard targetId != draggedId else { return nil }                       // 拖到自己身上
        guard crews.contains(where: { $0.id == targetId }) else { return nil } // 目标不在本组
        guard !descendants(of: draggedId, in: crews).contains(targetId) else { return nil } // 成环

        // 已经挂在目标之下：只有「换一条边过来」才有意义，同边即空拖。
        if dragged.parentCrewIds.contains(targetId), sourceParentId == targetId || sourceParentId == nil {
            return nil
        }
        return MovePlan(detach: sourceParentId.map { [$0] } ?? [], attach: targetId)
    }

    // MARK: - 拖拽负载编解码

    /// 拖拽负载 = `childId` + 当前渲染路径上的父 id（可能没有 = 顶层节点）。
    /// 走一个字符串（`String` 天然 `Transferable`），不额外注册 UTType。
    static func encode(crewId: String, parentId: String?) -> String {
        "crew-edge:\(crewId)|\(parentId ?? "")"
    }

    static func decode(_ raw: String) -> (crewId: String, parentId: String?)? {
        guard raw.hasPrefix("crew-edge:") else { return nil }
        let body = raw.dropFirst("crew-edge:".count)
        let parts = body.components(separatedBy: "|")
        guard parts.count == 2, !parts[0].isEmpty else { return nil }
        return (parts[0], parts[1].isEmpty ? nil : parts[1])
    }
}
