#if os(iOS)
import SwiftUI

/// The caller supplies one workspace per window and paired authority. This view
/// has no legacy PSK, account, socket-path or unauthenticated network fallback.
struct CrewdMobileWorkspaceView: View {
    @ObservedObject var workspace: CrewdWorkspace
    @Environment(\.colorScheme) private var scheme
    @State private var creatingCrew = false
    @State private var conversationPath: [String] = []
    @State private var sidebar = CrewdNavigation.Sidebar.contacts
    @State private var collapsed: Set<String> = []
    @State private var compactColumn = NavigationSplitViewColumn.sidebar
    var body: some View {
        NavigationSplitView(preferredCompactColumn: $compactColumn) {
            NavigationStack(path: $conversationPath) {
            VStack(spacing: 0) {
                Picker("视图", selection: $sidebar) {
                    ForEach(CrewdNavigation.Sidebar.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented).padding()
                List {
                    if sidebar == .contacts {
                        ForEach(CrewdNavigation.directory(workspace.crews, collapsed: collapsed)) { row in
                            HStack {
                                if row.expandable {
                                    Button { if !collapsed.insert(row.id).inserted { collapsed.remove(row.id) } }
                                    label: { Image(systemName: collapsed.contains(row.id) ? "chevron.right" : "chevron.down") }
                                    .buttonStyle(.borderless)
                                }
                                if let crew = workspace.crews.first(where: { $0["id"].string == row.object }) { crewButton(crew) }
                            }.padding(.leading, CGFloat(min(row.depth, 12)) * 10)
                        }
                    } else {
                        let rows = CrewdNavigation.timeline(workspace.crews, activity: workspace.crewActivity)
                        ForEach(rows.indices, id: \.self) { index in crewButton(rows[index]) }
                    }
                }.listStyle(.plain)
                Text(workspace.client.connected ? "已连接" : workspace.status).font(.caption).foregroundStyle(.secondary).padding(8)
            }.background(CrewdChatStyle.sidebar(scheme)).navigationTitle("机组")
                .navigationDestination(for: String.self) { crew in
                    CrewdSidebarConversations(workspace: workspace, crewID: crew, showContent: { compactColumn = .detail }, filtersChannelFolders: true)
                }
                .toolbar { Button("新建机组", systemImage: "plus") { creatingCrew = true }.disabled(!workspace.client.connected || workspace.busy) }
            }
        } detail: {
            if !workspace.crewID.isEmpty {
                CrewdMobileCrewPane(workspace: workspace).id(workspace.authorityRecordID)
            } else { Text("选择一个机组").foregroundStyle(.secondary) }
        }.onChange(of: workspace.presentationNamespace) { _, _ in conversationPath = [] }
        .onChange(of: workspace.crewID) { _, crew in
            if let current = conversationPath.last, current != crew { conversationPath = [] }
        }.sheet(isPresented: $creatingCrew) { CrewdCrewCreationView(workspace: workspace).id(workspace.authorityRecordID) }
    }
    private func crewButton(_ crew: CrewdValue) -> some View {
        let id = crew["id"].string ?? ""
        return HStack(spacing: 8) {
            Button { workspace.selectCrew(id); compactColumn = .detail } label: {
                HStack(spacing: 10) {
                    CrewdGroupAvatar(members: workspace.crewAvatarMembers[id] ?? [], channel: id, name: crew["title"].string ?? "机组")
                    VStack(alignment: .leading, spacing: 4) {
                        Text(crew["title"].string ?? "机组").foregroundStyle(CrewdChatStyle.ink(scheme))
                        if sidebar == .messages {
                            Text(workspace.crewActivity[id]?["text"].string ?? (workspace.activityRead.contains(id) ? "暂无消息" : "消息尚未读取"))
                                .font(.caption).lineLimit(1).foregroundStyle(.secondary)
                        }
                    }
                    Spacer()
                }.contentShape(Rectangle())
            }.buttonStyle(.plain)
            Button { workspace.selectCrew(id); conversationPath = [id]; compactColumn = .sidebar } label: {
                Image(systemName: "chevron.right").frame(minWidth: 30, minHeight: 36)
            }.buttonStyle(.plain).accessibilityLabel("查看机组会话")
        }.onTapGesture(count: 2) { workspace.selectCrew(id); conversationPath = [id]; compactColumn = .sidebar }
    }

}

private struct CrewdMobileCrewPane: View {
    @ObservedObject var workspace: CrewdWorkspace
    @State private var tab = CrewdNavigation.Tab.messages
    private var title: String { workspace.crews.first(where: { $0["id"].string == workspace.crewID })?["title"].string ?? "机组" }
    var body: some View {
        VStack(spacing: 0) {
            Picker("机组视图", selection: $tab) {
                ForEach(CrewdNavigation.Tab.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }.pickerStyle(.segmented).padding()
            if let error = workspace.error { Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled).padding(.horizontal) }
            ZStack {
                VStack(spacing: 0) {
                    if workspace.sessions.isEmpty {
                        CrewdSessionStartView(workspace: workspace, drafts: $workspace.messageDrafts).padding()
                    }
                    CrewdMobileMessages(workspace: workspace).id(workspace.crewID)
                }.opacity(tab == .messages ? 1 : 0).allowsHitTesting(tab == .messages)
                    .disabled(tab != .messages).accessibilityHidden(tab != .messages)
                if tab == .todo || tab == .progress {
                    CrewdWorkPanels(store: workspace.workStore, resources: workspace.resources,
                        resourceDrafts: $workspace.resourceDrafts, drafts: $workspace.requirementDrafts,
                        planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                        members: Array(workspace.knownSessions.values), crews: workspace.crews, progressOnly: tab == .progress)
                } else if tab == .documents { CrewdKnowledgeDocumentsEntry(workspace: workspace) }
            }
        }.navigationTitle(title)
        .onChange(of:workspace.knowledgeNavigation) { _,_ in tab = .messages }
        .toolbar { Button("刷新", systemImage: "arrow.clockwise") { workspace.scheduleSync(recoverHistory: true) }.disabled(!workspace.client.connected) }
    }
}

private struct CrewdMobileMessages: View {
    @ObservedObject var workspace: CrewdWorkspace
    var body: some View {
        VStack(spacing: 0) {
            CrewdMobileConversation(workspace: workspace, expectedChannel: workspace.channelID).id(workspace.channelID)
        }
    }

}

private struct CrewdMobileConversation: View {
    @ObservedObject var workspace: CrewdWorkspace
    let expectedChannel: String
    @Environment(\.colorScheme) private var scheme
    @State private var showTodo = false
    @State private var showMembers = false
    private var context: CrewdDrafts.Context { .message(crew: workspace.crewID, channel: workspace.channelID, session: workspace.sessionID) }
    private var input: Binding<String> { Binding(get: { workspace.messageDrafts.text(for: context) }, set: { workspace.setPersonalInput($0,context:context) }) }
    var body: some View {
        Group {
            if expectedChannel.isEmpty { Text("选择一个会话").foregroundStyle(.secondary) }
            else if expectedChannel != workspace.channelID { Text("正在读取会话…").foregroundStyle(.secondary) }
            else { conversation }
        }.task(id: expectedChannel) { if !expectedChannel.isEmpty, workspace.channelID != expectedChannel { workspace.selectConversation(expectedChannel) } }
    }
    private var conversation: some View {
        VStack(spacing: 8) {
            CrewPersonalDraftControls(workspace:workspace,context:context,payload:Binding(get:{.init(text:input.wrappedValue)},set:{input.wrappedValue=$0.text}))
            HStack {
                CrewdFolderAssignmentButton(store: workspace.resources, drafts: $workspace.resourceDrafts, view: .messages, object: CrewdFolderFilter.object("channel", workspace.channelID))
                Spacer()
                Button(showTodo ? "收起 Todo" : "本群 Todo") { showTodo.toggle() }
                Button(showMembers ? "收起成员" : "成员") { showMembers.toggle() }
            }
            GeometryReader { geometry in
                if geometry.size.width >= 650 {
                    HStack(alignment: .top) { transcript; if showTodo || showMembers { Divider(); auxiliary.frame(width: 220) } }
                } else { VStack(spacing: 8) { auxiliary; transcript } }
            }
            Picker("通知成员", selection: Binding(get: { workspace.sessionID }, set: { workspace.selectSession($0) })) {
                let recipients = workspace.selectedMembers.filter { $0["kind"].string == "session" }
                ForEach(recipients.indices, id: \.self) { index in
                    let id = recipients[index]["id"].string ?? ""
                    let known = workspace.knownSessions[id]
                    Text(CrewColorHash.emoji(for: id) + " " + (known?["displayName"].string ?? (known?["role"].string == "captain" ? "机长" : "成员"))).tag(id)
                }
            }
            TextField("给当前成员发消息", text: input, axis: .vertical).lineLimit(2...6)
            HStack { Text("发送会通知选中的成员").font(.caption).foregroundStyle(.secondary); Spacer(); Button("发送") { send() }
                .disabled(input.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || workspace.busy || !workspace.client.connected || workspace.channelID.isEmpty || workspace.sessionID.isEmpty) }
        }.padding(12)
    }
    @ViewBuilder private var auxiliary: some View {
        if showTodo {
            CrewdChannelTodoView(group: workspace.channelWork, work: workspace.workStore,
                                 linkDrafts: $workspace.channelLinkDrafts, requirementDrafts: $workspace.requirementDrafts,
                                 planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                                 members: Array(workspace.knownSessions.values), crews: workspace.crews, openCrew: workspace.selectCrew)
                .frame(maxHeight: 260)
        }
        if showMembers {
            ScrollView { VStack(alignment: .leading, spacing: 8) {
                ForEach(workspace.selectedMembers.indices, id: \.self) { index in
                    let member = workspace.selectedMembers[index]
                    HStack { if let seed = CrewdAvatarIdentity.seed(member) { CrewdMemberAvatar(seed: seed, colorSeed: workspace.channelID) }
                        Text(member["kind"].string == "human" ? "你" : workspace.knownSessions[member["id"].string ?? ""]?["displayName"].string ?? "成员资料尚未加载") }
                }
            }}.frame(maxHeight: 140)
        }
    }
    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if workspace.restartPrompt != .null { restart }
                    if workspace.inspection["runnerRequest"] != .null {
                        CrewdHumanRequestView(workspace: workspace, request: workspace.inspection["runnerRequest"], decision: workspace.inspection["live"]["pendingDecision"])
                            .id(workspace.inspection["runnerRequest"]["fingerprint"].string ?? "")
                    }
                    ForEach(workspace.messages.indices, id: \.self) { index in
                        let message = workspace.messages[index], human = message["senderKind"].string == "human"
                        HStack(alignment: .top, spacing: 8) {
                            if human { Spacer(minLength: 24) }
                            else if let sender = message["senderSessionId"].string { CrewdMemberAvatar(seed: sender, colorSeed: workspace.channelID) }
                            VStack(alignment: .leading, spacing: 4) {
                                Text(message["senderName"].string ?? (human ? "你" : "成员")).font(.caption).foregroundStyle(.secondary)
                                Text(message["text"].string ?? "").textSelection(.enabled).padding(10)
                                    .background(human ? CrewdChatStyle.selection(scheme) : CrewdChatStyle.bubble(scheme), in: RoundedRectangle(cornerRadius: 6))
                            }
                            if human { CrewdMemberAvatar(seed: "human", colorSeed: workspace.channelID) }
                            else { Spacer(minLength: 24) }
                        }.id(message["id"].string ?? "\(index)")
                    }
                    DisclosureGroup("成员工作与恢复记录") {
                        if !workspace.history.isEmpty { Text(workspace.history).textSelection(.enabled) }
                        ForEach(workspace.runnerItems.indices, id: \.self) { index in CrewdRunnerItemView(item: workspace.runnerItems[index]) }
                        CrewdCodexSessionControls(workspace:workspace,sessionID:workspace.sessionID)
                        CrewdSessionStartView(workspace: workspace, drafts: $workspace.messageDrafts)
                        HStack { Button("打断") { action("session.interrupt") }; Button("停止") { action("session.stop") }; Button("恢复") { action("session.resume") } }.disabled(workspace.busy || !workspace.client.connected || workspace.sessionID.isEmpty)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }.onChange(of: workspace.messages.last?["id"].string) { _, last in if let last { proxy.scrollTo(last, anchor: .bottom) } }
        }
    }
    private var restart: some View {
        let prompt = workspace.restartPrompt
        return VStack(alignment: .leading, spacing: 8) {
            Text("后台重新启动，是否恢复此前的会话？")
            HStack { Button("恢复这些会话") { answerRestart(prompt, "resume") }; Button("暂不恢复") { answerRestart(prompt, "later") } }
                .disabled(workspace.busy || !workspace.client.connected)
        }
    }
    private func answerRestart(_ prompt: CrewdValue, _ answer: String) { Task { _ = await workspace.perform("restart.answer", .object(["id": prompt["id"], "answer": .string(answer)])) } }
    private func action(_ method: String) {
        var fields: [String: CrewdValue] = ["sessionId": .string(workspace.sessionID)]
        if method == "session.stop" { fields["reason"] = .string("人类在移动客户端停止会话") }
        Task { _ = await workspace.perform(method, .object(fields)) }
    }
    private func send() {
        let snapshot = workspace.messageDrafts.snapshot(for: context)
        let fields: CrewdValue = .object(["channelId": .string(snapshot.context.channel), "text": .string(snapshot.text), "mentions": .array([.object(["kind": .string("session"), "targetId": .string(snapshot.context.session)])])])
        Task { _ = await workspace.performText("message.post", fields, snapshot:snapshot) }
    }
}
#endif
