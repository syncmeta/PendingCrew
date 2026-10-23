#if os(macOS)
import SwiftUI
#if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
import Inject
#endif

/// 中栏（spec v2 §9）：选中 crew 的**群聊会话页**（白板 + 沟通渠道）。
///
/// 机组详情从侧栏右键打开；成员和会话控制在同一工作区内展示。
struct CrewCenterView: View {
    #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
    @ObserveInjection private var injection
    #endif
    @ObservedObject private var personalService=CrewPersonalDraftService.shared
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    @EnvironmentObject private var sessionRunner: CrewSessionRunner
    /// 「只看 @ 我的消息」（Todo #61 立、#155 改成默认不点亮）。开关钮在 toolbar 上，
    /// 状态喂给 `CrewChatView` 的时间线。**放在这里而不是 chat 里面**：
    /// `CrewChatView` 带 `.id(crewId)`，切 crew 会整个重建 —— 状态放里面就没法从
    /// toolbar 驱动它。切 crew 时下面显式归位（筛选状态不跨群带走）。
    @State private var onlyMentions = CrewMentionFilter.defaultOnlyMentions
    @State private var communicationView: CrewTimelineFilter.CommunicationView = .human
    @State private var searchScope = CrewSearchScope.current
    @State private var searchQuery = ""
    @State private var searchTargetMessageId: String?
    @State private var showingSearch = false
    @FocusState private var searchFocused: Bool

    private var hasWorkspaceHeader: Bool {
        let editingDraft = crewStore.pendingCrew.map { crewStore.selectedCrewId == $0.id } ?? false
        return !editingDraft && crewStore.selectedDetail != nil && model.crewdBackend?.workspace != nil
    }
    var body: some View {
        VStack(spacing: 0) {
            if !hasWorkspaceHeader {
                VStack(spacing: 0) {
                    HStack { Spacer(minLength: 0); searchButton }
                        .padding(.horizontal, 14).padding(.vertical, 8)
                        .frame(minHeight: 54)
                    Divider()
                }
            }
        Group {
            if let draft = crewStore.pendingCrew, crewStore.selectedCrewId == draft.id {
                PendingCrewChatView(draft: draft).id(draft.id)
            } else if let detail = crewStore.selectedDetail {
                CrewChatView(
                    crewId: detail.crew.id,
                    crewTitle: detail.crew.title,
                    onOpenSession: { runId in
                        // 右栏常驻；点 session 头像只切到「终端」模式 + 选中该 run。
                        sessionRunner.openSession(runId)
                    },
                    onNewSession: {
                        sessionRunner.composeNew()
                        sessionRunner.viewingTerminal = true
                    },
                    showOnlyHumanMentions: $onlyMentions,
                    communicationView: $communicationView,
                    searchQuery: Binding(get: { searchScope == .current ? searchQuery : "" }, set: { searchQuery = $0 }),
                    searchTargetMessageId: $searchTargetMessageId,
                    // 引用胶囊的跳转（人类 Todo #132/#133）。真正动 store 的那几行
                    // 在这里 —— 中栏本来就订阅着它，群聊那棵子树不该为此被拉进订阅。
                    onJumpToMessage: { messageId, from in
                        crewStore.jumpToMessage(
                            crewId: from.crewId, messageId: messageId, from: from)
                    },
                    onJumpToCrew: { targetCrewId, from in
                        crewStore.jumpToCrew(crewId: targetCrewId, from: from)
                    }
                )
                // 切 crew 强制重建（对齐 iPad 的 `IPadShell`）。少了它，detail 已缓存时
                // 视图实例被复用，会先用「新 crewId + 上一个 crew 的 entries」渲染一帧，
                // 滚底正好打在上一个 crew 的内容上（Todo #45 的 macOS 错位）。重建也顺带
                // 掐掉草稿/回复目标跨群残留（在 A 群打一半的字出现在 B 群、可能误发）——
                // 未发出的草稿由 `CrewComposerDraftStore` 按 crew 记着，切回来原样在。
                .id((model.crewdBackend?.workspace.presentationNamespace ?? "legacy") + "/" + detail.crew.id + "/" + personalService.projectionID.uuidString)
                .modifier(CrewdOriginalWorkIntegration(workspace: model.crewdBackend?.workspace,
                    searchControl: AnyView(searchButton)))
            } else if let id = crewStore.selectedCrewId, crewStore.loadingDetailIds.contains(id) {
                ProgressView("正在读取机组…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if crewStore.selectedCrewId != nil {
                empty("详情加载中…")
            } else {
                placeholder
            }
        }
        }
        // 窗口使用透明 titlebar + fullSizeContentView。只给中栏标题后面铺一层
        // 与聊天画布相同的不透明底色，挡住滚到标题下面的消息；不改变整窗 toolbar
        // 背景，sidebar 仍保留自己的材质。高度跟随系统安全区，窄窗口也不写死宽度。
        // 「回到刚才那条」（人类 Todo #132/#133）。**浮在群聊上方而不是插进版面**：
        // 插进去会把整条时间线往下推一格，而它是个临时件 —— 退回去之后就该消失，
        // 版面不该跟着抖两次。
        //
        // 它只在**跳转真的换了地方**时才有：开 Todo 窗口 / 开驾驶舱那几种胶囊
        // 群聊本身没动过，关掉那层就回来了，不需要也不该多一个返回件。
        .overlay(alignment: .top) {
            if let stop = crewStore.chatReturnTrail.top {
                Button { crewStore.returnToPreviousStop() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "arrow.uturn.backward")
                            .font(.system(size: 9, weight: .semibold))
                        Text(stop.crewId == crewStore.selectedCrewId
                             ? "回到刚才那条消息"
                             : "回到「\(stop.crewTitle)」")
                            .font(Theme.Fonts.caption)
                            .lineLimit(1)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(Capsule().fill(Theme.Palette.canvas))
                    .overlay(Capsule().stroke(Theme.Palette.hairline, lineWidth: 1))
                    .foregroundStyle(Theme.Palette.accent)
                    .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
                }
                .buttonStyle(.plain)
                .padding(.top, 8)
            }
        }
        .navigationTitle("")

        // 灰线/无缝由 WindowSeparatorRemover(标题栏透明 + 内容铺满到顶)统一处理。
        // **不能**在这里 .toolbarBackground 刷色：那是窗口级的，会连 sidebar 那半截 toolbar
        // 一起刷白，把侧栏顶部的半透明材质盖住（"toolbar 挡住 sidebar"）。透明标题栏让
        // sidebar 透出自己的侧栏材质、detail 透出白 canvas，两边各自对，互不打架。
        // chunk2 T5：captain 唤醒 = app 注入。常驻中栏**事件驱动**订阅待决策（去 2s
        // 轮询）：app 侧答复 + helper 跨进程 raise(目录监听)都推一个 tick,有新的(非
        // captain 自己 raise 的)就把提示注入在跑的 captain PTY。不依赖 inspector 是否打开。
        // `.task(id:)` 随选中 crew 切换重建订阅;无选中 crew 时 crewId=nil,不订阅。
        // 切 crew：筛选归位（Todo #61 立，#128 曾改为默认点亮，#155 改回关闭）。
        //
        // **#61 当初归位到「关」，理由是**：换个群还挂着「只看 @ 我」，新群大概率筛成
        // 空的 —— 人看到的是一个空聊天页，会以为这个群没消息 / 加载失败。
        //
        // #155 改成默认不点亮，这里仍归到 `defaultOnlyMentions`（= 关闭）。
        // 归位本身保留 —— 那是 #61 真正的意思：筛选状态不跨群带走。
        //
        // 手动打开筛选后仍可能筛成空；这时群聊空态会给一句
        // 「这个群里没有 @ 你的消息」和一颗「看全部」
        // （`CrewMentionFilter.showsClearFilterEscape` → `CrewChatView.emptyState`）。
        // **动这里之前先确认那条出路还在**，手动打开筛选后仍需要它。
        .onChange(of: crewStore.selectedCrewId) { _, _ in
            onlyMentions = CrewMentionFilter.defaultOnlyMentions
            if crewStore.chatSearchRequest == nil{communicationView = .human}
            // 跨群结果的 request 会在下面紧接着重新填回查询/定位；普通切群则归零。
            if crewStore.chatSearchRequest == nil {
                searchQuery = ""
                searchTargetMessageId = nil
            }
        }
        .onChange(of: searchQuery) { _, newValue in
            if !newValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                onlyMentions = false
            }
        }
        .onChange(of: crewStore.chatSearchRequest) { _, request in
            guard let request else { return }
            // 每个跨群搜索/引用请求都带具体消息 id；定位时打开完整记录。
            communicationView = .agents
            sessionRunner.viewingTerminal=false
            // **落到某一条消息 = 「把这条给我看」，筛选一律让路。**
            // 搜索那条老路是靠下面 `onChange(of: searchQuery)` 顺带收掉筛选的，
            // 而引用胶囊的定位 `query` 是空的 —— 走不到那一条。少了这一行，点一颗
            // 指向「没 @ 人类」的消息的胶囊会**什么都不发生**：目标被筛在时间线外，
            // `locateSearchTarget` 找不到它就直接返回，看起来跟胶囊坏了一样。
            onlyMentions = false
            searchQuery = request.query
            searchTargetMessageId = request.messageId
            // 下一拍再清 request：同一笔点击还会改变 selectedCrewId，先让上面的切群
            // handler 看见「这是搜索跳转」而不是普通切群，避免它把 query/target 清掉。
            Task { @MainActor in
                await Task.yield()
                if crewStore.chatSearchRequest?.id == request.id {
                    crewStore.chatSearchRequest = nil
                }
            }
        }
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }

    private var searchButton: some View {
        Button {
            if crewStore.selectedDetail == nil { searchScope = .all }
            showingSearch.toggle()
        } label: {
            Image(systemName: searchScope == .current && !searchQuery.isEmpty ? "magnifyingglass.circle.fill" : "magnifyingglass")
                .font(.system(size: 15, weight: .medium))
                .frame(width: 30, height: 30)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(searchQuery.isEmpty ? "搜索" : "搜索 · " + searchScope.rawValue + "：" + searchQuery)
        .accessibilityLabel("搜索")
        .keyboardShortcut("f", modifiers: .command)
        .popover(isPresented: $showingSearch, arrowEdge: .top) {
            VStack(spacing: 12) {
                Picker("搜索范围", selection: $searchScope) {
                    Text("当前群").tag(CrewSearchScope.current)
                    Text("全局 · 当前电脑").tag(CrewSearchScope.all)
                }.pickerStyle(.segmented)
                TextField("搜索", text: $searchQuery)
                    .textFieldStyle(.roundedBorder).focused($searchFocused)
                    .accessibilityLabel("搜索消息")
                if searchScope == .all {
                    CrewGlobalSearchResults(query: searchQuery) {
                        searchScope = .current
                        showingSearch = false
                    }.frame(height: 360)
                }
            }
            .padding(14).frame(width: 420)
            .onAppear { searchFocused = true }
        }
    }

    private func empty(_ text: String) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "rectangle.dashed")
                .font(.system(size: 40))
                .foregroundStyle(.tertiary)
            Text(text)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 360)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var placeholder: some View {
        CrewdGlassAddButton { crewStore.beginPendingCrew() }
            .frame(maxWidth:.infinity,maxHeight:.infinity)
    }
}

/// A new chat uses the same bottom composer as an existing conversation.
/// No crew, directory, message or session is created before explicit Send.
private struct PendingCrewChatView: View {
    #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
    @ObserveInjection private var injection
    #endif
    @EnvironmentObject private var crewStore: CrewStore
    @EnvironmentObject private var model: AppModel
    @Environment(\.colorScheme) private var scheme
    let draft: PendingCrewDraft
    @State private var error: String?
    @FocusState private var editingDirectory: Bool

    private func input(_ key: WritableKeyPath<PendingCrewDraft, String>) -> Binding<String> {
        Binding(get: {
                    if let current = crewStore.pendingCrew, current.id == draft.id { return current[keyPath: key] }
                    return draft[keyPath: key]
                },
                set: { value in
                    guard crewStore.pendingCrew?.id == draft.id else { return }
                    crewStore.editPendingCrew { $0[keyPath: key] = value }
                })
    }
    private var locked: Bool { crewStore.pendingCrewConfigurationLocked }
    private var canSend: Bool {
        !locked && !draft.firstMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button("取消") { crewStore.discardPendingCrew() }.disabled(locked)
            }.padding(.horizontal, 18).frame(minHeight: 44)
            Divider()
            Color.clear.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                configuration
                notices
                ComposerView(
                    input: input(\.firstMessage), pending: .constant([]),
                    photoItems: .constant([]), cameraImage: .constant(nil),
                    showFileImporter: .constant(false), showPhotoPicker: .constant(false), showCamera: .constant(false),
                    canSend: canSend, onSend: send,
                    showsAttachments: false, placeholder: "发消息以新建机组")
                    .disabled(locked)
                    .accessibilityIdentifier("pending-crew-composer")
            }
            .background(CrewdChatStyle.canvas(scheme))
        }
        .background(CrewdChatStyle.canvas(scheme))
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }

    private var configuration: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 148), spacing: 12, alignment: .leading)], alignment: .leading, spacing: 12) {
            field("目录") {
                HStack(spacing: 4) {
                    TextField("未指定", text: Binding(
                        get: { editingDirectory ? (draft.workingDirectory ?? "") : draft.directoryName },
                        set: { if editingDirectory { crewStore.setPendingWorkingDirectory($0) } }))
                        .focused($editingDirectory).onSubmit { editingDirectory = false }
                        .textFieldStyle(.plain).help(draft.workingDirectory ?? "未指定")
                        .accessibilityLabel("工作目录")
                    if !crewStore.usesCrewdAuthority || crewStore.crewdWorkspace?.dataDirectory != nil {
                        Button { chooseDirectory() } label: { Image(systemName: "folder") }
                            .buttonStyle(.plain).help("选择目录").accessibilityLabel("选择目录")
                    }
                }
            }
            field("机组名称") { TextField("未指定", text: input(\.title)).textFieldStyle(.plain).accessibilityLabel("机组名称") }
            field("coding agent") {
                Picker("coding agent", selection: input(\.executor)) {
                    Text("Codex").tag("codex")
                    Text("Claude Code").tag("claude")
                    Text("暂不启动，稍后添加").tag("none")
                }.labelsHidden().pickerStyle(.menu).frame(maxWidth: .infinity, alignment: .leading)
            }
            field("机器") {
                CrewdMachinePicker(window: model.crewdWindow)
            }
            field("父机组") {
                Picker("父机组", selection: Binding(get: { draft.parentCrewId ?? "" }, set: { value in crewStore.editPendingCrew { $0.parentCrewId = value.isEmpty ? nil : value } })) {
                    Text("无").tag("")
                    ForEach(crewStore.crews) { crew in Text(crew.title).tag(crew.id) }
                    if let parent = draft.parentCrewId, !crewStore.crews.contains(where: { $0.id == parent }) {
                        Text("原父机组（待确认）").tag(parent)
                    }
                }.labelsHidden().pickerStyle(.menu).frame(maxWidth: .infinity, alignment: .leading)
            }
        }.disabled(locked)
            .tint(Color(nsColor: .controlAccentColor)).foregroundStyle(.primary)
            .padding(.horizontal, 18).padding(.top, 10)
    }

    private func field<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.caption2).foregroundStyle(.secondary)
            content().font(.callout).frame(minHeight: 24, alignment: .leading)
        }.frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder private var notices: some View {
        if let error { Text(error).font(.caption).foregroundStyle(.red).padding(.horizontal, 18) }
        if let failure = crewStore.pendingCrewRollbackError { Text(failure).font(.caption).foregroundStyle(.red).padding(.horizontal, 18) }
        if let original = crewStore.pendingCrewRecoveryMessage { Text("原首条消息：\(original)").font(.caption).textSelection(.enabled).padding(.horizontal, 18) }
        if let pending = crewStore.pendingCrewDelivery {
            HStack {
                Text(pending).font(.caption).foregroundStyle(.orange)
                Spacer()
                Button("检查送达") {
                    Task { do { try await crewStore.checkPendingCrewDelivery(); error = nil } catch { self.error = error.localizedDescription } }
                }.disabled(crewStore.pendingCrewSending)
            }.padding(.horizontal, 18)
        }
        if let workspace = crewStore.crewdWorkspace,
           let original = workspace.originalFirstMessages.last(where: { $0.draftID == draft.id }),
           original.phase == .refused, original.crewID != nil,
           !workspace.hasOriginalUnknown(method: "crew.create", crew: ""),
           !workspace.originalFirstMessages.contains(where: { $0.draftID == draft.id && $0.phase == .unknown }) {
            Button("重试原首条消息") {
                Task {
                    do { try await crewStore.commitPendingCrew(draft.firstMessage, draftId: draft.id); error = nil }
                    catch { self.error = error.localizedDescription }
                }
            }
            .disabled(crewStore.pendingCrewSending || crewStore.pendingCrewDelivery != nil || crewStore.pendingCrewRollbackError != nil)
            .font(.caption).padding(.horizontal, 18)
        }
        if crewStore.pendingCrewSending { ProgressView("正在发送…").controlSize(.small).padding(.horizontal, 18) }
    }

    private func send() {
        guard canSend else { return }
        let text = draft.firstMessage
        Task {
            do { try await crewStore.commitPendingCrew(text, draftId: draft.id); error = nil }
            catch { self.error = error.localizedDescription }
        }
    }

    private func chooseDirectory() {
        let selected = crewStore.crewdWorkspace
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        if crewStore.usesCrewdAuthority {
            guard let selected else { error = "请先连接当前后台。"; return }
            do { panel.directoryURL = try selected.localDirectoryPickerRoot() }
            catch { self.error = error.localizedDescription; return }
        }
        if panel.runModal() == .OK, let url = panel.url {
            if crewStore.usesCrewdAuthority {
                guard crewStore.crewdWorkspace === selected, selected?.isRetired == false,
                      crewStore.pendingCrew?.id == draft.id else { return }
            }
            crewStore.setPendingWorkingDirectory(url.path)
        }
    }
}
#endif
