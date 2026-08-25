#if os(macOS)
import AppKit
import SwiftUI

/// 左栏的本地 crew **DAG 折叠树**。
///
/// 「家」在本地 —— crew 的组织/DAG 模型是纯本地的(`LocalCrewStore.parentCrewIds`),
/// 不走 edge。这里把扁平 `[CrewSummary]` 渲染成折叠树:
/// - **根** = `parentCrewIds` 为空的 crew。
/// - 每个节点下嵌它的**子**(其它 crew 的 parentCrewIds 含本 crew)。
/// - **多父 crew 在每个父下各出现一次**(引用式,不是唯一归属)。
/// - **防无限递归**:渲染带一条「祖先路径」`Set<String>`,某节点已在路径上则
///   不再下钻(attachParent 已禁环,渲染仍自保,防脏数据)。
///
/// **顺序**(Todo #67):同级按**最新活动时间**从新到旧,父的键 = max(自己, 全部后代)。
/// 推导在 `CrewHierarchyOrdering`(纯函数、有单测),这里只负责把时间喂进去。
///
/// 点击节点设 `crewStore.selectedCrewId`(沿用现有选中逻辑)。展开态每个节点
/// 自持(`@State isExpanded`),互不影响。
struct CrewDAGTreeView: View {
    /// 本组要渲染的 crew 子集（某台机器的 crew）。
    let crews: [CrewSummary]

    /// 行右键「在这下面建子 crew」写目标的去处 —— 值由侧栏（`CrewSidebarView`）
    /// 持有，表单也挂在那一层；这里只负责把被右键那一行的 crew 写进去。
    @Binding var childCrewTarget: CrewChildCreationTarget?

    /// 本组共享的拖拽态（谁在被拖 / 报错）—— 判定在 `CrewDragDropLogic`。
    @StateObject private var dragState = CrewDragState()

    /// 根 crew 黄字标注要看**全量** crew（父边可以跨机器），不能只看本组子集 ——
    /// 只喂子集会把跨机器的那条谱系判丢。所以这里取 store 的全量列表。
    @EnvironmentObject private var crewStore: CrewStore

    var body: some View {
        // 排序键（Todo #67）：**最新活动时间**，父取 max(自己, 全部后代) —— 一个安静
        // 的父部门底下有子部门在刷屏时，父不能沉到底，否则那个正在动的子部门就找不到了。
        // 时间来自 store 那份**后台算好的**快照（`lastWhiteboardMessages`），**body 里
        // 一个字节的白板都不读** —— 那正是 2026-08-17「开久了卡」的病根（见
        // `CrewSidebarCrewRow` 里那段注释）。没有消息的回落 `createdAt`，不是 1970。
        let lastMessages = crewStore.lastWhiteboardMessages
        let orderKeys = CrewHierarchyOrdering.activityKeys(crews: crews) { crew in
            CrewActivityTime.resolve(
                lastMessageCreatedAt: lastMessages[crew.id]?.createdAt,
                // 兜底用 createdAt 而不是 updatedAt：后者被改名/绑定碰一下就跳，
                // 「改个名就窜到顶」不是人心里「这个群有没有动静」的意思。
                crewUpdatedAt: crew.createdAt)
        }
        // childMap: parentId → 该父下直接子 crew（**每个父下各自排好序** —— 多父 crew
        // 在每个父下各出现一次，只对第一处生效是这类结构最容易漏的地方）。限定在子集内算。
        let childMap = CrewHierarchyOrdering.sortedChildMap(crews: crews, keys: orderKeys)
        // 名字后面那行黄字标注（`@根 crew`）—— 整组一次算完再分发给行。
        let rootTitles = CrewRootLineage.rootTitlesByCrew(in: crewStore.crews)
        let ids = Set(crews.map(\.id))
        // 组内根 = 无父，或父不在本组（跨机器父边 → 在本组当根）。
        let roots = CrewHierarchyOrdering.sortedSiblings(
            crews.filter { crew in
                crew.parentCrewIds.isEmpty || !crew.parentCrewIds.contains(where: ids.contains)
            },
            keys: orderKeys)

        ForEach(roots) { crew in
            CrewDAGNode(
                crew: crew,
                childMap: childMap,
                crewsById: Self.byId(crews),
                rootTitlesByCrew: rootTitles,
                ancestors: [],
                parentId: nil,
                groupCrews: crews,
                dragState: dragState,
                childCrewTarget: $childCrewTarget
            )
        }
        // 分组根区域：拖到这里 = 摘到顶层（摘掉所有父边）。只在拖拽进行中出现，
        // 平时不占位、不打扰。错误提示也挂在这条上（它常驻于拖拽全程）。
        CrewRootDropZone(groupCrews: crews, dragState: dragState)
    }

    // MARK: - 推导

    static func byId(_ crews: [CrewSummary]) -> [String: CrewSummary] {
        Dictionary(crews.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
    }
}

/// 一次拖拽的共享态（同一机器分组内）。谁在被拖、拖的是哪条父边、失败文案。
///
/// 高亮判定要用到「被拖的是谁」，而 SwiftUI 的 `dropDestination(isTargeted:)`
/// 只告诉你「指针在我头上」，不告诉你拖的是什么 —— 所以拖起时把负载写进这里，
/// 每行自己算 `canDrop` 决定高不高亮：**非法目标不高亮**。
@MainActor
final class CrewDragState: ObservableObject {
    @Published var draggingCrewId: String?
    @Published var draggingParentId: String?
    @Published var errorText: String?

    var isDragging: Bool { draggingCrewId != nil }

    /// 拖拽结束哨兵 —— SwiftUI 的 `.draggable` 没有「拖拽取消了」的回调，
    /// 拖到窗外松手就再也没人来清态，根落点会一直挂着。用鼠标键状态兜底：
    /// 松手即收工。
    private var releaseWatch: Task<Void, Never>?

    func begin(crewId: String, parentId: String?) {
        draggingCrewId = crewId
        draggingParentId = parentId
        releaseWatch?.cancel()
        releaseWatch = Task { [weak self] in
            while !Task.isCancelled, NSEvent.pressedMouseButtons != 0 {
                try? await Task.sleep(nanoseconds: 150_000_000)
            }
            guard !Task.isCancelled else { return }
            self?.end()
        }
    }

    func end() {
        releaseWatch?.cancel()
        releaseWatch = nil
        draggingCrewId = nil
        draggingParentId = nil
    }

    /// 这个目标此刻接不接受 drop（`targetId == nil` = 分组根区域）。
    func accepts(targetId: String?, in crews: [CrewSummary]) -> Bool {
        guard let dragged = draggingCrewId else { return false }
        return CrewDragDropLogic.canDrop(
            draggedId: dragged, sourceParentId: draggingParentId,
            targetId: targetId, crews: crews)
    }

    /// 落地：**先挂新边再摘旧边** —— 挂失败时旧边原样保留（同
    /// `LocalCrewStore.release` 的顺序保证），不会把 crew 摘成孤儿。
    func apply(_ plan: CrewDragDropLogic.MovePlan, crewId: String, store: CrewStore) async {
        do {
            if let parent = plan.attach {
                try await store.attachParent(crewId: crewId, parentCrewId: parent)
            }
            for parent in plan.detach where parent != plan.attach {
                try await store.detachParent(crewId: crewId, parentCrewId: parent)
            }
        } catch {
            errorText = (error as? LocalizedError)?.errorDescription ?? "\(error)"
        }
    }

    /// 从拖拽负载解析并落地。返回 drop 是否被接受（给 `dropDestination` 用）。
    func handleDrop(_ payloads: [String], targetId: String?, crews: [CrewSummary],
                    store: CrewStore) -> Bool {
        defer { end() }
        guard let raw = payloads.first, let edge = CrewDragDropLogic.decode(raw) else { return false }
        guard let plan = CrewDragDropLogic.plan(
            draggedId: edge.crewId, sourceParentId: edge.parentId,
            targetId: targetId, crews: crews) else { return false }
        Task { await apply(plan, crewId: edge.crewId, store: store) }
        return true
    }
}

/// 分组底部的「摘到顶层」落点。只在拖拽进行中显形；错误提示（attach/detach 抛错）
/// 也挂在这里 —— 它是全程常驻的那个视图。层级视图与时间流视图共用。
struct CrewRootDropZone: View {
    let groupCrews: [CrewSummary]
    @ObservedObject var dragState: CrewDragState

    @EnvironmentObject private var crewStore: CrewStore
    @State private var isTargeted = false

    private var accepts: Bool { dragState.accepts(targetId: nil, in: groupCrews) }

    var body: some View {
        Group {
            if dragState.isDragging {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.up.to.line")
                    Text("拖到这里 → 提为顶层 crew")
                }
                .font(Theme.Fonts.caption)
                .foregroundStyle(accepts ? Theme.Palette.ink : Theme.Palette.inkMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(
                    RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
                        .strokeBorder(
                            (isTargeted && accepts) ? Color.accentColor : Theme.Palette.inkMuted.opacity(0.35),
                            style: StrokeStyle(lineWidth: 1.5, dash: [4, 3]))
                        .background(
                            RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
                                .fill((isTargeted && accepts) ? Theme.Palette.accentBg : Color.clear))
                )
                .padding(.bottom, 6)
                .dropDestination(for: String.self) { items, _ in
                    dragState.handleDrop(items, targetId: nil, crews: groupCrews, store: crewStore)
                } isTargeted: { isTargeted = $0 }
            }
        }
        .alert("调整从属关系失败", isPresented: Binding(
            get: { dragState.errorText != nil },
            set: { if !$0 { dragState.errorText = nil } }
        )) {
            Button("好", role: .cancel) { dragState.errorText = nil }
        } message: {
            Text(dragState.errorText ?? "")
        }
    }
}

/// 单个 crew 树节点 —— 竖色条 + 标题,有子时带 disclosure 三角。
private struct CrewDAGNode: View {
    let crew: CrewSummary
    let childMap: [String: [CrewSummary]]
    let crewsById: [String: CrewSummary]
    /// crewId → 根 crew 标题（名字后面那行黄字标注）。整棵树共用一份，父视图算好传下来。
    let rootTitlesByCrew: [String: [String]]
    /// 当前渲染路径上的祖先 id(含到本节点为止的链)—— 防环下钻自保。
    let ancestors: Set<String>
    /// 本节点**这次渲染所处的那条父边**（nil = 组内根）。多父 crew 在每个父下
    /// 各画一次，拖走的是当前这条边，别的父边不动。
    let parentId: String?
    /// 本机器分组的全部 crew —— 合法性判定的数据面。
    let groupCrews: [CrewSummary]
    @ObservedObject var dragState: CrewDragState
    /// 右键「在这下面建子 crew」的目标（侧栏持有，表单也在那层弹）。
    @Binding var childCrewTarget: CrewChildCreationTarget?

    @State private var isExpanded = false

    private var children: [CrewSummary] { childMap[crew.id] ?? [] }
    /// 已在祖先路径上的子不再下钻(环自保);其余正常展开。
    private var expandableChildren: [CrewSummary] {
        children.filter { !ancestors.contains($0.id) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            row
            if isExpanded {
                ForEach(expandableChildren) { child in
                    CrewDAGNode(
                        crew: child,
                        childMap: childMap,
                        crewsById: crewsById,
                        rootTitlesByCrew: rootTitlesByCrew,
                        ancestors: ancestors.union([crew.id]),
                        parentId: crew.id,
                        groupCrews: groupCrews,
                        dragState: dragState,
                        childCrewTarget: $childCrewTarget
                    )
                    .padding(.leading, 16) // 每层缩进
                }
            }
        }
    }

    /// 行本体在 `CrewSidebarCrewRow` —— 与时间流视图**共用同一个行视图**，
    /// 状态点/黄字/摘要/时间/选中/右键/拖拽因此不可能两边不一致。树视图这边
    /// 只负责把展开三角的开关接上去（有可展开的子才给 binding）。
    private var row: some View {
        CrewSidebarCrewRow(
            crew: crew,
            crewsById: crewsById,
            rootTitles: rootTitlesByCrew[crew.id] ?? [],
            lineageLine: nil, // 层级视图靠缩进说"挂在谁下面"，不需要这一行
            expansion: expandableChildren.isEmpty ? nil : $isExpanded,
            parentId: parentId,
            groupCrews: groupCrews,
            dragState: dragState,
            childCrewTarget: $childCrewTarget
        )
    }
}
#endif
