#if os(macOS)
import Combine
import SwiftUI

/// 侧栏里的**一行 crew** —— 层级视图（`CrewDAGTreeView`）和时间流视图
/// （`CrewTimelineListView`）共用**同一个**行视图。
///
/// 为什么必须共用：行上挂的东西已经很多（状态点、根黄字标注、最新消息摘要、
/// 相对时间、选中高亮、右键菜单、拖拽改从属），新开一种视图最容易翻的车就是
/// "在新视图里少了一样"。共用一个行 = 两种视图**结构性地**不可能不一致，
/// 想掉也掉不了。两视图的差别只有三项，都由参数表达：
/// - `expansion`：展开三角的开关（nil = 不画三角，只占位保持左缘对齐）；
/// - `lineageLine`：扁平视图才有的"挂在谁下面"次要行（推导在
///   `CrewTimelineOrdering.lineageLine`）；
/// - `parentId`：本行这次渲染所处的那条父边（拖拽要摘的就是它）。
///
/// 会话行样式对齐 PendingBot `ConversationListRow`：细竖色条（谱系二分色，见
/// `CrewColorBar`）+ 无衬线-14 标题 + 最近一条消息 + 相对时间。
///
/// **不贴每行「本机/云端」location tag**（#369：层级视图已按 machine 分组；时间流
/// 视图是跨机器扁平的，机器归属由竖色条 + 血缘行承担）。
struct CrewSidebarCrewRow: View {
    let crew: CrewSummary
    /// 算竖色条色链用（本组/全量 crew 的 id → crew）。
    let crewsById: [String: CrewSummary]
    /// 名字后面那行黄字标注（根 crew 标题）。整表一次算好传进来，别每行重算。
    let rootTitles: [String]
    /// 扁平视图的血缘次要行；层级视图传 nil（缩进本身就说明了层级）。
    var lineageLine: String?
    /// 展开三角；nil = 无子节点或本视图不支持展开 → 画透明占位（左缘仍对齐）。
    var expansion: Binding<Bool>?
    /// 本行这次渲染所处的那条父边（nil = 顶层）。多父 crew 在每个父下各画一次，
    /// 拖走的是当前这条边，别的父边不动。
    let parentId: String?
    /// 合法性判定的数据面（层级视图 = 本机器分组；时间流视图 = 全量）。
    let groupCrews: [CrewSummary]
    /// 用**这个 crew 的机长自己填的那句状态**替掉「最后一条消息」那一行（Todo #136）。
    ///
    /// `nil` = 照旧显示最后一条消息（层级 / 时间流两个老视图永远是 nil）。
    ///
    /// 被替掉的那个东西**是一手的、永远为真**（它就是那条消息）；这句状态是机长
    /// 自己报的，跟消息同一次动作产生，所以不会凭空烂 —— 但**沿用来的那句可能来自
    /// 三天前**。年龄已经由 `CrewStatusLine` 拼进正文，这里只负责画。
    var statusLine: CrewStatusLine.Line?

    /// 这一行**能不能靠拖拽改隶属关系**。默认能（层级/时间流两个老视图不受影响）。
    ///
    /// 总机长视图（Todo #102）传 `false`：那个视图是**整理用的**，人在里面看到的
    /// 分段是「现在该管什么」，不是组织结构。如果在那儿一拖就改了汇报线，人会以为
    /// 自己只是在归置列表，结果动了真的组织树 —— 所以这两件事在界面上是**结构性
    /// 分开**的，不是靠一句提示文案区分：改组织树只有层级视图那一条路。
    var allowsReparentDrag: Bool = true

    /// 画不画左边那道谱系竖色条（连同浮在它右上角的状态点）。默认画 —— 三个视图里
    /// 的普通行一个字不受影响。
    ///
    /// **总机组那一行传 `false`**。人类 2026-09-12 原话：「总机组和普通机组的样式要
    /// 一样 包括sidebar里的 颜色条可以不使用」。色条画的是**谱系二分色**（沿父链算
    /// 出来的），而总机组按设计没有也永远不会有父边，那道条对它本来就没有含义。
    ///
    /// **传 false 时色条的位置留空、不塌缩** —— 标题因此仍与其它行左缘对齐，
    /// 那才是「样式一样」。真要让它吃掉那 3pt，改这里一行。
    ///
    /// ⚠️ **状态点挂在色条上，所以它跟着一起没了。** 对总机组这是安全的：那个点的
    /// 两个信号源（本 crew 的人类 Todo、沿父边聚合的后代 Todo）对它结构上都取不到值
    /// —— 它没有 session 去提 Todo，也没有存下来的子边可聚合。**别把这条推广到普通行。**
    var showsColorBar: Bool = true

    /// 挂不挂右键菜单。默认挂。
    ///
    /// **总机组那一行传 `false`**，而这一条不是样式，是挡住两个点了会坏的动作：
    /// - 「在这下面建子 crew」：建子要走 `attachParent`，那里第一行 `refuseBuiltin`
    ///   直接抛 —— 点了只会拿到一个错误。
    /// - 「藏起来」：`setManuallyHidden` **没有**拦内建那一层，而「已隐藏的群」那份
    ///   列表喂的是 `crewStore.crews`（不含 builtin）—— 藏完它不在那份列表里，
    ///   也没有第二个入口把它取回来。
    var showsContextMenu: Bool = true
    @ObservedObject var dragState: CrewDragState
    /// 右键「在这下面建子 crew」的目标（侧栏持有，表单也在那层弹）。
    @Binding var childCrewTarget: CrewChildCreationTarget?
    @EnvironmentObject private var crewStore: CrewStore
    /// session 状态源（isWorking/health/退出）—— 状态点聚合用。由 `MacRootView`
    /// 注入 sidebar（与中/右栏同一实例，才看得到同一批 run）。
    @EnvironmentObject private var sessionRunner: CrewSessionRunner
    @State private var isDropTargeted = false

    /// 此刻拖着的那个 crew 能不能落到本行上（非法目标不高亮、也不接受）。
    private var acceptsDrop: Bool { dragState.accepts(targetId: crew.id, in: groupCrews) }
    private var isSelected: Bool { crewStore.selectedCrewId == crew.id }

    var body: some View {
        Group {
            if allowsReparentDrag {
                rowCore
                    .contextMenu { if showsContextMenu { menuItems } }
                    // 拖起：负载带上「哪个 crew + 当前这条父边」，落地时才知道该摘哪条边。
                    .draggable(CrewDragDropLogic.encode(crewId: crew.id, parentId: parentId)) {
                        Text(crew.title)
                            .font(Theme.Fonts.system(size: 13, weight: .semibold))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                            .onAppear { dragState.begin(crewId: crew.id, parentId: parentId) }
                    }
                    .dropDestination(for: String.self) { items, _ in
                        dragState.handleDrop(items, targetId: crew.id, crews: groupCrews, store: crewStore)
                    } isTargeted: { isDropTargeted = $0 }
            } else {
                // 整理用的视图：右键菜单照旧（建子 crew / 藏起来都不改隶属关系），
                // 但**没有** draggable / dropDestination —— 改汇报线在这里根本没有入口。
                rowCore.contextMenu { if showsContextMenu { menuItems } }
            }
        }
        // 每个 crew 之间留一道竖向呼吸间距 —— 加在高亮 pill 之外(背景/点击区已闭合),
        // 所以是 crew 之间的留白,不是把 pill 撑高;根 crew(独立 list row)与展开的
        // 子 crew(同 VStack 内堆叠)都吃这道下边距,间距统一。
        .padding(.bottom, 6)
    }

    private var rowCore: some View {
        HStack(alignment: .top, spacing: 6) {
            // 三角槽**恒占位**（无三角时画透明占位）—— 同层级的色条/标题左缘必须
            // 对齐，不能因为某行有子带三角就被推出去。
            if let expansion {
                Button { expansion.wrappedValue.toggle() } label: {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .rotationEffect(.degrees(expansion.wrappedValue ? 90 : 0))
                }
                .buttonStyle(.plain)
                .frame(width: 12)
                .padding(.top, 10)
            } else {
                Color.clear.frame(width: 12, height: 1)
            }

            if showsColorBar {
                CrewColorBar(colors: CrewColorBar.chain(for: crew, crewsById: crewsById))
                    // 状态点浮在色条右上角（Todo #71/#73）：红=错误、黄=本 crew 或
                    // 后代有给人类的 Todo（呼吸）、绿=干活中；静止/退出不画。
                    .overlay(alignment: .topTrailing) {
                        CrewStatusDotView(
                            // 黄点唯一来源：人类 Todo 那本还有几条没回应；后台快照已把
                            // 后代条数沿父边递归聚合，并保留 own/descendant 语义。
                            // 读的是 `CrewStore` 后台指纹门控算好的快照，一次字典查表 ——
                            // **不在这里现读 Todo 文件**（那就是 2026-08-17 的形状）。
                            attention: crewStore.humanTodoAttention[crew.id] ?? .none,
                            runs: sessionRunner.runs.filter { $0.crewId == crew.id }
                        )
                        .offset(x: 6, y: -5)
                    }
            } else {
                // 位置留着、不上色：标题左缘因此仍与其它行对齐（「样式一样」的那一半）。
                Color.clear.frame(width: CrewColorBar.defaultWidth)
            }

            let last = resolvedLastMessage
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    CrewTitleRootBadge(
                        title: crew.title,
                        titleFont: Theme.Fonts.system(size: 14, weight: .semibold),
                        titleColor: Theme.Palette.ink,
                        rootTitles: rootTitles,
                        badgeSize: 11
                    )
                    Spacer(minLength: 8)
                    if let date = CrewActivityTime.resolve(
                        lastMessageCreatedAt: last?.createdAt, crewUpdatedAt: crew.updatedAt) {
                        // 相对时间要随 now 老化 —— 不包 TimelineView 的话首次渲染
                        // 算死的文案会一直冻住（同 CrewTimeSeparator）。
                        TimelineView(.everyMinute) { _ in
                            Text(date.formatted(.relative(presentation: .numeric)))
                                .font(Theme.Fonts.caption2)
                                .foregroundStyle(Theme.Palette.inkMuted)
                                // 同 PendingBot 会话行：加了黄字标注后横向变紧，
                                // 不钉住的话相对时间会折行把整行撑高。该让位的是黄字。
                                .lineLimit(1)
                                .fixedSize(horizontal: true, vertical: false)
                        }
                    }
                }
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    if let statusLine {
                        // 「还没有」画得更淡：它不是内容，是一句「这里还没有内容」。
                        Text(statusLine.text)
                            .font(Theme.Fonts.caption)
                            .foregroundStyle(statusLine.isMissing
                                             ? Theme.Palette.inkMuted.opacity(0.6)
                                             : Theme.Palette.inkMuted)
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .help(statusLine.isMissing
                                  ? "这个机组的机长还没报过状态（这里不显示最新消息，也不替他编一句）"
                                  : "这是机长发消息时自己报的状态，不是这个群的最新消息")
                    } else {
                        Text(CrewSidebarCrewRow.preview(of: last))
                            .font(Theme.Fonts.caption)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                    if crew.parentCrewIds.count > 1 {
                        Spacer(minLength: 6)
                        Image(systemName: "arrow.triangle.branch")
                            .font(Theme.Fonts.caption2)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .help("此 crew 挂在多个父 crew 之下,在每个父下都会出现")
                    }
                }
                // 扁平视图专有：直接父 crew 名。层级视图靠缩进说这件事，不画。
                if let lineageLine {
                    HStack(spacing: 3) {
                        Image(systemName: "arrow.turn.up.right")
                            .font(Theme.Fonts.caption2)
                        Text(lineageLine)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted.opacity(0.75))
                    .help("挂在 \(lineageLine) 之下")
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
                .fill(isSelected ? Theme.Palette.accentBg : Color.clear)
        )
        // 拖拽落点高亮：只有**合法**目标才描边（自己 / 自己的子树不高亮，
        // 也接不了 drop）—— 判定全在 `CrewDragDropLogic`。
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
                .strokeBorder(Color.accentColor, lineWidth: 2)
                .opacity(isDropTargeted && acceptsDrop ? 1 : 0)
        )
        .opacity(dragState.draggingCrewId == crew.id && dragState.draggingParentId == parentId ? 0.4 : 1)
        .contentShape(Rectangle())
        .onTapGesture { crewStore.selectCrew(crew.id) }
    }

    /// 行右键菜单（Todo #35）。目标恒为**本行这个 crew**（`crew` 是本行自己的
    /// 数据），不看 `crewStore.selectedCrewId` —— 右键不改变选中，看选中就会建到
    /// 别的 crew 下面。解析走 `CrewChildCreationTarget.forRow`（单测钉死）。
    ///
    /// 两项都**不改隶属关系**（建子是新增、藏起来只动可见性），所以总机长视图那种
    /// 只整理不改结构的地方照样挂得上。
    @ViewBuilder
    private var menuItems: some View {
        Button {
            childCrewTarget = .forRow(crew)
        } label: {
            Label("在这下面建子 crew", systemImage: "plus")
        }
        Divider()
        // 「藏起来」= 从侧栏消失，**聊天记录留着、不真删**，crew 里的 session
        // 照常干活。取回的路在侧栏底部那行「已隐藏的群」。
        // 目标同样恒为本行这个 crew，不看选中态。
        Button {
            requestHide()
        } label: {
            Label("藏起来", systemImage: "eye.slash")
        }
    }

    /// 右键「藏起来」。判定全在 `CrewHiding.decide`（纯函数、单测钉死），这里只
    /// 负责取信号 + 把结果交给侧栏那层去弹。
    ///
    /// **数据面取全量 `crewStore.crews` 而不是 `groupCrews`**：子 crew 的父边可以
    /// 跨机器，只喂本机器分组会把「底下还有个正在跑的子」判丢 —— 与黄字标注
    /// （`rootTitles`）同一个理由。
    ///
    /// 判定当场算、不进 `body`：菜单一次点击一次，摊到每行每帧就是几十次可达性
    /// 计算，侧栏这条路不该背这个。
    private func requestHide() {
        let active = Set(sessionRunner.runs.filter { $0.status == .running }.map(\.crewId))
        switch CrewHiding.decide(hiding: crew.id, in: crewStore.crews, activeSessionCrewIds: active) {
        case .blocked(let titles):
            crewStore.hideBlockedNotice = """
                「\(crew.title)」底下还有子 crew 有 session 正在跑：\(titles.joined(separator: "、"))。

                藏了它，这些子 crew 也会跟着从侧栏消失 —— 它们还在干活，却没有入口进得去了。\
                先停掉它们，或者先把它们各自藏起来，再来藏这一个。
                """
        case .allowed(let alsoHidden) where alsoHidden > 0:
            crewStore.pendingSubtreeHide = CrewStore.PendingSubtreeHide(
                crewId: crew.id, crewTitle: crew.title, alsoHiddenCount: alsoHidden)
        case .allowed:
            Task { await crewStore.hideCrewFromUI(crew.id) }
        }
    }

    /// 本行的末条白板消息。**只读 store 的现成快照，不碰磁盘**（`CrewStore`
    /// 在后台按指纹门控算好、真变了才发布 —— 见 `lastWhiteboardMessages`）。
    ///
    /// 这里以前是 `LocalWhiteboardStore.shared.list(crewId:).last`：在 SwiftUI
    /// body 里、主线程上、每个 crew 各解一份整板 JSON，目录一有动静就来一遍。
    /// 那是 2026-08-17「开久了卡」的头号病根，别再改回去。
    private var resolvedLastMessage: LocalWhiteboardMessage? {
        crewStore.lastWhiteboardMessages[crew.id]
    }

    /// 最近一条消息的预览文案;空则"还没有消息"。
    static func preview(of last: LocalWhiteboardMessage?) -> String {
        let text = last?.text ?? ""
        return text.isEmpty ? "还没有消息" : text
    }
}

/// 头像右上角的 crew 状态点。聚合逻辑在 `CrewStatusAggregation`（纯函数，单测
/// 覆盖优先级），这里只负责取信号 + 画点。
///
/// **嵌套 ObservableObject 观察**：`run.isWorking` / `run.health` / `run.status`
/// 是各 run 自己的 `@Published` —— 父视图观察 `CrewSessionRunner` 看不到 run 内
/// 部变更（同 `SessionBarItemView` 的问题）。这里 merge 所有 run 的
/// `objectWillChange` 撞一下本视图的 `@State revision`，任一 run 状态变化即重
/// 渲染重新聚合。runs 列表本身增删由父视图（观察 runner.runs）驱动重建。
struct CrewStatusDotView: View {
    /// 人类 Todo 的后台聚合快照；自身/后代分开，供可访问文案明确指路。
    var attention: CrewHumanTodoAttention = .none
    let runs: [CrewSessionRun]

    @State private var revision = 0

    var body: some View {
        Group {
            if let color = dotColor {
                ZStack {
                    // 2pt 背景色描边 —— 让点从头像盘上浮出（点 10pt + 描边圈 14pt）。
                    // 带数字时描边跟着长，胶囊照样是「从盘上浮出来」那个视觉。
                    Capsule()
                        .fill(Theme.Palette.canvas)
                        .frame(width: badgeWidth + 4, height: 14)
                    if let badge = yellowBadge(color) {
                        // 黄色 = 有事等人拍板，**只有它带数字**：红是错误、绿是在干活，
                        // 都没有「几件」可数。数字口径见 `CrewHumanTodoAttention.badge`。
                        Capsule()
                            .fill(fill(color))
                            .frame(width: badgeWidth, height: 10)
                        Text(badge)
                            .font(.system(size: 8, weight: .bold, design: .rounded))
                            .foregroundStyle(.black.opacity(0.75))
                    } else if color.breathes {
                        BreathingDot(size: 10, color: fill(color))
                    } else {
                        Circle()
                            .fill(fill(color))
                            .frame(width: 10, height: 10)
                    }
                }
                .help(helpText(color) ?? "")
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(accessibilityLabel(color))
            }
        }
        .onReceive(Publishers.MergeMany(runs.map(\.objectWillChange))) { _ in
            revision &+= 1
        }
    }

    private var dotColor: CrewStatusDotColor? {
        _ = revision // 把 revision 变成 body 依赖：run 内部状态一变即重新聚合。
        return CrewStatusAggregation.dot(
            sessions: runs.map {
                CrewSessionStatusSignal(
                    isAlive: $0.status == .running,
                    isWorking: $0.activityIsWorking,
                    hasHealthIssue: $0.health != nil,
                    // `awaitingReply` 仍留在信号快照供其它状态消费，但 Todo #71 起不再
                    // 把“等回复”当错误染红；要人处理的事由人类 Todo 黄点表达。
                    isAwaitingReply: $0.awaitingReply != nil,
                    // 卡在终端那个等人选的框上 —— 人类 2026-09-12 要求它染红不染黄。
                    isBlockedOnTerminalChoice: $0.pendingDecision != nil)
            },
            attention: attention)
    }

    /// 黄点上要显示的数字；没有数字（或不是黄色）时为 nil。
    /// **0 不显示**由 `CrewHumanTodoAttention.badge` 保证，这里不重复判。
    private func yellowBadge(_ color: CrewStatusDotColor) -> String? {
        color == .yellow ? attention.badge : nil
    }

    /// 数字越长胶囊越宽；没数字就是原来那个 10pt 圆点的宽度（画圆时也用它）。
    private var badgeWidth: CGFloat {
        guard let badge = attention.badge, dotColor == .yellow else { return 10 }
        return max(10, 6 + CGFloat(badge.count) * 5)
    }

    /// 配色对齐右栏切换条状态点（`SessionBarItemView`）：系统语义色。
    private func fill(_ color: CrewStatusDotColor) -> Color {
        switch color {
        case .red: return .red
        case .yellow: return .yellow
        case .green: return .green
        }
    }

    /// 悬浮提示：黄明确区分「本 crew」与「下属 crew」；红 = 首个异常 detail。
    private func helpText(_ color: CrewStatusDotColor) -> String? {
        switch color {
        case .yellow: return attention.accessibilityLabel
        case .red: return runs.first(where: { $0.health != nil })?.health?.detail
        case .green: return nil
        }
    }

    private func accessibilityLabel(_ color: CrewStatusDotColor) -> String {
        switch color {
        case .yellow: return attention.accessibilityLabel ?? "有人类 Todo 等你拍板"
        case .red: return helpText(color) ?? "crew 有运行错误"
        case .green: return "crew 正在工作"
        }
    }
}
#endif
