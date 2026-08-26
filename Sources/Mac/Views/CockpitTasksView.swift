#if os(macOS)
import SwiftUI

/// 驾驶舱「任务」段 —— **三段式 glance**：在做什么 / 接下来做什么 / 做了什么（#542）。
///
/// 取代原来的五列看板（todo·doing·pending-qa·done·dropped + 范围开关 + 「N/M 个任务」
/// 计数）——那是给机器看的状态机，人要的是一眼读出活儿的三个时态。
///
/// **两本账合流**：人类 Todo（每 crew 一份，人给的活）和 task 账（机器视角的活）
/// 按同一套时态摆进同三段，不再各占一个 tab（handbook 说这两本本就是「机器视角 vs
/// 人视角，配对」）。Todo 行带「人」标，点开看回应时间线；改 Todo 状态 / 重开仍在右栏
/// 那块常驻 Todo 面板做（同一份 store，不在这儿复制一套编排）。
///
/// **机长作战板也并进来**（Todo #66，`CockpitPlanStore`）：它是这里唯一由机长第一手
/// 写下的来源，其余几种都是从别人的账上读来的。它自己另有一块可写的面板（第三个药丸），
/// 但**不并进这个 glance 就会变成孤岛** —— 「在做什么 / 接下来 / 做了什么」这个总摘要
/// 少了机长自己的计划就不成其为总摘要。行尾如实带上「最后更新 N 天前」。
///
/// task 账读哪本由 `CockpitTaskLedger` 判定 —— 活跃账优先，回落仓库 markdown 账时
/// **顶部如实说明**，不把停更的数据画得好看点糊弄人。
struct CockpitTasksGlance: View {
    let data: CockpitData
    let crewId: String?

    @State private var todos: [LocalTodoItem] = []
    /// `.human` 那本（Todo #62）—— 只为判断「卡在 #N」那条引用还在不在。
    /// **和 `todos` 一样是 `@State`，不是现读**：判定发生在 `planItems` 的 body
    /// 求值路径上，在那儿调 `LocalTodoStore.item()` 就是 flock + 整份 JSON 解码。
    /// 这里不另造指纹门控缓存 —— 那套是给侧栏「N 个 crew × 每次目录 tick」准备的；
    /// 本页只认一个 crew，跟旁边两条订阅同形即可。
    @State private var humanTodos: [LocalTodoItem] = []
    @State private var plans: [CockpitPlanItem] = []
    @State private var expanded: Set<String> = []      // 展开看回应时间线的 Todo 行 id

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 26) {
                if case let .repoLedger(reason) = data.taskSource, !reason.isEmpty {
                    fallbackNote(reason)
                }
                ForEach(CockpitTaskLedger.bands(merged)) { group in
                    bandSection(group)
                }
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 22)
            .frame(maxWidth: 720, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.Palette.canvas)
        .task(id: crewId ?? "") {
            guard let crewId else { todos = []; return }
            todos = LocalTodoStore.shared.list(crewId: crewId)
            for await _ in LocalTodoStore.shared.todoChanges(crewId: crewId) {
                todos = LocalTodoStore.shared.list(crewId: crewId)
            }
        }
        // 作战板单开一条订阅：上面那条 `for await` 永不返回，跟它挤在同一个 task 里
        // 第二条流一辈子跑不起来。
        .task(id: crewId ?? "") {
            guard let crewId else { plans = []; return }
            plans = CockpitPlanStore.shared.list(crewId: crewId)
            for await _ in CockpitPlanStore.shared.planChanges(crewId: crewId) {
                plans = CockpitPlanStore.shared.list(crewId: crewId)
            }
        }
        // `.human` 那本单开一条 —— 同上，`for await` 永不返回，不能跟别的挤一个 task。
        // 走 `shared(_:)` 而不是 `humanShared`：这一侧是 app 进程，共享实例吃的就是
        // 默认目录，是对的（helper 侧相反，见 `McpServer.blockerState`）。
        .task(id: crewId ?? "") {
            guard let crewId else { humanTodos = []; return }
            let store = LocalTodoStore.shared(.human)
            humanTodos = store.list(crewId: crewId)
            for await _ in store.todoChanges(crewId: crewId) {
                humanTodos = store.list(crewId: crewId)
            }
        }
    }

    // MARK: - 合流

    /// Todo + task 合成一串,交给 `CockpitTaskLedger.bands` 归段。Todo 的 id 加前缀防撞号
    /// （Todo 是 #3、task 也可能是 #3）。
    private var merged: [CockpitTaskItem] {
        // 从新到旧（与右栏 Todo 面板同一套 `TodoListPresentation.newestFirst`）——
        // 段内再按更新时间排，没有时间戳时这个顺序就是最终顺序。
        let todoItems = TodoListPresentation.newestFirst(todos).map { t in
            CockpitTaskItem(
                id: Self.todoPrefix + String(t.number),
                title: t.text,
                statusRaw: t.status,
                origin: .humanTodo,
                updated: Self.iso.date(from: t.responses.last?.createdAt ?? t.createdAt),
                badge: "#\(t.number)",
                note: t.responses.last.map { "\($0.senderName ?? "session")：\($0.text)" } ?? "")
        }
        return planItems + todoItems + data.taskItems
    }

    /// 机长作战板 → 统一条目。`note` 那一行按重要性挑：**卡住的卡点压过最近进度**
    /// （卡住是唯一需要有人动一下的一档），都没有时退回「进行中 · 最后更新 3 天前」。
    private var planItems: [CockpitTaskItem] {
        let now = Date()
        return CockpitPlan.newestFirst(plans).map { p in
            let updated = Self.iso.date(from: p.updatedAt)
            let blockerLine = p.blockedBy.map {
                CockpitPlan.blockerLine($0, state: CockpitPlan.blockerState(
                    $0,
                    agentTodoExists: { n in todos.contains { $0.number == n } },
                    humanTodoExists: { n in humanTodos.contains { $0.number == n } }))
            } ?? ""
            let statusLine = CockpitPlan.statusLine(statusRaw: p.status, updated: updated, now: now)
            let recent = p.updates.last.map { "最近：\($0.text)" } ?? ""
            let note = [blockerLine, recent].first { !$0.isEmpty } ?? statusLine
            return CockpitTaskItem(
                id: Self.planPrefix + String(p.number),
                title: p.title,
                statusRaw: p.status,
                origin: .captainPlan,
                updated: updated,
                badge: "#\(p.number)",
                note: note)
        }
    }

    private static let planPrefix = "plan:"
    private static let todoPrefix = "todo:"
    private static let iso = ISO8601DateFormatter()

    private func todo(_ item: CockpitTaskItem) -> LocalTodoItem? {
        guard item.origin == .humanTodo,
              let n = Int(item.id.dropFirst(Self.todoPrefix.count))
        else { return nil }
        return todos.first { $0.number == n }
    }

    // MARK: - 段

    @ViewBuilder private func bandSection(_ group: CockpitBandGroup) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(group.band.rawValue)
                .font(Theme.Fonts.caption.weight(.semibold))
                .foregroundStyle(Theme.Palette.inkMuted)
                .padding(.bottom, 6)
            if group.items.isEmpty {
                Text("—")
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .padding(.vertical, 4)
            } else {
                ForEach(group.items) { row($0, band: group.band) }
            }
            if group.hiddenCount > 0 {
                Text("更早的 \(group.hiddenCount) 条不在这儿")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .padding(.top, 6)
            }
        }
    }

    // MARK: - 行

    @ViewBuilder private func row(_ item: CockpitTaskItem, band: CockpitBand) -> some View {
        let todoItem = todo(item)
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 9) {
                // 人类 Todo 用右栏那套状态圆圈（空心 / 呼吸 / 实心），两处一致；
                // task 账的行仍是段色小圆点（它没有 Todo 的三态语义）。
                if todoItem != nil {
                    CrewTodoStatusCircle(status: item.statusRaw, size: 11)
                } else {
                    Circle()
                        .fill(dotColor(band))
                        .frame(width: 6, height: 6)
                        .offset(y: -1)
                }
                Text(item.title)
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(band == .done ? Theme.Palette.inkMuted : Theme.Palette.ink)
                    .lineLimit(2)
                    .textSelection(.enabled)
                Spacer(minLength: 8)
                if item.origin == .captainPlan {
                    Text("机长")
                        .font(Theme.Fonts.caption2)
                        .foregroundStyle(Theme.Palette.accent)
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(Theme.Palette.accent.opacity(0.12), in: Capsule())
                }
                if item.origin == .humanTodo {
                    Text("人")
                        .font(Theme.Fonts.caption2)
                        .foregroundStyle(Theme.Palette.plum)
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(Theme.Palette.plumBg, in: Capsule())
                }
                Text(item.badge)
                    .font(Theme.Fonts.caption2.monospaced())
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
            if !item.note.isEmpty, !expanded.contains(item.id) {
                Text(item.note)
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .lineLimit(1)
                    .padding(.leading, 15)
            }
            if let todoItem, expanded.contains(item.id) {
                todoDetail(todoItem)
            }
        }
        .padding(.vertical, 5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .onTapGesture {
            // 密集信息（Todo 的全部回应）收进展开的二级视图,不一上来糊在脸上。
            guard todoItem != nil else { return }
            if expanded.contains(item.id) { expanded.remove(item.id) } else { expanded.insert(item.id) }
        }
    }

    private func dotColor(_ band: CockpitBand) -> Color {
        switch band {
        case .doing: return Theme.Palette.accent
        case .next:  return Theme.Palette.inkMuted.opacity(0.5)
        case .done:  return Theme.Palette.success.opacity(0.6)
        }
    }

    // MARK: - Todo 展开（回应时间线）

    @ViewBuilder private func todoDetail(_ item: LocalTodoItem) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            // 条目自带的图（Todo #52）—— 与右栏面板同一块缩略图条，点开看大图。
            CrewTodoAttachmentStrip(attachments: item.attachments ?? [], cell: 44)
            ForEach(item.responses) { resp in
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline, spacing: 4) {
                        Image(systemName: "arrow.turn.down.right")
                            .font(.system(size: 9))
                            .foregroundStyle(Theme.Palette.inkMuted)
                        Text("\(resp.senderName ?? "session:\(resp.sessionId.prefix(6))")：\(resp.text)")
                            .font(Theme.Fonts.caption)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .textSelection(.enabled)
                    }
                    CrewTodoAttachmentStrip(attachments: resp.attachments ?? [], cell: 44)
                        .padding(.leading, 13)
                }
            }
        }
        .padding(.leading, 15)
        .padding(.top, 2)
    }

    // MARK: - 回落说明

    /// 读的不是活跃账时如实说一句 —— 这正是 #542 的病根（一本停更的账被当现状看）。
    private func fallbackNote(_ reason: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Image(systemName: "exclamationmark.triangle")
                .font(Theme.Fonts.caption2)
                .foregroundStyle(Theme.Palette.amber)
            Text("这里读的是仓库 docs/tasks（可能已停更）——\(reason)")
                .font(Theme.Fonts.caption2)
                .foregroundStyle(Theme.Palette.inkMuted)
        }
    }
}
#endif
