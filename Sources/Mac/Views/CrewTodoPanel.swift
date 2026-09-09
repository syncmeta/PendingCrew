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
    /// agent 那本里**正卡在人类身上**的条目（人类 Todo #139）。看「人类的」那本时
    /// 借显在同一屏里 —— **借显，不是复制**：条目仍只有一条，躺在 agent 那本。
    /// 看「Agent 的」那本时用不上（那本自己就全在 `todos` 里），保持空。
    @State private var waitingOnHuman: [LocalTodoItem] = []
    /// 当前看的是哪本账（Todo #62）。两个药丸「Agent 的 / 人类的」切它。
    @State private var ledger: TodoLedger = .agent

    /// 这一屏的行（排序 + 跨本账借显都在纯逻辑里，有单测钉住）。
    private var rows: [TodoListPresentation.Row] {
        ledger == .human
            ? TodoListPresentation.rows(for: .human, human: todos, agent: waitingOnHuman)
            : TodoListPresentation.rows(for: .agent, human: [], agent: todos)
    }
    private let layout = TodoListPresentation.overviewLayout

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text("待做")
                    .font(Theme.Fonts.headline.weight(.semibold))
                    .foregroundStyle(Theme.Palette.ink)
                CrewTodoLedgerPills(ledger: $ledger)
                Spacer(minLength: 8)
                Button(layout.detailButtonTitle) { openDetail(ledger: ledger, focus: nil) }
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
        // 「人类的」那本还要跟着 agent 那本走 —— 那边翻成 `blocked_on_human` 时，
        // 这一屏要当场多出一行。**只在看人类那本时才订**（看 agent 那本时它就是
        // `todos` 自己，再订一份是白烧一条目录监听）。
        .task(id: TodoFeedKey(crewId: crewId, ledger: ledger)) {
            guard ledger == .human else { waitingOnHuman = []; return }
            let agentStore = LocalTodoStore.shared(.agent)
            waitingOnHuman = agentStore.list(crewId: crewId)
            for await _ in agentStore.todoChanges(crewId: crewId) {
                waitingOnHuman = agentStore.list(crewId: crewId)
            }
        }
    }

    /// 详细窗口入口。每 crew 最多一个窗口，重复调用只前置 —— 但 `focus` 每次都会送进去。
    ///
    /// `focus` = 这次要人看的那一条的 #N（人类 Todo #122）。点某一行传它的号，
    /// 顶部「放大看」按钮传 nil（那是**列表**入口，不是某一条）。
    /// `ledger` 传**这一行自己那本**，不是药丸选的那本 —— 借显过来的行点进去，
    /// 要落在 agent 那本的 #N 上，否则会打开人类那本里号码相同的另一件事。
    private func openDetail(ledger: TodoLedger, focus: Int?) {
        CrewTodoDetailWindowPresenter.shared.open(
            crewId: crewId, crewName: crewName, ledger: ledger, focus: focus,
            runner: runner, appModel: appModel,
            colorScheme: (AppearanceMode(rawValue: appearanceRaw) ?? .default).colorScheme)
    }

    // MARK: - 行渲染

    @ViewBuilder
    private func todoRow(_ row: TodoListPresentation.Row) -> some View {
        let item = row.item
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
                // 借显过来的行要带上本账名（#139）：两本账的 #N 各自从 1 起，
                // 人类那本里裸写一个「7」，他会去人类那本找 #7 —— 那是另一件事。
                Text(TodoListPresentation.rowNumberLabel(row, shownIn: ledger))
                    .font(Theme.Fonts.footnote.weight(.semibold).monospacedDigit())
                    .foregroundStyle(Theme.Palette.accent)
            }

            VStack(alignment: .leading, spacing: 8) {
                // 已完成只变灰，**不加删除线**（人类明确要求）。
                //
                // 正文渲染 markdown（人类 Todo #119），字号仍是 footnote(13pt) ——
                // 「ui 格式要和外面的没点放大看进去之前一样」。截断在**源文本层**做
                // （`cardMarkdown`）：`.lineLimit` 对 markdown 是逐 block 生效的，
                // 单靠它一条长 Todo 就能把卡片撑到 12.5 倍高。lineLimit 仍留着当兜底。
                MarkdownText(
                    text: TodoListPresentation.cardMarkdown(
                        item.text, lineBudget: layout.bodyLineLimit),
                    variant: .todo,
                    dimmed: icon.dimsText)
                    .lineLimit(layout.bodyLineLimit)
                    .textSelection(.enabled)

                Text(TodoListPresentation.metadataText(for: item))
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted)

                // 条目带的图（Todo #52）：概览给小格子、最多 3 张，点开看大图。
                CrewTodoAttachmentStrip(attachments: item.attachments ?? [],
                                        cell: 36, maxVisible: 3)

                // 已回复项只露最近一条精简回应；历史与全文在「放大看」里读。
                if let response = TodoListPresentation.overviewResponse(for: item) {
                    // 回应也在「todo 页面」里，同一套样式（Todo #119）。`overviewResponse`
                    // 已经把它折成一行，所以这里只需再按 1 行预算兜一次。
                    MarkdownText(
                        text: TodoListPresentation.cardMarkdown(
                            response, lineBudget: layout.responseLineLimit),
                        variant: .todoNote,
                        dimmed: true)
                        .lineLimit(layout.responseLineLimit)
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
        .onTapGesture { openDetail(ledger: row.ledger, focus: item.number) }
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
