#if os(macOS)
import SwiftUI

/// 左栏的**时间流视图**（Todo #50）：不按组织结构、**按最近有动静**排。
///
/// 层级视图回答的是"谁挂在谁下面"；这台机器上 crew 一多、嵌套一深，人真正想知道
/// 的往往是"**刚才哪儿有动静**"—— 那个问题只有扁平 + 倒序能回答。所以这里是一条
/// 拉平的列表（不分机器、不分层），按 `CrewActivityTime` 倒序，最新的在最上。
///
/// 扁平了也不丢上下文：行仍是 `CrewSidebarCrewRow`（竖色条编码血缘、名字后黄字
/// 标根祖先），再加一行直接父 crew 名（`CrewTimelineOrdering.lineageLine`，与黄字
/// 重复时不画）。行内其它一切（状态点/摘要/时间/选中高亮/右键/拖拽）与层级视图
/// 是同一份代码，不存在"新视图里少一样"。
struct CrewTimelineListView: View {
    /// 要列的 crew（Mac = 全量本地 crew，跨机器一起排 —— 时间流问的是"什么时候"，
    /// 不是"在哪台机器"）。
    let crews: [CrewSummary]

    /// 行右键「在这下面建子 crew」的目标；值由侧栏持有，表单也挂在那一层。
    @Binding var childCrewTarget: CrewChildCreationTarget?

    /// 本视图共享的拖拽态（谁在被拖 / 报错）—— 判定在 `CrewDragDropLogic`。
    @StateObject private var dragState = CrewDragState()

    @EnvironmentObject private var crewStore: CrewStore

    var body: some View {
        // 白板一变（含 helper 子进程跨进程写）就重排重渲染 —— 时间流的排序键就是
        // 白板末条消息时间，不订阅这个计数它会一直停在打开那一刻的顺序。
        let revision = crewStore.whiteboardRevision
        let crewsById = Dictionary(crews.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        // 黄字标注要看**全量** crew（父边可以跨机器），与层级视图同一口径。
        let rootTitles = CrewRootLineage.rootTitlesByCrew(in: crewStore.crews)
        // 每个 crew 的白板只读一遍：排序键和行里显示的摘要/时间用的是**同一条**
        // 末条消息，两处必然一致（也省掉一轮整份 JSON 解码）。
        let lastMessages = Dictionary(uniqueKeysWithValues: crews.map { crew in
            (crew.id, CrewSidebarCrewRow.lastMessage(crewId: crew.id, revision: revision))
        })
        let entries = CrewTimelineOrdering.ordered(crews: crews) { crew in
            CrewActivityTime.resolve(
                lastMessageCreatedAt: lastMessages[crew.id]??.createdAt,
                crewUpdatedAt: crew.updatedAt)
        }

        if entries.isEmpty {
            emptyRow
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        } else {
            ForEach(entries) { entry in
                CrewSidebarCrewRow(
                    crew: entry.crew,
                    crewsById: crewsById,
                    rootTitles: rootTitles[entry.crew.id] ?? [],
                    lineageLine: CrewTimelineOrdering.lineageLine(
                        for: entry.crew,
                        crewsById: crewsById,
                        rootTitles: rootTitles[entry.crew.id] ?? []),
                    expansion: nil, // 扁平列表没有展开
                    // 扁平行代表 crew 本身而不是某条父边；拖走时按"第一父"那条边算
                    // （竖色条上溯的也是这一条，两处一致）。
                    parentId: entry.crew.parentCrewIds.first,
                    groupCrews: crews,
                    dragState: dragState,
                    childCrewTarget: $childCrewTarget,
                    lastMessageSource: .prefetched(lastMessages[entry.crew.id] ?? nil)
                )
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
            }
            // 拖到这里 = 摘到顶层（与层级视图同一个落点视图、同一套判定）。
            CrewRootDropZone(groupCrews: crews, dragState: dragState)
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        }
    }

    @ViewBuilder
    private var emptyRow: some View {
        VStack(alignment: .leading, spacing: 4) {
            if crewStore.loadingList {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("加载中…").foregroundStyle(.secondary)
                }
            } else {
                Text("还没有 crew").font(.callout).foregroundStyle(.secondary)
                Text("点 + 新建第一个").font(.caption).foregroundStyle(.tertiary)
            }
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 10)
    }
}
#endif
