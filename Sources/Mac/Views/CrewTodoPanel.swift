#if os(macOS)
import SwiftUI

/// 驾驶舱「Todo」段的人类 Todo 列表（task #487；原 #478 的右栏 inspector 面板挪入）。
///
/// 每 crew 一个 todo 列表：**只有人类能加条目** —— 新增入口在群聊 composer 的
/// Todo 切换按钮（CrewChatView：点亮后发送 = 创建，群里出「To do +1: #N」）；
/// MCP 不暴露新增工具。机器人经 `respond_todo` 追加回应 + 推进状态
/// （待办 → 进行中 → 完成），回应以时间序缩进显示在条目下。
///
/// 数据层 `LocalTodoStore`：本进程人类新增即推；机器人回应来自 helper 子进程
/// （跨进程写盘），由 `todoChanges` 合流的目录监听补齐 —— 列表即时刷新，无轮询。
///
/// **这块是概览**（Todo #4/#5/#11）：从新到旧（`TodoListPresentation.newestFirst`）、
/// 提醒事项风格的状态圆圈（`CrewTodoStatusCircle`）、已完成只变灰不划线、每条最多
/// 显示最近一条回应。要读全量回应或**重开**（Todo #12）走「详细」——
/// 顶部按钮或点任意一行都开 `CrewTodoDetailWindowPresenter` 的独立窗口，
/// runner 由调用方（CockpitView / CrewSessionWindowView）显式传入并转交给窗口
/// （cockpit 子树没有 sessionRunner 环境对象，详细窗口的重开要靠它唤醒机长）。
struct CrewTodoPanel: View {
    let crewId: String
    /// 机长唤醒用（idle 注入 / 未跑拉起）。不做 @ObservedObject —— 只在动作时读。
    let runner: CrewSessionRunner
    /// 详细窗口标题用（「Todo — <crew 名>」）。拿不到就只显「Todo」。
    var crewName: String? = nil

    @EnvironmentObject private var appModel: AppModel
    @AppStorage(AppearanceMode.storageKey) private var appearanceRaw = AppearanceMode.default.rawValue

    @State private var todos: [LocalTodoItem] = []
    /// 当前看的是哪本账（Todo #62）。两个药丸「Agent 的 / 人类的」切它。
    @State private var ledger: TodoLedger = .agent

    /// 从新到旧 —— 人类明确要求新建的在最上面（纯逻辑有单测钉住）。
    private var rows: [LocalTodoItem] { TodoListPresentation.newestFirst(todos) }
    private let layout = TodoListPresentation.overviewLayout

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text("待做")
                    .font(Theme.Fonts.headline.weight(.semibold))
                    .foregroundStyle(Theme.Palette.ink)
                CrewTodoLedgerPills(ledger: $ledger)
                Spacer(minLength: 8)
                Button(layout.detailButtonTitle) { openDetail() }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .font(Theme.Fonts.caption)
                    .tint(Theme.Palette.accent)
                    .help("打开 Todo 详细窗口：全量回应 + 重开")
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            if rows.isEmpty {
                Text(TodoListPresentation.emptyHint(ledger))
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 6)
            } else {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(rows) { todoRow($0) }
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        // 首拉 + 订阅变更（人类新增本进程即推；机器人回应经目录监听跨进程补齐）。
        // `id` 带上 ledger —— 换药丸就换一本账重订（两本各自一个文件、一把锁）。
        // 读全量只在这条 task 里做，**不在 body 求值路径上**（那条红线）。
        .task(id: TodoFeedKey(crewId: crewId, ledger: ledger)) {
            let store = LocalTodoStore.shared(ledger)
            todos = store.list(crewId: crewId)
            for await _ in store.todoChanges(crewId: crewId) {
                todos = store.list(crewId: crewId)
            }
        }
    }

    /// 详细窗口入口（顶部按钮 + 点行都走这儿）。每 crew 最多一个窗口，重复调用只前置。
    private func openDetail() {
        CrewTodoDetailWindowPresenter.shared.open(
            crewId: crewId, crewName: crewName, ledger: ledger,
            runner: runner, appModel: appModel,
            colorScheme: (AppearanceMode(rawValue: appearanceRaw) ?? .default).colorScheme)
    }

    // MARK: - 行渲染

    @ViewBuilder
    private func todoRow(_ item: LocalTodoItem) -> some View {
        let icon = TodoListPresentation.statusIcon(item.status)
        let corners = layout.cardCorners
        let cardShape = UnevenRoundedRectangle(
            cornerRadii: .init(
                topLeading: CGFloat(corners.topLeading),
                bottomLeading: CGFloat(corners.bottomLeading),
                bottomTrailing: CGFloat(corners.bottomTrailing),
                topTrailing: CGFloat(corners.topTrailing)),
            style: .continuous)
        VStack(alignment: .leading, spacing: 7) {
            // 附图的层级：状态圆点 + 序号先单独成行，正文另进下面的卡片。
            HStack(alignment: .center, spacing: 6) {
                CrewTodoStatusCircle(status: item.status, size: 15)
                Text("\(item.number)")
                    .font(Theme.Fonts.footnote.weight(.semibold).monospacedDigit())
                    .foregroundStyle(Theme.Palette.accent)
            }

            VStack(alignment: .leading, spacing: 8) {
                // 已完成只变灰，**不加删除线**（人类明确要求）。
                Text(item.text)
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(icon.dimsText ? Theme.Palette.inkMuted : Theme.Palette.ink)
                    .lineLimit(layout.bodyLineLimit)
                    .truncationMode(.tail)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)

                // 条目带的图（Todo #52）：概览给小格子、最多 3 张，点开看大图。
                CrewTodoAttachmentStrip(attachments: item.attachments ?? [],
                                        cell: 36, maxVisible: 3)

                // 已回复项只露最近一条精简回应；历史与全文在「放大看」里读。
                if let response = TodoListPresentation.overviewResponse(for: item) {
                    Text(response)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .lineLimit(layout.responseLineLimit)
                        .truncationMode(.tail)
                }
            }
            .padding(.vertical, 10)
            .padding(.horizontal, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(cardShape.fill(Theme.Palette.surface))
            .overlay(cardShape.strokeBorder(Theme.Palette.hairline, lineWidth: 0.5))
        }
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .onTapGesture { openDetail() }
    }
}

/// 「Agent 的 / 人类的」两个药丸（Todo #62）—— 概览面板与详细窗口共用一份，
/// 免得两处各长一个样子。人类原话「弄两个药丸选择」。
struct CrewTodoLedgerPills: View {
    @Binding var ledger: TodoLedger

    var body: some View {
        HStack(spacing: 4) {
            ForEach(TodoLedger.allCases, id: \.self) { l in
                Button(l.pillTitle) { ledger = l }
                    .buttonStyle(.plain)
                    .font(Theme.Fonts.caption2.weight(ledger == l ? .semibold : .regular))
                    .foregroundStyle(ledger == l ? Theme.Palette.accent : Theme.Palette.inkMuted)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(
                        Capsule().fill(ledger == l
                            ? Theme.Palette.accent.opacity(0.14)
                            : Theme.Palette.surfaceMuted.opacity(0.6)))
                    .help(l == .agent
                          ? "人类派给 agent 的活 —— 机器人经 respond_todo 回应"
                          : "agent 请人类拍板的事 —— 你回应后会叫醒当初提它的那个 session")
            }
        }
    }
}

/// `.task(id:)` 的复合键：crew 换了、或药丸换了本账，都得重订。
struct TodoFeedKey: Equatable {
    let crewId: String
    let ledger: TodoLedger
}
#endif
