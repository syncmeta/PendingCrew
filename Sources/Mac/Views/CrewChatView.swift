import SwiftUI
import PhotosUI
import UniformTypeIdentifiers
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// 中栏 = crew 群聊会话页（spec v2 §9：群聊 = 沟通渠道 + 白板）。
///
/// - 时间线渲染 crew 白板（`crew_announcements`，经 `listCrewWhiteboard`）。
/// - 底部 composer 发消息（`postCrewMessage`，默认 broadcast = 全 session）。
/// - ask_human 交互卡（`payload.kind == "interaction"`，spec §10.6）就地渲染
///   问题 + 回答框，操作者直接在群聊里答（`answerInteraction`），不必跳别处。
///
/// 详细 IO（stdout/stderr/工具日志）不在这——那在右栏 session 窗口（§9.3）。
struct CrewChatView: View {
    let crewId: String
    let crewTitle: String
    /// macOS:点 session(右侧成员行 / session 气泡)时打开 inspector 并选中。
    /// 由 CrewCenterView 注入(用它持有的 inspectorPresented + sessionRunner)。iOS 传 nil。
    var onOpenSession: ((UUID) -> Void)? = nil
    /// macOS:成员区「+」起新 session(进新建态 + 弹 inspector)。iOS 传 nil。
    var onNewSession: (() -> Void)? = nil
    /// 「只看 @ 我的消息」筛选开关（Todo #61）。开关**钮**在窗口 toolbar 上（归
    /// `CrewCenterView` 持有），这里只吃它的状态 —— 所以是 `Binding?` 而不是
    /// `@State`：iOS 侧不传（nil = 恒关、不渲染任何筛选相关 UI），Mac 侧由
    /// toolbar 那一份 `@State` 驱动。判定逻辑全在 `CrewMentionFilter`。
    var showOnlyHumanMentions: Binding<Bool>? = nil

    @EnvironmentObject private var appModel: AppModel
    #if os(macOS)
    /// MacThreePaneView 注入（经 CrewCenterView）—— session 发的消息点击可
    /// 跳到右栏对应 session（chunk2 T6 中栏跳转）。
    @EnvironmentObject private var sessionRunner: CrewSessionRunner
    #endif

    @State private var entries: [CrewWhiteboardEntry] = []
    @State private var draft = ""
    /// Source-of-truth attachment list (file importer writes here on both platforms).
    @State private var pendingAttachments: [PendingComposerAttachment] = []
    @State private var loadError: String?
    @State private var sending = false
    /// Per-interaction reply drafts, keyed by permission_request_id.
    @State private var replyDrafts: [String: String] = [:]
    /// Crew roster (spec §9 — everyone is a member): rendered as a participant strip.
    @State private var members: [CrewMember] = []
    @State private var captainBotId: String?
    /// 「跟不跟着底部走」+ 未读计数（Todo #45 → #47）。默认跟随；用户滑上去看历史就
    /// 松开、新消息只记数不移动，滑回底部 / 点箭头 / 自己发一条再挂上并清零。
    @State private var bottomPin = CrewChatBottomFollow.Pin()
    /// 「用户的手还在滚动上」——`BottomOnContentGrowth` 靠它避开「人正往上翻、懒容器把
    /// 更早那几行真实量出来、内容一长高就被拽回底部」（Todo #54）。
    ///
    /// 引用型、**故意不进依赖图**：相位一变就写 `@State` 会让 body 失效 → `LazyVStack`
    /// 全量重测（#443 的热点），在手势刚开始那一下做这件事最伤。这个 `@State` 从头到尾
    /// 没人赋值，只是拿一个跨 body 稳定的实例（与 `selectionOwner` 同一套做法）。
    @State private var scrollPhaseBox = CrewChatBottomFollow.ScrollPhaseBox()

    /// 渲染窗口上限（#443）：只把最近这么多条交给 `ForEach`。切 crew 时归位到一页。
    @State private var renderLimit = CrewChatWindow.pageSize

    /// @-候选浮层限高用的两把尺（Todo #69）：群聊那一栏的总高、以及 composer 那
    /// 一截（回复横幅 + 输入胶囊 + 错误行）的高。两者相减 = composer 上方真正
    /// 剩下的空间，喂给纯函数 `CrewMentionPickerLayout` 算上限。
    ///
    /// **量的是 composer 里「不含浮层」的那部分**，故意的：把浮层自己也量进去会
    /// 形成回路（浮层长高 → 可用空间变小 → 浮层变矮 → 又变高），在窗口边界上会
    /// 抖个不停。
    @State private var chatColumnHeight: CGFloat = 0
    @State private var composerCoreHeight: CGFloat = 0

    /// 文本选中的归属（#443）：同一时刻只有一条气泡挂 `SelectionOverlay`。
    /// 存在 `@State` 里只为拿一个跨 body 稳定的实例 —— 它不是 `ObservableObject`，
    /// 从头到尾没人往这个 `@State` 赋值，所以**不会**让 body 失效。
    @State private var selectionOwner = CrewBubbleSelectionOwner()

    // MARK: - @-mention / reply state (Phase 6)
    /// Mentions staged for the next send, paired with the readable `@token`
    /// sitting in `draft`. `send()` flushes these; `reconcile` (on draft change)
    /// drops any whose token the user deleted.
    @State private var stagedMentions: [CrewStagedMention] = []
    /// The in-progress `@<prefix>` token the user is mid-typing, if any —
    /// drives the autocomplete popover's visibility + candidate filter.
    @State private var activeMentionQuery: CrewComposerMentionParser.ActiveQuery?
    /// Active "reply to this message" target — shows the quote banner above the
    /// composer and carries `reply_to` + an auto-@ on send. nil = not replying.
    @State private var replyTarget: CrewReplyTarget?

    /// Todo 模式（task #487）：composer 的 checklist 钮点亮时，发送 = 创建 Todo
    /// （LocalTodoStore + 群里「To do +1: #N」提示），发完自动熄灭。仅 macOS
    /// 启用（todo 数据与机长唤醒都在本机；iOS 侧不传 binding → 按钮不渲染）。
    @State private var todoMode = false

    // MARK: - ComposerView bindings
    /// Forwarded to ComposerView; ChatActionSheet flips this to open
    /// the system document picker (.fileImporter works on iOS 14+ too).
    @State private var showFileImporter = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var cameraImage: PlatformImage? = nil
    @State private var showPhotoPicker = false
    @State private var showCamera = false
    /// 有文件正悬在聊天面板上（拖拽反馈用）。
    @State private var isDropTargeted = false

    /// Bridge: convert PendingComposerAttachment → PendingAttachment for ComposerView's
    /// pending tray display. Nothing is uploaded from the tray (upload/落盘 happens on
    /// send), so uploadState = .uploaded. 缩略图取 `previewData` —— 拖进来的大图只有
    /// 一张 ImageIO 缩略图在内存里，原始字节留在暂存文件里。
    private var pendingForDisplay: Binding<[PendingAttachment]> {
        Binding(
            get: {
                pendingAttachments.map { a in
                    PendingAttachment(
                        id: a.id.uuidString,
                        mime: a.mime,
                        size: a.byteSize,
                        filename: a.isImage ? nil : a.filename,
                        uploadState: .uploaded,
                        localPreviewData: a.previewData
                    )
                }
            },
            set: { newValue in
                // Removals: ComposerView's tray "x" button removes by id.
                let keptIds = Set(newValue.map { $0.id })
                // 从托盘里删掉 = 那份暂存文件也该走，别在 temp 里留着一份几百 MB。
                for dropped in pendingAttachments where !keptIds.contains(dropped.id.uuidString) {
                    if let staged = dropped.stagedURL {
                        CrewChatAttachmentStore.discardStaged(staged)
                    }
                }
                pendingAttachments.removeAll { !keptIds.contains($0.id.uuidString) }
            }
        )
    }

    /// Todo 模式与普通发送同一口径：有字 **或** 有附件就能发（Todo #52 —— 「如图
    /// xxx 请解决」是人建 Todo 最常见的形状，图不能被挡在 Todo 外面）。
    private var canSend: Bool {
        guard !sending else { return false }
        return !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
               || !pendingAttachments.isEmpty
    }

    /// Todo 切换钮只在 macOS 亮出（见 `todoMode`）。
    private var todoModeBinding: Binding<Bool>? {
        #if os(macOS)
        return $todoMode
        #else
        return nil
        #endif
    }

    var body: some View {
        content
        .background(Theme.Palette.canvas)
        // 整个聊天面板（消息区 + composer）是一块 drop 区 —— 从 Finder / 文件 App
        // 拖文件进来落进附件托盘，和「+ 面板▸文件」走**同一个** `ingestFiles`。
        //
        // 只登记 `.fileURL`：网页里拖一段文字 / 一个链接给的是 `public.url` /
        // `public.utf8-plain-text`，不匹配 → 光标就不会变成"可放下"，不会把网址
        // 当附件收。别为了"更兼容"改成 `.item` —— 那把文本拖拽也放进来了。
        .onDrop(of: [.fileURL], isTargeted: $isDropTargeted, perform: handleDrop)
        .overlay { dropHighlight }
        .animation(.easeOut(duration: 0.12), value: isDropTargeted)
        .task(id: crewId) { await subscribe() }
        // Todo #69：@-候选浮层的限高要跟着窗口走，所以这两把尺在这里收口。
        // 只在窗口改尺寸 / composer 行数变化时各写一次 @State —— 浮层自己不在被
        // 量的子树里，所以不会「越算越矮」地自激（见 composerCoreHeight）。
        .onPreferenceChange(CrewChatColumnHeightKey.self) { chatColumnHeight = $0 }
        .onPreferenceChange(CrewComposerCoreHeightKey.self) { composerCoreHeight = $0 }
        // 切 crew 视图整个重建（macOS/iPad 都带 `.id(crewId)`），所以未发出的草稿不能只
        // 活在 @State 里 —— 进屏取回、变动存回，切走再切回来原样在。
        .onAppear { restoreDraft() }
        .onChange(of: replyTarget) { _, _ in saveDraft() }
        // File importer — .fileImporter is available on iOS 14+ and macOS 11+.
        // On iOS the system presents a document picker; on macOS a Finder sheet.
        // `.item` 而不是 `.data` —— 对齐 PendingBot。`.data` 选不了 package/bundle
        // 型（`.key` / `.app` / `.xcodeproj` 只 conform `public.item`），于是那些
        // 文件在选择器里是灰的、人以为"发不了"。放开选中之后它们由
        // `CrewFileAttachmentIntake` 明确拒收并说明理由（它们是目录不是文件）。
        .fileImporter(
            isPresented: $showFileImporter,
            allowedContentTypes: [.item],
            allowsMultipleSelection: true,
            onCompletion: handleImport
        )
        // Photo picker — iOS only; wires the showPhotoPicker binding from ComposerView.
        // macOS 侧 ChatActionSheet 不渲染「图片」tile（Mac 没有照片图库概念，
        // 「文件」入口已经能选图）—— 那个 tile 曾经点了毫无反应，是死钮。
        #if os(iOS)
        .photosPicker(
            isPresented: $showPhotoPicker,
            selection: $photoItems,
            matching: .images
        )
        .onChange(of: photoItems) { _, items in
            Task { await handlePhotoItems(items) }
        }
        // 「拍照」tile 的宿主 —— 照搬 PendingBot `ConversationView` 那套
        // （`CameraPicker` 已随 ChatActionSheet 一起 vendored 进来）。此前
        // showCamera/cameraImage 全程无消费者，点「拍照」同样毫无反应。
        .fullScreenCover(isPresented: $showCamera) {
            CameraPicker { image in
                showCamera = false
                if let image { cameraImage = image }
            }
            .ignoresSafeArea()
        }
        .onChange(of: cameraImage) { _, image in
            guard let image else { return }
            handleCameraImage(image)
        }
        #endif
    }

    // MARK: - drag & drop

    /// 拖拽悬停时的视觉反馈 —— 没有它人不知道能不能松手。
    @ViewBuilder
    private var dropHighlight: some View {
        if isDropTargeted {
            let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
            shape
                .fill(Theme.Palette.accent.opacity(0.08))
                .overlay {
                    shape.strokeBorder(
                        Theme.Palette.accent,
                        style: StrokeStyle(lineWidth: 2, dash: [7, 5]))
                }
                .overlay {
                    Label("松手把文件放进这个群", systemImage: "tray.and.arrow.down")
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.accent)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(
                            Capsule().fill(Theme.Palette.canvas.opacity(0.92)))
                }
                .padding(8)
                .allowsHitTesting(false) // 高亮只是装饰，别挡住 drop 本身
                .transition(.opacity)
        }
    }

    /// 收下这次拖放。返回 false = 这堆 provider 里没有文件，交还给系统。
    private func handleDrop(_ providers: [NSItemProvider]) -> Bool {
        let fileProviders = providers.filter { $0.canLoadObject(ofClass: URL.self) }
        guard !fileProviders.isEmpty else { return false }
        for provider in fileProviders {
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                guard let url, url.isFileURL else { return }
                // 复制发生在这个回调里（iOS 的 security scope 出了回调就没了），
                // 且这里本来就不在主线程 —— 复制几百 MB 不该卡住界面。
                let result = CrewFileAttachmentIntake.intake(urls: [url])
                Task { @MainActor in apply(result) }
            }
        }
        return true
    }

    /// intake 结果落进 @State：收下的进托盘，被拒的走既有软报错通道（errorBar）。
    @MainActor
    private func apply(_ result: CrewFileAttachmentIntake.Result) {
        pendingAttachments.append(contentsOf: result.accepted)
        if !result.errors.isEmpty {
            loadError = result.errors.joined(separator: "\n")
        } else if !result.accepted.isEmpty {
            loadError = nil
        }
    }

    @ViewBuilder
    private var content: some View {
        #if os(macOS)
        // 成员列表 + 待审批已搬进 inspector(CrewSessionWindowView 的成员列表模式)。
        // 中栏只留群聊本体。macOS 宿主跟 PendingBot 一样给 composer 用纯 canvas
        // 底色；输入胶囊自身的轻阴影留在 vendored ComposerView 里。
        //
        // 标题栏透明 + fullSizeContentView(WindowSeparatorRemover)会让消息滚进
        // navigationTitle 底下叠字 —— 顶部滚动边缘用 .hard：标题后面是一条不透明底,
        // 消息滚到顶被它挡住,标题始终可读。
        timelineWithTopEdge
            .safeAreaInset(edge: .bottom, spacing: 0) {
                VStack(spacing: 0) {
                    errorBar
                    composer
                }
                .background(Theme.Palette.canvas)
            }
            .measuringChatColumnHeight()
        #else
        VStack(spacing: 0) {
            if !members.isEmpty {
                CrewRosterBar(members: members, captainBotId: captainBotId)
                Divider()
            }
            timeline
            errorBar
            Divider()
            composer
        }
        .measuringChatColumnHeight()
        #endif
    }

    #if os(macOS)
    @ViewBuilder
    private var timelineWithTopEdge: some View {
        if #available(macOS 26.0, *) {
            timeline.scrollEdgeEffectStyle(.hard, for: .top)
        } else {
            timeline
        }
    }
    #endif

    @ViewBuilder
    private var errorBar: some View {
        if let errorText = loadError {
            Text(errorText)
                .font(Theme.Fonts.caption)
                .foregroundStyle(.red)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 14)
                .padding(.top, 4)
        }
    }

    private var composer: some View {
        VStack(spacing: 0) {
            mentionPickerBar
            composerCore
        }
    }

    /// Inline @-mention autocomplete, rendered as a sibling directly
    /// above the composer (so it floats just over the input, Slack-style)
    /// rather than mutating the vendored `ComposerView`. Driven entirely
    /// from `$draft` via `onDraftChange` below.
    ///
    /// **不在 `composerCore` 里面**（Todo #69）：那一截要被 GeometryReader 量高，
    /// 把浮层自己量进去就成了回路（见 `composerCoreHeight`）。
    @ViewBuilder
    private var mentionPickerBar: some View {
        if let query = activeMentionQuery {
            let cands = mentionCandidates(for: query.prefix)
            if !cands.isEmpty {
                CrewMentionPicker(
                    candidates: cands,
                    availableHeight: mentionPickerAvailableHeight
                ) { pickMention($0) }
                    .padding(.horizontal, Theme.Metrics.gutter)
                    .padding(.bottom, 4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .transition(.opacity)
            }
        }
    }

    /// 回复横幅 + 输入胶囊本体 —— 也就是「composer 那一截」的真实高度来源。
    private var composerCore: some View {
        VStack(spacing: 0) {
            if let replyTarget {
                replyBanner(replyTarget)
            }
            ComposerView(
                input: $draft,
                pending: pendingForDisplay,
                photoItems: $photoItems,
                cameraImage: $cameraImage,
                showFileImporter: $showFileImporter,
                showPhotoPicker: $showPhotoPicker,
                showCamera: $showCamera,
                canSend: canSend,
                onSend: { Task { await send() } },
                isStreaming: false,
                showsAttachments: true,
                placeholder: "发群消息", // 发送态；todoMode 点亮时 ComposerView 自动切「新建 Todo」
                todoMode: todoModeBinding,
                onPasteAttachments: pasteAttachmentsHandler // Todo #3：⌘V 粘贴图片/文件
            )
        }
        .onChange(of: draft) { _, newValue in onDraftChange(newValue) }
        .animation(.easeOut(duration: 0.12), value: activeMentionQuery)
        .measuringComposerCoreHeight()
    }

    // MARK: - @-mention picker plumbing (Phase 6)

    /// Re-evaluate the open `@`-token + drop staged mentions whose token text
    /// the user deleted. Called on every `draft` change.
    private func onDraftChange(_ newValue: String) {
        activeMentionQuery = CrewComposerMentionParser.activeQuery(in: newValue)
        stagedMentions = CrewComposerMentionParser.reconcile(
            staged: stagedMentions, draft: newValue)
        saveDraft()
    }

    // MARK: - per-crew 草稿（切 crew 视图重建，草稿不能跟着没）

    /// 进屏取回本 crew 记着的草稿。文本 / 已挂上的 @ / 正在回复谁一起取 —— 只取文本的话
    /// 恢复出来的 `@某人` 就成了一串没有目标的字（看着 @ 了，发出去谁也没 @ 到）。
    private func restoreDraft() {
        let saved = CrewComposerDraftStore.shared.draft(for: crewId)
        guard !saved.isEmpty else { return }
        draft = saved.text
        replyTarget = saved.replyTarget
        // 放最后：写 `draft` 会触发 `onDraftChange` → `reconcile`，那会照着当时还空的
        // staged 列表把恢复出来的 @ 全判成"用户删掉了"。
        stagedMentions = saved.stagedMentions
    }

    /// 存回去（草稿一动就存；发送成功后由 `send()` 清）。
    private func saveDraft() {
        CrewComposerDraftStore.shared.save(
            .init(text: draft, stagedMentions: stagedMentions, replyTarget: replyTarget),
            for: crewId)
    }

    /// The roster member id that represents "me" (skipped from the candidate
    /// list — you don't @ yourself). Local human uses the synthetic local id;
    /// logged-in matches the current user.
    private var selfMemberId: String? {
        let uid = localUserId
        return members.first {
            $0.memberKind == "human" && $0.userId != nil && $0.userId == uid
        }?.id
    }

    /// @ 候选的成员源。macOS 本地态把**在跑的 worker session**(`sessionRunner.runs`)
    /// 合成成 `code_session` 成员并入 —— roster(`LocalBackend.listCrewMembers`)只含
    /// captain + 本机人类,本地 session 不在里头(#2:普通 session 进不了 @ 名单的根因)。
    /// captain 已由 `.captain` 候选覆盖,这里跳过(`role != .captain`);已在 roster 的
    /// session(登录态)按 codeSessionId 去重。
    private var mentionMembers: [CrewMember] {
        #if os(macOS)
        let known = Set(members.compactMap {
            $0.memberKind == "code_session" ? $0.codeSessionId : nil
        })
        let runMembers: [CrewMember] = sessionRunner.runs
            // 只取**本 crew** 的 run —— sessionRunner.runs 持有本机所有 crew 的 run,
            // 不按 crewId 滤会把别的 crew 的 session 混进本群 @ 候选(#7)。roster 那半
            // 走 listCrewMembers(crewId) 本就 crew-scoped。
            .filter { $0.crewId == crewId && $0.status == .running
                      && $0.role != .captain && !known.contains($0.sessionId) }
            .map { run in
                CrewMember(
                    id: run.sessionId, memberKind: "code_session", userId: nil, botId: nil,
                    codeSessionId: run.sessionId, displayName: run.displayName, role: nil,
                    status: "active", representsCrewId: nil,
                    sessionStatus: run.isWorking ? "running" : nil)
            }
        return members + runMembers
        #else
        return members
        #endif
    }

    /// Candidates for the current `@<prefix>`, filtered from the roster (+ local runs).
    private func mentionCandidates(for prefix: String) -> [CrewMentionCandidate] {
        crewMentionCandidates(
            members: mentionMembers,
            captainBotId: captainBotId,
            prefix: prefix,
            selfMemberId: selfMemberId)
    }

    /// Pick a candidate: replace the open `@prefix` token with `@label ` and
    /// stage the mention. Closes the popover.
    private func pickMention(_ candidate: CrewMentionCandidate) {
        guard let ins = CrewComposerMentionParser.insert(candidate: candidate, into: draft) else {
            activeMentionQuery = nil
            return
        }
        draft = ins.newDraft
        stagedMentions.append(ins.staged)
        // `draft` mutation re-fires onDraftChange → activeMentionQuery becomes
        // nil (token now closed by the trailing space), so the popover closes.
    }

    /// 用本地活 run 的实时状态覆写气泡 sender 的 `sessionStatus`（#2 群聊气泡缺状态圆点）。
    /// nil sender（自己的消息）/无 senderSessionId / 查无对应 run 时原样返回。状态推导与
    /// 成员列表 `CrewSessionWindowView.senderForRun`、点名快照共用 `CrewSessionStateDerivation`
    /// 那一份，颜色见 `SessionStatusDot`。仅 macOS（本地 run 在 macOS）。**只读 runs**。
    private func liveStatusSender(
        _ sender: GroupBubbleSender?, senderSessionId: String?
    ) -> GroupBubbleSender? {
        #if os(macOS)
        guard var s = sender, let sid = senderSessionId,
              let run = sessionRunner.runs.first(where: { $0.sessionId == sid }) else { return sender }
        s.sessionStatus = CrewSessionStateDerivation.state(
            isRunning: run.status == .running, health: run.health,
            isWorking: run.isWorking, awaitingDecision: run.pendingDecision != nil,
            awaitingReply: run.awaitingReply != nil)
        return s
        #else
        return sender
        #endif
    }

    // MARK: - avatar → @-mention (right-click / long-press the sender avatar)

    /// Build the staged mention for @-ing a bubble's sender from its avatar.
    /// captain → `.captain`(不按 id);session → `.session(id)`;human → human
    /// target。泛 bot(非机长)无 mention 目标 → nil(不显示 @ 菜单)。id 取值对齐
    /// `CrewChatAdapter.adapt`:session=senderSessionId、human=senderUserId。
    private func avatarMention(_ sender: GroupBubbleSender) -> CrewStagedMention? {
        // strip 前导 @：作者名自带 @（如 relay 远端名）时不叠成 @@。appendMention 再兜
        // 底归一化一次，双保险。
        let token = "@" + sender.displayName.drop(while: { $0 == "@" })
        if sender.isCaptain { return CrewStagedMention(token: token, mention: .captain) }
        if sender.isSession { return CrewStagedMention(token: token, mention: .session(sender.id)) }
        if sender.kind == .user {
            return CrewStagedMention(token: token, mention: CrewMention(kind: "human", targetId: sender.id))
        }
        return nil
    }

    /// Append the avatar-picked mention to the composer draft + staged set
    /// (dedup by target; no-op if already @'d). Pure logic in
    /// `CrewComposerMentionParser.appendMention`; here just apply to @State.
    private func appendMentionFromAvatar(_ staged: CrewStagedMention) {
        let r = CrewComposerMentionParser.appendMention(staged, to: draft, existing: stagedMentions)
        draft = r.newDraft
        stagedMentions = r.staged
    }

    // MARK: - reply plumbing (Phase 6)

    /// Begin replying to `entry`: show the quote banner. The auto-@ on the
    /// original sender lives implicitly in `replyTarget.mention` and is merged
    /// at send time — it is NOT pushed into `stagedMentions` (those are
    /// token-backed and would be reconciled away since no `@token` is typed).
    private func beginReply(to entry: CrewWhiteboardEntry) {
        let sender = CrewSenderResolver.resolve(
            entry, members: members, captainBotId: captainBotId, localUserId: localUserId)
        replyTarget = CrewReplyTargetBuilder.make(entry: entry, senderName: sender.displayName)
    }

    /// Cancel the active reply — clears the banner (and its implicit auto-@).
    private func cancelReply() {
        replyTarget = nil
    }

    /// The full mention set to send: token-backed staged mentions + the reply
    /// auto-@ (if any), de-duplicated.
    private func mentionsForSend() -> [CrewMention] {
        var all = stagedMentions
        if let replyMention = replyTarget?.mention {
            all.append(CrewStagedMention(token: "", mention: replyMention))
        }
        return CrewComposerMentionParser.mentionsToSend(all)
    }

    // MARK: - in-bubble reply reference (#377)

    /// Resolve the "被回复" reference for a timeline entry against the currently
    /// loaded entries (local lookup, no server join). nil when the entry isn't a
    /// reply; a degraded placeholder when the target is outside the loaded window.
    private func replyReference(for entry: CrewWhiteboardEntry) -> CrewReplyReference? {
        CrewReplyReferenceResolver.resolve(
            for: entry, in: entries,
            members: members, captainBotId: captainBotId, localUserId: localUserId)
    }

    /// Small quote strip rendered above a reply bubble: an accent rule + the
    /// quoted sender + a one-line snippet. The degraded case (`found == false`)
    /// shows just the generic placeholder snippet, no sender prefix.
    @ViewBuilder
    private func replyQuoteStrip(_ ref: CrewReplyReference) -> some View {
        HStack(spacing: 6) {
            Rectangle()
                .fill(Theme.Palette.accent.opacity(0.6))
                .frame(width: 2)
            Text(ref.found ? "回复 \(ref.senderName)：\(ref.snippet)" : ref.snippet)
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.Palette.inkMuted)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.horizontal, Theme.Metrics.gutter)
    }

    /// The reply quote banner shown above the composer while a reply is staged.
    @ViewBuilder
    private func replyBanner(_ target: CrewReplyTarget) -> some View {
        HStack(spacing: 8) {
            Rectangle()
                .fill(Theme.Palette.accent)
                .frame(width: 2)
                .frame(maxHeight: .infinity)
            VStack(alignment: .leading, spacing: 1) {
                Text("回复 \(target.quotedSender)")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.accent)
                Text(target.quotedSnippet)
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Button {
                cancelReply()
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(Theme.Fonts.glyph(size: 15))
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
            .buttonStyle(.plain)
        }
        .frame(height: 34)
        .padding(.horizontal, Theme.Metrics.gutter)
        .padding(.top, 6)
    }

    // macOS：登录后用真实 user id（`AppModel.currentUserId`，由
    // `CrewStore.refreshSubjects()` 回填）—— 这样 relay 从其它设备（如 iOS）
    // 回流的、同一账号发的消息也能被 `CrewSenderResolver` 认成"我"(右对齐)。
    // 未登录 BYOK 沿用哨兵常量 `LocalWhiteboardStore.localUserId`。
    // （composer 本地行永远标哨兵常量，不随登录态变；`CrewSenderResolver`
    // 对两者都判"我"，登录态切换不会让自己刚发的话变成"别人"。）
    // iOS: 暂无本地后端 — 使用 AppModel.currentUserId。
    private var localUserId: String? {
        #if os(macOS)
        return appModel.currentUserId ?? LocalWhiteboardStore.localUserId
        #else
        return appModel.currentUserId
        #endif
    }

    /// 筛选开关当前是不是开着（没传 binding = 恒关）。
    private var onlyMentions: Bool { showOnlyHumanMentions?.wrappedValue ?? false }

    /// composer 上方剩下多少空间（Todo #69）。没量到时是 0 —— `CrewMentionPickerLayout`
    /// 认这个值走兜底上限，仍然有界。
    private var mentionPickerAvailableHeight: CGFloat {
        max(0, chatColumnHeight - composerCoreHeight)
    }

    /// 判定用的花名册快照（Todo #61）。显示名取 `CrewSenderNaming.groupSender`
    /// 那一份 —— 与气泡、成员列表、@-菜单同一套名字，正文里的 `@小绿` 才对得上。
    private var mentionRoster: CrewMentionFilter.Roster {
        CrewMentionFilter.Roster.from(members: members, captainBotId: captainBotId)
    }

    /// 时间线渲染的消息。macOS 把交互卡滤掉(挪去右上待审批区);iOS 保留 inline。
    ///
    /// **筛选开关开着时再筛一道**（Todo #61）：只留「@ 了人类」的 + 人类自己发的。
    /// 筛在这里而不是筛在 `windowedEntries` 上，是因为渲染窗口那一整套
    /// （`renderLimit` / `hasMore` / 「上面还有 N 条」/ `anchorOnExpand` /
    /// `insertedAbove` / `afterInsert`）全都读 `timelineEntries` —— 从源头筛，
    /// 它们自动按**筛选后**的列表算。筛在下游的话会出现「显示还有 300 条、点开
    /// 什么都没有」。
    ///
    /// 关着的时候一条判定都不跑（`guard onlyMentions`），成本与改动前逐字相同；
    /// 开着时每条的常见路径是「正文里找一个 `@`，找不到就走人」（#443 的口径：
    /// 这个属性每次访问都重算，所以判定必须廉价）。
    private var timelineEntries: [CrewWhiteboardEntry] {
        #if os(macOS)
        let base = entries.filter { !$0.isInteraction }
        #else
        let base = entries
        #endif
        guard onlyMentions else { return base }
        return CrewMentionFilter.onlyHumanMentions(
            base, roster: mentionRoster, includingFrom: localUserId)
    }

    /// 时间线的一行：消息本体 + 「它上面要不要插一条时间分隔」。
    ///
    /// 分隔要看**前一条**的时刻，但那个「前一条」必须在建列表时就定下来，
    /// **不能留到行闭包里现去索引**（2026-08-07 16:07 闪退的病根，见下）。
    private struct TimelineRow: Identifiable {
        let entry: CrewWhiteboardEntry
        /// 非 nil = 这行上面插一条该时刻的分隔。
        let separator: Date?
        var id: String { entry.id }
    }

    /// 真正交给 `ForEach` 的那一段 —— **只有最近 `renderLimit` 条**（#443 第三道闸）。
    ///
    /// 两份真实 hang 报告（见 `CrewChatWindow` 顶部）指的是同一件事：这一下有多贵，
    /// 只取决于「视图树里有多少行」。所以打开一个 crew 的成本必须与「这个 crew 聊过
    /// 多少条」脱钩。更早的消息一条没丢，顶部占位点一下往前放一页。
    private var windowedEntries: [CrewWhiteboardEntry] {
        CrewChatWindow.window(timelineEntries, limit: renderLimit)
    }

    /// 一次算完，行闭包只读自己那一份。
    ///
    /// 原来这里是 `ForEach(Array(timelineEntries.enumerated()), id: \.element.id)`，
    /// 行闭包里再 `timelineEntries[idx - 1]` 取前一条 —— 两处 `timelineEntries` 是
    /// **计算属性，每次访问都重新 filter 一遍 `entries`**。于是 `idx` 来自建列表那一刻的
    /// 快照，下标却打在「现在这一刻」重算出来的数组上：只要这中间白板少了条目
    /// （撤回 / 切 crew / 订阅重放），下标就越界，Swift 直接 trap 打死进程。
    ///
    /// 2026-08-07 16:07 那次崩溃栈正是这么走的：`LazyStack.firstIndex(of:)` ←
    /// `_LazyLayout_Subviews.firstIndex(id:)` ← `ForEachState.item(at:offset:)` →
    /// 掉进 app 自己的代码里 trap。触发它的是程序化 `scrollTo` —— 滚到某个 id 要在
    /// 懒容器里找它的位置，那次遍历会拿着旧快照的下标去跑行闭包。
    ///
    /// 所以这里改成**建列表时就把前后关系定死**：预先算好每行的分隔时刻，
    /// 行闭包里不再有任何索引运算，也不再重复 filter（原来每行都要重算一遍，O(n²)）。
    private var timelineRows: [TimelineRow] {
        var rows: [TimelineRow] = []
        var prev: Date?
        for entry in windowedEntries {
            let cur = CrewTimeSeparator.parse(entry.createdAt)
            let sep = cur.flatMap {
                CrewTimeSeparator.needsSeparator(prev: prev, cur: $0) ? $0 : nil
            }
            rows.append(TimelineRow(entry: entry, separator: sep))
            // 无条件跟进（含 nil）—— 与原来「prev = 紧邻上一条的 parse 结果，
            // 解析不出就是 nil」一字不差，别改成「记住最后一个解析成功的」。
            prev = cur
        }
        return rows
    }

    /// 影响行高的第二波异步数据（成员名册 / 机长 id / 本机 user id）揉成一个令牌 ——
    /// 它一变，每行的头像列与名字行可能整体翻面，行高剧变（Todo #45 根因之二）。
    private var bubbleLayoutToken: String {
        CrewChatBottomFollow.layoutToken(
            memberIds: members.map(\.id),
            captainBotId: captainBotId,
            localUserId: localUserId)
    }

    /// 时间线。
    ///
    /// **滚动视图恒在**，空态只是盖在它上面的一层 —— 别改回「空 → emptyState / 非空 →
    /// messageScroll」的二选一分支（Todo #45 根因之一）：那样进任何 crew 的第一帧都走
    /// 空态、`ScrollView` 此刻还不存在，数据到位时它是**新建**的，`.onChange(of: count)`
    /// 收不到 0→N（onChange 不为初始值触发），首屏落底就只剩原生锚点一个执行者。
    private var timeline: some View {
        messageScroll
            // 空白态：只居中放一个聊天图标，别的什么都不要。
            .overlay { if timelineEntries.isEmpty { emptyState } }
    }

    private var messageScroll: some View {
        ScrollViewReader { proxy in
            ScrollView {
                // 首屏 12 条刻意用 eager VStack 一次量完；LazyVStack 会先拿估算行高解
                // `scrollTo(tail)`，真实高度回缩后视口可能留在内容范围外，直到用户滚一下
                // 系统才钳回来 —— Todo #56 的空白现场。人点「加载更早」后改回 lazy，
                // 避免一路翻到几百条又把主线程撑爆。分支只由按钮这个离散事件改变，绝不
                // 用滚动位置驱动内容增长。
                Group {
                    if CrewChatWindow.usesEagerInitialLayout(limit: renderLimit) {
                        VStack(alignment: .leading, spacing: 0) { timelineContent(proxy) }
                    } else {
                        LazyVStack(alignment: .leading, spacing: 0) { timelineContent(proxy) }
                    }
                }
                .padding(.vertical, 10)
            }
            // 打开聊天默认落在最新消息（IM 惯例；Todo #28 起，#45 补首屏路径，#47 补
            // 「跟随只许自己开、不许自己关」与未读计数）。
            //
            // 初始定位交给原生底部锚点，另外三个**离散事件**各补一记确定性滚底：
            //   - 条目数变了（含首屏 0→N —— 靠上面「滚动视图恒在」才收得到）
            //   - 行高输入变了（成员/机长/本机身份第二波到位 → 每行高度剧变）
            //   - 内容真实高度长高了（懒容器把估算行高换成真实行高那一下，Todo #54）
            // 三者都过 `bottomPin`：用户自己滑上去看历史就不再拽他回来，改成右下角
            // 一个箭头 + 未读数字。
            //
            // 第三记是 Todo #54 补的，缺了它前两记会**双双跑在测量之前**：本地 backend
            // 是 @MainActor 且方法体里一次不 await，`entries` 与 `members` 两次写入被
            // SwiftUI 合并成同一次 body 更新，两个 onChange 背靠背触发，`scrollTo` 手里
            // 只有估算行高 —— 算不满一屏时目标偏移就是 0，人看到的正是「停在最早那条」。
            // 详细推导见 `CrewChatBottomFollow` 顶部「第四次」那一节。
            //
            // **不许加定时器 / asyncAfter / 每帧反复滚**来兜底 —— 滚动锚点 + 程序化
            // scrollTo + 永不结束的动画三者凑齐会把布局打成自激（2026-07-26 事故，
            // 见 TypingDotsLayerView 顶部与 LayoutLoopRegressionTests）。第三记不是那个
            // 形状：它只认**长高**（回缩不动），所以我们自己的滚动无法经由「缩」这条边
            // 回头触发自己；窗口内行数有限，量完高度就不再变，自己停。
            .modifier(ChatScrollAnchor(isFollowing: bottomPin.isFollowing))
            .modifier(BottomPinTracker(pin: $bottomPin, phaseBox: scrollPhaseBox))
            // Todo #56：未读按钮跟真实位置走。投影为 Bool，只在跨过到底阈值时写一次，
            // 不会逐帧改 @State，也不改变内容高度，因此没有布局自激的反馈边。
            .modifier(BottomReachedTracker(pin: $bottomPin))
            .modifier(BottomOnContentGrowth(
                isFollowing: bottomPin.isFollowing,
                phaseBox: scrollPhaseBox
            ) {
                landAtBottom(proxy, animated: false)
            })
            .overlay(alignment: .bottomTrailing) {
                if bottomPin.unread > 0 { unreadJumpButton(proxy) }
            }
            .animation(.easeOut(duration: 0.12), value: bottomPin.unread > 0)
            .onChange(of: timelineEntries.count) { old, new in
                let added = new - old
                // 已经翻开过更早的话，把新增条数补进上限 —— 否则来一条新消息就把
                // 他刚翻出来的最老那条挤出窗口，正在读的内容从上面消失（#443）。
                renderLimit = CrewChatWindow.afterInsert(limit: renderLimit, added: added)
                // 在底部 → 跟着走（行为 2）；不在底部 → 位置一动不动，只把未读加上去
                // （行为 3）。判定收口在 `Pin.received`。
                if bottomPin.received(added) { landAtBottom(proxy, animated: true) }
            }
            // 切换筛选（Todo #61）：列表整个换了一批内容，不是「来了新消息」。
            // 窗口深度归位到一页 + 跟随/未读归位 + 落到最新一条 —— 与切 crew 同一
            // 套动作（`subscribe()` 里那两行）。
            //
            // 与上面那记 `onChange(of: timelineEntries.count)` **两种执行顺序都收敛**：
            // 开筛选时条数变少，`Pin.received` 的 `added > 0` 直接挡掉；关筛选时条数
            // 暴涨，若它先跑，这里随后把 pin 和 renderLimit 双双归位；若它后跑，面对的
            // 已是一个 `isFollowing == true` 的新 pin（不记未读、只落底）和
            // `renderLimit == pageSize`（`afterInsert` 的 `limit > pageSize` 挡掉增长）。
            // 两条路的终点都是「一页 + 贴在最新一条」，所以不依赖 SwiftUI 的 onChange
            // 触发次序。
            .onChange(of: onlyMentions) { _, _ in
                renderLimit = CrewChatWindow.pageSize
                bottomPin = CrewChatBottomFollow.Pin()
                landAtBottom(proxy, animated: false, force: true)
            }
            .onChange(of: bubbleLayoutToken) { _, _ in
                // 第二波数据落地这一记不做动画：它不是「来了新消息」，只是把偏移量补正到
                // 变高之后的底部，动画反而像画面自己在抖。
                landAtBottom(proxy, animated: false)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// 右下角那个「箭头 + 未读数字」（Todo #47 行为 3）。人类明确要的是**只标数字**，
    /// 所以这里没有「N 条新消息」之类文案，也没有别的修饰。点它 = 落底 + 清零 + 重新
    /// 挂上跟随。只在真有未读时出现。
    private func unreadJumpButton(_ proxy: ScrollViewProxy) -> some View {
        Button {
            bottomPin.jumpToBottom()
            landAtBottom(proxy, animated: true, force: true)
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "arrow.down")
                    .font(Theme.Fonts.glyph(size: 11, weight: .semibold))
                Text("\(bottomPin.unread)")
                    .font(Theme.Fonts.caption)
                    .monospacedDigit()
            }
            .foregroundStyle(Theme.Palette.onAccent)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(Capsule().fill(Theme.Palette.accent))
        }
        .buttonStyle(.plain)
        .padding(.trailing, Theme.Metrics.gutter)
        .padding(.bottom, 10)
        .transition(.opacity)
    }

    /// 落底 —— 只在还跟随底部时动（`force` 是「点了箭头」那条明确指令，绕过跟随开关）。
    /// 跟随开关由 `BottomPinTracker` 按**用户自己的**滚动更新。
    private func landAtBottom(_ proxy: ScrollViewProxy, animated: Bool, force: Bool = false) {
        guard force || bottomPin.isFollowing else { return }
        if animated {
            withAnimation(.linear(duration: 0.05)) { proxy.scrollTo(CrewChatBottomFollow.bottomAnchorID, anchor: .bottom) }
        } else {
            proxy.scrollTo(CrewChatBottomFollow.bottomAnchorID, anchor: .bottom)
        }
    }

    @ViewBuilder
    private func timelineContent(_ proxy: ScrollViewProxy) -> some View {
        // 渲染窗口之上还有更早的 → 一条「加载更早」占位（#443）。
        // 只认「点一下」这个离散事件，**不做滚到顶自动加载** —— 滚动位置驱动内容增长、
        // 内容增长又改滚动位置，正是布局自激的配方（2026-07-26 / 08-10 两次事故）。
        if CrewChatWindow.hasMore(total: timelineEntries.count, limit: renderLimit) {
            loadEarlierRow(proxy)
        }
        ForEach(timelineRows) { row in
            if let sep = row.separator {
                CrewTimeSeparator(date: sep)
            }
            rowView(row.entry).id(row.entry.id)
        }
        #if os(macOS)
        // Todo #4：iMessage 式「正在输入」。本 crew 每个在跑 run 一行，行内自己观察
        // isWorking 决定显隐（父视图只管 runs 增删）；出现时滚到底让指示器可见。
        ForEach(sessionRunner.runs.filter { $0.crewId == crewId }, id: \.runID) { run in
            CrewTypingIndicatorRow(run: run, captainBotId: captainBotId) {
                landAtBottom(proxy, animated: true)
            }
        }
        #endif
        // 落底哨兵 —— 必须留在所有消息行**之后**，落点才是「时间线末端」而不是
        // 渲染窗口边界（Todo #47 行为 1）。
        Color.clear.frame(height: 1).id(CrewChatBottomFollow.bottomAnchorID)
    }

    @ViewBuilder
    private func rowView(_ entry: CrewWhiteboardEntry) -> some View {
        if entry.isInteraction {
            let reqId = entry.payload?.permissionRequestId ?? entry.id
            CrewInteractionCard(
                entry: entry,
                reply: Binding(get: { replyDrafts[reqId] ?? "" }, set: { replyDrafts[reqId] = $0 })
            ) { Task { await answer(reqId) } }
        } else {
            let (msg, adaptedSender) = CrewChatAdapter.adapt(
                entry,
                members: members,
                captainBotId: captainBotId,
                localUserId: localUserId
            )
            // #2 群聊气泡状态圆点：adapt 出来的 sender.sessionStatus 走 roster 静态成员
            // （无 live status）→ 本地 session 恒 nil → 圆点隐藏。按 entry.senderSessionId
            // 查本 crew 的活 run，用与成员列表同一套映射（CrewSessionWindowView.senderForRun）
            // 覆写进去。只读 runs。
            let sender = liveStatusSender(adaptedSender, senderSessionId: entry.senderSessionId)
            // #377 — 被回复引用条:在已加载 entries 里本地查 entry.inReplyTo 指向的
            // 那条,取发送者名 + 摘要渲染成「回复 X:…」。找不到 → 退化占位,不崩。
            // mine(右对齐)时引用条也右对齐贴齐气泡。
            let replyRef = replyReference(for: entry)
            // 右键(macOS)/长按(iOS)头像 → @ 该发送者。仅他人消息(sender != nil)且
            // 能解析出 mention 目标(captain/session/human)时给闭包;泛 bot 无 @ 目标 → nil。
            let mentionStaged = sender.flatMap(avatarMention)
            let bubbleBody = BubbleView(
                message: msg,
                botName: sender?.displayName ?? "机组",
                conversationID: crewId,
                currentUserId: localUserId,
                groupSender: sender,
                // BubbleView 的附件区被 `serverURL != nil` 门禁挡着；本地模式
                // （无 imageAuth）附件 url 是 file:// 绝对 URL、渲染端不用
                // serverURL —— 给个占位兜底让本地附件也能渲染（Todo #3）。
                serverURL: appModel.imageAuth?.baseURL ?? URL(string: "file:///")!,
                onRetry: nil,
                onMentionSender: mentionStaged.map { staged in { _ in appendMentionFromAvatar(staged) } },
                // MarkdownUI 内部是一段一个 Text/selection overlay，跨 block 的拖选会在
                // sibling 边界截断。整条复制因此必须是气泡菜单的一等入口；而且要接在
                // BubbleView 自己的内层 contextMenu，外层菜单会被它优先命中。
                menu: { CrewMessageContextMenuContent(
                    text: msg.content,
                    onReply: { beginReply(to: entry) }
                ) }
            )
            // #443 病根 3：文本选中只给指针底下那一条 —— 常开会让窗口内每段文字都挂
            // 一个真 NSTextField（`SelectionOverlay`），0.1.7 那份 68.68s hang 的头号
            // 热点就是它们的 updateNSView。见 CrewBubbleTextSelection.swift。
            let bubble = Group {
                if let replyRef {
                    VStack(alignment: msg.mine ? .trailing : .leading, spacing: 2) {
                        replyQuoteStrip(replyRef)
                        bubbleBody
                    }
                    .frame(maxWidth: .infinity, alignment: msg.mine ? .trailing : .leading)
                } else {
                    bubbleBody
                }
            }
            .modifier(BubbleTextSelection(owner: selectionOwner, id: entry.id))
            #if os(macOS)
            // session 发的消息可点 → 右栏切到对应 session（chunk2 T6 中栏跳转）。
            // 找不到对应 run（已移除/别的 crew）则点击无事发生。
            if let senderSessionId = entry.senderSessionId {
                bubble
                    .contentShape(Rectangle())
                    .onTapGesture {
                        if let run = sessionRunner.runs.first(where: { $0.sessionId == senderSessionId }) {
                            onOpenSession?(run.runID)
                        }
                    }
                    .modifier(MessageContextMenu(text: msg.content) { beginReply(to: entry) })
            } else {
                bubble
                    .modifier(MessageContextMenu(text: msg.content) { beginReply(to: entry) })
            }
            #else
            bubble
                .modifier(MessageContextMenu(text: msg.content) { beginReply(to: entry) })
            #endif
        }
    }

    /// 渲染窗口顶部的「加载更早的消息」（#443）。一条都没丢，点一下往前放一页。
    private func loadEarlierRow(_ proxy: ScrollViewProxy) -> some View {
        let remaining = CrewChatWindow.remaining(
            total: timelineEntries.count, limit: renderLimit)
        return HStack {
            Spacer(minLength: 0)
            Button {
                expandEarlier(proxy)
            } label: {
                Text("加载更早的 \(min(remaining, CrewChatWindow.pageSize)) 条（上面还有 \(remaining) 条）")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(
                        Capsule().fill(Theme.Palette.surfaceMuted)
                    )
            }
            .buttonStyle(.plain)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 8)
    }

    /// 点「加载更早」：往前放一页，**并把展开前视口顶上那条钉回顶部**（Todo #60）。
    ///
    /// ## 为什么非补这一记不可
    ///
    /// 人往上翻看历史时 `isFollowing` 已经松开，`ChatScrollAnchor` 把尺寸变化锚在
    /// **内容顶端**（Todo #47 行为 3：新消息在**下面**长时视口一动不动）。而「加载更早」
    /// 是内容在**上面**长 —— 同一个锚点对这两个方向要求正好相反：锚顶端 = 他正在读的
    /// 那段整体下移一页的高度，视口没跟，于是看到的是新那一页的开头。这不是「忘了补偿」，
    /// 是那个 anchor 策略对这个方向本来就是错的。
    ///
    /// 另外第一次点击（12→24）还额外撞一次容器身份翻面：`usesEagerInitialLayout` 的判据
    /// 是 `limit <= pageSize`，那一下 `if VStack / else LazyVStack` 分支翻面，SwiftUI 视图
    /// 身份变了、整棵内容树重建，滚动位置被彻底重置。锚点靠的是同一棵树的连续性，树重建了
    /// 就没了 —— **在翻面被保留下来的前提下**，只有程序化 `scrollTo` 能按住这一下。
    ///
    /// ## 选型：选了 scrollTo，代价是翻面被留着；anchor 那条**没有被证伪**
    ///
    /// 另一条路是「这一拍把 `.sizeChanges` 的锚临时翻成 `.bottom`」：内容在上面长、锚底部
    /// = 视口一像素不动，是原生机制，而且**对懒容器估算高度回填天然免疫**（上面那些行真实
    /// 高度陆续回填时也一样锚底部）—— 这一条恰恰是本实现**不**免疫的地方：`scrollTo` 只在
    /// 点击那一拍把位置钉一次，之后高度再变就没人管了。
    ///
    /// 本次没走它，但**理由不是「它不行」**，记录别写歪（2026-08-23 机长指出）：那条路要成立
    /// 得先用单独一笔提交**把容器身份翻面消灭掉**（跟随底部时边界照常漂、用户滚上去期间冻结、
    /// 回到底部再同步；解冻判据就是现成的 `isFollowing`），而本次**没做这一步**。拿「翻面还
    /// 存在」去否掉 anchor 是循环论证 —— 前提本可以先被消灭。所以事实是：**anchor + 消灭翻面
    /// 这条组合从未被评估过，它没有被证伪。**
    ///
    /// 落地这条真实存在的代价：翻面留着；锚点只在点击那一拍钉一次，懒行回填期间会不会漂
    /// **没有被验证过**。这条尾巴与「未走的路」都记在 `docs/tech-debt.md`；真出问题时正确的
    /// 方向是走那条未走的路，而不是给这里加补丁。
    ///
    /// ## 为什么这不是一根新柴火
    ///
    /// 触发源是**用户点一下**这个离散事件，不是滚动位置 —— 没有「滚到顶→加载→改滚动
    /// 位置→再触发加载」那条自激边（`CrewChatWindow` 顶部讲的配方）。这里只排一次主线程
    /// hop、只滚一次、滚完就完，不留定时器 / 不反复重排（`CrewChatView` 时间线那段的
    /// 「不许加定时器 / asyncAfter / 每帧反复滚」说的是**兜底**柴火，这一记不是兜底，是
    /// 点击的直接后果）。hop 是必需的：`renderLimit` 写下去那一刻新的一页还没进视图树，
    /// 第一次点击那棵树甚至还没重建完，同一拍里 `scrollTo` 抓不到重建后的目标。
    ///
    /// 不加动画 —— 动画期间锚点会被后续 body 更新打断，而这一记要的是「位置本来就没变」，
    /// 不是「看着它移过去」（同 `landAtBottom` 里 `animated: false` 那一路的理由）。
    private func expandEarlier(_ proxy: ScrollViewProxy) {
        // 判断收口在 CrewChatWindow（可测）：还跟着底部时返回 nil —— 那时尺寸变化锚的是
        // 底部，视口本来就不动，再滚一记反而把人从底部拽走，还要和 landAtBottom 抢同一拍。
        let anchorID = CrewChatWindow.anchorOnExpand(
            windowedEntries, limit: renderLimit, isFollowing: bottomPin.isFollowing)?.id
        renderLimit = CrewChatWindow.expanded(renderLimit, total: timelineEntries.count)
        guard let anchorID else { return }
        DispatchQueue.main.async { proxy.scrollTo(anchorID, anchor: .top) }
    }

    /// 空态。默认只有一个聊天图标（人类明确要过：空白态别堆文案）。
    /// **但筛选开着时必须说清楚**（Todo #61）——否则「筛掉了所有消息」和「这个群
    /// 一条消息都没有」长得一模一样，人会以为聊天记录没了。
    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: onlyMentions ? "at.circle" : "bubble.left.and.bubble.right")
                .font(.system(size: 40))
                .foregroundStyle(.tertiary)
            if onlyMentions {
                Text("这个群里没有 @ 你的消息")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - data

    /// 事件驱动订阅（Phase 5：去 3s 轮询）。建立订阅前先 refresh 一次兜住订阅前
    /// 的状态，再 `for await` backend 的白板变更流 —— 每个 tick refresh 一次。
    /// 流由 backend 在远端重连后会续推（hub client 自带退避重连），所以无需本地
    /// 定时兜底。`.task(id: crewId)` 切 crew / 视图消失时取消本 Task，AsyncStream
    /// 的 `onTermination` 随之退订上游（Local 退 Combine / Edge 关 hub）。
    private func subscribe() async {
        entries = []
        // 切 crew：渲染窗口归位到一页，别把上一个 crew 翻开的深度带过来（#443）。
        renderLimit = CrewChatWindow.pageSize
        // 切 crew：跟随/未读也归位 —— 进一个新群就该停在它的最新一条、未读从 0 算起
        // （Todo #47 边界口径）。两个宿主都带 `.id(crewId)`、视图本会重建，这里是
        // 「万一哪天 .id 掉了也别把上一个群的『用户滑走了』带过来」的显式兜底。
        bottomPin = CrewChatBottomFollow.Pin()
        guard let backend = appModel.backend else {
            loadError = "未配置 backend"
            return
        }
        await refresh()
        for await _ in backend.whiteboardChanges(crewId: crewId) {
            if Task.isCancelled { return }
            await refresh()
        }
    }

    private func refresh() async {
        // 走 backend(双轨):登录态 → edge;BYOK → 本地白板 store。
        guard let backend = appModel.backend else {
            loadError = "未配置 backend"
            return
        }
        // #443：内容没变就一个 @State 都别碰。重拉整板廉价，重排不廉价 ——
        // 往 @State 重新赋值必然让 body 失效、LazyVStack 全量重新测量整条消息
        // 列表（现场就是主线程 100% busy 卡在这里）。判定收口在 CrewChatRefreshGate。
        do {
            let loaded = try await backend.listCrewWhiteboard(crewId: crewId)
            if let fresh = CrewChatRefreshGate.changed(current: entries, fresh: loaded) {
                entries = fresh
            }
            if loadError != nil { loadError = nil }
        } catch {
            let message = error.localizedDescription
            if loadError != message { loadError = message }
        }
        // Roster is best-effort decoration — a failure leaves the prior strip.
        if let roster = try? await backend.listCrewMembers(crewId: crewId) {
            // 单一排序真值（#15）：新建的 session 在前，机长/人类保持置顶。
            let sorted = CrewMemberOrdering.sortedMembers(
                roster.members, captainBotId: roster.captainBotId)
            if let fresh = CrewChatRefreshGate.changed(
                current: captainBotId, fresh: roster.captainBotId) {
                captainBotId = fresh
            }
            if let fresh = CrewChatRefreshGate.changed(current: members, fresh: sorted) {
                members = fresh
            }
        }
    }

    private func send() async {
        #if os(macOS)
        if todoMode {
            await sendTodo()
            return
        }
        #endif
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let toUpload = pendingAttachments
        // Allow attachment-only sends (no text) as long as something is present.
        guard !text.isEmpty || !toUpload.isEmpty,
              let backend = appModel.backend else { return }
        sending = true
        defer { sending = false }
        do {
            var attachmentIds: [String] = []
            var localAttachments: [LocalWhiteboardAttachment] = []
            #if os(macOS)
            // Todo #3：macOS 恒 LocalBackend —— 附件落盘 app 数据目录
            // （attachments/<crewId>/），挂到本地白板消息上，气泡从本地文件渲染。
            //
            // 落盘规则（搬 vs 写、原名留不留）收口在 `CrewLocalAttachmentPersist`——
            // 群聊、新建 Todo、Todo 追问三条入口共用同一份，图都进同一个目录。
            do {
                localAttachments = try CrewLocalAttachmentPersist.persist(
                    toUpload, crewId: crewId)
            } catch {
                loadError = error.localizedDescription
                return
            }
            #else
            // 附件上传是登录后的能力叠加(走 edge 上传)：未登录 → 软报错、不发。
            // 上传走 loggedAPIClient(edge 专属),拿到 ids 再交给 backend 发文。
            if !toUpload.isEmpty {
                guard appModel.isAuthenticated, let api = try? appModel.loggedAPIClient() else {
                    loadError = "群聊附件需登录 PendingBot"
                    return
                }
                for att in toUpload {
                    // 拖入的文件到这一步才读字节（edge 上传要整份 body）。
                    // intake 的本地上限在 iOS 上就等于 edge 的 25 MiB，所以这里
                    // 不会读进一份注定 413 的大文件。
                    guard let data = att.loadDataForUpload() else {
                        loadError = "附件读取失败：\(att.filename)"
                        return
                    }
                    let id = try await api.uploadAttachment(
                        data: data, filename: att.filename, mime: att.mime)
                    attachmentIds.append(id)
                }
            }
            #endif
            let mentions = mentionsForSend()
            let replyToId = replyTarget?.replyToId
            try await backend.postCrewMessage(
                crewId: crewId, text: text, mentions: mentions,
                attachmentIds: attachmentIds, replyToId: replyToId,
                localAttachments: localAttachments)
            // Local-first @ wake: after the message lands on the (local)
            // whiteboard, directly inject it into any idle local run that was
            // @-mentioned — idle runs have no next turn to pull it in (Phase 6
            // 单元 3). No-op for the edge backend (那走 Phase 4b hub→inbox→waker).
            // Todo #3：注入文本追加每个附件的绝对路径提示行（claude Read 即可看图）。
            let injectText = ([text] + localAttachments.map(\.agentHint))
                .filter { !$0.isEmpty }
                .joined(separator: "\n")
            injectMentionedLocalRuns(mentions: mentions, text: injectText)
            // 自己说话就把视线带回最新一条，且不给自己发的话记未读（Todo #47 边界口径，
            // 与 iMessage / Slack / 微信一致）。哪怕人刚才正在往回翻，主动发言也是
            // 「我要回到现场」的明确意图。
            bottomPin.didSendOwnMessage()
            draft = ""
            // 发出去了 —— 暂存区清场（macOS 走 move 时文件已经搬走，这里收掉空壳；
            // iOS 走读字节上传，这里才是真正删掉那份副本的地方）。
            CrewLocalAttachmentPersist.discardStaging(toUpload)
            pendingAttachments = []
            stagedMentions = []
            replyTarget = nil
            activeMentionQuery = nil
            CrewComposerDraftStore.shared.clear(crewId) // 发出去了就不是草稿了
            await refresh()
        } catch {
            loadError = error.localizedDescription
        }
    }

    #if os(macOS)
    /// Todo 模式发送（task #487；原 TodoInspectorPanel.addTodo 挪来）：走
    /// `LocalTodoStore` 既有添加路径拿 #N → 群里发「To do +1: #N xxx」（人类
    /// 身份、无 @）→ 复用下面「无 @ 默认给机长」的注入/拉起路径唤醒机长认领。
    /// **附件参与**（Todo #52）：托盘里的图/文件与群消息走同一条落盘路径，挂到条目
    /// 上、也挂到群里那条「To do +1」上。@ 不参与（todo 没有收件人）；发完 Todo 模式
    /// 保持 sticky（Todo #6），只有用户手动再点 Todo 钮才熄灭。
    private func sendTodo() async {
        let toUpload = pendingAttachments
        guard let text = TodoListPresentation.newTodoText(
                draft: draft, attachmentCount: toUpload.count,
                allImages: toUpload.allSatisfy(\.isImage)),
              let backend = appModel.backend else { return }
        sending = true
        defer { sending = false }
        do {
            // 图先落盘（与群聊同一个 attachments/<crewId>/ 目录、同一套命名）。
            let saved = try CrewLocalAttachmentPersist.persist(toUpload, crewId: crewId)
            // 落账 + 回执 + 唤醒机长的编排在 `CrewLocalTodoLanding`（Task 10 抽出，
            // 与 relay 落地远端 crew_todo_add 共用）。
            _ = try await CrewLocalTodoLanding.land(
                crewId: crewId, text: text, attachments: saved,
                backend: backend, sessionRunner: sessionRunner,
                onError: { loadError = $0 })
            // 建 Todo 也会往群里发一条「To do +1: #N」—— 同样是自己说话（Todo #47）。
            bottomPin.didSendOwnMessage()
            draft = ""
            // Todo #6: 发送后 **不** 熄灭 Todo 模式 —— 点亮后保持 sticky，连发多条
            // Todo 不用每次重点，只有用户手动再点 Todo 钮才灭。placeholder 也随之
            // 持续显示「新建 Todo」直到手动熄灭。
            CrewLocalAttachmentPersist.discardStaging(toUpload)
            pendingAttachments = []
            stagedMentions = []
            activeMentionQuery = nil
            CrewComposerDraftStore.shared.clear(crewId)
            await refresh()
        } catch {
            loadError = error.localizedDescription
        }
    }
    #endif

    /// Local直投唤醒（Phase 6 单元 3）：对每个 `@session` 目标，找到对应的本地
    /// 活跃 run，空闲就直接 `send(...)` 唤醒；busy 不打断。决策 + IO 编排都在
    /// `CrewLocalMentionDelivery`（Task 10 抽出，供 relay 落地远端 todo_add 共用）；
    /// 这里只是把本视图的 environment（sessionRunner / backend / loadError）接进去。
    /// 仅 macOS（本地 run 在 macOS）。
    private func injectMentionedLocalRuns(mentions: [CrewMention], text: String) {
        #if os(macOS)
        // 项10:人发的、无具体 @ 的群消息 → 默认当 @机长 处理,且注入文本用 IM 式
        // 「名：正文」(不套「有人@你」壳,因为不是定向 @)。只兜底真广播(mentions
        // 为空);带 reply 会自动 @ 原发送者故非空,不落此路径。这里的 send() 只在
        // 人类经 composer 发送时触发 → 天然满足「人发的」边界(session 广播 / 系统
        // 消息不走此路径,不会误唤醒机长)。
        CrewLocalMentionDelivery.injectAndWake(
            crewId: crewId, mentions: mentions, text: text, senderName: "人",
            sessionRunner: sessionRunner, backend: appModel.backend,
            onError: { loadError = $0 })
        #endif
    }

    private func answer(_ reqId: String) async {
        let reply = (replyDrafts[reqId] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !reply.isEmpty, let api = try? appModel.loggedAPIClient() else { return }
        do {
            try await api.answerInteraction(reqId: reqId, reply: reply)
            replyDrafts[reqId] = nil
            await refresh()
        } catch {
            loadError = error.localizedDescription
        }
    }

    // MARK: - File import (cross-platform via .fileImporter)

    /// Called by the `.fileImporter` modifier bound to `showFileImporter`.
    /// Works on both macOS (Finder sheet) and iOS (document picker).
    ///
    /// 与拖拽**共用** `CrewFileAttachmentIntake.intake` —— 大小闸门、目录/package
    /// 拒收、mime 推导、复制进暂存区全在那一份里。别在这儿写第二条并行的路。
    private func handleImport(_ result: Result<[URL], Error>) {
        switch result {
        case .failure(let error):
            loadError = error.localizedDescription
        case .success(let urls):
            Task {
                // 复制可能是几百 MB，挪出主线程。
                let intake = await Task.detached(priority: .userInitiated) {
                    CrewFileAttachmentIntake.intake(urls: urls)
                }.value
                await apply(intake)
            }
        }
    }

    // MARK: - ⌘V paste (Todo #3 / Todo #9)

    /// ComposerView 的粘贴拦截闭包：读剪贴板 → 交给纯逻辑判定 → 收进托盘。
    /// 返回 true = 已收下、吞掉本次 paste；false = 走默认文本粘贴。
    private var pasteAttachmentsHandler: (() -> Bool)? {
        return {
            // 读剪贴板收口在 `CrewPasteboardReader`（Todo 追问的粘贴走同一份）。
            let items = CrewPasteboardReader.attachments()
            guard !items.isEmpty else { return false }
            pendingAttachments.append(contentsOf: items)
            return true
        }
    }

    // MARK: - Photo picker (iOS)

    #if os(iOS)
    /// Decode PhotosPickerItems into PendingComposerAttachments and append.
    @MainActor
    private func handlePhotoItems(_ items: [PhotosPickerItem]) async {
        for item in items {
            guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
            // Best-effort MIME detection from the supported content types.
            let mime: String
            if let types = item.supportedContentTypes.first,
               let mimeType = types.preferredMIMEType {
                mime = mimeType
            } else {
                mime = "image/jpeg"
            }
            // Generate a filename from a timestamp so the tray shows something readable.
            let ext = mime.components(separatedBy: "/").last ?? "jpg"
            let filename = "photo_\(Int(Date().timeIntervalSince1970)).\(ext)"
            pendingAttachments.append(PendingComposerAttachment(
                filename: filename,
                mime: mime,
                data: data
            ))
        }
        // Clear selection so the same photo can be re-picked if needed.
        photoItems = []
    }

    /// 「拍照」拍回来的一张图 → 附件托盘。JPEG 编码（相机原图是位图，PNG 会大一个
    /// 量级）；编不出来就软报错，不静默丢。
    private func handleCameraImage(_ image: PlatformImage) {
        defer { cameraImage = nil }
        guard let data = image.jpegData(quality: 0.85) else {
            loadError = "照片编码失败，没有加进附件。"
            return
        }
        pendingAttachments.append(PendingComposerAttachment(
            filename: "photo_\(Int(Date().timeIntervalSince1970)).jpg",
            mime: "image/jpeg",
            data: data))
    }
    #endif
}

/// 把「用户自己滑走了 / 滑回底部了」记进 `CrewChatBottomFollow.Pin`（Todo #45 → #47）。
///
/// 只在**滚动相位回到 idle** 那一刻读几何、写一次状态 —— 不是每帧写。滚动过程中反复写
/// @State 会把布局搅进「改状态→重解析→再改」的环里，正是这屏历史上出过事的形状。
///
/// ## 这里就是 Todo #45 修法失效的那一行（Todo #47 查清）
///
/// 原来写的是 `onScrollPhaseChange { _, phase, _ in ... }` —— **把 oldPhase 丢了**，
/// 于是「用户滑上去停住」和「我们自己 `scrollTo` 的那一下收尾」走同一条路。
/// `LazyVStack` 在程序化滚动收尾的那一瞬往往还在用估算行高、内容高度随后才被修正，
/// 那一刻读到的几何就是「没到底」→ 跟随被自己刚要修的那件事关掉 → 此后每一记
/// `landAtBottom` 全部空转。修法把自己的保险丝烧了，人类看到的就是「一点没修」。
///
/// 所以现在把 oldPhase 传进 `settleIsUserDriven`：程序化收尾只许把跟随**打开**
/// （停在底部时），永远不许关掉。
///
/// 读几何要 `onScrollPhaseChange`（macOS 15 / iOS 18）。更老的系统上没有这条信息，
/// 退回「永远跟随」= Todo #45 之前的行为，不会更差（部署下限是 macOS 14 / iOS 17）。
private struct BottomPinTracker: ViewModifier {
    @Binding var pin: CrewChatBottomFollow.Pin
    /// 相位标志写进引用型盒子，**不写 @State** —— 手势刚开始那一下让 body 失效
    /// 会把整条列表重新测一遍（#443 的热点）。见 `ScrollPhaseBox` 顶部。
    let phaseBox: CrewChatBottomFollow.ScrollPhaseBox

    func body(content: Content) -> some View {
        if #available(macOS 15.0, iOS 18.0, *) {
            content.onScrollPhaseChange { oldPhase, phase, context in
                // 先记「手在不在滚动上」—— `BottomOnContentGrowth` 靠它避开
                // 「人正往上翻、内容一长高就被拽回底部」（Todo #54）。
                phaseBox.phaseChanged(to: Self.kind(phase))
                guard phase == .idle else { return }
                let geo = context.geometry
                pin.settled(
                    atBottom: CrewChatBottomFollow.isAtBottom(
                        contentOffsetY: geo.contentOffset.y,
                        containerHeight: geo.containerSize.height,
                        contentHeight: geo.contentSize.height,
                        insetTop: geo.contentInsets.top,
                        insetBottom: geo.contentInsets.bottom),
                    byUser: CrewChatBottomFollow.settleIsUserDriven(
                        previous: Self.kind(oldPhase)))
            }
        } else {
            content
        }
    }

    /// SwiftUI `ScrollPhase` → 可测的中立镜像。
    @available(macOS 15.0, iOS 18.0, *)
    private static func kind(_ phase: ScrollPhase) -> CrewChatBottomFollow.ScrollPhaseKind {
        switch phase {
        case .idle: return .idle
        case .tracking: return .tracking
        case .interacting: return .interacting
        case .decelerating: return .decelerating
        case .animating: return .animating
        // 认不出的新相位一律当「不是用户滑的」—— 与本文件的不变式同向：宁可少关一次
        // 跟随，也不要再出一次「跟随被自己关掉」。
        @unknown default: return .idle
        }
    }
}

/// 真实滚动位置到达底部就清未读（Todo #56）。
///
/// 原实现只在 `phase → idle` 时读一次几何；人已经滑到底但那次相位回调没覆盖到最终位置，
/// `Pin` 仍停在「不跟随 + 有未读」，按钮就一直不消失。这里直接观察 at-bottom 这个 Bool：
/// 滚动期间不逐帧写状态，只在 false/true 跨阈值时收到回调；且 false 什么都不做，离开底部
/// 仍只由明确的用户滚动相位关闭跟随，守住 Todo #47 的程序化滚动不许关保险丝不变式。
private struct BottomReachedTracker: ViewModifier {
    @Binding var pin: CrewChatBottomFollow.Pin

    func body(content: Content) -> some View {
        if #available(macOS 15.0, iOS 18.0, *) {
            content.onScrollGeometryChange(for: Bool.self) { geo in
                CrewChatBottomFollow.isAtBottom(
                    contentOffsetY: geo.contentOffset.y,
                    containerHeight: geo.containerSize.height,
                    contentHeight: geo.contentSize.height,
                    insetTop: geo.contentInsets.top,
                    insetBottom: geo.contentInsets.bottom)
            } action: { _, atBottom in
                guard atBottom else { return }
                pin.reachedBottom()
            }
        } else {
            content
        }
    }
}

/// 首屏落底的**第三个执行者**（Todo #54）：内容真实高度长高了就补一记落底。
///
/// ## 为什么非要有它
///
/// #47 之后跟随开关不再被自己关掉了，人类却仍报「有时落最新、有时落最早」。查下来
/// 不是跟随的问题，是**那两记落底跑得太早**：`PendingCrewBackend` 是 `@MainActor`
/// 协议，`LocalPendingCrewBackend.listCrewWhiteboard` / `listCrewMembers` 方法体里
/// 一次 `await` 都没有（纯内存读 store），于是 `refresh()` 里 `entries` 与
/// `members`/`captainBotId` 两次 `@State` 写入之间从不让出主线程 —— SwiftUI 把它们
/// 合并成同一次 body 更新，`onChange(of: count)` 与 `onChange(of: bubbleLayoutToken)`
/// 背靠背触发，两记 `scrollTo(tail, anchor: .bottom)` 手里都只有 `LazyVStack` 的
/// **估算行高**。估算按整屏算不满时目标偏移就是 0 —— 那正是人类看到的「停在最早那条」。
///
/// 「有时又对」= 这一趟碰巧多了一记晚到的落底（crew 里正好有 session 在跑，
/// `CrewTypingIndicatorRow` 冒出来会回调落底；或第二波数据真晚了一拍）。那一记发生在
/// 量完之后，落点就对。所以表现像随机，其实是「落底跑在测量前还是测量后」。
///
/// ## 为什么这不是又一根「兜底柴火」
///
/// 判定收口在 `CrewChatBottomFollow.shouldLandOnContentGrowth`（可测），三条守则：
/// 只认**长高**（回缩不动 → 我们自己的滚动无法经由「缩」这条边回头触发自己）、
/// 用户手在滚动上时不动、不跟随时不动。渲染窗口内行数有限，量完高度就不再变，自己停。
/// 落底用 `animated: false` —— 这是把偏移量补正到量完之后的底部，不是「来消息了跟着走」，
/// 动画反而像画面自己在抖；也不给动画图留任何永不结束的事务（2026-07-26 事故的那条边）。
///
/// macOS 15 / iOS 18 起才有 `onScrollGeometryChange`；更老的系统退回 #47 的行为
/// （和 `ChatScrollAnchor` / `BottomPinTracker` 同一条下限线）。
private struct BottomOnContentGrowth: ViewModifier {
    let isFollowing: Bool
    let phaseBox: CrewChatBottomFollow.ScrollPhaseBox
    let land: () -> Void

    func body(content: Content) -> some View {
        if #available(macOS 15.0, iOS 18.0, *) {
            content.onScrollGeometryChange(for: CGFloat.self) { geo in
                geo.contentSize.height
            } action: { old, new in
                guard CrewChatBottomFollow.shouldLandOnContentGrowth(
                    oldHeight: old,
                    newHeight: new,
                    isFollowing: isFollowing,
                    isUserScrolling: phaseBox.isUserScrolling)
                else { return }
                land()
            }
        } else {
            content
        }
    }
}

/// 滚动锚点（Todo #47 行为 2/3）。
///
/// - 开屏落点恒为底部（行为 1 的原生那一半；程序化落底是另一半）。
/// - 内容变高时的对齐**跟着跟随开关走**：还在跟随 → 锚底部，新消息进来画面自然跟着
///   走；已经松开 → 锚顶部，内容在下面长、视口**一动不动**（行为 3 要的「不许自动
///   跳到底部」）。
///
/// 不写成一句 `.defaultScrollAnchor(.bottom)` 是因为那把「开屏落点」和「内容变高时
/// 怎么对齐」绑成同一个值 —— 而这两件事在行为 3 下要求相反。角色化的那个重载是
/// macOS 15 / iOS 18 起才有的，更老的系统退回旧写法（行为 3 在那里保证不了，已记
/// tech-debt）。
private struct ChatScrollAnchor: ViewModifier {
    let isFollowing: Bool

    func body(content: Content) -> some View {
        if #available(macOS 15.0, iOS 18.0, *) {
            content
                .defaultScrollAnchor(.bottom, for: .initialOffset)
                .defaultScrollAnchor(isFollowing ? .bottom : .top, for: .sizeChanges)
        } else {
            content.defaultScrollAnchor(.bottom)
        }
    }
}

/// 整条消息菜单的内容。BubbleView 内层与含引用条的外层共用这一份：内层保证右键正文
/// 不会被 BubbleView 自己的 contextMenu 截成空菜单，外层让引用条空白区域也能命中。
private struct CrewMessageContextMenuContent: View {
    let text: String
    let onReply: () -> Void

    var body: some View {
        Button {
            CrewMessageClipboard.copy(text)
        } label: {
            Label("复制整条", systemImage: "doc.on.doc")
        }
        Button {
            onReply()
        } label: {
            Label("回复", systemImage: "arrowshape.turn.up.left")
        }
    }
}

/// macOS 右键 / iOS 长按共用的菜单壳。
private struct MessageContextMenu: ViewModifier {
    let text: String
    let onReply: () -> Void

    func body(content: Content) -> some View {
        content.contextMenu {
            CrewMessageContextMenuContent(text: text, onReply: onReply)
        }
    }
}

private enum CrewMessageClipboard {
    @MainActor
    static func copy(_ text: String) {
        #if os(macOS)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        #else
        UIPasteboard.general.string = text
        #endif
    }
}

// MARK: - Todo #69：@-候选浮层限高用的两把尺

/// 群聊那一栏的总高（窗口给这一栏多少就是多少）。
private struct CrewChatColumnHeightKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}

/// composer 那一截（回复横幅 + 输入胶囊）的高 —— **不含** @-候选浮层本身。
private struct CrewComposerCoreHeightKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}

private extension View {
    /// 量「群聊这一栏有多高」。用 `.background` 而不是包一层 GeometryReader ——
    /// GeometryReader 会把子视图按左上对齐并吃掉它的理想尺寸，包在外面等于改布局。
    func measuringChatColumnHeight() -> some View {
        background(
            GeometryReader { geo in
                Color.clear.preference(key: CrewChatColumnHeightKey.self, value: geo.size.height)
            })
    }

    /// 量「composer 那一截有多高」。同上。
    func measuringComposerCoreHeight() -> some View {
        background(
            GeometryReader { geo in
                Color.clear.preference(key: CrewComposerCoreHeightKey.self, value: geo.size.height)
            })
    }
}
