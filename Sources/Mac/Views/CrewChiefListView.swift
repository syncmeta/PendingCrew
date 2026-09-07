#if os(macOS)
import SwiftUI

/// 左栏的**总机长视图**（Todo #102 / 人类 #109 第一步）：不列全部 crew，
/// 按「**现在该管什么**」收敛成三段。
///
/// 层级视图回答「谁挂在谁下面」，时间流视图回答「刚才哪儿有动静」—— 这台机器上
/// 40+ 个 crew，这两个问题的答案都是一份同样长的列表。人类原话是「现在消息太多
/// 太乱了」，他要的不是第三份列表，是**一屏看完现在该管什么**。所以这里的段是
/// ①在等你回应 ②还在跑 ③安静（默认折起来），推导在 `CrewChiefOverview`。
///
/// ## 这一版一个 agent 都不参与，这是刻意的
/// 三段的判据全是机械事实（未回应的人类 Todo 条数、最新活动时间），app 今天就在
/// 算。所以这个视图**在没有任何 agent 跑的时候就是有用的**；后面让总机长 session
/// 接管的是「临时分类」那种判断，不是这里。一个 agent 的判断可以决定「推荐你先看
/// 什么」，**但不能决定「这台机器上有什么」** —— 层级视图和时间流视图必须在总机长
/// session 死掉、跑飞、压根没启动的时候，长得跟现在一模一样。
///
/// ## 这里拖不动组织树
/// 行传 `allowsReparentDrag: false`。这个视图是**整理用的**，人看到的分段是「现在
/// 该管什么」而不是组织结构；如果在这儿一拖就改了汇报线，人会以为自己只是在归置
/// 列表，结果动了真的组织树。改组织树只有层级视图那一条路 —— 两件事在界面上是
/// 结构性分开的，不是靠一句提示文案区分。
struct CrewChiefListView: View {
    /// 要收进来的 crew（= 侧栏当前可见的全部，与另两个视图同一份）。
    let crews: [CrewSummary]

    /// 行右键「在这下面建子 crew」的目标；值由侧栏持有，表单也挂在那一层。
    @Binding var childCrewTarget: CrewChildCreationTarget?

    /// 行视图要一个拖拽态；本视图不开拖拽，给它一个自己的实例即可（永远是空的）。
    @StateObject private var dragState = CrewDragState()
    /// 哪些段被人折起来了。「安静」默认折着（`Section.collapsedByDefault`）。
    @State private var collapsed: Set<CrewChiefOverview.Section> = Set(
        CrewChiefOverview.Section.allCases.filter(\.collapsedByDefault))

    @EnvironmentObject private var crewStore: CrewStore

    var body: some View {
        let crewsById = Dictionary(crews.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        // 黄字标注要看**全量** crew（父边可以跨机器），与另两个视图同一口径。
        let rootTitles = CrewRootLineage.rootTitlesByCrew(in: crewStore.crews)
        // 与时间流视图同一份快照：**body 里不碰磁盘**（2026-08-17「开久了卡」的病根）。
        let lastMessages = crewStore.lastWhiteboardMessages
        let attention = crewStore.humanTodoAttention
        let groups = CrewChiefOverview.sections(
            crews: crews,
            // 只算**自己**那本，不含后代 —— 后代自己就在同一份扁平列表里占一行，
            // 再算进祖先会让同一条 Todo 出现两次。
            unanswered: { attention[$0.id]?.ownUnanswered ?? 0 },
            activity: { crew in
                CrewActivityTime.resolve(
                    lastMessageCreatedAt: lastMessages[crew.id]?.createdAt,
                    crewUpdatedAt: crew.updatedAt)
            },
            now: Date())

        if groups.isEmpty {
            emptyRow
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        } else {
            ForEach(groups) { group in
                sectionHeader(group)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                if !collapsed.contains(group.section) {
                    ForEach(group.entries) { entry in
                        CrewSidebarCrewRow(
                            crew: entry.crew,
                            crewsById: crewsById,
                            rootTitles: rootTitles[entry.crew.id] ?? [],
                            lineageLine: CrewTimelineOrdering.lineageLine(
                                for: entry.crew,
                                crewsById: crewsById,
                                rootTitles: rootTitles[entry.crew.id] ?? []),
                            expansion: nil, // 收敛列表是扁平的，没有展开
                            parentId: entry.crew.parentCrewIds.first,
                            groupCrews: crews,
                            allowsReparentDrag: false,
                            dragState: dragState,
                            childCrewTarget: $childCrewTarget
                        )
                        .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func sectionHeader(_ group: CrewChiefOverview.Group) -> some View {
        let isCollapsed = collapsed.contains(group.section)
        Button {
            if isCollapsed { collapsed.remove(group.section) }
            else { collapsed.insert(group.section) }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.semibold))
                    .rotationEffect(.degrees(isCollapsed ? 0 : 90))
                    .foregroundStyle(.secondary)
                Image(systemName: group.section.systemImage)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(group.section.title)
                    .font(Theme.Fonts.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.Palette.inkMuted)
                Text("\(group.entries.count)")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted.opacity(0.75))
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 10)
        .padding(.top, 8)
        .padding(.bottom, 4)
        .help(helpText(for: group.section))
    }

    private func helpText(for section: CrewChiefOverview.Section) -> String {
        switch section {
        case .awaitingHuman: return "这些 crew 那本「人类 Todo」还有你没回应的条目"
        case .running: return "最近两小时内有过动静"
        case .quiet: return "两小时以上没动静（含从来没动静过的）"
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
