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

    var body: some View {
        Group {
            if let draft = crewStore.pendingCrew, crewStore.selectedCrewId == draft.id {
                PendingCrewChatView(draft: draft)
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
                    searchQuery: $searchQuery,
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
                .modifier(CrewdOriginalWorkIntegration(workspace: model.crewdBackend?.workspace))
            } else if let id = crewStore.selectedCrewId, crewStore.loadingDetailIds.contains(id) {
                ProgressView("正在读取机组…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if crewStore.selectedCrewId != nil {
                empty("详情加载中…")
            } else {
                placeholder
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

        .opacity(searchScope == .all ? 0:1)
        .allowsHitTesting(searchScope != .all)
        .accessibilityHidden(searchScope == .all)
        .overlay {
            if searchScope == .all {CrewGlobalSearchResults(query:searchQuery){searchScope = .current}}
        }
        .searchable(text:$searchQuery,placement:.toolbar,prompt:"搜索")
        .searchScopes($searchScope){
            Text("当前群").tag(CrewSearchScope.current)
            Text("全局 · 当前电脑").tag(CrewSearchScope.all)
        }
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

/// The new crew has no whiteboard, directory, or sessions until Send succeeds.
private struct PendingCrewChatView: View {
    @EnvironmentObject private var crewStore: CrewStore
    let draft: PendingCrewDraft
    @State private var message = ""
    @State private var error: String?

    var body: some View {
        ScrollView {
        VStack(alignment:.leading,spacing:20) {
            CrewdPanelHeader(title:"开始一件具体的事",detail:"机组："+draft.title+" · 发送第一条消息后创建",symbol:"bubble.left.and.bubble.right")
            Text("工作目录").font(CrewdType.label.weight(.semibold))
            HStack(spacing: 8) {
                Image(systemName: "folder")
                Group {
                if crewStore.usesCrewdAuthority, crewStore.crewdWorkspace?.dataDirectory == nil {
                    TextField("所连接电脑上的完整工作目录", text: Binding(
                        get: { draft.workingDirectory ?? "" },
                        set: { crewStore.setPendingWorkingDirectory($0) }))
                        .disabled(crewStore.pendingCrewSending || crewStore.pendingCrewDelivery != nil)
                } else {
                    Text(draft.workingDirectory ?? (crewStore.usesCrewdAuthority ? "请明确选择工作目录" : "~/CrewGround/\(draft.title)"))
                }
                }.lineLimit(1).truncationMode(.middle)
                Spacer()
                if draft.workingDirectory != nil {
                    Button(crewStore.usesCrewdAuthority ? "清除选择" : "恢复默认") { crewStore.setPendingWorkingDirectory(nil) }
                        .disabled(crewStore.pendingCrewDelivery != nil
                                  || crewStore.pendingCrewRollbackError != nil)
                }
                if !crewStore.usesCrewdAuthority || crewStore.crewdWorkspace?.dataDirectory != nil {
                    Button("选择目录…") { chooseDirectory() }
                        .disabled(crewStore.pendingCrewSending || crewStore.pendingCrewDelivery != nil
                                  || crewStore.pendingCrewRollbackError != nil)
                }
            }
            .font(.callout)
            .padding(.horizontal, 18)
            if crewStore.usesCrewdAuthority {
                Text("发送会创建机组并保存首条消息，再按新机组的实际执行能力启动 Codex 机长；不可用时保留机组和消息，机长不会启动。")
                    .font(.caption).foregroundStyle(.secondary).padding(.horizontal, 18)
            }
            Text("第一条任务").font(CrewdType.label.weight(.semibold))
            Text("直接写下希望完成的事情，原文将作为新会话的首条任务。").font(CrewdType.caption).foregroundStyle(.secondary)
            TextEditor(text: $message)
                .disabled(crewStore.pendingCrewDelivery != nil
                          || crewStore.pendingCrewRollbackError != nil)
                .frame(minHeight: 72, maxHeight: 110)
                .padding(8)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(.quaternary))
                .padding(.horizontal, 18)
            if let error {
                Text(error).foregroundStyle(.red).font(.caption)
                    .padding(.horizontal, 18)
            }
            if let rollbackError = crewStore.pendingCrewRollbackError {
                Text(rollbackError).foregroundStyle(.red).font(.caption)
                    .padding(.horizontal, 18)
            }
            if let original = crewStore.pendingCrewRecoveryMessage {
                Text("原首条消息：\(original)")
                    .font(.caption)
                    .textSelection(.enabled)
                    .padding(.horizontal, 18)
            }
            if let pending = crewStore.pendingCrewDelivery {
                HStack {
                    Text(pending).foregroundStyle(.orange).font(.caption)
                    Spacer()
                    Button("检查送达") {
                        Task {
                            do {
                                try await crewStore.checkPendingCrewDelivery()
                                self.error = nil
                            } catch {
                                self.error = error.localizedDescription
                            }
                        }
                    }
                    .disabled(crewStore.pendingCrewSending)
                }
                .padding(.horizontal, 18)
            }
            HStack {
                Button("取消") { crewStore.discardPendingCrew() }
                    .disabled(crewStore.pendingCrewDelivery != nil
                              || crewStore.pendingCrewRollbackError != nil)
                Spacer()
                Button {
                    let text = message
                    Task {
                        do {
                            try await crewStore.commitPendingCrew(text, draftId: draft.id)
                            self.error = nil
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                } label: {
                    if crewStore.pendingCrewSending { ProgressView().controlSize(.small) }
                    else if crewStore.pendingCrewDelivery != nil { Text("等待送达") }
                    else { Text("发送并创建") }
                }
                .buttonStyle(CrewdPrimaryButtonStyle())
                .disabled(message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                          || crewStore.pendingCrewSending
                          || crewStore.pendingCrewDelivery != nil
                          || crewStore.pendingCrewRollbackError != nil)
            }
            .padding(18)
        }.padding(24).frame(maxWidth:660,alignment:.leading)
            .background(Theme.Palette.surface,in:RoundedRectangle(cornerRadius:16))
            .overlay(RoundedRectangle(cornerRadius:16).strokeBorder(Theme.Palette.hairline.opacity(0.65),lineWidth:0.7))
            .padding(24).frame(maxWidth:.infinity)
        }.background(Theme.Palette.surfaceMuted.opacity(0.25))
        .navigationTitle("新机组")
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
