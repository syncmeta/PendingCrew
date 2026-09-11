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
        // 状态那张表跟末条快照出自同一次解码（`CrewLastMessageCache.Digest`），
        // body 里只是一次字典查表 —— **不碰磁盘**。
        let statusCarriers = crewStore.crewStatusCarriers
        let now = Date()
        // 顶上那个固定入口 + 下面那份排好序的列表，**一次算出来**
        // （`CrewChiefOverview.rows`）。入口在不在、排第几、指向谁，是那边的
        // 单元测试压着的，不是这里目视出来的。
        let rows = CrewChiefOverview.rows(
            chiefLayer: crewStore.chiefLayer,
            crews: crews,
            activity: { crew in
                CrewActivityTime.resolve(
                    lastMessageCreatedAt: lastMessages[crew.id]?.createdAt,
                    crewUpdatedAt: crew.updatedAt)
            },
            arrangement: arrangement?.crewIds ?? [])
        let entries = rows.compactMap { row -> CrewChiefOverview.Entry? in
            if case .crew(let entry) = row { return entry }
            return nil
        }

        // 总机组那一层的门。它在**溯源行之上** —— 那行讲的是「下面这份列表是
        // 谁排的」，跟这一层无关，压在它下面会读成「这一层也是排出来的」。
        ForEach(rows.filter { if case .chiefLayer = $0 { return true } else { return false } }) { row in
            if case .chiefLayer(let chief) = row {
                CrewChiefLayerEntryRow(crew: chief)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                Divider()
                    .padding(.horizontal, 10)
                    .padding(.vertical, 2)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
            }
        }

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
                    statusLine: CrewStatusLine.make(
                        resolved: statusCarriers[entry.crew.id].map {
                            ($0.crewStatus ?? "", CrewTimestamp.parse($0.createdAt))
                        },
                        now: now),
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
            resortButton
        }
        .font(Theme.Fonts.caption2)
        .foregroundStyle(Theme.Palette.inkMuted)
        .padding(.horizontal, 10)
        .padding(.top, 6)
        .padding(.bottom, 2)
        .help(arrangement.map { "为什么这么排：\($0.reason)" }
              ?? "没有人排过顺序，这里按每个 crew 最近一次有动静的时间倒序")
    }

    /// 手动刷新（人类 Todo #145：「再给个手动刷新按钮，手动出发让总机长重新…排序」）。
    ///
    /// 它**不是刷新界面**（顺序本来就是实时读的），是**请总机长现在跑一轮**。
    /// 所以图标用「叫人」而不是循环箭头 —— 循环箭头会让人以为是重新加载数据，
    /// 按下去半天没变化就以为坏了。真正要等的是一个 agent 醒过来、想一遍、写回来。
    ///
    /// 判定在 `ChiefResortRequest`、动作在 `CrewStore.requestChiefResort`，
    /// 这里只负责按和显示回执。
    @ViewBuilder
    private var resortButton: some View {
        Button {
            Task { await crewStore.requestChiefResort() }
        } label: {
            Image(systemName: "arrow.trianglehead.clockwise.rotate.90")
                .font(.system(size: 9))
        }
        .buttonStyle(.plain)
        .foregroundStyle(Theme.Palette.inkMuted)
        .help("请总机长现在重新排一次顺序（会在总机组群聊里留一条）")
        .accessibilityLabel("请总机长重排")
        // 回执**就画在按钮旁边**：发出去了 / 被冷却挡了 / 没有总机组，三种都要看得见。
        .popover(isPresented: Binding(
            get: { crewStore.chiefResortNote != nil },
            set: { if !$0 { crewStore.chiefResortNote = nil } })) {
            Text(crewStore.chiefResortNote ?? "")
                .font(Theme.Fonts.caption2)
                .padding(10)
                .frame(maxWidth: 240)
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
