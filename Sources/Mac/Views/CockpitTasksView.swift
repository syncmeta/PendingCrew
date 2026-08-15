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
/// task 账读哪本由 `CockpitTaskLedger` 判定 —— 活跃账优先，回落仓库 markdown 账时
/// **顶部如实说明**，不把停更的数据画得好看点糊弄人。
struct CockpitTasksGlance: View {
    let data: CockpitData
    let crewId: String?

    @State private var todos: [LocalTodoItem] = []
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
        return todoItems + data.taskItems
    }

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
