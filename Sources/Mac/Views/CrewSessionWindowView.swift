#if os(macOS)
import SwiftUI

/// 右栏（inspector）：两模式合一。
///
/// - **成员列表模式**（`sessionRunner.viewingTerminal == false`，平时）：待审批区 +
///   crew 成员富列表，「+ 新 session」作为成员列表第一行（session
///   行带简介 + 最新一步动作，可点进终端）。
/// - **终端模式**（`viewingTerminal == true`）：顶部「‹ 成员」返回，下面是原样的
///   session 切换条 + 终端 + composer + 内联审批卡。
///
/// 终端那一层只观察 `CrewSessionRunner.current`（哪个 run 在前台）。具体 run 的
/// status 变化由内层 `SessionRunContentView` 通过 `@ObservedObject` 观察 ——
/// **必须分两层**：`CrewSessionRun` 是嵌套的 ObservableObject，父 view 只
/// `@EnvironmentObject` 持有 runner 时不会订阅到 run 自己的 `@Published`，否则
/// status 更新了 UI 不刷新。
///
/// **数据刷新（Phase 5：去轮询）**：成员列表模式订阅 `backend.whiteboardChanges`
/// （事件驱动）刷本 crew 的白板/roster，中栏 CrewChatView 各自订阅同一条流 ——
/// 两端各订各的，但都已无 3s/2s 定时器。未做共享数据层的合并（v1 可接受）。
struct CrewSessionWindowView: View {
    @EnvironmentObject private var sessionRunner: CrewSessionRunner
    @EnvironmentObject private var appModel: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    @Environment(\.colorScheme) private var colorScheme

    @State private var draft = ""
    @State private var starting = false
    @State private var localError: String?
    /// 成员行右键「设为机长」的待确认目标。确认后不是原地翻角色，而是续接同一
    /// agent conversation 以 captain 世界观/工具权限重新挂起。
    @State private var pendingCaptainRun: CrewSessionRun?
    /// composer 里给「新 worker session」选的 agent kind。初值从 crew 的
    /// `captainAgentKind` 派生（见 `.onAppear` / `.onChange`），crew 没记 → `.codex`。
    @State private var selectedKind: LocalCodingAgentKind = .codex
    /// 新建 agent session 时是否直接以机长身份启动。纯终端不能成为 crew 成员，
    /// 当前已有运行中机长时也不能再占一个 captain slot。
    @State private var startsAsCaptain = false

    // MARK: - 成员列表模式 state（自轮询白板 + roster，与中栏各拉各的）

    /// 本 crew 白板条目（拿来取「最新一步动作」）。
    @State private var entries: [CrewWhiteboardEntry] = []
    /// 本 crew server 成员名册。
    @State private var members: [CrewMember] = []
    @State private var captainBotId: String?
    /// 未读角标的轻量刷新计数（chunk2 T6）—— 每次白板变更事件自增，逼 badgeCount
    /// 重算（store 数据在文件里，无 ObservableObject 推送）。Phase 5 把驱动从 2s
    /// 墙钟 timer 换成 `whiteboardChanges` 订阅：agent 经 `post_to_crew` 写白板
    /// （跨进程，目录监听捕获）或本进程 append 都会推一个事件 → bump 一次。
    @State private var badgeTick = 0

    /// spec §9.3 + 用户心智：右栏是**常驻**和 claude/codex 交互的面（像 Codex
    /// Desktop）。没有 session 时就地给一个 composer —— **第一条消息即开始
    /// session**（零配置，用 crew 默认目录/runner/权限模式；无标题、无单独的
    /// 「新建」表单）。运行中再发 = 直接把文本注入终端续聊。
    var body: some View {
        Group {
            if sessionRunner.viewingTerminal {
                terminalMode
            } else {
                memberListMode
            }
        }
        // 填满 inspector 栏 —— 内容是弹性的，跟随固定列宽布局，不溢出。
        .frame(maxWidth: .infinity)
        // 无 navigationTitle —— 这是 inspector 内容,标题会漏到主窗标题栏(显成
        // "Session" 盖掉 crew 名)。窗标题由中栏 toolbar 的 crew 名负责。
        // 成员列表模式要的白板/roster 数据 —— 事件驱动订阅（去 3s 轮询，与中栏各订各的）。
        .task(id: crewStore.selectedDetail?.crew.id) { await subscribeRoster() }
        // 切前台 = 看过了：清掉新选中 run 的未读（T6）。
        .onChange(of: sessionRunner.selectedRunId) { _, newId in
            if let run = sessionRunner.runs.first(where: { $0.runID == newId }) {
                SessionUnreadStore.shared.markViewed(run.sessionId)
            }
        }
        // composer 的 agent kind 默认跟随 crew 建好时选的 captainAgentKind ——
        // 初次出现 + 切 crew 都重算（crew 没记 → `.codex`）。
        .onAppear {
            selectedKind = Self.resolveAgentKind(crewStore.selectedDetail)
            startsAsCaptain = false
        }
        .onChange(of: crewStore.selectedDetail?.crew.id) { _, _ in
            selectedKind = Self.resolveAgentKind(crewStore.selectedDetail)
            startsAsCaptain = false
        }
        .onChange(of: selectedKind) { _, kind in
            if kind == .terminal { startsAsCaptain = false }
        }
        .confirmationDialog(
            pendingCaptainRun.map { "把「\($0.displayName)」设为机长？" } ?? "重新指定机长？",
            isPresented: Binding(
                get: { pendingCaptainRun != nil },
                set: { if !$0 { pendingCaptainRun = nil } }),
            titleVisibility: .visible,
            presenting: pendingCaptainRun
        ) { run in
            Button("重新指定机长", role: .destructive) {
                pendingCaptainRun = nil
                Task { await reassignCaptain(to: run) }
            }
            Button("取消", role: .cancel) { pendingCaptainRun = nil }
        } message: { run in
            Text("当前机长会停止；这个 \(run.kind.displayName) session 会续接原 conversation，并以机长权限重新启动。")
        }
    }

    /// 把 crew 的 `captainAgentKind`（"claude_code" / "codex"）映射成
    /// `LocalCodingAgentKind`；缺省 / nil / 未知值 → `.codex`（机长默认 Codex）。
    static func resolveAgentKind(_ detail: CrewDetail?) -> LocalCodingAgentKind {
        LocalCodingAgentKind.captainDefault(detail?.crew.captainAgentKind)
    }

    // MARK: - 终端模式（原样保留：切换条 + 终端 + composer + 审批卡）

    private var terminalMode: some View {
        HStack(spacing: 0) {
            // 左侧成员头像竖条 —— 始终保留(含新建 session 占位),当前 session/新建态高亮。
            terminalRail
            Divider()
            terminalContent
        }
    }

    /// 终端模式左侧的成员头像竖条:顶「‹」回成员列表;成员头像(session 可点切换,
    /// 选中高亮);末尾「+」起新 session(新建态时「+」高亮)。
    private var terminalRail: some View {
        VStack(spacing: 0) {
            Button { sessionRunner.viewingTerminal = false } label: {
                Image(systemName: "chevron.left")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
            .buttonStyle(.plain)
            .padding(.vertical, 10)
            .help("返回成员列表")
            ScrollView {
                LazyVStack(spacing: 6) {
                    ForEach(memberRowItems) { railAvatar($0) }
                    // 新建 session 占位 —— composing 时高亮,代表"正在建的那个 session"。
                    Button {
                        sessionRunner.composeNew()
                        sessionRunner.viewingTerminal = true
                    } label: {
                        Image(systemName: "plus")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .frame(width: 34, height: 34)
                            .overlay(
                                Circle().strokeBorder(
                                    Theme.Palette.inkMuted.opacity(0.4),
                                    style: StrokeStyle(lineWidth: 1.5, dash: [3, 2])))
                            .padding(8)
                            .background(
                                RoundedRectangle(cornerRadius: 12, style: .continuous)
                                    .fill(sessionRunner.isComposingNew
                                          ? railSelectionColor : .clear))
                    }
                    .buttonStyle(.plain)
                    .help("起一个新 session")
                }
                .padding(.vertical, 8)
            }
        }
        .frame(width: 64)
    }

    /// 头像栏选中框底色：浅色 = sidebar 选中 crew 同款很浅绿（accentBg）；
    /// 深色 = 很浅的白灰（白低透明）—— 深色下 accentBg 偏「绿盒子」太重。
    private var railSelectionColor: Color {
        colorScheme == .dark ? Color.white.opacity(0.12) : Theme.Palette.accentBg
    }

    @ViewBuilder
    private func railAvatar(_ item: MemberRowItem) -> some View {
        let isSelected = item.run != nil
            && item.run?.runID == sessionRunner.selectedRunId && !sessionRunner.isComposingNew
        // 选中态 = 圆角矩形框（不是圆）——头像四周留 8pt 呼吸,框距栏边 7pt。
        let av = CrewAvatarBadges(sender: item.sender, size: 34)
            .padding(8)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(isSelected ? railSelectionColor : .clear))
        if let run = item.run {
            Button { sessionRunner.select(run.runID) } label: { av }
                .buttonStyle(.plain)
                .help(item.sender.displayName)
        } else {
            av.help(item.sender.displayName)
        }
    }

    private var terminalContent: some View {
        VStack(spacing: 0) {
            if sessionRunner.isComposingNew {
                // 「新建 session」态：显零配置启动面（composer 在下方）。
                idleState
            } else if let run = sessionRunner.current {
                // `.id(run.runID)` 强制换 run 时整个终端视图重建。
                SessionRunContentView(
                    run: run,
                    onSwitchProfile: { model, effort in
                        Task {
                            await sessionRunner.applyProfileChange(
                                SessionProfileChangeRequest(
                                    crewId: run.crewId, sessionId: run.sessionId,
                                    model: model, effort: effort))
                        }
                    },
                    onSwitchApproval: { reviewer in
                        Task {
                            await sessionRunner.applyCodexApprovalMode(
                                to: run, reviewer: reviewer)
                        }
                    })
                    .id(run.runID)
                if run.kind.isAgent {
                    SessionApprovalCardsView(crewId: run.crewId, sessionId: run.sessionId)
                }
            } else {
                idleState
            }
            // 底部输入按态分家：claude 在跑 = 不给（真终端本身可交互，双输入框
            // 反而歧义）；codex 在跑 = 群聊同款输入胶囊（transcript 不可直接打字）；
            // 新建 / 已退出 = 原 composer（runner picker + 首条指令；model/effort
            // 建后在终端页头部选，#485）。
            if isContinuing {
                if sessionRunner.current?.kind == .codex {
                    codexComposer
                        .background(Theme.Palette.canvas)
                }
            } else {
                Divider()
                composer
            }
        }
    }

    // MARK: - 成员列表模式（平时：上半 Todo 面板 | 下半 待审批 + 成员富列表）

    /// Todo #16：右栏拆上下两半 —— 上半人类 Todo 列表（与驾驶舱同一个
    /// `CrewTodoPanel`），下半原有的待审批 + 成员列表。用原生 `VSplitView`：
    /// 分割线可拖、两 pane 各自 ScrollView 滚动，min 高度小到窗口矮时也不塌
    ///（VSplitView 在 NavigationSplitView 的固定列内做垂直分割，不影响列宽协商）。
    private var memberListMode: some View {
        VStack(spacing: 0) {
            // 启动 Captain 失败原因（含「未在 PATH 中找到 codex/claude」「无工作目录」）——
            // 点击入口（captain 成员行）就在这屏，错误必须在这显，否则点了像没反应。
            if let err = sessionRunner.lastStartError {
                Text(err)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                Divider()
            }
            if let crewId = crewStore.selectedDetail?.crew.id {
                VSplitView {
                    ScrollView {
                        CrewTodoPanel(crewId: crewId, runner: sessionRunner,
                                      crewName: crewStore.selectedDetail?.crew.title)
                    }
                    .frame(minHeight: 100, idealHeight: 240, maxHeight: .infinity)
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            memberSection
                        }
                    }
                    .frame(minHeight: 140, maxHeight: .infinity)
                }
            } else {
                Spacer(minLength: 0)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .overlay {
            if crewStore.selectedDetail == nil {
                Text("先在左侧选一个 crew").foregroundStyle(.secondary)
            }
        }
    }

    // MARK: 成员富列表

    /// 本 crew 的本地 agent run —— 列进成员列表（session 行可点进终端）。
    /// 纯终端只出现在 session 切换条，绝不是成员或 @ 候选。
    private var memberSessionRuns: [CrewSessionRun] {
        sessionRunner.runs.filter {
            $0.crewId == crewStore.selectedDetail?.crew.id && $0.kind.isAgent
        }
    }

    private var memberSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("成员 \(memberRowItems.count)")
                .font(Theme.Fonts.caption.weight(.semibold))
                .foregroundStyle(Theme.Palette.inkMuted)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
            LazyVStack(spacing: 2) {
                // 「+ 新 session」占成员列表第一行 —— 起新 session 的常驻入口
                //（原顶栏按钮撤了；终端模式左条末尾的「+」仍在）。
                if crewStore.selectedDetail != nil {
                    newSessionRow
                }
                ForEach(memberRowItems) { memberRow($0) }
            }
            .padding(.horizontal, 8)
            .padding(.bottom, 10)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// 成员行统一模型：server 成员（去掉已有本地 run 的 code_session）+ 本地 run。
    private struct MemberRowItem: Identifiable {
        let id: String
        let sender: GroupBubbleSender
        /// 非 nil = 本地 session run，可点进终端 + 显简介/最新动作。
        let run: CrewSessionRun?
        /// 已登记但当前没有 live run 的本地 agent 成员。非 nil 时点击即续接原
        /// session，而不是落到「新 session」composer（Todo #80）。
        let persistedMember: LocalSessionMember?
        /// 机长 / 人类 —— 固定置顶，不参与创建时刻倒序（#15）。
        let isPinned: Bool
        /// 创建时刻（session 成员登记时刻；本地 run 没登记则退回进程启动时刻）。
        let createdAt: Date?

        var orderKey: CrewMemberOrdering.Key {
            .init(id: id, isPinned: isPinned, createdAt: createdAt)
        }
    }

    private var memberRowItems: [MemberRowItem] {
        let runSessionIds = Set(memberSessionRuns.map { $0.sessionId })
        let crewId = crewStore.selectedDetail?.crew.id ?? ""
        let persistedBySessionId = Dictionary(
            uniqueKeysWithValues: LocalCrewStore.shared.sessionMembers(crewId: crewId)
                .map { ($0.sessionId, $0) })
        // 有在跑的 captain run 时，server 的 captain 成员收敛成那条可点的 run 行 ——
        // 否则会重复出现「机长」(不可点) + 「Captain」(可点) 两条（用户要"点头像进
        // session"，留一条带星标可点的即可）。
        let hasCaptainRun = memberSessionRuns.contains { $0.role == .captain }
        let serverRows = members
            .filter { m in
                // code_session 成员已有本地 run 的 → 去重（用 run 行）。
                if m.memberKind == "code_session",
                   m.codeSessionId.map(runSessionIds.contains) ?? false { return false }
                // captain 成员在有在跑 captain run 时 → 去重（用 run 行）。
                if hasCaptainRun,
                   m.memberKind == "captain" || (m.botId != nil && m.botId == captainBotId) {
                    return false
                }
                return true
            }
            .map { m in
                MemberRowItem(
                    id: m.id,
                    sender: CrewSenderNaming.groupSender(for: m, captainBotId: captainBotId),
                    run: nil,
                    persistedMember: m.codeSessionId.flatMap { persistedBySessionId[$0] },
                    isPinned: m.memberKind == "human" || m.memberKind == "captain"
                        || (m.botId != nil && m.botId == captainBotId),
                    createdAt: CrewMemberOrdering.parseDate(m.createdAt))
            }
        // run 行的创建时刻取 server 成员那条的登记时刻（同一个 session 的真创建时刻），
        // 没登记（如还没落盘的新 run）才退回进程启动时刻。
        let createdBySessionId = Dictionary(
            members.compactMap { m -> (String, Date)? in
                guard let sid = m.codeSessionId, let d = CrewMemberOrdering.parseDate(m.createdAt)
                else { return nil }
                return (sid, d)
            }, uniquingKeysWith: { a, _ in a })
        let runRows = memberSessionRuns.map { run in
            MemberRowItem(
                id: "run-\(run.sessionId)", sender: senderForRun(run), run: run,
                persistedMember: nil,
                isPinned: run.role == .captain,
                createdAt: createdBySessionId[run.sessionId] ?? run.startedAt)
        }
        return CrewMemberOrdering.sorted(serverRows + runRows) { $0.orderKey }
    }

    @ViewBuilder
    private func memberRow(_ item: MemberRowItem) -> some View {
        if let run = item.run {
            // session 行：富行 —— 头像 + 名 + 简介 + 最新一步动作，点进终端。
            // 只读 model/effort pill 收在 sessionRowContent 里、与最新一步动作同一行
            // （落在选中框内，不再浮在框外右侧 —— 切换入口仍只在终端页头部，#485/#489）。
            Button {
                sessionRunner.select(run.runID)
                sessionRunner.viewingTerminal = true
            } label: {
                sessionRowContent(sender: item.sender, run: run)
            }
            .buttonStyle(.plain)
            .help("打开这个 session 的终端")
            .contextMenu {
                if run.role == .worker && run.kind.isAgent {
                    Button("设为机长") { pendingCaptainRun = run }
                }
            }
        } else if let member = item.persistedMember {
            // 持久成员没有 live run：点它就带原 sessionId + agent conversation id
            // 续接，并在成功后直接打开这一个 session（Todo #80）。
            Button {
                Task { await openPersistedSession(member) }
            } label: {
                memberPlainRow(item.sender).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(starting)
            .help("恢复并打开这个 session")
        } else if item.sender.isCaptain {
            // captain 没在跑:点头像即进入它的 session —— 没在跑就当场起一个再进。
            // 用户定调:不要常驻/自动起,只要「点头像就进 session」;这条是纯点击触发,
            // 起成功后切到终端,失败则留在成员列表(错误经 lastStartError 显示)。
            Button {
                Task {
                    await startCaptain()
                    if hasRunningCaptain { sessionRunner.viewingTerminal = true }
                }
            } label: {
                memberPlainRow(item.sender)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(starting)
            .help("进入 Captain session（没在跑会先起一个）")
        } else {
            // human 行：只头像 + 名(无 session 可进)。
            memberPlainRow(item.sender)
        }
    }

    private func openPersistedSession(_ member: LocalSessionMember) async {
        guard let detail = crewStore.selectedDetail else { return }
        starting = true
        defer { starting = false }
        sessionRunner.lastStartError = nil
        do {
            try await sessionRunner.restartMember(
                detail: detail,
                backend: appModel.backend,
                member: member,
                wakeText: "人类点击了这个未运行的 session，请恢复原 conversation 并继续待命。")
            guard let run = sessionRunner.runs.first(where: {
                $0.sessionId == member.sessionId && $0.status == .running
            }) else {
                throw SessionOpenError.restartedRunMissing
            }
            sessionRunner.select(run.runID)
            sessionRunner.viewingTerminal = true
        } catch {
            sessionRunner.lastStartError = "恢复 \(member.displayName) 失败：\(error.localizedDescription)"
        }
    }

    private enum SessionOpenError: LocalizedError {
        case restartedRunMissing
        var errorDescription: String? { "恢复请求返回后没有找到运行中的 session" }
    }

    /// 成员列表第一行：起新 session。排版对齐成员行（圆形「+」当头像位）。
    private var newSessionRow: some View {
        Button {
            sessionRunner.composeNew()
            sessionRunner.viewingTerminal = true
        } label: {
            HStack(spacing: 10) {
                Image(systemName: "plus")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Theme.Palette.accent)
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(Theme.Palette.accentBg))
                Text("新 session")
                    .font(Theme.Fonts.callout)
                    .foregroundStyle(Theme.Palette.ink)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(starting)
        .help("起一个新 session（与在跑的并存）")
    }

    /// 成员行的「头像 + 名」基础排版（captain 入口与 human 行共用）。
    private func memberPlainRow(_ sender: GroupBubbleSender) -> some View {
        HStack(spacing: 10) {
            CrewAvatarBadges(sender: sender, size: 30)
            Text(sender.displayName)
                .font(Theme.Fonts.callout)
                .foregroundStyle(Theme.Palette.ink)
                .lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
    }

    @ViewBuilder
    private func sessionRowContent(sender: GroupBubbleSender, run: CrewSessionRun) -> some View {
        let isSelected = run.runID == sessionRunner.selectedRunId
        HStack(alignment: .top, spacing: 10) {
            CrewAvatarBadges(sender: sender, size: 30)
            VStack(alignment: .leading, spacing: 2) {
                // 主名 = displayName（worker=精简 title、captain=机长）—— 与群聊气泡名
                // 单一真值,一致且 ≤18 字。次要行仍展示「在干嘛」(latestStep 兜到 taskBrief),
                // 用户明确要「还能知道在干嘛」,brief 不丢。
                Text(run.displayName)
                    .font(Theme.Fonts.callout)
                    .foregroundStyle(Theme.Palette.ink)
                    .lineLimit(1)
                // 最新一步动作 + 只读 model/effort pill 同一行：pill 靠右端，
                // 和「在干嘛」共处选中框内（#489）。
                HStack(spacing: 6) {
                    Text(latestStep(for: run))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    SessionProfileReadonlyPill(run: run)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(isSelected ? Theme.Palette.accentBg : .clear)
        )
        .contentShape(Rectangle())
    }

    /// session 行的「最新一步动作」：取该 session 在本 crew 白板上最近一条消息的
    /// 文本；没有就退回 run 状态（running / 已退出）。
    /// **尾巴**：只有走 `post_to_crew` 写进白板的进展才拿得到 —— 纯终端 stdout
    /// （没 post_to_crew）这里看不到，退回状态展示。
    ///
    /// **取数走 `entries`，body 里不碰磁盘**（2026-08-26）。这里以前是
    /// `LocalWhiteboardStore.shared.list(crewId:)`：那是 **flock + 整份白板 JSON
    /// 全量解码**，而这个方法在 `sessionRowContent` 的 body 里、**每个 session 行
    /// 各来一次、每次重绘各来一次** —— 跟 2026-08-17「开久了卡」是同一个形状，
    /// 当年那次修只扫到了侧栏，漏了这个窗口。
    ///
    /// `entries` 是同一份数据：`subscribeRoster` 在 `.task` 里订
    /// `whiteboardChanges` 拉 `listCrewWhiteboard`，本地实现就是把
    /// `LocalWhiteboardStore.list` 逐条映射成 entry（`summary` = 消息原文、
    /// 不截断、不分页），所以显示内容与改前一致，只是读的时机从「每帧」挪到了
    /// 「白板真变了」。行只覆盖当前选中的 crew（`memberSessionRuns` 就按
    /// `selectedDetail` 过滤），与 `entries` 的范围一致。
    private func latestStep(for run: CrewSessionRun) -> String {
        // 健康异常优先于最新动作 —— 挂了就该一眼看到怎么修,别被普通进展盖住。
        if let health = run.health, run.status == .running {
            return "⚠️ \(health.detail)"
        }
        // 首帧 `entries` 还是空的（订阅尚未拉回）→ 落到下面的 taskBrief 兜底，
        // 白板一拉回就自动补上。不为这一帧去主线程读盘。
        if let last = entries.last(where: { $0.senderSessionId == run.sessionId }) {
            return last.summary ?? last.payload?.text ?? ""
        }
        // 还没有白板动作 → 退回完整 taskBrief（主名现在是精简 title,brief 挪到这作
        // 「在干嘛」的详情,别丢）。captain 的 brief 是开场报到 prompt,不拿来当详情。
        if run.role != .captain, !run.taskBrief.isEmpty { return run.taskBrief }
        return run.status == .running ? "running" : "已退出"
    }

    /// 本地 run → session 头像 sender（状态点 running=绿/退出=灰）。从中栏搬来。
    /// captain run 带星标（isCaptain）+ captainBotId 作头像 seed —— 与 server
    /// captain 成员头像视觉一致（去重后这条是唯一的机长行）。
    private func senderForRun(_ run: CrewSessionRun) -> GroupBubbleSender {
        let isCaptain = run.role == .captain
        return GroupBubbleSender(
            // captain 的 emoji/色种子对齐 server captain 成员(captainBotId),否则
            // 同一 captain"在跑/没跑"会换脸;worker 仍用 sessionId。
            kind: .bot, id: isCaptain ? (captainBotId ?? run.sessionId) : run.sessionId,
            displayName: run.displayName,
            avatarPath: nil,
            avatarSeed: isCaptain ? (captainBotId ?? run.sessionId) : run.sessionId,
            isCaptain: isCaptain,
            // 状态走共享推导（`CrewSessionStateDerivation`，与点名快照/inspect_session
            // 同一份），别在视图里现拼 —— 现拼那版漏了 awaitingDecision / launchFailed /
            // rateLimited，卡在待决策上的 session 会被画成「空闲」。颜色见 SessionStatusDot。
            sessionStatus: CrewSessionStateDerivation.state(
                isRunning: run.status == .running, health: run.health,
                isWorking: run.isWorking, awaitingDecision: run.pendingDecision != nil,
                awaitingReply: run.awaitingReply != nil),
            isSession: true)
    }

    // MARK: - 成员列表模式数据订阅 + 答复

    /// 事件驱动订阅本 crew 的白板（待审批 + 最新动作）和 roster（去 3s 轮询）。
    /// 先 refresh 一次兜住订阅前状态，再 `for await` backend 变更流：每个 tick
    /// refresh 一次并 bump `badgeTick`（驱动切换条角标重算 —— agent 跨进程 post
    /// 进白板会经目录监听推一个事件）。与中栏 CrewChatView 各订各的，但都已无定时器。
    private func subscribeRoster() async {
        entries = []
        members = []
        guard let crewId = crewStore.selectedDetail?.crew.id,
              let backend = appModel.backend else { return }
        await refreshRoster()
        badgeTick &+= 1
        for await _ in backend.whiteboardChanges(crewId: crewId) {
            if Task.isCancelled { return }
            await refreshRoster()
            badgeTick &+= 1
            // Auto-wake captain: human sent a message and no captain is running → start one.
            if let senderId = entries.last?.senderMemberId,
               members.contains(where: { $0.id == senderId && $0.memberKind == "human" }),
               !hasRunningCaptain,
               let detail = crewStore.selectedDetail {
                Task {
                    do {
                        try await sessionRunner.startCaptain(detail: detail, backend: appModel.backend)
                    } catch {
                        // 机长起不来必须有提示 —— 与手动启动路同落 lastStartError
                        // 横幅,不静默吞（人发了话却没人应,还查不到原因）。#541 起
                        // 再补一条白板（不 @机长 —— 挂的就是它,@ 会成环）。
                        sessionRunner.reportStartFailure(
                            crewId: detail.crew.id, brief: nil, error: error,
                            mentionCaptain: false)
                    }
                }
            }
        }
    }

    private func refreshRoster() async {
        guard let crewId = crewStore.selectedDetail?.crew.id,
              let backend = appModel.backend else { return }
        if let wb = try? await backend.listCrewWhiteboard(crewId: crewId) {
            entries = wb
        }
        if let roster = try? await backend.listCrewMembers(crewId: crewId) {
            // 单一排序真值（#15）：新建的 session 在前，机长/人类保持置顶。
            captainBotId = roster.captainBotId
            members = CrewMemberOrdering.sortedMembers(
                roster.members, captainBotId: roster.captainBotId)
        }
    }

    /// composer 是否处于「续聊当前 run」态（反之 = 起新 session 态）。
    /// 新建态、无 run、当前 run 已退出，都算起新 session。
    private var isContinuing: Bool {
        !sessionRunner.isComposingNew && sessionRunner.current?.status == .running
    }

    // MARK: - session bar (chunk2 T2)

    /// 当前选中 crew 的 runs，captain 置顶，其余按启动顺序。
    private var barRuns: [CrewSessionRun] {
        let crewId = crewStore.selectedDetail?.crew.id
        let runs = sessionRunner.runs.filter { $0.crewId == crewId }
        return runs.filter { $0.role == .captain } + runs.filter { $0.role != .captain }
    }

    /// 未读数（chunk2 T6）：本 session 的 pending 待办 + 上次查看后该 session
    /// 发的白板消息。`badgeTick` 由 `subscribeRoster` 的白板变更事件 bump，强制
    /// 重算（store 数据在文件里，没有 ObservableObject 推送）。前台选中的 run 不显
    /// 角标（看着呢）。
    private func badgeCount(for run: CrewSessionRun) -> Int {
        _ = badgeTick // 依赖 tick，白板变更事件到来时让 SwiftUI 重算
        if run.runID == sessionRunner.selectedRunId { return 0 }
        return SessionUnreadStore.shared.unreadCount(
            crewId: run.crewId, sessionId: run.sessionId,
            approvals: .shared, whiteboard: .shared)
    }

    /// 横向滚动的 session 切换条：每个 run 一个 capsule（名字 + 状态点 +
    /// 未读 badge 占位），点击切前台；已退出的带 ✕ 可移除。
    /// 选中 crew 是否已有在跑的 captain run（启动按钮的显隐 + startCaptain 防重）。
    private var hasRunningCaptain: Bool {
        guard let crewId = crewStore.selectedDetail?.crew.id else { return false }
        return sessionRunner.runs.contains {
            $0.crewId == crewId && $0.role == .captain && $0.status == .running
        }
    }

    private var sessionBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                if crewStore.selectedDetail != nil && !hasRunningCaptain {
                    Button {
                        Task { await startCaptain() }
                    } label: {
                        Text("⚓ 启动 Captain")
                            .font(.caption.weight(.semibold))
                    }
                    .buttonStyle(.bordered)
                    .tint(.indigo)
                    .controlSize(.small)
                    .disabled(starting)
                    .help("起一个机长 session：读白板、报到、答复待决策")
                }
                ForEach(barRuns) { run in
                    SessionBarItemView(
                        run: run,
                        isSelected: !sessionRunner.isComposingNew && run.runID == sessionRunner.selectedRunId,
                        badgeCount: badgeCount(for: run),
                        select: { sessionRunner.select(run.runID) },
                        remove: { sessionRunner.remove(run.runID) }
                    )
                }
                // 「+」起一个新 session（不动在跑的 run）。选中态高亮。
                Button { sessionRunner.composeNew() } label: {
                    Label("新 session", systemImage: "plus")
                        .font(.caption.weight(.semibold))
                }
                .buttonStyle(.bordered)
                .tint(sessionRunner.isComposingNew ? .accentColor : nil)
                .controlSize(.small)
                .disabled(starting)
                .help("起一个新 session（与在跑的并存）")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
    }

    private var idleState: some View {
        VStack(spacing: 16) {
            Image(systemName: "apple.terminal")
                .font(.system(size: 40))
                .foregroundStyle(.tertiary)
            if crewStore.selectedDetail == nil {
                Text("先在左侧选一个 crew").foregroundStyle(.secondary)
            } else {
                sessionKindControls
            }
        }
        .padding(.horizontal, 24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// codex 续聊输入框 —— 照搬群聊的 ComposerView 输入胶囊（附件功能关掉：
    /// 消息注入的是 codex 会话，不走群聊附件通道）。发送 = 注入当前 run。
    private var codexComposer: some View {
        ComposerView(
            input: $draft,
            pending: .constant([]),
            photoItems: .constant([]),
            cameraImage: .constant(nil),
            showFileImporter: .constant(false),
            showPhotoPicker: .constant(false),
            showCamera: .constant(false),
            canSend: !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            onSend: { Task { await send() } },
            isStreaming: false,
            showsAttachments: false,
            placeholder: "继续对 agent 说…"
        )
    }

    /// 新建 / 已退出态能否发送（= 起 agent session）：有非空草稿、未在起、已选 crew。
    /// ComposerView 无 disabled 入口 —— 用它 gate 发送钮，兼防起 session 途中重复提交。
    private var canStartSession: Bool {
        !starting
            && crewStore.selectedDetail != nil
            && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// 新建 / 已退出态输入区（`composer` 只在 `!isContinuing` 渲染，见 terminalContent）。
    /// 三项 runner 药丸在图标下方；agent 显示首条指令输入，纯终端只显示打开按钮。
    /// model/effort 不在建前选（#485：建好后在终端页头部切）。
    private var composer: some View {
        VStack(spacing: 6) {
            // 点回一个已退出的 run 时仍保留它的终端现场；在现场下方另起 session
            // 也必须能改 kind。常用的「+」新建页仍只在终端图标下显示这组药丸。
            if !sessionRunner.isComposingNew, sessionRunner.current?.status != .running {
                sessionKindControls
                .padding(.top, 8)
            }
            if let localError {
                Text(localError).font(.caption).foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
            }
            if selectedKind == .terminal {
                Button {
                    Task { await startSession(firstPrompt: "") }
                } label: {
                    Label("打开终端", systemImage: "apple.terminal")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(starting || crewStore.selectedDetail == nil)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            } else {
                // 群聊同款输入胶囊（附件关掉：起 session 不走群聊附件通道）。
                ComposerView(
                    input: $draft,
                    pending: .constant([]),
                    photoItems: .constant([]),
                    cameraImage: .constant(nil),
                    showFileImporter: .constant(false),
                    showPhotoPicker: .constant(false),
                    showCamera: .constant(false),
                    canSend: canStartSession,
                    onSend: { Task { await send() } },
                    isStreaming: false,
                    showsAttachments: false,
                    placeholder: "下第一条指令开始…"
                )
            }
        }
        .padding(.top, 6)
    }

    /// 新建 session 的类型选择：按人的阅读顺序从上到下，一行一个完整药丸。
    /// 同一组控件也承载「设为机长」；这不是显示标签，发送时会走 captain 的
    /// persona / MCP / role 启动入口。
    private var sessionKindControls: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Session 类型")
                .font(.caption)
                .foregroundStyle(Theme.Palette.inkMuted)
            VStack(spacing: 8) {
                sessionKindPill(.claudeCode, title: "Claude Code")
                sessionKindPill(.codex, title: "Codex")
                sessionKindPill(.terminal, title: "终端")
            }
            Toggle("设为机长", isOn: $startsAsCaptain)
                .toggleStyle(.checkbox)
                .disabled(starting || selectedKind == .terminal)
                .help("以机长世界观和工具权限启动这个新 session")
            if startsAsCaptain && hasRunningCaptain {
                Text("启动后会停止当前机长，由这个新 session 接任。")
                    .font(.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
        }
        .frame(maxWidth: 300, alignment: .leading)
        .disabled(starting)
    }

    private func sessionKindPill(_ kind: LocalCodingAgentKind, title: String) -> some View {
        Button {
            selectedKind = kind
        } label: {
            HStack(spacing: 8) {
                Text(title)
                    .font(.callout.weight(.medium))
                Spacer(minLength: 8)
                if selectedKind == kind {
                    Image(systemName: "checkmark")
                        .font(.caption.weight(.bold))
                }
            }
            .foregroundStyle(selectedKind == kind
                             ? Theme.Palette.accent : Theme.Palette.ink)
            .padding(.horizontal, 14)
            .frame(maxWidth: .infinity, minHeight: 34)
            .background(
                Capsule().fill(selectedKind == kind
                               ? Theme.Palette.accentBg : Theme.Palette.canvas))
            .overlay(
                Capsule().strokeBorder(
                    selectedKind == kind
                        ? Theme.Palette.accent.opacity(0.65)
                        : Theme.Palette.inkMuted.opacity(0.28),
                    lineWidth: 1))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selectedKind == kind ? .isSelected : [])
    }

    // MARK: - actions

    private func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        if isContinuing, let run = sessionRunner.current {
            // Running session（且非新建态）→ inject the text straight into the
            // embedded terminal (the agent's interactive prompt picks it up).
            run.send(text)
            draft = ""
        } else {
            // 无 run / 已退出 / 新建态 → 开一个新 session。
            await startSession(firstPrompt: text)
        }
    }

    /// Zero-config start: crew default dir + runner kind + permission mode, with
    /// `firstPrompt` as the initial turn. No title, no task brief. The first
    /// prompt is injected into the terminal once the agent's REPL is up.
    private func startSession(firstPrompt: String) async {
        guard let detail = crewStore.selectedDetail else { return }
        guard let wd = detail.crew.workingDirectory, !wd.isEmpty else {
            localError = "这个 crew 没有工作目录 —— 在建 crew 时设置后再开 session。"
            return
        }
        let dir = URL(fileURLWithPath: (wd as NSString).expandingTildeInPath)
        starting = true
        defer { starting = false }
        localError = nil

        do {
            if startsAsCaptain {
                try await sessionRunner.startFreshCaptain(
                    detail: detail,
                    backend: appModel.backend,
                    openingBrief: firstPrompt,
                    kind: selectedKind,
                    userInitiated: true)
                draft = ""
                startsAsCaptain = false
                return
            }
            var cfg = SessionConfig(
                kind: selectedKind,
                initialPrompt: selectedKind == .terminal ? nil : firstPrompt)
            // isolation=on → 独立 worktree;off → crew 共享目录（spec §8/§11）。
            let workdir = try SessionWorkspace.resolve(
                crewDirectory: dir, isolation: cfg.isolation, hint: firstPrompt)
            // 本地 session 的合成 id —— 世界观 / mcp-config / hook settings 三处一致。
            let localSessionId = UUID().uuidString.lowercased()
            // 世界观 + comms 接线按 kind 分叉：claude 走文件标志（--append-system-prompt-file
            // / --settings / --mcp-config）；codex 没这些通道 —— 世界观经
            // developerInstructions（字符串）、MCP 经 mcpServers dict 走协议传入。
            // best-effort：渲染失败 nil 不挡 session 启动。
            var codexDevInstructions: String? = nil
            var codexMcp: [String: Any]? = nil
            // worker session 的显示名 = 精简 title（单一真值）：手动新建没有显式 title,
            // 从首条 prompt 兜底 derive（≤18 字、剥项目名）。既作 helper --label 白板署名,
            // 又落到 run.title —— 群聊气泡、成员列表、白板署名三处同一值,不再裸「agent·id6」。
            let workerTitle = selectedKind == .terminal
                ? LocalCodingAgentKind.terminal.displayName
                : CrewSessionTitle.derive(fromBrief: firstPrompt)
            switch selectedKind {
            case .claudeCode:
                cfg.appendSystemPromptFile = await renderWorldModelFile(
                    detail: detail, taskBrief: firstPrompt, workdir: workdir, sessionId: localSessionId)
                let comms = prepareLocalCommsConfig(
                    crewId: detail.crew.id, sessionId: localSessionId, label: workerTitle)
                cfg.settingsFile = comms.settings
                cfg.mcpConfigFile = comms.mcp
            case .codex:
                codexDevInstructions = await renderWorldModelString(
                    detail: detail, taskBrief: firstPrompt, workdir: workdir, sessionId: localSessionId)
                codexMcp = LocalSessionLaunch.codexMcpServers(
                    crewId: detail.crew.id, sessionId: localSessionId, label: workerTitle)
            case .terminal:
                break
            }
            try await sessionRunner.start(
                crewId: detail.crew.id,
                sessionId: localSessionId,
                config: cfg,
                workingDirectory: workdir,
                taskBrief: firstPrompt,
                title: workerTitle,
                developerInstructions: codexDevInstructions,
                codexMcpServers: codexMcp,
                // 人在新建面板里填完按下的这一下 —— 这条是唯一该跳过去的路（#42）。
                userInitiated: true
            )
            // 指令已在 argv 里(positional prompt)—— 不再等 REPL 就绪 sleep + 事后注入。
            draft = ""
        } catch {
            localError = error.localizedDescription
        }
    }

    /// 手动启动机长（按钮兜底：建 crew 会自动起，这里给"机长退出后重启"用）。
    /// 核心工序在 `CrewSessionRunner.startCaptain` —— 与建 crew 自动起共用一份，
    /// 不分叉（chunk2 T3）。这里只补 view 持有的 starting / localError 反馈。
    private func startCaptain() async {
        guard let detail = crewStore.selectedDetail else { return }
        starting = true
        defer { starting = false }
        // 走 runner 上的共享错误通道（不是 view-local 的 localError）—— 这样错误能在
        // 按钮所在的成员列表模式里显出来，而不是只在终端模式 composer 里（点了像没反应）。
        sessionRunner.lastStartError = nil
        do {
            try await sessionRunner.startCaptain(detail: detail, backend: appModel.backend)
        } catch {
            sessionRunner.reportStartFailure(
                crewId: detail.crew.id, brief: nil, error: error, mentionCaptain: false)
        }
    }

    /// 人从 crew 成员行明确选择的新机长。runner 负责持久化意图、审计、停止旧角色
    /// 与续接 conversation；view 只提供忙态/错误反馈并刷新 detail 里的默认 runner。
    private func reassignCaptain(to run: CrewSessionRun) async {
        guard let detail = crewStore.selectedDetail else { return }
        starting = true
        defer { starting = false }
        sessionRunner.lastStartError = nil
        do {
            try await sessionRunner.reassignCaptain(
                to: run, detail: detail, backend: appModel.backend)
            await crewStore.refreshDetail(detail.crew.id)
            sessionRunner.viewingTerminal = true
        } catch {
            sessionRunner.lastStartError = "重新指定机长失败：\(error.localizedDescription)"
        }
    }

    /// 渲染本地世界观 system prompt → 临时 `.md` → 返回路径供
    /// `--append-system-prompt-file`。接合 v2：恒为本地渲染（无 mode 分叉）。
    /// best-effort：任一步失败返 nil，session 仍照常启动（只是没世界观）。
    /// `appendPersona` 非 nil 时（captain session）把 persona 拼到世界观后面 ——
    /// captain 与 worker 的差异只是「追加 persona + --captain 工具门禁」（chunk2 §2）。
    /// 渲染 + comms 接线的实现提取到 `LocalSessionLaunch`（#242）—— 机长排队起
    /// session 与这里走同一份工序。这俩 wrapper 只补 view 持有的依赖
    /// （backend 拉 members、selectedKind）。
    private func renderWorldModelFile(detail: CrewDetail, taskBrief: String, workdir: URL, sessionId: String,
                                      appendPersona: String? = nil) async -> String? {
        guard let backend = appModel.backend else { return nil }
        let members = (try? await backend.listCrewMembers(crewId: detail.crew.id))?.members ?? []
        return LocalSessionLaunch.renderWorldModelFile(
            detail: detail, members: members, taskBrief: taskBrief, workdir: workdir,
            sessionId: sessionId, runnerKind: selectedKind, appendPersona: appendPersona)
    }

    /// codex 变体：渲染同一份世界观但返回字符串（→ thread/start.developerInstructions），
    /// 不写临时文件。与上面的 file 版同样从 backend 拉 members。best-effort：失败 nil。
    private func renderWorldModelString(detail: CrewDetail, taskBrief: String, workdir: URL, sessionId: String,
                                        appendPersona: String? = nil) async -> String? {
        guard let backend = appModel.backend else { return nil }
        let members = (try? await backend.listCrewMembers(crewId: detail.crew.id))?.members ?? []
        return LocalSessionLaunch.renderWorldModelString(
            detail: detail, members: members, taskBrief: taskBrief, workdir: workdir,
            sessionId: sessionId, appendPersona: appendPersona)
    }

    private func prepareLocalCommsConfig(crewId: String, sessionId: String,
                                         captain: Bool = false,
                                         label: String? = nil) -> (settings: String?, mcp: String?) {
        LocalSessionLaunch.prepareLocalCommsConfig(
            crewId: crewId, sessionId: sessionId, captain: captain, label: label)
    }

}

/// 切换条上的单个 session 项。`@ObservedObject` 订阅 run 的 `@Published status`
/// 驱动状态点/✕（嵌套 ObservableObject —— 父 view 观察不到 run 自己的变更）。
private struct SessionBarItemView: View {
    @ObservedObject var run: CrewSessionRun
    let isSelected: Bool
    let badgeCount: Int
    let select: () -> Void
    let remove: () -> Void

    var body: some View {
        HStack(spacing: 5) {
            // 与头像状态点同一套推导与配色（`SessionStatusDot`）：绿=干活、黄=空闲、
            // 红=需要人出手(异常 ∨ 卡住等人拍板，呼吸)、灰=已退出。原来这里是手写的
            // 三元表达式，压根画不出红 —— 撞限额/卡菜单的 session 在切换条上装空闲。
            statusDot
            Text(run.displayName)
                .font(.caption.weight(isSelected ? .semibold : .regular))
                .lineLimit(1)
            if badgeCount > 0 {
                Text("\(badgeCount)")
                    .font(.caption2.weight(.semibold))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(Color.red, in: Capsule())
                    .foregroundStyle(.white)
            }
            if run.status != .running {
                Button(action: remove) {
                    Image(systemName: "xmark")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("移除这个 session")
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(
            isSelected ? Color.accentColor.opacity(0.18) : Color.secondary.opacity(0.08),
            in: Capsule()
        )
        .overlay(
            Capsule().strokeBorder(
                isSelected ? Color.accentColor.opacity(0.5) : .clear, lineWidth: 1)
        )
        .contentShape(Capsule())
        .onTapGesture(perform: select)
    }

    @ViewBuilder private var statusDot: some View {
        let state = CrewSessionStateDerivation.state(
            isRunning: run.status == .running, health: run.health,
            isWorking: run.isWorking, awaitingDecision: run.pendingDecision != nil,
            awaitingReply: run.awaitingReply != nil)
        if let dot = SessionStatusDotDerivation.dot(state: state) {
            if dot.breathes {
                BreathingDot(size: 7, color: dot.color)
            } else {
                Circle().fill(dot.color).frame(width: 7, height: 7)
            }
        }
    }
}

/// 单个 run 的内容视图。`@ObservedObject` 订阅 run 的 `@Published`（status /
/// exitCode）驱动 header 刷新；内容区按 backend 类型分支：claude 显示
/// `AgentTerminalView`（真终端），codex 显示 `CodexTranscriptView`（结构化 transcript）。
private struct SessionRunContentView: View {
    @ObservedObject var run: CrewSessionRun
    /// 终端页头部切换控件的回调（→ `applyProfileChange`）。只带改动的那一个档位。
    let onSwitchProfile: (_ model: String?, _ effort: String?) -> Void
    /// Codex-only native approval reviewer switch.
    let onSwitchApproval: (_ reviewer: CodexProtocol.ApprovalsReviewer) -> Void

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let terminalView = run.terminalView {
                // 左右留白：终端网格自绘不透明底,贴边显得挤。padding 区用
                // Theme.canvas 补色 —— AgentTerminalView.applyTheme 的底色就是
                // 对齐 canvas 的(浅 #FFFFFF/深 #161512),缝是隐形的。
                AgentTerminalView(terminalView: terminalView)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    // 外置 overlay 滚动条：叠在终端 trailing（padding 之前）那条 SwiftTerm
                    // 预留的 ~15pt 空当里，取代被藏掉的内部条，不压最右列字符。
                    .overlay(alignment: .trailing) {
                        if let term = run.agentTerminalSession {
                            TerminalScrollbarOverlay(session: term)
                        } else if let remote = run.remoteSessionBackend,
                                  remote.kind == .claudeCode {
                            TerminalScrollbarOverlay(session: remote)
                        }
                    }
                    .padding(.horizontal, 12)
                    .background(Theme.Palette.canvas)
            } else if let codex = run.backend as? CodexAppServerBackend {
                CodexTranscriptView(transcript: codex.transcript)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let remote = run.remoteSessionBackend,
                      let transcript = remote.transcript {
                CodexTranscriptView(transcript: transcript)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    @ViewBuilder
    private var header: some View {
        // 三排各自占满可用宽度（Todo #82）。旧版把名称、配置、审批和停止全塞进
        // 一个 HStack；右栏一窄，「机长」会被压成逐字竖排，模型只剩一个字母。
        VStack(alignment: .leading, spacing: 7) {
            // 第一排：名字 + 退出状态/停止。
            HStack(spacing: 10) {
                Text(run.displayName)
                    .font(.callout.weight(.semibold))
                    .lineLimit(1)
                    .layoutPriority(1)
                Spacer(minLength: 8)
                if run.status != .running {
                    statusBadge(run.status, exitCode: run.exitCode)
                } else {
                    Button { run.stop() } label: {
                        Image(systemName: "stop.fill")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(.white)
                            .frame(width: 22, height: 22)
                            .background(Circle().fill(.red))
                    }
                    .buttonStyle(.plain)
                    .help("停止这个 session")
                }
            }

            // 第二排：模型与 effort 分开选，不再藏在一个含混的小药丸里。
            if run.kind.isAgent {
                SessionProfileControl(run: run, onSwitch: onSwitchProfile)
            }

            // 第三排：Codex 原生审批模式。
            if run.kind == .codex {
                HStack(spacing: 8) {
                    Text("审批模式")
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                    SessionApprovalModeControl(run: run, onSwitch: onSwitchApproval)
                    Spacer(minLength: 0)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    @ViewBuilder
    private func statusBadge(_ status: CrewSessionRun.Status, exitCode: Int32?) -> some View {
        let (text, color): (String, Color) = {
            switch status {
            case .running: return ("running", .blue)
            case .completed:
                if let code = exitCode {
                    return code == 0 ? ("退出 0", .green) : ("退出 \(code)", .orange)
                }
                return ("已完成", .green)
            case .cancelled: return ("已取消", .secondary)
            case .failed:
                if let code = exitCode {
                    return ("失败 \(code)", .red)
                }
                return ("失败", .red)
            }
        }()
        Text(text)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(color.opacity(0.15), in: Capsule())
            .foregroundStyle(color)
    }
}

/// Codex's native reviewer switch. Manual mode is the only mode allowed to create
/// PendingCrew approval cards; auto_review keeps decisions inside Codex.
private struct SessionApprovalModeControl: View {
    @ObservedObject var run: CrewSessionRun
    let onSwitch: (CodexProtocol.ApprovalsReviewer) -> Void

    var body: some View {
        Menu {
            ForEach(CodexProtocol.ApprovalsReviewer.allCases, id: \.rawValue) { reviewer in
                Button {
                    guard reviewer != run.approvalsReviewer else { return }
                    onSwitch(reviewer)
                } label: {
                    if reviewer == run.approvalsReviewer {
                        Label(reviewer.displayName, systemImage: "checkmark")
                    } else {
                        Text(reviewer.displayName)
                    }
                }
            }
        } label: {
            Label(
                run.approvalsReviewer?.displayName ?? "Approve for me",
                systemImage: run.approvalsReviewer == .user ? "hand.raised" : "checkmark.shield")
                .font(Theme.Fonts.caption)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .disabled(run.status != .running)
        .help("Approve for me 由 Codex 原生代审；手动批准会在本 session 内显示可操作审批卡")
    }
}

/// session 当前 model/effort 的展示 label：「模型 · effort」，模型走友好名
/// （Fable 5 / Opus 4.8 …，见 `SessionLaunchOptions.displayName`），未收录值原样。
private func profileLabel(model: String?, effort: String?) -> String {
    let m = model.map { SessionLaunchOptions.displayName(for: $0) } ?? "默认"
    if let e = effort, !e.isEmpty { return "\(m) · \(e)" }
    return m
}

/// model/effort 的小 pill（成员行只读展示与终端页切换控件共用同一视觉）。
private struct SessionProfilePillLabel: View {
    let text: String
    /// false = 只读/置灰（成员行、codex），true = 可点（claude 切换菜单的 label）。
    let active: Bool

    var body: some View {
        // 左侧不再画 ⇅：外层 Menu(.borderlessButton) 右侧已有原生 disclosure
        // 箭头，两个箭头冗余。`active` 仍保留（下面控制 opacity 区分可点/只读）。
        HStack(spacing: 3) {
            Text(text).lineLimit(1)
        }
        .font(Theme.Fonts.caption2)
        .foregroundStyle(Theme.Palette.inkMuted)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(
            RoundedRectangle(cornerRadius: 5, style: .continuous)
                .fill(Theme.Palette.surfaceMuted)
        )
        .opacity(active ? 1 : 0.6)
    }
}

/// 成员行尾部的**只读** model/effort pill —— 纯展示，无菜单/点击（用户定调：
/// 成员列表不做选择入口，切换在 session 详情/终端页里做）。
/// `@ObservedObject` 观察 run —— 切换（终端页/MCP 自切）回写 `run.model` /
/// `run.effort` 后这里即时刷新（父 view 订不到嵌套 run 的 `@Published`）。
private struct SessionProfileReadonlyPill: View {
    @ObservedObject var run: CrewSessionRun

    var body: some View {
        SessionProfilePillLabel(
            text: profileLabel(model: run.model, effort: run.effort), active: false)
    }
}

/// 终端页头部的运行态 model / effort 两个独立菜单（Todo #82）。Claude 经空闲时
/// 斜杠命令切；Codex 经 app-server `thread/settings/update` 切。两边都只在底层确认
/// 成功后回写 run，UI 不抢先显示假配置。
private struct SessionProfileControl: View {
    @ObservedObject var run: CrewSessionRun
    /// 可用模型表（Todo #37）：菜单候选来自现探的 models.json，探不到才回落手工
    /// 兜底表 —— 不再在这里硬编模型名。
    @ObservedObject private var catalog = ModelCatalogCenter.shared
    /// 只带**改动的那一个**（另一个传 nil），避免误发未变的档位。
    let onSwitch: (_ model: String?, _ effort: String?) -> Void

    init(run: CrewSessionRun, onSwitch: @escaping (_ model: String?, _ effort: String?) -> Void) {
        self.run = run
        self.onSwitch = onSwitch
    }

    var body: some View {
        switch run.kind {
        case .claudeCode, .codex:
            HStack(spacing: 8) {
                modelMenu
                effortMenu
                if run.pendingProfile != nil {
                    ProgressView().controlSize(.small)
                }
                Spacer(minLength: 0)
            }
        case .terminal:
            EmptyView()
        }
    }

    private var availableModels: [String] {
        SessionLaunchOptions.models(for: run.kind, catalog: catalog.file)
    }

    private var availableEfforts: [String] {
        SessionLaunchOptions.efforts(for: run.kind, catalog: catalog.file)
    }

    private var modelMenu: some View {
        Menu {
            ForEach(availableModels, id: \.self) { model in
                Button {
                    if model != run.model { onSwitch(model, nil) }
                } label: {
                    let name = SessionLaunchOptions.displayName(for: model, catalog: catalog.file)
                    if model == run.model { Label(name, systemImage: "checkmark") }
                    else { Text(name) }
                }
            }
        } label: {
            let name = run.model.map {
                SessionLaunchOptions.displayName(for: $0, catalog: catalog.file)
            } ?? "默认"
            SessionProfilePillLabel(text: "模型  \(name)", active: true)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .disabled(run.status != .running)
        .help("手动选择这个 session 的模型")
    }

    private var effortMenu: some View {
        Menu {
            ForEach(availableEfforts, id: \.self) { effort in
                Button {
                    if effort != run.effort { onSwitch(nil, effort) }
                } label: {
                    if effort == run.effort { Label(effort, systemImage: "checkmark") }
                    else { Text(effort) }
                }
            }
        } label: {
            SessionProfilePillLabel(text: "Effort  \(run.effort ?? "默认")", active: true)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .disabled(run.status != .running)
        .help("手动选择这个 session 的思考强度")
    }
}
#endif
