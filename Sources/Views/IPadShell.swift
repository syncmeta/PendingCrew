#if os(iOS)
import SwiftUI

/// iPhone / iPad 认证后的根视图 —— NavigationSplitView：
///   sidebar = CrewListView（crew 列表）
///   detail  = CrewChatView（所选群聊）
///
/// **iPhone（compact）不另起一套 shell**：NavigationSplitView 在 compact
/// 下会自己塌成一个 NavigationStack —— sidebar 当根，选中 crew 就把 detail
/// 推上去，返回键由系统给。所以「列表 → 群聊 → 返回」是原生行为，不用手糊
/// NavigationStack + navigationDestination。这里只补 compact 才需要的一件事：
/// 群聊页的标题（regular 下 detail 是并排的一列，加标题反而多一条空导航条，
/// 所以按 size class 分）。返回时的选中态复位由塌陷后的 NavigationStack 自己管。
///
/// 依赖注入清单（runtime crash 防护）：
///   - CrewListView 需要 @EnvironmentObject var crewStore: CrewStore
///   - CrewChatView 需要 @EnvironmentObject var appModel: AppModel
///     （sessionRunner 是 macOS-only，iOS 不需要）
/// 两者均由 WindowGroup → RootView 链路上的 .environmentObject(...) 注入，
/// IPadShell 自身声明同样的 @EnvironmentObject 让 SwiftUI 知道它用到了它们。
struct IPadShell: View {
    @ObservedObject private var personalService=CrewPersonalDraftService.shared
    @EnvironmentObject private var crewStore: CrewStore
    @EnvironmentObject private var appModel: AppModel
    /// compact = iPhone 竖屏 / 大多数 iPhone 形态；regular = iPad 与
    /// iPhone Max 横屏（那时两栏并排，行为与 iPad 一致）。
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass

    @State private var selectedCrewId: String?
    @State private var sidebarPath: [String] = []
    @State private var compactColumn = NavigationSplitViewColumn.sidebar
    @State private var showingPairing = false
    @State private var showingNewCrew = false
    @State private var didReadComputerList = false
    @State private var showingRecovery = false
    @State private var showingRetainedInputs = false
    @State private var showingOfflineDrafts = false
    @State private var showingLocalCodexInputs=false
    @State private var showingConnectionStatus=false
    @State private var selectedRemoteSessionID: String?
    @State private var communicationView: CrewTimelineFilter.CommunicationView = .human
    @State private var searchQuery=""
    @State private var searchScope = CrewSearchScope.current
    @State private var searchTargetMessageId:String?
    @State private var showingSettings=false

    private var isCompact: Bool { horizontalSizeClass == .compact }

    var body: some View {
        NavigationSplitView(preferredCompactColumn: $compactColumn) {
            NavigationStack(path: $sidebarPath) {
            CrewListView(connectComputer: canChooseComputer ? { showingPairing = true } : nil,
                         hasPairedComputer: hasPairedComputer,
                         readingComputers: !didReadComputerList || appModel.connectingAuthority,
                         showsComputerIntroduction: isCompact,
                         selection: $selectedCrewId)
                .navigationTitle("机组")
                .environment(\.crewdOpenSidebarConversations, { crew in
                    if selectedCrewId != crew { selectedCrewId = crew }
                    if let workspace = appModel.crewdBackend?.workspace { workspace.selectCrew(crew) }
                    sidebarPath = [crew]; compactColumn = .sidebar
                })
                .navigationDestination(for: String.self) { crew in
                    if let workspace = appModel.crewdBackend?.workspace {
                        CrewdSidebarConversations(workspace: workspace, crewID: crew,
                            openSession: { selectedRemoteSessionID = $0 },
                            showContent: { compactColumn = .detail })
                    }
                }
                .safeAreaInset(edge:.bottom){
                    HStack{
                        Button("设置",systemImage:"gearshape"){showingSettings=true}
                        Spacer()
                        if appModel.crewdBackend?.workspace != nil{Button("新建机组",systemImage:"plus"){showingNewCrew=true}}
                    }.padding(12)
                }

            }
        } detail: {
            detail
                .opacity(searchScope == .all ? 0:1)
                .allowsHitTesting(searchScope != .all)
                .accessibilityHidden(searchScope == .all)
                .overlay{if searchScope == .all{CrewGlobalSearchResults(query:searchQuery){searchScope = .current}}}
        }
        .searchable(text:$searchQuery,placement:.automatic,prompt:"搜索")
        .searchScopes($searchScope){Text("当前群").tag(CrewSearchScope.current);Text("全局 · 当前电脑").tag(CrewSearchScope.all)}
        .sheet(isPresented:$showingSettings,onDismiss:{let action=pendingSettingsAction;pendingSettingsAction=nil;action?()}){mobileSettings}
        .modifier(CrewdWindowMaterial())
        .onChange(of: selectedCrewId) { _, id in
            if let current = sidebarPath.last, current != id { sidebarPath = [] }
            crewStore.selectCrew(id) // selectCrew 是同步方法，接受 String?
            if crewStore.chatSearchRequest == nil{communicationView = .human;searchQuery="";searchTargetMessageId=nil}
            if let id, id != crewStore.pendingCrew?.id, let workspace = appModel.crewdBackend?.workspace {
                workspace.selectCrew(id)
            }
        }
        .onChange(of: crewStore.selectedCrewId) { _, id in
            if selectedCrewId != id { selectedCrewId = id; selectedRemoteSessionID = nil }
        }
        .onChange(of:crewStore.chatSearchRequest){_,request in
            guard let request else{return}
            selectedRemoteSessionID=nil
            communicationView = .agents
            searchQuery=request.query;searchTargetMessageId=request.messageId
            Task{@MainActor in await Task.yield();if crewStore.chatSearchRequest?.id == request.id{crewStore.chatSearchRequest=nil}}
        }
        .task {
            await appModel.connectCrewdAuthority()
            didReadComputerList = true
            await crewStore.refreshList()
        }
        .onChange(of: appModel.crewdBackend?.workspace.presentationNamespace) { _, _ in
            crewStore.retireOriginalAuthorityPresentation()
            selectedCrewId = nil; selectedRemoteSessionID = nil; sidebarPath = []
            Task { await crewStore.refreshList() }
        }
        .sheet(isPresented: $showingNewCrew, onDismiss: { Task { await crewStore.refreshList() } }) {
            if let workspace = appModel.crewdBackend?.workspace {
                CrewdCrewCreationView(workspace: workspace).id(workspace.presentationNamespace)
            }
        }
        .sheet(isPresented:$showingConnectionStatus){
            if let workspace=appModel.crewdBackend?.workspace{CrewMobileConnectionReadback(workspace:workspace)}
            else {
                NavigationStack {
                    ContentUnavailableView("后台尚未连接",systemImage:"wifi.exclamationmark",description:Text(appModel.backendConfigurationError ?? "请从电脑菜单核对连接。"))
                        .toolbar{ToolbarItem(placement:.cancellationAction){Button("关闭"){showingConnectionStatus=false}}}
                }
            }
        }
        .sheet(isPresented:$showingLocalCodexInputs){CrewdCodexLocalInputs(directory:appModel.crewdDirectory)}
        .sheet(isPresented: $showingOfflineDrafts) {
            IOSOfflineComposerDrafts(dataDirectory: appModel.crewdDirectory)
        }
        .sheet(isPresented: $showingRetainedInputs) {
            NavigationStack {
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        Text("以下只供查看原连接的输入，不会从此处提交或连接。")
                            .foregroundStyle(.secondary)
                        ForEach(Array(appModel.crewdWindow.inputReviewWorkspaces.enumerated()), id: \.offset) { _, workspace in
                            CrewdRemoteRetainedInputs(workspace: workspace)
                                .id(ObjectIdentifier(workspace))
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading).padding()
                }
                .navigationTitle("原连接的输入")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("关闭") { showingRetainedInputs = false } } }
            }
        }
        .sheet(isPresented: $showingRecovery) {
            if let workspace = appModel.crewdBackend?.workspace {
                ScrollView { CrewdPublicRecoveryView(workspace: workspace, openCrew: { crewStore.selectCrew($0) }).padding() }
            }
        }
        .sheet(isPresented: $showingPairing) {
            IOSRemotePairingView()
                .environmentObject(appModel)
                .environmentObject(crewStore)
        }
    }

    @ViewBuilder
    private var detail: some View {
        if let id = selectedCrewId, let title = crewTitle(for: id) {
            CrewChatView(
                crewId: id, crewTitle: title,
                onOpenRemoteSession: { selectedRemoteSessionID = $0 },
                communicationView:$communicationView,searchQuery:$searchQuery,searchTargetMessageId:$searchTargetMessageId)
                .modifier(CrewdOriginalWorkIntegration(workspace: crewStore.crewdWorkspace,
                    openSession: { selectedRemoteSessionID = $0 }))
                .environment(\.crewdMessageSearchTarget,searchTargetMessageId)
                .id((appModel.crewdBackend?.workspace.presentationNamespace ?? "unconnected") + "/" + id + "/" + personalService.projectionID.uuidString)
                // compact 下这一屏是被推上来的，得有标题；regular 下留空
                // 标题 = 与改动前一致（别动 iPad）。不碰 .toolbar 可见性 ——
                // iPad detail 列那条导航条上挂着系统的侧栏开关，隐了就没了。
                .navigationTitle("")
                .navigationBarTitleDisplayMode(.inline)
                .navigationDestination(item: $selectedRemoteSessionID) { sessionID in
                    IOSRemoteSessionView(crewID: id, sessionID: sessionID)
                }
        } else {
            if canChooseComputer {
                if isCompact {
                    MobileComputerIntroduction(hasPairedComputer: hasPairedComputer,
                                               connect: { showingPairing = true })
                } else {
                    emptyWorkspaceAction
                }
            } else if !didReadComputerList || crewStore.loadingList || appModel.connectingAuthority {
                ProgressView("正在读取机组")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let error = appModel.crewdWindow.error ?? appModel.crewdWindow.workspace?.error ?? crewStore.error {
                ContentUnavailableView {
                    Label("连接需要处理", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                } actions: {
                    Button("连接电脑") { showingPairing = true }
                    Button("重试") { Task { await crewStore.refreshList() } }
                }
            } else if !isCompact {
                emptyWorkspaceAction
            } else {
                ContentUnavailableView("选择一个机组", systemImage: "bubble.left.and.bubble.right",
                                       description: Text("从左侧选择机组，继续查看消息、待办工作和资料。"))
            }
        }
    }

    private func settingsAction(_ action:@escaping ()->Void){
        showingSettings=false
        // The original root owns every operation sheet and its retained state.
        // Present only after Settings has completed dismissal.
        pendingSettingsAction=action
    }
    @State private var pendingSettingsAction:(()->Void)?
    private var mobileSettings:some View {
        NavigationStack{
            List{
                Section("个人草稿与同步"){
                    NavigationLink("个人草稿与同步") {
                        List {
                            Section { CrewPersonalHostDirectory() }
                            Section { CrewPersonalDraftArchive() }
                        }.navigationTitle("个人草稿与同步")
                    }
                }
                Section("电脑与连接"){
                    Button("连接状态与未完成操作"){settingsAction{showingConnectionStatus=true}}
                    Button("连接或添加电脑"){settingsAction{showingPairing=true}}
                    if let workspace=appModel.crewdBackend?.workspace{
                        NavigationLink("任务与工作通知"){CrewNotificationSettings(workspace:workspace,embedded:true)}
                    }else{Text("连接后可查看任务与工作通知设置").foregroundStyle(.secondary)}
                }
                Section("本机输入与原请求"){
                    Button("本机 Codex 原输入"){settingsAction{showingLocalCodexInputs=true}}
                    Button("历史聊天输入（仅本机）"){settingsAction{showingOfflineDrafts=true}}
                    if !appModel.crewdWindow.inputReviewWorkspaces.isEmpty{Button("原连接的输入"){settingsAction{showingRetainedInputs=true}}}
                    if appModel.crewdBackend?.workspace.client.capabilities.contains("command.receipts") == true{Button("原请求核对"){settingsAction{showingRecovery=true}}}
                }
            }.navigationTitle("设置")
                .toolbar{ToolbarItem(placement:.confirmationAction){Button("完成"){showingSettings=false}}}
        }
    }

    private var emptyWorkspaceAction: some View {
        let connected = appModel.crewdBackend?.workspace.client.readyForWork == true
        return CrewdGlassAddButton(title: connected ? "新建机组" : hasPairedComputer ? "选择电脑" : "连接电脑") {
            if connected { showingNewCrew = true }
            else { showingPairing = true }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // Observe the original startup read before classifying an empty list. This
    // local view readiness is not a credential flag and never selects a peer.
    private var hasPairedComputer: Bool { appModel.crewdWindow.computers.contains(where: \.paired) }
    private var canChooseComputer: Bool {
        let window = appModel.crewdWindow
        return didReadComputerList && !appModel.connectingAuthority
            && window.workspace == nil && window.selected == nil && window.error == nil
            && window.computers.allSatisfy(\.paired)
            && window.inputReviewWorkspaces.isEmpty
    }

    /// 从 crewStore.crews 中找到对应 crew 的 title，供 CrewChatView 初始化用。
    private func crewTitle(for id: String) -> String? {
        crewStore.crews.first(where: { $0.id == id })?.title
    }
}

struct MobileComputerIntroduction: View {
    let hasPairedComputer: Bool
    let connect: () -> Void
    var body: some View {
        ContentUnavailableView {
            Label(hasPairedComputer ? "选择你的电脑" : "连接你的电脑", systemImage: "desktopcomputer")
        } description: {
            Text(hasPairedComputer
                 ? "选择一台已配对的电脑，继续查看机组消息、待办工作和资料。"
                 : "这里是电脑端机组的随身前端。连接你的电脑，查看消息、待办工作和资料。")
        } actions: {
            Button("连接电脑", action: connect)
                .buttonStyle(.borderedProminent).tint(Theme.Palette.accent)
        }
    }
}
/// This surface never creates a workspace or returns input to a live composer.
private struct IOSOfflineComposerDrafts: View {
    let dataDirectory: URL?
    @Environment(\.dismiss) private var dismiss
    @State private var readback: CrewOfflineComposerDrafts.Readback?
    @State private var reading = false
    @State private var epoch = UUID()
    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text("本机历史输入，不代表电脑当前服务授权或后台回执。这里不会连接电脑、恢复任务或重新发送。")
                    Button("读取本机已保存草稿") { read() }.disabled(reading)
                    if reading { ProgressView("正在只读读取") }
                }
                if let readback {
                    if !readback.issues.isEmpty {
                        Section("未完整读取，原文件保留") {
                            ForEach(Array(readback.issues.enumerated()), id: \.offset) { _, issue in Text(issue).foregroundStyle(.red) }
                        }
                    }
                    if readback.items.isEmpty, readback.complete {
                        Text("没有本机已保存的草稿。")
                    }
                    ForEach(readback.items) { item in
                        NavigationLink {
                            ScrollView {
                                VStack(alignment: .leading, spacing: 12) {
                                    Text("仅本机历史输入，不代表当前授权或后台成功回执。")
                                        .foregroundStyle(.secondary)
                                    Text("原本地记录：" + item.record).font(.caption)
                                    Text("原作用域：" + item.namespace).font(.caption)
                                    Text("原机组：" + (item.crew ?? "未记录") + "\n原群聊：" + (item.channel ?? "未记录")).font(.caption)
                                    Text(item.text).textSelection(.enabled)
                                    if !item.mentionTokens.isEmpty { Text("原提及：" + item.mentionTokens.joined(separator: " · ")).textSelection(.enabled) }
                                    if let reply = item.reply { Text("原回复引用：\n" + reply).textSelection(.enabled) }
                                }.frame(maxWidth: .infinity, alignment: .leading).padding()
                            }.navigationTitle("已保存草稿")
                        } label: {
                            VStack(alignment: .leading) {
                                Text((item.crew ?? "原机组") + " · " + (item.channel ?? "原群聊"))
                                Text(item.text.isEmpty ? "已保存提及或回复引用" : item.text).lineLimit(2).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }.navigationTitle("本机已保存草稿")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("关闭") { epoch = UUID(); dismiss() } } }
        }.onDisappear { epoch = UUID(); reading = false }
    }
    private func read() {
        guard !reading else { return }
        guard let dataDirectory else {
            readback = .init(issues: ["未完整读取，原文件保留：本 App 数据目录未能确定。"]); return
        }
        reading = true
        let token = epoch
        Task {
            let snapshot = await Task.detached(priority: .userInitiated) { CrewOfflineComposerDrafts.read(dataDirectory: dataDirectory) }.value
            guard epoch == token else { return }
            readback = snapshot; reading = false
        }
    }
}

#endif
