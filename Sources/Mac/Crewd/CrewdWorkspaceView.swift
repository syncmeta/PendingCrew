#if os(macOS)
import SwiftUI
import AppKit

struct CrewdWorkspaceView: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var workspace: CrewdWorkspace
    var autoConnect = true
    var machineLabel = "本机"
    @State private var creatingCrew = false
    @State private var sidebarMode = CrewdNavigation.Sidebar.contacts
    @State private var tab = CrewdNavigation.Tab.messages
    @State private var collapsedCrews: Set<String> = []
    @State private var todoExpanded = true
    @State private var membersExpanded = true
    @State private var runnerExpanded = false
    private var messageContext: CrewdDrafts.Context {
        .message(crew: workspace.crewID, channel: workspace.channelID, session: workspace.sessionID)
    }
    private var compose: String { workspace.messageDrafts.text(for: messageContext) }
    private func draftBinding(_ context: CrewdDrafts.Context) -> Binding<String> {
        Binding(get: { workspace.messageDrafts.text(for: context) }, set: { workspace.messageDrafts.set($0, for: context) })
    }
    var body: some View {
        NavigationSplitView {
            VStack(spacing:0) {
                Picker("侧栏",selection:$sidebarMode) {ForEach(CrewdNavigation.Sidebar.allCases,id:\.self){Text($0.rawValue).tag($0)}}.pickerStyle(.segmented).padding(12)
                ScrollView {
                    VStack(alignment:.leading,spacing:6) {
                        if sidebarMode == .contacts {
                            Label(machineLabel,systemImage:"desktopcomputer").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme)).padding(.vertical,8)
                            ForEach(CrewdNavigation.directory(workspace.crews,collapsed:collapsedCrews)) {row in
                                if let crew=workspace.crews.first(where:{$0["id"].string == row.object}) {
                                    HStack(spacing:5) {
                                        if row.expandable {Button {if !collapsedCrews.insert(row.id).inserted {collapsedCrews.remove(row.id)}} label:{Image(systemName:collapsedCrews.contains(row.id) ? "chevron.right":"chevron.down")}.buttonStyle(.plain).font(.caption)}
                                        crewButton(crew)
                                    }.padding(.leading,CGFloat(row.depth)*16)
                                }
                            }
                        } else {
                            let rows=CrewdNavigation.timeline(workspace.crews,activity:workspace.crewActivity)
                            ForEach(rows.indices,id:\.self) {i in crewButton(rows[i])}
                        }
                    }.padding(.horizontal,12)
                }
                Spacer(minLength:0)
                VStack(alignment:.leading,spacing:8) {
                    Button("新建机组",systemImage:"plus") {creatingCrew=true}
                    Text(workspace.status).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                    Button("重新连接") {Task {await workspace.connect()}}
                }.padding(14).frame(maxWidth:.infinity,alignment:.leading)
            }.background(CrewdChatStyle.sidebar(colorScheme)).navigationTitle("PendingCrew").navigationSplitViewColumnWidth(min:220,ideal:260,max:340)
        } detail: {
            VStack(spacing:0) {
                if workspace.restartPrompt != .null {restartQuestion}
                if let error=workspace.error {HStack {Image(systemName:"exclamationmark.triangle"); Text(error).textSelection(.enabled); Spacer(); Button("关闭"){workspace.error=nil}}.padding(10).background(.orange.opacity(0.12))}
                if workspace.crewID.isEmpty {ContentUnavailableView("选择机组",systemImage:"person.3")}
                else {
                    VStack(alignment:.leading,spacing:12) {
                        Text(workspace.crews.first(where:{$0["id"].string == workspace.crewID})?["title"].string ?? "机组").font(.title2)
                        HStack(spacing:8) {ForEach(CrewdNavigation.Tab.allCases,id:\.self){item in
                            Button(item.rawValue){tab=item}.buttonStyle(.plain).padding(.horizontal,18).padding(.vertical,8).background(tab == item ? CrewdChatStyle.selection(colorScheme):CrewdChatStyle.bubble(colorScheme),in:RoundedRectangle(cornerRadius:5))
                        }}
                    }.padding(16).frame(maxWidth:.infinity,alignment:.leading)
                    Divider()
                    switch tab {
                    case .messages: messageLayout
                    case .todo: CrewdWorkPanels(store:workspace.workStore,resources:workspace.resources,resourceDrafts:$workspace.resourceDrafts,drafts:$workspace.requirementDrafts,planDrafts:$workspace.planDrafts,humanDrafts:$workspace.humanDrafts,members:Array(workspace.knownSessions.values).sorted(by:{($0["id"].string ?? "") < ($1["id"].string ?? "")}),crews:workspace.crews)
                    case .progress: CrewdWorkPanels(store:workspace.workStore,resources:workspace.resources,resourceDrafts:$workspace.resourceDrafts,drafts:$workspace.requirementDrafts,planDrafts:$workspace.planDrafts,humanDrafts:$workspace.humanDrafts,members:Array(workspace.knownSessions.values).sorted(by:{($0["id"].string ?? "") < ($1["id"].string ?? "")}),crews:workspace.crews,progressOnly:true)
                    case .documents: CrewdDocumentsView(store:workspace.resources,drafts:$workspace.resourceDrafts)
                    }
                }
            }.background(CrewdChatStyle.canvas(colorScheme))
        }.foregroundStyle(CrewdChatStyle.ink(colorScheme)).frame(minWidth:1150,minHeight:700)
        .sheet(isPresented:$creatingCrew){CrewdCrewCreationView(workspace:workspace)}
        .task {if autoConnect {await workspace.connect()}}
        .onChange(of:workspace.inspection["runnerRequest"]) {_,value in if value != .null {runnerExpanded=true}}
    }
    private func crewButton(_ crew:CrewdValue) -> some View {
        let id=crew["id"].string ?? "", latest=workspace.crewActivity[id]
        return Button {workspace.selectCrew(id)} label:{
            HStack(spacing:8) {
                CrewdGroupAvatar(members:crewAvatarMembers(id),channel:id,name:crew["title"].string ?? "")
                VStack(alignment:.leading,spacing:4) {
                Text(crew["title"].string ?? "机组").font(.body)
                if sidebarMode == .messages,let time=latest?["createdAt"].string {Text(time).font(.caption2).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                Text(latest?["text"].string ?? (workspace.activityRead.contains(id) ? crew["statusLine"].string ?? "暂无消息":"尚未读取消息")).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme)).lineLimit(1)
                }
            }.padding(9).frame(maxWidth:.infinity,alignment:.leading).background(workspace.crewID == id ? CrewdChatStyle.selection(colorScheme):Color.clear,in:RoundedRectangle(cornerRadius:5))
        }.buttonStyle(.plain)
    }
    private func crewAvatarMembers(_ id:String)->[CrewdValue] {
        if let members=workspace.crewAvatarMembers[id] {return members}
        let roster=Array(workspace.knownSessions.values)+workspace.sessions
        let rows=Dictionary(roster.filter {$0["crewId"].string == id}.compactMap {row in row["id"].string.map {($0,row)}},uniquingKeysWith:{a,_ in a})
        return rows.keys.sorted().map {.object(["kind":.string("session"),"id":.string($0)])}
    }
    private var messageFolderSelection:Binding<String>{Binding(get:{workspace.resourceDrafts.selection(workspace.crewID,.messages)},set:{workspace.resourceDrafts.select(workspace.crewID,.messages,$0)})}
    private var messageLayout: some View {
        HSplitView {
            VStack(alignment:.leading,spacing:8) {
                CrewdFolderControls(store:workspace.resources,drafts:$workspace.resourceDrafts,selection:messageFolderSelection,view:.messages).padding(8)
                Text("群聊与单聊").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme)).padding(.horizontal,10)
                ScrollView {VStack(alignment:.leading,spacing:6){ForEach(workspace.channels.indices.filter{CrewdFolderFilter.includes(CrewdFolderFilter.object("channel",workspace.channels[$0]["id"].string ?? ""),selection:messageFolderSelection.wrappedValue,snapshot:workspace.resources.folders[.messages])},id:\.self){i in
                    let channel=workspace.channels[i]
                    HStack(spacing:2){Button {workspace.selectConversation(channel["id"].string ?? "")} label:{
                        HStack {
                            let members=workspace.channelSummaries.first(where:{$0["channel"]["id"] == channel["id"]})?["members"].array ?? []
                            if channel["kind"].string == "dm",let peer=members.compactMap(CrewdAvatarIdentity.seed).first(where:{$0 != "human"}) {CrewdMemberAvatar(seed:peer,colorSeed:channel["id"].string ?? "")}
                            else {CrewdGroupAvatar(members:members,channel:channel["id"].string ?? "",name:channelName(channel))}
                            Text(channelName(channel)); Spacer()
                        }.padding(10).background(workspace.channelID == channel["id"].string ? CrewdChatStyle.selection(colorScheme):Color.clear,in:RoundedRectangle(cornerRadius:5))
                    }.buttonStyle(.plain)
                    CrewdFolderAssignmentButton(store:workspace.resources,drafts:$workspace.resourceDrafts,view:.messages,object:CrewdFolderFilter.object("channel",channel["id"].string ?? ""))}
                }}.padding(8)}
                Spacer(minLength:0)
            }.frame(minWidth:165,idealWidth:190,maxWidth:245)
            VStack(spacing:0) {
                conversation
                Divider()
                DisclosureGroup("会话运行、记录与审批",isExpanded:$runnerExpanded){session.frame(maxHeight:360)}.padding(10)
            }.frame(minWidth:350)
            VStack(alignment:.leading,spacing:8) {
                DisclosureGroup("本群 Todo",isExpanded:$todoExpanded) {
                    CrewdChannelTodoView(group:workspace.channelWork,work:workspace.workStore,linkDrafts:$workspace.channelLinkDrafts,requirementDrafts:$workspace.requirementDrafts,planDrafts:$workspace.planDrafts,humanDrafts:$workspace.humanDrafts,members:Array(workspace.knownSessions.values).sorted(by:{($0["id"].string ?? "") < ($1["id"].string ?? "")}),crews:workspace.crews,openCrew:{id in workspace.selectCrew(id);tab = .todo}).frame(minHeight:100,maxHeight:.infinity)
                }
                Divider()
                DisclosureGroup("本群成员",isExpanded:$membersExpanded) {
                    ScrollView {VStack(alignment:.leading,spacing:12){ForEach(workspace.selectedMembers.indices,id:\.self){i in
                        let member=workspace.selectedMembers[i], id=member["id"].string ?? ""
                        let row=workspace.sessions.first(where:{$0["id"].string == id})
                        Button {if member["kind"].string == "session" {workspace.selectSession(id); runnerExpanded=true}} label:{
                            HStack {if let seed=CrewdAvatarIdentity.seed(member) {CrewdMemberAvatar(seed:seed,colorSeed:workspace.channelID)}; Text(member["kind"].string == "human" ? "你":row?["displayName"].string ?? id)}
                        }.buttonStyle(.plain)
                    }}.frame(maxWidth:.infinity,alignment:.leading)}.frame(minHeight:100,maxHeight:.infinity)
                }
                Spacer(minLength:0)
            }.padding(12).frame(minWidth:185,idealWidth:215,maxWidth:280)
        }
    }

    private var restartQuestion: some View {
        let prompt = workspace.restartPrompt
        let count = prompt["sessionIds"].array.count
        return VStack(alignment: .leading, spacing: 8) {
            Label(prompt["reason"].string == "hibernate" ? "后台已从休眠启动" : "后台已重新启动", systemImage: "arrow.clockwise")
                .font(.headline)
            Text("上次运行的 \(count) 个会话正在等待恢复；尚未自动启动。")
            if !prompt["interrupted"].array.isEmpty {
                Text("其中 \(prompt["interrupted"].array.count) 个会话的工作曾被打断，恢复后请核对结果。")
                    .font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
            }
            HStack {
                Button("恢复这些会话") { answerRestart(prompt, "resume") }
                Button("暂不恢复") { answerRestart(prompt, "later") }
                    .disabled(prompt["answer"].string == "later")
                if prompt["answer"].string == "later" {
                    Text("已暂缓，你可以稍后恢复。") .font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                }
            }.disabled(workspace.busy || !workspace.client.connected)
        }.frame(maxWidth: .infinity, alignment: .leading).padding(12).background(.blue.opacity(0.08))
    }

    private func answerRestart(_ prompt: CrewdValue, _ answer: String) {
        Task { _ = await workspace.perform("restart.answer", .object(["id": prompt["id"], "answer": .string(answer)])) }
    }

    private var conversation: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(channelName(workspace.channels.first(where:{$0["id"].string == workspace.channelID}) ?? .null)).font(.headline)
                Spacer();if !workspace.channelID.isEmpty{CrewdFolderAssignmentButton(store:workspace.resources,drafts:$workspace.resourceDrafts,view:.messages,object:CrewdFolderFilter.object("channel",workspace.channelID))}
            }
            if !workspace.pendingAsks.isEmpty {
                DisclosureGroup("全组织等待你的答复（\(workspace.pendingAsks.count)）") {
                    ScrollView {
                        ForEach(workspace.pendingAsks.indices, id: \.self) { i in
                            let ask = workspace.pendingAsks[i]["ask"]
                            CrewdCrewQuestionView(workspace: workspace, ask: ask).id((ask["crewId"].string ?? "") + "/" + (ask["ask"].string ?? ""))
                        }
                    }.frame(maxHeight: 180)
                }
            }
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 14) {
                        ForEach(workspace.messages.indices, id: \.self) { i in
                            let message = workspace.messages[i]
                            let human=message["senderKind"].string == "human"
                            HStack(alignment:.top,spacing:9) {
                                if human {Spacer(minLength:40)} else {if let sender=message["senderSessionId"].string {CrewdMemberAvatar(seed:sender,colorSeed:workspace.channelID)} else {Text(CrewColorHash.initial(for:message["senderName"].string ?? "")).frame(width:30)}}
                                VStack(alignment:.leading,spacing:5) {
                                    Text(message["senderName"].string ?? (human ? "你":"成员")).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                    Text(message["text"].string ?? "").textSelection(.enabled).padding(12).background(human ? CrewdChatStyle.selection(colorScheme):CrewdChatStyle.bubble(colorScheme),in:RoundedRectangle(cornerRadius:6))
                                }
                                if human {CrewdMemberAvatar(seed:"human",colorSeed:workspace.channelID)} else {Spacer(minLength:40)}
                            }.id(message["id"].string ?? "\(i)")
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 8)
                }
                .onChange(of: workspace.messages.count) { _, _ in
                    if let last = workspace.messages.last?["id"].string { proxy.scrollTo(last, anchor: .bottom) }
                }
            }
            TextField("发给当前 Codex 会话", text: draftBinding(messageContext), axis: .vertical).lineLimit(2...6)
            HStack {
                Text("发送会通知选中的会话").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                Spacer()
                Button("发送") {
                    let sent = workspace.messageDrafts.snapshot(for: messageContext)
                    let text = sent.text, channel = sent.context.channel, target = sent.context.session
                    Task {
                        let mentions: [CrewdValue] = target.isEmpty ? [] : [.object(["kind": .string("session"), "targetId": .string(target)])]
                        _ = await workspace.performText("message.post", .object(["channelId": .string(channel), "text": .string(text), "mentions": .array(mentions)]),snapshot:sent)
                    }
                }.disabled(compose.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || workspace.busy || workspace.channelID.isEmpty || workspace.sessionID.isEmpty)
            }
        }.padding()
    }

    private var session: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Picker("会话", selection: Binding(get: { workspace.sessionID }, set: { workspace.selectSession($0) })) {
                    ForEach(workspace.sessions.indices, id: \.self) { i in
                        let row = workspace.sessions[i]
                        Text(row["displayName"].string ?? row["title"].string ?? row["role"].string ?? "Codex").tag(row["id"].string ?? "")
                    }
                }
                if !workspace.sessionID.isEmpty {
                    Button("打断") { action("session.interrupt") }
                    Button("停止") { action("session.stop", extra: ["reason": .string("人类在 Mac 客户端停止会话")]) }
                    Button("恢复") { action("session.resume") }
                }
            }.disabled(workspace.busy)
            if !workspace.sessionID.isEmpty {
                Label(workspace.inspection["runnerRequest"] != .null ? "等待你的答复" : workspace.inspection["live"]["working"] == .bool(true) ? "正在工作" : workspace.inspection["live"]["running"] == .bool(true) ? "等待下一步" : "会话尚未运行", systemImage: workspace.inspection["live"]["working"] == .bool(true) ? "gearshape" : "circle")
                    .font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
            }
            if let row = workspace.sessions.first(where: {$0["id"].string == workspace.sessionID}) {
                Text([row["model"].string, row["effort"].string].compactMap {$0}.joined(separator: " · ")).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
            }
            if workspace.inspection["live"]["health"] != .null {
                Text(workspace.inspection["live"]["health"]["detail"].string ?? "会话状态异常").foregroundStyle(.orange).textSelection(.enabled)
            }
            if workspace.inspection["runnerRequest"] != .null {
                CrewdHumanRequestView(workspace: workspace, request: workspace.inspection["runnerRequest"], decision: workspace.inspection["live"]["pendingDecision"])
                    .id(workspace.inspection["runnerRequest"]["fingerprint"].string ?? "")
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if !workspace.history.isEmpty {
                        DisclosureGroup("已恢复的会话记录") { Text(workspace.history).font(.system(.body, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                    }
                    ForEach(workspace.runnerItems.indices, id: \.self) { i in CrewdRunnerItemView(item: workspace.runnerItems[i]) }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            Divider()
            CrewdSessionStartView(workspace:workspace,drafts:$workspace.messageDrafts)
        }.padding()
    }
    private func action(_ method: String, extra: [String: CrewdValue] = [:]) {
        var p = extra; p["sessionId"] = .string(workspace.sessionID)
        Task { _ = await workspace.perform(method, .object(p)) }
    }
    private func channelName(_ row: CrewdValue) -> String {
        switch row["kind"].string { case "human": return "与我沟通"; case "lobby": return "大厅"; default: return row["name"].string ?? "群" }
    }

}

private struct CrewdCrewQuestionView: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var workspace: CrewdWorkspace
    let ask: CrewdValue
    @State private var answer = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(ask["question"].string ?? "机组的问题").textSelection(.enabled)
            HStack {
                TextField("你的答复", text: $answer)
                Button("答复") {
                    let text = answer
                    Task {
                        if await workspace.perform("ask.answer", .object(["crewId": ask["crewId"], "ask": ask["ask"], "text": .string(text)])), answer == text { answer = "" }
                    }
                }.disabled(answer.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || workspace.busy)
            }
        }.padding(.vertical, 6)
    }
}

struct PendingCrewNextApp: App {
    @StateObject private var workspace: CrewdWorkspace
    init() {
        let explicit = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"]
        let directory = explicit.map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrewNext")
        _workspace = StateObject(wrappedValue: CrewdWorkspace(dataDirectory: directory))
    }
    private var candidateTitle:String {
        let ref=Bundle.main.object(forInfoDictionaryKey:"PendingCrewSourceRef") as? String
        return "PendingCrew · 开发候选" + (ref.map{" · " + String($0.prefix(8))} ?? "")
    }
    var body: some Scene {
        WindowGroup(candidateTitle) { CrewdWorkspaceView(workspace: workspace) }
        .commands { CommandGroup(replacing: .help) { Text("Codex 首版使用独立数据目录，旧版数据保留。") } }
    }
}
#endif
