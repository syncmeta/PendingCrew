#if os(macOS)
import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// Todo 详细窗口（Todo #11）：完整列表 + 可读回应 + 重开入口。
///
/// **为什么是自建 NSWindow 而不是 `WindowGroup` 场景**：重开要走 `CrewSessionRunner`
/// 唤醒机长，而 runner 归 app 级的 `SessionHost` 持有（前后端分离 P0；此前是
/// `MacRootView` 的 view-local `@StateObject`），独立 Scene 拿不到它。用
/// NSHostingController 起一个真窗口、把依赖显式传进去，既是真窗口（可缩放、独立于
/// 主窗，人能一直开着看）又不用把 runner 提成全局单例。
///
/// 每 crew 最多一个窗口：再次调用 `open` 只是前置已有窗口。
@MainActor
final class CrewTodoDetailWindowPresenter: NSObject, NSWindowDelegate {
    static let shared = CrewTodoDetailWindowPresenter()

    private var windows: [String: NSWindow] = [:]

    /// `ledger` 只作**首次打开**时停在哪个药丸上（Todo #62）—— 每 crew 一个窗口，
    /// 已经开着的那个由它自己的药丸说了算，不被概览面板隔空拨走。
    func open(crewId: String, crewName: String?, ledger: TodoLedger = .agent,
              runner: CrewSessionRunner, appModel: AppModel,
              colorScheme: ColorScheme?) {
        if let existing = windows[crewId] {
            existing.makeKeyAndOrderFront(nil)
            return
        }
        let root = CrewTodoDetailView(crewId: crewId, ledger: ledger, runner: runner)
            .environmentObject(appModel)
            .preferredColorScheme(colorScheme)
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: Self.defaultContentSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = crewName.map { "Todo — \($0)" } ?? "Todo"
        // `sizingOptions = []` 是「窗口开出来是最小的」那个 bug 的根（Todo #21）：
        // NSHostingController 默认把 SwiftUI 的**理想尺寸**装进窗口约束，而这棵树是
        // 一个 ScrollView（理想高度＝内容高度，条目少时几乎为 0），于是 contentRect
        // 给的 520×620 当场被压回内容的最小尺寸。关掉自动定尺，尺寸由我们自己说了算。
        let hosting = NSHostingController(rootView: root)
        hosting.sizingOptions = []
        window.contentViewController = hosting
        window.contentMinSize = NSSize(width: 420, height: 360)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.setContentSize(Self.defaultContentSize)
        // 记住人拉过的尺寸/位置：所有 crew 共用一个 autosave 名（Todo 窗口是同一种
        // 窗口，人调一次该对所有 crew 生效）。没存过 → 首次居中。
        window.setFrameAutosaveName(Self.frameAutosaveName)
        if !window.setFrameUsingName(Self.frameAutosaveName) { window.center() }
        windows[crewId] = window
        window.makeKeyAndOrderFront(nil)
    }

    /// 首次打开的尺寸：够一眼看清列表 + 一条正文摊开，不用马上去拉窗户。
    private static let defaultContentSize = NSSize(width: 620, height: 720)
    private static let frameAutosaveName = "CrewTodoDetailWindow"

    func windowWillClose(_ notification: Notification) {
        guard let closing = notification.object as? NSWindow else { return }
        windows = windows.filter { $0.value !== closing }
    }
}

/// 详细窗口内容：全量条目（从新到旧）+ 每条的机器人/人类回应 + 改 / 删 / 追问。
///
/// **三件事都在这个窗口里做完**（Todo #21，人类原话「在详细的列表里面回复」）：
/// 点「改」「追问」就地长出输入框，删走行内二次确认，不弹 sheet、不跳窗口 ——
/// 人一边看着上下文一边动手，视线不用离开这条 Todo。
struct CrewTodoDetailView: View {
    let crewId: String
    let runner: CrewSessionRunner

    @EnvironmentObject private var appModel: AppModel

    /// 当前看的是哪本账（Todo #62）。初值由打开它的那个面板给。
    @State private var ledger: TodoLedger
    @State private var todos: [LocalTodoItem] = []
    init(crewId: String, ledger: TodoLedger = .agent, runner: CrewSessionRunner) {
        self.crewId = crewId
        self.runner = runner
        _ledger = State(initialValue: ledger)
    }

    /// 同一时刻只有一行摊开一个编辑器 —— 换行/换动作自动收掉上一个。
    @State private var editor: RowEditor?
    @State private var draft = ""
    @State private var busy = false
    @State private var errorText: String?
    /// 追问要带的图（Todo #52）—— 粘贴 / 选文件收进来，确认时才落盘。与编辑器
    /// 同生命周期：换行、收起、发完都清空（暂存副本一起收掉）。
    @State private var followUpAttachments: [PendingComposerAttachment] = []
    @State private var showFileImporter = false

    /// 行内编辑器的三种形态（都挂在某个 #N 上）。
    private enum RowEditor: Equatable {
        case followUp(Int)      // 追问 / 重开（同一条通道）
        case edit(Int)          // 改正文
        case confirmDelete(Int) // 删除二次确认
        case respond(Int)       // 人类回应（Todo #62：只在「人类的」那本上）
    }

    private var rows: [LocalTodoItem] { TodoListPresentation.newestFirst(todos) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                CrewTodoLedgerPills(ledger: $ledger)
                Spacer(minLength: 8)
                Text(ledger == .agent ? "你派给 agent 的活" : "agent 请你拍板的事")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            Divider()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    if rows.isEmpty {
                        Text(TodoListPresentation.emptyHint(ledger))
                            .font(Theme.Fonts.footnote)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.top, 8)
                    } else {
                        ForEach(rows) { row($0) }
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .background(Theme.Palette.canvas)
        // 换药丸 = 换一本账重订（两本各自一个文件、一把锁）。换的同时收掉编辑器 ——
        // 编辑器挂在 #N 上，两本账的 #N 指两件事，留着会张冠李戴。
        .task(id: TodoFeedKey(crewId: crewId, ledger: ledger)) {
            closeEditor()
            let store = LocalTodoStore.shared(ledger)
            todos = store.list(crewId: crewId)
            for await _ in store.todoChanges(crewId: crewId) {
                todos = store.list(crewId: crewId)
            }
        }
    }

    @ViewBuilder
    private func row(_ item: LocalTodoItem) -> some View {
        let icon = TodoListPresentation.statusIcon(item.status)
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                CrewTodoStatusCircle(status: item.status, size: 14)
                Text("#\(item.number)")
                    .font(Theme.Fonts.footnote.weight(.semibold).monospacedDigit())
                    .foregroundStyle(Theme.Palette.inkMuted)
                // 已完成只变灰，**不加删除线**。
                Text(item.text)
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(icon.dimsText ? Theme.Palette.inkMuted : Theme.Palette.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                rowActions(item)
            }
            // 条目自带的图（Todo #52）—— 点开看大图，与群聊气泡同一个查看器。
            CrewTodoAttachmentStrip(attachments: item.attachments ?? [])
                .padding(.leading, 22)
            // 三种行内编辑器共用同一块位置：一次只摊开一个。
            if editor == .respond(item.number) {
                respondEditor(item)
            } else if editor == .followUp(item.number) {
                followUpEditor(item)
            } else if editor == .edit(item.number) {
                editEditor(item)
            } else if editor == .confirmDelete(item.number) {
                deleteConfirm(item)
            }
            // 回应全量按时间序展开 —— 详细窗口就是「读得进去」的那一层。
            ForEach(item.responses) { resp in
                VStack(alignment: .leading, spacing: 2) {
                    Text(resp.senderName ?? "session:\(resp.sessionId.prefix(6))")
                        .font(Theme.Fonts.caption2.weight(.semibold))
                        .foregroundStyle(Theme.Palette.inkMuted)
                    Text(resp.text)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.ink)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    // 追问带的图（Todo #52）挂在那条追问下面，不与条目本身的图混。
                    CrewTodoAttachmentStrip(attachments: resp.attachments ?? [], cell: 60)
                }
                .padding(.leading, 22)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.Palette.surfaceMuted.opacity(0.5),
                    in: RoundedRectangle(cornerRadius: 8))
    }

    /// 每行的动作。**不分状态** —— 已完成、已被回复过的条目照样动得了
    /// （Todo #21 的整条诉求就是「随时」）。
    ///
    /// 两本账的动作不同（Todo #62）：`.agent` 那本是人类派活，所以是改 / 追问 / 删；
    /// `.human` 那本是 agent 请人拍板，人类要做的是**回应**（回应会叫醒当初提它的
    /// 那个 session），外加一颗「不办」把黄点按灭 —— 有些事人看过就决定不办，
    /// 没有这个开关，那一条会把黄点永久钉死。
    @ViewBuilder
    private func rowActions(_ item: LocalTodoItem) -> some View {
        HStack(spacing: 10) {
            actionButton("pencil", "修改这条 Todo 的正文") { open(.edit(item.number), draft: item.text) }
            if ledger == .human {
                actionButton("arrowshape.turn.up.left", "回应这条 —— 发进群里并叫醒当初提它的那个 session") {
                    open(.respond(item.number), draft: "")
                }
                actionButton(item.dismissedAt == nil ? "bell.slash" : "bell",
                             item.dismissedAt == nil
                             ? "不打算回应 —— 只把提醒按灭，不加回应、不动状态"
                             : "重新算作未回应（提醒会再亮起来）") {
                    toggleDismissed(item)
                }
            } else {
                actionButton("arrowshape.turn.up.left",
                             item.status == "completed" ? "追问（会把这条翻回待办）" : "追问这条") {
                    open(.followUp(item.number), draft: "")
                }
            }
            actionButton("trash", "删除这条 Todo") { open(.confirmDelete(item.number), draft: "") }
        }
    }

    @ViewBuilder
    private func actionButton(_ symbol: String, _ help: String,
                              _ action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: symbol).font(Theme.Fonts.caption) }
            .buttonStyle(.borderless)
            .foregroundStyle(Theme.Palette.inkMuted)
            .disabled(busy)
            .help(help)
    }

    /// 摊开一个编辑器（同一颗再点一次 = 收起）。换行/换动作自动丢掉上一个草稿。
    private func open(_ target: RowEditor, draft newDraft: String) {
        if editor == target { closeEditor(); return }
        editor = target
        draft = newDraft
        errorText = nil
    }

    private func closeEditor() {
        // 收起 = 这些图不发了：暂存副本一起收掉，别在 temp 里留着（Todo #52）。
        CrewLocalAttachmentPersist.discardStaging(followUpAttachments)
        followUpAttachments = []
        editor = nil
        draft = ""
        errorText = nil
    }

    // MARK: - 行内编辑器（都在这个窗口里做完，不弹 sheet 不跳窗）

    /// 追问编辑器：一行输入 + **带图**（Todo #52）+ 确认 / 收起。
    ///
    /// 三条进图的路：⌘V（`onPasteCommand` —— 剪贴板里只有截图时文本框自己不接
    /// 这次粘贴，事件冒到这里）、回形针（文件选择器）、剪贴板钮（⌘V 不灵时的
    /// 确定性入口）。三条都汇进 `followUpAttachments`，与群聊 composer 同一套
    /// intake（大小闸门 / 目录拒收 / 暂存复制）。
    @ViewBuilder
    private func followUpEditor(_ item: LocalTodoItem) -> some View {
        let isReopen = item.status == "completed"
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                TextField(isReopen ? "补充追问说明（可留空）" : "追问点什么（可留空）", text: $draft)
                    .textFieldStyle(.roundedBorder)
                    .font(Theme.Fonts.caption)
                    .onSubmit { Task { await performFollowUp(item) } }
                actionButton("paperclip", "选一张图/文件附在这条追问上") {
                    showFileImporter = true
                }
                actionButton("doc.on.clipboard", "把剪贴板里的图附在这条追问上") {
                    pasteFollowUpAttachments()
                }
                Button(isReopen ? "重开" : "追问") { Task { await performFollowUp(item) } }
                    .font(Theme.Fonts.caption)
                    .disabled(busy)
                Button { closeEditor() } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain)
                    .disabled(busy)
            }
            CrewTodoPendingAttachmentTray(items: $followUpAttachments)
            errorLine
        }
        .padding(.leading, 22)
        .onPasteCommand(of: [.png, .tiff, .fileURL]) { _ in pasteFollowUpAttachments() }
        .fileImporter(isPresented: $showFileImporter,
                      allowedContentTypes: [.item],
                      allowsMultipleSelection: true,
                      onCompletion: handleFollowUpImport)
    }

    /// 剪贴板 → 待发托盘（读取收口在 `CrewPasteboardReader`，与群聊 ⌘V 同一份）。
    private func pasteFollowUpAttachments() {
        let items = CrewPasteboardReader.attachments()
        guard !items.isEmpty else {
            errorText = "剪贴板里没有图片或文件。"
            return
        }
        errorText = nil
        followUpAttachments.append(contentsOf: items)
    }

    /// 文件选择器 → 待发托盘。与群聊拖入/选文件**共用** `CrewFileAttachmentIntake`
    /// （大小闸门、目录/package 拒收、mime 推导、复制进暂存区都在那一份里）。
    private func handleFollowUpImport(_ result: Result<[URL], Error>) {
        switch result {
        case .failure(let error):
            errorText = error.localizedDescription
        case .success(let urls):
            Task {
                // 复制可能是几百 MB，挪出主线程。
                let intake = await Task.detached(priority: .userInitiated) {
                    CrewFileAttachmentIntake.intake(urls: urls)
                }.value
                followUpAttachments.append(contentsOf: intake.accepted)
                errorText = intake.errors.first
            }
        }
    }

    @ViewBuilder
    private func editEditor(_ item: LocalTodoItem) -> some View {
        inlineEditor(placeholder: "改这条的正文", confirmTitle: "保存") {
            await performEdit(item)
        }
    }

    /// 人类回应「人类 Todo」（Todo #62）。发出去要做三件：落账 → 群里出一行
    /// 「回应 人类 To Do #N：…」→ 叫醒当初提它的那个 session（退出了就回落机长
    /// 转达）。编排在 `CrewHumanTodoRespond`，这里只收一行字。
    @ViewBuilder
    private func respondEditor(_ item: LocalTodoItem) -> some View {
        inlineEditor(placeholder: "你的答复 —— 会发进群里并叫醒提这件事的那个 session",
                     confirmTitle: "回应") {
            await performRespond(item)
        }
    }

    /// 追问 / 改正文共用的一行输入 + 确认 + 收起。
    @ViewBuilder
    private func inlineEditor(placeholder: String, confirmTitle: String,
                              commit: @escaping () async -> Void) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                TextField(placeholder, text: $draft)
                    .textFieldStyle(.roundedBorder)
                    .font(Theme.Fonts.caption)
                    .onSubmit { Task { await commit() } }
                Button(confirmTitle) { Task { await commit() } }
                    .font(Theme.Fonts.caption)
                    .disabled(busy)
                Button { closeEditor() } label: { Image(systemName: "xmark") }
                    .buttonStyle(.borderless)
                    .disabled(busy)
            }
            errorLine
        }
        .padding(.leading, 22)
    }

    /// 删除二次确认 —— 人类手输的 Todo，误点一下就没了不行。
    @ViewBuilder
    private func deleteConfirm(_ item: LocalTodoItem) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text("删掉 #\(item.number)？回应记录会一起从列表消失。")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
                Button("删除") { performDelete(item) }
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.danger)
                    .disabled(busy)
                Button("取消") { closeEditor() }
                    .font(Theme.Fonts.caption)
                    .disabled(busy)
            }
            errorLine
        }
        .padding(.leading, 22)
    }

    @ViewBuilder
    private var errorLine: some View {
        if let errorText {
            Text(errorText)
                .font(Theme.Fonts.caption2)
                .foregroundStyle(Theme.Palette.amber)
        }
    }

    // MARK: - 动作

    /// 追问：数据 → 发群 → 唤醒机长（completed 的顺带翻回待办）。人的追问要真能
    /// 推动事情，所以走的是和重开同一条编排（`CrewTodoFollowUp`）。
    private func performFollowUp(_ item: LocalTodoItem) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        let note = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        // 图先落盘（与群聊同一个 attachments/<crewId>/ 目录、同一套命名）。
        let saved: [LocalWhiteboardAttachment]
        do {
            saved = try CrewLocalAttachmentPersist.persist(followUpAttachments, crewId: crewId)
        } catch {
            errorText = error.localizedDescription
            return
        }
        guard await CrewTodoFollowUp.perform(
            crewId: crewId, number: item.number, note: note, attachments: saved,
            runner: runner, appModel: appModel) != nil
        else {
            errorText = "追问失败：这条已经不在了。"
            return
        }
        closeEditor()
    }

    /// 回应：编排全在 `CrewHumanTodoRespond`（顺序、失败措辞出自共享剧本
    /// `TodoLandingFlow`）。这里只做两件：不许发空回应；把它返回的那句话原样亮出来。
    private func performRespond(_ item: LocalTodoItem) async {
        guard !busy else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            errorText = "写一句再发 —— 空回应对提问的那个 session 没有意义。"
            return
        }
        busy = true
        defer { busy = false }
        if let problem = await CrewHumanTodoRespond.perform(
            crewId: crewId, item: item, text: text, runner: runner, appModel: appModel) {
            // 没走完三步就如实说，别关编辑器（人还要重试 / 复制那段话）。
            errorText = problem
            return
        }
        closeEditor()
    }

    /// 「不办」开关（Todo #62）：只打 `dismissedAt` 标记 —— 不加回应、不动状态，
    /// 单纯让黄点算得出「这条不再算未回应」。再点一次反悔。
    private func toggleDismissed(_ item: LocalTodoItem) {
        guard !busy else { return }
        guard LocalTodoStore.shared(ledger).setDismissed(
            crewId: crewId, number: item.number, dismissed: item.dismissedAt == nil) else {
            errorText = "改不动：这条已经不在了。"
            return
        }
        errorText = nil
    }

    private func performEdit(_ item: LocalTodoItem) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        guard LocalTodoStore.shared(ledger).edit(
            crewId: crewId, number: item.number, text: draft) != nil
        else {
            errorText = "改不动：正文不能为空，或这条已经不在了。"
            return
        }
        closeEditor()
    }

    private func performDelete(_ item: LocalTodoItem) {
        guard !busy else { return }
        guard LocalTodoStore.shared(ledger).delete(crewId: crewId, number: item.number) else {
            errorText = "删不掉：这条已经不在了。"
            return
        }
        closeEditor()
    }
}
#endif
