#if os(macOS)
import SwiftUI

/// 左栏的**总机长视图**（Todo #102 / 人类 #109，口径按 #113 改过）。
///
/// ## 这一版不分类，只排序
/// 第一版按「在等你回应 / 还在跑 / 安静」分三段。人类 #113 把它推翻了：
/// 「先不分类 先注重排序 什么是最近活跃处理的 就放到前面 就是把手头上要用到的
/// 尽可能放前面」。所以这里**一个分组标题都不画**，就是一条按最近活动倒序的扁平列表。
///
/// ## 那它跟「时间流」差在哪
/// 差的就是 #109 里没被推翻的那一条：**顺序由总机长那个 session 决定**。
/// agent 排的那份顺序是**覆盖层**，叠在确定性基础序（最近活动倒序）上面 ——
/// 一个 agent 的判断可以决定「推荐你先看什么」，**但不能决定「这台机器上有什么」**：
/// 它死了、跑飞了、压根没启动，这个视图仍然长得跟「时间流」一样能用，
/// 而「层级」「时间流」两个视图任何时候都不受它影响。
///
/// ## 这里拖不动组织树
/// 行传 `allowsReparentDrag: false`。这个视图是**给人看的排序**，不是组织结构；
/// 在这儿一拖就改了汇报线，人会以为自己只是在归置列表。改组织树只有层级视图那条路。
struct CrewChiefListView: View {
    /// 要列的 crew（= 侧栏当前可见的全部，与另两个视图同一份）。
    let crews: [CrewSummary]

    /// 行右键「在这下面建子 crew」的目标；值由侧栏持有，表单也挂在那一层。
    @Binding var childCrewTarget: CrewChildCreationTarget?

    /// 总机长排的那份顺序（侧栏读好传进来，本视图不碰磁盘）。
    /// `nil` = 没人排过 / 读不出来 → 纯基础序，**而且要在界面上说出来**。
    var arrangement: CrewArrangement?

    /// 行视图要一个拖拽态；本视图不开拖拽，给它一个自己的实例即可（永远是空的）。
    @StateObject private var dragState = CrewDragState()

    @EnvironmentObject private var crewStore: CrewStore

    var body: some View {
        let crewsById = Dictionary(crews.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        // 黄字标注要看**全量** crew（父边可以跨机器），与另两个视图同一口径。
        let rootTitles = CrewRootLineage.rootTitlesByCrew(in: crewStore.crews)
        // 与时间流视图同一份快照：**body 里不碰磁盘**（2026-08-17「开久了卡」的病根）。
        let lastMessages = crewStore.lastWhiteboardMessages
        let entries = CrewChiefOverview.ordered(
            crews: crews,
            activity: { crew in
                CrewActivityTime.resolve(
                    lastMessageCreatedAt: lastMessages[crew.id]?.createdAt,
                    crewUpdatedAt: crew.updatedAt)
            },
            arrangement: arrangement?.crewIds ?? [])

        provenanceHeader
            .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))

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
                    parentId: entry.crew.parentCrewIds.first,
                    groupCrews: crews,
                    allowsReparentDrag: false,
                    dragState: dragState,
                    childCrewTarget: $childCrewTarget
                )
                // 被 agent 顶上来的那几行**要看得出来**：不然人看到一个不合意的
                // 顺序，分不清是规则算的还是谁排的，只会觉得「这东西乱」。
                .overlay(alignment: .topLeading) {
                    if entry.pinnedByArrangement {
                        Image(systemName: "pin.fill")
                            .font(.system(size: 8))
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .padding(.leading, 2)
                            .padding(.top, 6)
                            .help("这一条是总机长排上来的，不是按最近活动排的")
                    }
                }
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
            }
        }
    }

    /// 这一版顺序**是怎么来的** —— 有人排过就说是谁、多久前、为什么；
    /// 没人排过就明说「按最近活动排的」。
    ///
    /// 这一行是第 4 条要求的落点：**退回基础序时也得看得出来是退回了**，
    /// 不能让人以为 agent 就是排成这样的。
    @ViewBuilder
    private var provenanceHeader: some View {
        HStack(spacing: 5) {
            Image(systemName: arrangement == nil ? "clock" : "pin.fill")
                .font(.system(size: 9))
            if let arrangement {
                let who = arrangement.bySenderName ?? "总机长"
                Text("\(who) 排的")
                TimelineView(.everyMinute) { _ in
                    if let at = CrewTimestamp.parse(arrangement.createdAt) {
                        Text(at.formatted(.relative(presentation: .numeric)))
                    }
                }
            } else {
                Text("按最近活动排的（还没人排过）")
            }
            Spacer(minLength: 0)
        }
        .font(Theme.Fonts.caption2)
        .foregroundStyle(Theme.Palette.inkMuted)
        .padding(.horizontal, 10)
        .padding(.top, 6)
        .padding(.bottom, 2)
        .help(arrangement.map { "为什么这么排：\($0.reason)" }
              ?? "没有人排过顺序，这里按每个 crew 最近一次有动静的时间倒序")
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
