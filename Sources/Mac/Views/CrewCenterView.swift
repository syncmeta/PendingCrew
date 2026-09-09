#if os(macOS)
import SwiftUI

/// 中栏（spec v2 §9）：选中 crew 的**群聊会话页**（白板 + 沟通渠道）。
///
/// crew 元数据 / 机长 / DAG / 责任比例从这里搬到了 `CrewDetailInspector`
/// (顶栏 ⓘ 打开)。session 终端不再常驻,收进按需 inspector（`CrewSessionWindowView`,
/// 由 MacThreePaneView 挂在 .inspector），从 toolbar 终端开关弹出。
struct CrewCenterView: View {
    @EnvironmentObject private var crewStore: CrewStore
    @EnvironmentObject private var sessionRunner: CrewSessionRunner
    /// 驾驶舱开关位的**写句柄**（人类 Todo #96）。`@Environment` 取一个 class 值
    /// **不订阅**它的 `objectWillChange` —— 中栏只按按钮，不需要知道驾驶舱开着没有。
    /// 换成 `@EnvironmentObject` 会让开关驾驶舱重新把整条中栏（连着群聊）作废。
    @Environment(\.cockpitPresentation) private var cockpitPresentation
    @State private var showingDetail = false
    /// chunk2 T5（captain 唤醒=app 注入）：已通知过 captain 的 decision id ——
    /// 防止重复注入同一条。**放在常驻中栏**（而非按需 inspector），captain 编排
    /// 不能依赖 session 终端面板是否打开。
    @State private var notifiedDecisionIds: Set<String> = []
    /// 「只看 @ 我的消息」（Todo #61 立、#128 改成默认点亮）。开关钮在 toolbar 上，
    /// 状态喂给 `CrewChatView` 的时间线。**放在这里而不是 chat 里面**：
    /// `CrewChatView` 带 `.id(crewId)`，切 crew 会整个重建 —— 状态放里面就没法从
    /// toolbar 驱动它。切 crew 时下面显式归位（筛选状态不跨群带走）。
    @State private var onlyMentions = CrewMentionFilter.defaultOnlyMentions
    @State private var searchQuery = ""
    @State private var searchTargetMessageId: String?

    var body: some View {
        Group {
            if let detail = crewStore.selectedDetail {
                CrewChatView(
                    crewId: detail.crew.id,
                    crewTitle: detail.crew.title,
                    onOpenSession: { runId in
                        // 右栏常驻；点 session 头像只切到「终端」模式 + 选中该 run。
                        sessionRunner.select(runId)
                        sessionRunner.viewingTerminal = true
                    },
                    onNewSession: {
                        sessionRunner.composeNew()
                        sessionRunner.viewingTerminal = true
                    },
                    showOnlyHumanMentions: $onlyMentions,
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
                .id(detail.crew.id)
            } else if let id = crewStore.selectedCrewId, crewStore.loadingDetailIds.contains(id) {
                ProgressView("加载 crew 详情…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if crewStore.selectedCrewId != nil {
                empty("详情加载中…")
            } else {
                placeholder
            }
        }
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
        // 标题走原生 navigationTitle —— 位置/收合行为和系统窗口标题完全一致（不会跑到
        // 红绿灯后面、收 sidebar 也不冒第二个标题），纯文字无胶囊。操作按钮放 trailing
        // （macOS 26 自动给原生白色液态玻璃岛 + 阴影）。
        //
        // 标题文字与按钮组都 gate 在 selectedCrewId（而非 selectedDetail）——后者在切换
        // crew 时会短暂为 nil（detail 异步加载），导致标题清空 + 工具栏项目消失再出现，
        // 表现为"切换后 toolbar 闪一下"。用 selectedCrewId（切换瞬间即非 nil）+ 列表里的
        // title 兜底，整条就稳定不抖。
        .navigationTitle(crewStore.selectedDetail?.crew.title ?? crewStore.selectedCrew?.title ?? "")
        .searchable(text: $searchQuery, placement: .toolbar, prompt: "搜索当前群")
        // 灰线/无缝由 WindowSeparatorRemover(标题栏透明 + 内容铺满到顶)统一处理。
        // **不能**在这里 .toolbarBackground 刷色：那是窗口级的，会连 sidebar 那半截 toolbar
        // 一起刷白，把侧栏顶部的半透明材质盖住（"toolbar 挡住 sidebar"）。透明标题栏让
        // sidebar 透出自己的侧栏材质、detail 透出白 canvas，两边各自对，互不打架。
        .toolbar {
            if let crewId = crewStore.selectedCrewId {
                ToolbarItem {
                    // Todo #79 当初把它钉在最右（`.primaryAction`）；#128 人类要它挪到
                    // 那三个按钮**左侧**，所以改成普通 ToolbarItem 并**声明在最前**
                    // —— toolbar 的排布跟声明顺序走。点亮色与发送键共用
                    // Theme.Palette.accent，不继承系统蓝。文字仍是人类钉死的「仅@你」四字。
                    Toggle(isOn: $onlyMentions) { Text("仅@你") }
                        .toggleStyle(.button)
                        .tint(Theme.Palette.accent)
                        .disabled(crewStore.selectedDetail == nil)
                        .help(onlyMentions
                              ? "正在只显示 @ 你的消息 + 你自己发的；点一下显示全部"
                              : "只显示 @ 你的消息 + 你自己发的")
                }
                ToolbarItem {
                    Button { showingDetail = true } label: {
                        Label("crew 详情", systemImage: "info.circle")
                    }
                    .disabled(crewStore.selectedDetail == nil)
                }
                ToolbarItem {
                    // 驾驶舱 = 叠在群聊之上的临时窗口（#542）；群聊这一栏不卸载，
                    // 关掉驾驶舱回来草稿和滚动位置原样在。关闭在驾驶舱左上角圆形叉。
                    //
                    // 原来旁边还有个「Todo」按钮直达 Todo 段（Todo #12）——Todo 已并进
                    // 任务段、右栏又常驻 Todo 面板，两个按钮开同一扇门，删掉一个。
                    Button { cockpitPresentation.open() } label: {
                        Label("驾驶舱", systemImage: "speedometer")
                    }
                    .disabled(crewStore.selectedDetail == nil)
                }
                ToolbarItem {
                    Button { Task { await crewStore.refreshDetail(crewId) } } label: {
                        Label("刷新", systemImage: "arrow.clockwise")
                    }
                }
                // 「Session 终端」开关已去掉 —— 右栏(成员/终端)在原生三栏里常驻;
                // 成员列表 ↔ 终端 的切换由右栏内部(viewingTerminal / 点 session)管。
            }
        }
        .sheet(isPresented: $showingDetail) {
            if let detail = crewStore.selectedDetail {
                CrewDetailInspector(detail: detail)
                    .environmentObject(crewStore)
                    // 「更改工作目录」要读在跑的 session（在跑就拒绝迁）。sheet 不继承
                    // 父视图的 environmentObject，得显式再喂一次。
                    .environmentObject(sessionRunner)
            }
        }
        // chunk2 T5：captain 唤醒 = app 注入。常驻中栏**事件驱动**订阅待决策（去 2s
        // 轮询）：app 侧答复 + helper 跨进程 raise(目录监听)都推一个 tick,有新的(非
        // captain 自己 raise 的)就把提示注入在跑的 captain PTY。不依赖 inspector 是否打开。
        // `.task(id:)` 随选中 crew 切换重建订阅;无选中 crew 时 crewId=nil,不订阅。
        // 切 crew：筛选归位（Todo #61 立，#128 改了归到哪儿）。
        //
        // **#61 当初归位到「关」，理由是**：换个群还挂着「只看 @ 我」，新群大概率筛成
        // 空的 —— 人看到的是一个空聊天页，会以为这个群没消息 / 加载失败。
        //
        // **#128 人类要「默认点亮」**，于是这里改成归到 `defaultOnlyMentions`（= 点亮）。
        // 归位本身保留 —— 那是 #61 真正的意思：筛选状态不跨群带走。
        //
        // ⚠️ **两条合起来 = 每次进群都是点亮的 = 正是 #61 当初要防的那种空**。
        // 这个冲突没有被消除，是被**接住**了：筛完一条不剩时，群聊空态会给一句
        // 「这个群里没有 @ 你的消息」和一颗「看全部」
        // （`CrewMentionFilter.showsClearFilterEscape` → `CrewChatView.emptyState`）。
        // 人类要的是默认看到跟自己有关的，不是要一个看起来坏掉的界面 ——
        // **动这里之前先确认那条出路还在**，没有它，这一段就退回 #61 描述的那个坑。
        .onChange(of: crewStore.selectedCrewId) { _, _ in
            onlyMentions = CrewMentionFilter.defaultOnlyMentions
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
        .task(id: crewStore.selectedCrewId) {
            guard let crewId = crewStore.selectedCrewId else { return }
            notifyCaptainOfNewDecisions()
            for await _ in LocalApprovalStore.shared.approvalChanges(crewId: crewId) {
                if Task.isCancelled { return }
                notifyCaptainOfNewDecisions()
            }
        }
    }

    /// 发现新待决策 → 注入运行中的 captain session（spec §2 唤醒=app 注入）。
    /// 没有在跑的 captain 就什么都不做（等 captain 启动后第一个 tick 再补通知）。
    private func notifyCaptainOfNewDecisions() {
        guard let crewId = crewStore.selectedDetail?.crew.id,
              let captainRun = sessionRunner.runs.first(where: {
                  $0.crewId == crewId && $0.role == .captain && $0.status == .running
              })
        else { return }
        let items = LocalApprovalStore.shared.pending(crewId: crewId).filter {
            $0.kind == "decision"
                && $0.sessionId != captainRun.sessionId
                && !notifiedDecisionIds.contains($0.id)
        }
        for item in items {
            captainRun.send("有新的待决策 #\(item.id)（来自 session \(item.sessionId)）：\(item.summary)\n能拍板就用 answer_decision 工具答它（reqId=\(item.id)）；拍不了就用 post_to_crew 发消息 @human 说明需要人类决策的理由与你的建议。")
            notifiedDecisionIds.insert(item.id)
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
        PendingCrewPlaceholderIcon(size: 64)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
#endif
