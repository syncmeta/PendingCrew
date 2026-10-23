import SwiftUI
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif

private struct CrewdOriginalChannelKey: EnvironmentKey { static let defaultValue = "" }
extension EnvironmentValues {
    var crewdOriginalChannelID: String {
        get { self[CrewdOriginalChannelKey.self] }
        set { self[CrewdOriginalChannelKey.self] = newValue }
    }
}

struct CrewdMessageFolderPresentation {
    var filter:CrewTimelineFilter.MessageFolder = .all
    var clear:(()->Void)? = nil
    var classify:((String)->Void)? = nil
}
private struct CrewdMessageFolderKey:EnvironmentKey {
    static let defaultValue=CrewdMessageFolderPresentation()
}
extension EnvironmentValues {
    var crewdMessageFolders:CrewdMessageFolderPresentation {
        get{self[CrewdMessageFolderKey.self]}
        set{self[CrewdMessageFolderKey.self]=newValue}
    }
}

private struct CrewdMessageSearchTargetKey:EnvironmentKey{static let defaultValue:String?=nil}
extension EnvironmentValues{
    var crewdMessageSearchTarget:String?{
        get{self[CrewdMessageSearchTargetKey.self]}
        set{self[CrewdMessageSearchTargetKey.self]=newValue}
    }
}

struct CrewdOriginalWorkIntegration: ViewModifier {
    let workspace: CrewdWorkspace?
    var openSession: ((String) -> Void)? = nil
    var searchControl: AnyView? = nil
    func body(content: Content) -> some View {
        if let workspace { CrewdOriginalCrewContent(workspace: workspace, content: { content }, openSession: openSession, searchControl: searchControl) }
        else { content }
    }
}
private struct CrewdOriginalCrewContent<Content: View>: View {
    @ObservedObject var workspace: CrewdWorkspace
    @EnvironmentObject private var crewStore: CrewStore
    @Environment(\.colorScheme) private var scheme
    @ViewBuilder let content: () -> Content
    var openSession: ((String) -> Void)? = nil
    var searchControl: AnyView? = nil
    @State private var workScopes:[String:String]=[:]
    @State private var todoQueries: [String: String] = [:]
    @State private var todoTags: [String: String] = [:]
    var body: some View {
        VStack(spacing:0){
            sectionHeader
            CrewdOriginalMessages(workspace:workspace,header:nil,openSession:openSession){sectionContent}
        }
    }
    private var sectionHeader:some View {
        VStack(spacing:0){
            HStack(spacing:12){
                CrewdGlassWorkspaceTabs(selection:$workspace.originalSection)
                    .frame(maxWidth:.infinity,alignment:.leading)
                if let searchControl { searchControl }
            }.padding(.horizontal,14).padding(.vertical,8)
            Divider()
        }
    }
    private var sectionContent: some View {
        VStack(spacing: 0) {
            ZStack {
                // Keep the original chat instance mounted: drafts, anchors and
                // its existing pagination/identity adapter survive other panels.
                CrewdOriginalMessageSurface(workspace:workspace){content()}
                    .environment(\.crewdOriginalChannelID, workspace.channelID).id(workspace.channelID)
                    .opacity(workspace.originalSection == "消息" ? 1 : 0)
                    .allowsHitTesting(workspace.originalSection == "消息")
                    .disabled(workspace.originalSection != "消息")
                    .accessibilityHidden(workspace.originalSection != "消息")
                if workspace.originalSection == "Todo" || workspace.originalSection == "真人Todo" {
                    scopedTodo(human: workspace.originalSection == "真人Todo").padding(14)
                } else if workspace.originalSection == "文档" {
                    CrewdKnowledgeDocumentsEntry(workspace: workspace)
                } else if workspace.originalSection == "进展" {
                    scopedProgress
                }

            }
        }
    }
    private func scope(_ section:String,default value:String)->Binding<String>{
        Binding(get:{workScopes[section] ?? value},set:{workScopes[section]=$0})
    }
    private func ledger(domain:CrewdWorkPanelDomain,progress:Bool=false,humanCrewOnly:Bool=false)->some View{
        CrewdWorkPanels(store:workspace.workStore,resources:workspace.resources,
            resourceDrafts:$workspace.resourceDrafts,drafts:$workspace.requirementDrafts,
            planDrafts:$workspace.planDrafts,humanDrafts:$workspace.humanDrafts,
            members:workspace.sessions,crews:workspace.crews,domain:domain,humanCrewOnly:humanCrewOnly,progressOnly:progress)
    }
    private func scopedTodo(human:Bool)->some View {
        let selection=scope(human ? "human":"ai",default:human ? "crew":"channel")
        return VStack(alignment:.leading,spacing:8){
            Picker("范围",selection:selection){
                Text("当前群").tag("channel");Text("机组全部").tag("crew")
                if human{Text("所有可访问待人处理").tag("inbox")}
            }.pickerStyle(.menu).fixedSize()
            if selection.wrappedValue == "channel"{channelTodo(human:human)}
            else{ledger(domain:human ? .human:.requirements,humanCrewOnly:selection.wrappedValue=="crew")}
        }
    }
    private var scopedProgress:some View {
        let selection=scope("progress",default:"crew")
        return VStack(alignment:.leading,spacing:8){
            Picker("计划范围",selection:selection){Text("当前群").tag("channel");Text("机组全部").tag("crew")}.pickerStyle(.menu).fixedSize().padding(.horizontal,14)
            if selection.wrappedValue=="channel"{CrewdOriginalChannelProgress(workspace:workspace)}
            else{ledger(domain:.all,progress:true)}
        }
    }
    private func channelTodo(human: Bool) -> some View {
        let key = workspace.crewID + "/" + workspace.channelID + (human ? "/human" : "/ai")
        return CrewdChannelTodoView(group: workspace.channelWork, work: workspace.workStore,
            linkDrafts: $workspace.channelLinkDrafts, requirementDrafts: $workspace.requirementDrafts,
            planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
            members: workspace.sessions.filter { session in workspace.selectedMembers.contains { $0["kind"] == .string("session") && $0["id"] == session["id"] } }, crews: workspace.crews,
            openCrew: { crewStore.selectCrew($0) }, showTodoKinds: true, humanOnly: human,
            externalQuery: Binding(get: { todoQueries[key] ?? "" }, set: { todoQueries[key] = $0 }),
            externalTag: Binding(get: { todoTags[key] ?? "" }, set: { todoTags[key] = $0 }),
            mutationDrafts: $workspace.todoMutationDrafts)
    }
}

/// Message folders classify actual source message IDs inside this channel.
/// The original timeline cache includes the classification in its cache key.
private struct CrewdOriginalMessageSurface<Content:View>:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var resources:CrewdResources
    let content:()->Content
    private struct Target:Identifiable {let id:String;let crew:String;let channel:String}
    @State private var target:Target?
    init(workspace:CrewdWorkspace,@ViewBuilder content:@escaping ()->Content){self.workspace=workspace;self.resources=workspace.channelResources;self.content=content}
    private var selection:Binding<String>{Binding(get:{workspace.resourceDrafts.selection(resources.crew,.messages,channel:resources.channel)},set:{workspace.resourceDrafts.select(resources.crew,.messages,$0,channel:resources.channel)})}
    private var classification:CrewdMessageFolderPresentation {
        let value=selection.wrappedValue
        let filter:CrewTimelineFilter.MessageFolder
        if value == "all"{filter = .all}
        else if let snapshot=resources.folders[.messages] {
            let assignments=snapshot.assignments.filter{$0["object"]["kind"] == .string("message")}
            if value == "none"{filter = .exclude(Set(assignments.compactMap{$0["object"]["id"].string}))}
            else{filter = .include(Set(assignments.filter{$0["folderId"] == .string(value)}.compactMap{$0["object"]["id"].string}))}
        }else{filter = .include([])}
        return .init(filter:filter,clear:{selection.wrappedValue="all"},classify:{id in target = .init(id:id,crew:resources.crew,channel:resources.channel)})
    }
    var body:some View {
        VStack(spacing:0){
            content().environment(\.crewdMessageFolders,classification)
        }
        .sheet(item:$target){target in
            if resources.crew == target.crew,resources.channel == target.channel {
                CrewdFolderEditor(store:resources,drafts:$workspace.resourceDrafts,view:.messages,object:CrewdFolderFilter.object("message",target.id))
            }
        }
        .onChange(of:resources.crew+resources.channel){_,_ in target=nil}
    }
}

/// Group membership comes only from channel.workList. The shared WorkStore
/// supplies each linked plan's lifecycle; no second Plan or local membership.
private struct CrewdOriginalChannelProgress: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var group: CrewdChannelWorkStore
    @State private var owner = UUID()
    @State private var failure: String?
    @State private var refreshAttempt = UUID()
    init(workspace: CrewdWorkspace) {
        self.workspace = workspace; self.group = workspace.channelWork
    }
    private var planIDs: Set<String> {
        Set(group.items.compactMap { row in
            guard let id = CrewdChannelWorkStore.workID(row["object"]), id.kind == .plan else { return nil }
            return id.raw
        })
    }
    private var refreshKey: String {
        group.crew + "/" + group.channel + "/" + group.activationEpoch.uuidString
            + "/" + (group.linkRevision ?? "") + "/" + (group.sourceRevision ?? "")
    }
    var body: some View {
        VStack(spacing: 0) {
            if !group.capabilities.contains("channelWork") {
                CrewdEmptySurface(title: "群进展尚不可用", detail: "当前后台尚未开放群工作引用。", symbol: "list.bullet.indent")
            } else if let error = group.failure?.localizedDescription ?? failure {
                VStack(spacing: 12) {
                    Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
                    Button("重新读取本群计划") {
                        failure = nil
                        Task {
                            do { try await group.refresh(); refreshAttempt = UUID() }
                            catch { if !(error is CancellationError) { failure = error.localizedDescription } }
                        }
                    }.disabled(group.loading)
                }.padding(20).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if group.channel.isEmpty || !group.loaded || group.loading {
                ProgressView("正在读取本群计划").frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                CrewdWorkPanels(store: workspace.workStore, resources: workspace.channelResources,
                    resourceDrafts: $workspace.resourceDrafts, drafts: $workspace.requirementDrafts,
                    planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                    members: workspace.sessions, crews: workspace.crews, progressOnly: true,
                    openTodo: { workspace.originalSection = "Todo" }, visiblePlanIDs: planIDs)
            }
        }
        .onAppear { group.setActive(true, owner: owner) }
        .onDisappear { group.setActive(false, owner: owner) }
        .task(id: refreshKey + refreshAttempt.uuidString + group.capabilities.sorted().joined(separator: ",")) {
            guard group.capabilities.contains("channelWork"), !group.channel.isEmpty else { return }
            let crew = group.crew, channel = group.channel
            do {
                if !group.loaded { try await group.refresh() }
                for raw in planIDs.sorted() {
                    try Task.checkCancellation()
                    guard group.crew == crew, group.channel == channel else { return }
                    if let id = CrewdWorkID(raw, kind: .plan) { _ = try await workspace.workStore.detail(id) }
                }
                guard group.crew == crew, group.channel == channel else { return }
                failure = nil
            } catch {
                if !(error is CancellationError), group.crew == crew, group.channel == channel { failure = error.localizedDescription }
            }
        }
    }
}
/// Original right-hand Todo area presents the same authority-owned group
/// requirements and human rulings as the center. No legacy ledger instance.
struct CrewdOriginalTodoOverview: View {
    @ObservedObject var workspace: CrewdWorkspace
    let crewID: String
    var body: some View {
        CrewdOriginalTodoSummary(group: workspace.channelWork, currentCrew: crewID,
            showDetails: { human in
                workspace.originalSection = human ? "真人Todo" : "Todo"
            })
    }
}
private struct CrewdOriginalTodoSummary: View {
    @ObservedObject var group: CrewdChannelWorkStore
    @Environment(\.colorScheme) private var scheme
    let currentCrew: String
    let showDetails: (Bool) -> Void
    @State private var owner = UUID()
    @State private var human=true
    private func items(_ human:Bool)->[CrewdValue]{
        group.items.filter{row in
            CrewdTodoAttention.pending(row,attributes:group.todoAttributes[row["object"]["id"].string ?? ""],human:human)
        }
    }
    private func caption(_ human:Bool)->String {
        let title=human ? "人":"AI"
        guard group.loaded,!group.loading,group.failure == nil,group.crew==currentCrew else{return title}
        let count=items(human).count
        return count>0 ? title+"  \(count)":title
    }
    var body: some View {
        VStack(alignment:.leading,spacing:12){
            HStack{
                Text("待办工作").font(CrewdType.label.weight(.semibold))
                Spacer()
                Button("详细"){showDetails(human)}.buttonStyle(.plain).font(CrewdType.caption).foregroundStyle(CrewdChatStyle.accent(scheme))
            }.foregroundStyle(CrewdChatStyle.muted(scheme))
            Picker("待办归属",selection:$human){Text(caption(true)).tag(true);Text(caption(false)).tag(false)}
                .pickerStyle(.segmented).labelsHidden().controlSize(.small)
            if group.crew != currentCrew || group.loading {
                ProgressView("正在读取当前群").controlSize(.small).font(.caption)
            }else if let failure=group.failure {
                CrewdInlineNotice(title:"待办工作暂未更新",detail:failure.localizedDescription,symbol:"exclamationmark.triangle",warning:true)
            }else if !group.capabilities.contains("channelWork") {
                Text("当前后台尚未开放群工作引用。").font(CrewdType.caption).foregroundStyle(CrewdChatStyle.muted(scheme))
            }else if !group.loaded {
                Text("尚未读取当前会话事项").font(CrewdType.caption).foregroundStyle(CrewdChatStyle.muted(scheme))
            }else if items(human).isEmpty {
                Text(human ? "暂无待处理的人工待办":"暂无未完成的 AI 待办")
                    .font(CrewdType.caption).foregroundStyle(CrewdChatStyle.muted(scheme)).padding(.vertical,12)
            }else {
                let rows=items(human)
                ForEach(rows.indices,id:\.self){index in
                    let row=rows[index]
                    Button{showDetails(human)}label:{
                        HStack(alignment:.top,spacing:8){
                            Circle().strokeBorder(human ? Theme.Palette.amber:CrewdChatStyle.accent(scheme),lineWidth:1.2).frame(width:12,height:12).padding(.top,3)
                            VStack(alignment:.leading,spacing:6){
                                Text(row["label"].string ?? "关联工作").font(CrewdType.body).lineLimit(4).foregroundStyle(CrewdChatStyle.ink(scheme))
                                Text(CrewdWorkStatusLabel.display(row["status"].string ?? ""))
                                    .font(CrewdType.caption).foregroundStyle(CrewdChatStyle.muted(scheme))
                            }.frame(maxWidth:.infinity,alignment:.leading)
                        }.padding(.vertical,9).padding(.horizontal,10)
                            .background(CrewdChatStyle.sidebar(scheme).opacity(0.25),in:RoundedRectangle(cornerRadius:7))
                    }.buttonStyle(.plain)
                }
            }
        }.padding(12)
        .onAppear { group.setActive(true, owner: owner) }
        .onDisappear { group.setActive(false, owner: owner) }
        .task(id: group.crew + group.channel + group.capabilities.sorted().joined(separator: ",") + group.activationEpoch.uuidString) {
            if group.capabilities.contains("channelWork"), !group.channel.isEmpty { try? await group.refresh() }
        }
    }
}

/// One group presentation shared by both clients. Phone navigation changes
/// only the visible column; business data and commands stay in the workspace.
private struct CrewdOriginalMessages<Content: View>: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var resources: CrewdResources
    @ViewBuilder let content: () -> Content
    let header: AnyView?
    let openSession: ((String) -> Void)?
    init(workspace: CrewdWorkspace, header: AnyView? = nil, openSession: ((String) -> Void)? = nil,
         @ViewBuilder content: @escaping () -> Content) {
        self.workspace = workspace; self.resources = workspace.resources; self.content = content
        self.header = header; self.openSession = openSession
    }
    @Environment(\.colorScheme) private var scheme
    #if os(iOS)
    @Environment(\.crewdMessageSearchTarget) private var messageSearchTarget
    @State private var mobileChat = false
    @State private var showingNewMember = false
    @State private var showingMobileInspector = false
    #endif
    #if os(macOS)
    @EnvironmentObject private var runner: CrewSessionRunner
    @State private var showingMembers = false
    @State private var openedSession = false
    @State private var showConversationPopover = false
    @State private var conversationListVisible = true
    private var showingSession: Bool { runner.viewingTerminal }
    private var visibleChat: Bool { !showingSession && workspace.channels.contains { $0["id"].string == workspace.channelID } }
    private var visibleContent: Bool { workspace.originalSection != "消息" || visibleChat }
    private var visibleSession: Bool { workspace.originalSection == "消息" && showingSession }
    #endif
    var body: some View {
        #if os(macOS)
        GeometryReader { geometry in
        HStack(spacing: 0) {
            if geometry.size.width >= 600 && conversationListVisible {
                conversationList.frame(width: min(208, geometry.size.width * 0.30))
                Divider()
            }
            VStack(spacing: 0) {
                if geometry.size.width < 600 || !conversationListVisible {
                    HStack {
                        Button { if geometry.size.width>=600 {conversationListVisible=true} else {showConversationPopover=true} } label: {
                            Label("群聊与会话", systemImage: "bubble.left.and.bubble.right")
                        }.buttonStyle(.plain).font(.system(size: 12))
                        Button("成员",systemImage:"person.2"){showingMembers=true}.buttonStyle(.plain)
                        Spacer()
                        if showingSession {
                            Button("返回群聊") { runner.viewingTerminal = false }
                                .buttonStyle(.plain).font(.system(size: 12))
                        }
                    }.padding(.horizontal, 12).frame(height: 32)
                        .popover(isPresented: $showConversationPopover) {
                            conversationList.frame(width: 248, height: 440)
                        }
                    if geometry.size.width<600 {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(workspace.channels.compactMap { $0["id"].string }, id: \.self) { id in
                                if let row = workspace.channels.first(where: { $0["id"].string == id }) {
                                    Button {
                                        workspace.selectConversation(id); runner.viewingTerminal = false
                                    } label: {
                                        Text(CrewdConversationProjection.title(row))
                                            .font(.system(size: 12, weight: !showingSession && workspace.channelID == id ? .semibold : .regular))
                                            .padding(.horizontal, 10).padding(.vertical, 5)
                                            .background(!showingSession && workspace.channelID == id ? CrewdChatStyle.bubble(scheme) : .clear, in: Capsule())
                                    }.buttonStyle(.plain)
                                }
                            }
                        }.padding(.horizontal, 8)
                    }.frame(height: 32)
                    }
                    Divider()
                }
            if let header { header }
            ZStack {
                content()
                    .opacity(visibleContent ? 1 : 0).allowsHitTesting(visibleContent).disabled(!visibleContent).accessibilityHidden(!visibleContent)
                if openedSession || showingSession {
                    CrewSessionWindowView(presentation: .conversation)
                        .opacity(visibleSession ? 1 : 0).allowsHitTesting(visibleSession)
                        .disabled(!visibleSession).accessibilityHidden(!visibleSession)
                }
                if workspace.originalSection == "消息" && !showingSession && !visibleChat {
                    CrewdEmptySurface(title: "选择一段会话", detail: "从左侧选择群聊或成员。不同会话的消息和草稿分别保留。", symbol: "bubble.left.and.bubble.right")
                }
            }.frame(minWidth: 0, maxWidth: .infinity, maxHeight: .infinity)
            }.frame(minWidth: 0, maxWidth: .infinity, maxHeight: .infinity)
        }.frame(width: geometry.size.width, height: geometry.size.height)
        }
        .opacity(showingMembers ? 0:1).allowsHitTesting(!showingMembers).accessibilityHidden(showingMembers)
        .overlay {
            if showingMembers {
                VStack(spacing:0){
                    HStack{Button("返回会话",systemImage:"chevron.left"){showingMembers=false};Spacer();Text("本群成员与工作")}.padding(12)
                    Divider();CrewSessionWindowView(presentation:.inspector)
                }.background(Theme.Palette.canvas)
            }
        }
        .onChange(of:workspace.crewID){_,_ in showingMembers=false}
        .onChange(of:workspace.originalSection){_,_ in showingMembers=false}
        .background(CrewdConversationFocusReset(key: workspace.originalSection + workspace.crewID + workspace.channelID + (showingSession ? runner.selectedRunId?.uuidString ?? "new" : "channel")))
        .onChange(of: showingSession) { _, value in if value { openedSession = true } }
        .task(id: workspace.originalSection + resources.activationEpoch.uuidString) {
            if workspace.originalSection == "消息" { await resources.activate(.messages, force: true) }
        }
        #else
        GeometryReader { geometry in
            let wide = geometry.size.width >= 650
            HStack(spacing: 0) {
                if wide { mobileConversationList.frame(width: 210); Divider() }
                VStack(spacing: 0) {
                    HStack {
                        if !wide {Button { mobileChat = false } label: { Label("会话", systemImage: "chevron.left") }}
                        Spacer()
                        Button{showingMobileInspector=true}label:{Label("待办工作与成员",systemImage:"sidebar.right")}
                            .buttonStyle(CrewdToolButtonStyle()).accessibilityLabel("查看当前群的待办工作与成员")
                    }.font(CrewdType.label).frame(minHeight:44).padding(.horizontal,14)
                    if let header { header }
                    content()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .opacity(wide || mobileChat ? 1 : 0)
                .allowsHitTesting(wide || mobileChat).disabled(!wide && !mobileChat)
                .accessibilityHidden(!wide && !mobileChat)
            }
            .overlay {
                if !wide && !mobileChat { mobileConversationList }
            }
        }
        .background(CrewdConversationFocusReset(key: workspace.originalSection + workspace.crewID + workspace.channelID + (mobileChat ? "detail" : "list")))
        .opacity(showingMobileInspector ? 0:1).allowsHitTesting(!showingMobileInspector).accessibilityHidden(showingMobileInspector)
        .overlay {
            if showingMobileInspector {
                VStack(spacing:0){
                    HStack{Button("返回会话",systemImage:"chevron.left"){showingMobileInspector=false};Spacer();Text("本群成员")}.padding(12)
                    ScrollView{VStack(alignment:.leading,spacing:18){
                        CrewdOriginalTodoSummary(group:workspace.channelWork,currentCrew:workspace.crewID,showDetails:{human in workspace.originalSection=human ? "真人Todo":"Todo";mobileChat=true;showingMobileInspector=false})
                        ForEach(workspace.selectedMembers.indices,id:\.self){index in
                            let member=workspace.selectedMembers[index];let id=member["id"].string ?? ""
                            HStack{CrewdMemberAvatar(seed:CrewdAvatarIdentity.seed(member) ?? id,colorSeed:workspace.channelID)
                                if member["kind"].string=="human"{Text("你")}
                                else{Button(workspace.knownSessions[id]?["displayName"].string ?? "成员资料尚未加载"){workspace.selectSession(id);showingMobileInspector=false;openSession?(id)}.disabled(openSession==nil || id.isEmpty)}
                            }.frame(minHeight:44)
                        }
                    }.padding(16)}
                }.background(Theme.Palette.canvas)
            }
        }
        .onChange(of:workspace.originalSection){_,_ in showingMobileInspector=false}
        .onChange(of:messageSearchTarget,initial:true){_,id in if id != nil{mobileChat=true;showingMobileInspector=false}}
        .sheet(isPresented:$showingNewMember) {
            NavigationStack {
                ScrollView {CrewdSessionStartView(workspace:workspace,drafts:$workspace.messageDrafts).padding(20)}
                    .navigationTitle("新成员会话").navigationBarTitleDisplayMode(.inline)
                    .toolbar {ToolbarItem(placement:.cancellationAction){Button("关闭"){showingNewMember=false}}}
            }
        }
        .onChange(of: workspace.crewID) { _, _ in mobileChat = false; showingNewMember=false;showingMobileInspector=false }
        .onChange(of: workspace.knowledgeNavigation) { _, _ in
            workspace.originalSection = "消息"; mobileChat = true
        }
        #endif
    }
    private var messageFolderMenu:some View {
        CrewdFolderControls(store:workspace.channelResources,drafts:$workspace.resourceDrafts,
            selection:Binding(get:{workspace.resourceDrafts.selection(workspace.crewID,.messages,channel:workspace.channelID)},
                              set:{workspace.resourceDrafts.select(workspace.crewID,.messages,$0,channel:workspace.channelID)}),
            view:.messages,compact:true,iconOnly:true)
    }
    #if os(iOS)
    private var mobileConversationList: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 3) {
                HStack {
                    Text("会话").font(CrewdType.label.weight(.medium))
                    Spacer()
                    messageFolderMenu
                    Button{showingNewMember=true}label:{Image(systemName:"plus")}
                        .buttonStyle(CrewdToolButtonStyle()).accessibilityLabel("新建成员会话")
                }.foregroundStyle(CrewdChatStyle.muted(scheme)).padding(.horizontal,12).padding(.vertical,4)
                ForEach(workspace.channels.compactMap { $0["id"].string }, id: \.self) { id in
                    if let row = workspace.channels.first(where: { $0["id"].string == id }) {
                        CrewdConversationRow(title: CrewdConversationProjection.title(row), subtitle: conversationSubtitle(row),
                            selected: workspace.channelID == id,
                            action: { workspace.selectConversation(id); mobileChat = true }) {
                                channelAvatar(row)
                            }.accessibilityHint(CrewdConversationProjection.context(row))
                    }
                }
                if !workspace.sessions.isEmpty {
                    Text("Agent 会话").font(.system(size: 11, weight: .medium))
                        .foregroundStyle(CrewdChatStyle.muted(scheme)).padding(12)
                }
                ForEach(workspace.sessions.compactMap { $0["id"].string }, id: \.self) { id in
                    if let row = workspace.sessions.first(where: { $0["id"].string == id }) {
                        CrewdConversationRow(title: row["displayName"].string ?? row["role"].string ?? "成员",
                            subtitle: (row["executor"].string ?? "") + " · 成员会话", selected: false,
                            action: { workspace.selectSession(id); openSession?(id) }) {
                                CrewdMemberAvatar(seed: id, colorSeed: workspace.crewID)
                            }.disabled(openSession == nil)
                    }
                }
            }.padding(.horizontal, 5).frame(maxWidth: .infinity, alignment: .leading)
        }.background(CrewdChatStyle.canvas(scheme))
    }
    @ViewBuilder private func channelAvatar(_ channel: CrewdValue) -> some View {
        let id = channel["id"].string ?? ""
        let members = workspace.channelSummaries.first { $0["channel"]["id"].string == id }?["members"].array ?? []
        if channel["kind"].string == "dm", let peer = members.compactMap(CrewdAvatarIdentity.seed).first(where: { $0 != "human" }) {
            CrewdMemberAvatar(seed: peer, colorSeed: id)
        } else {
            CrewdAttentionAvatar(group:workspace.channelWork,crew:workspace.crewID,channel:id){
                CrewdGroupAvatar(members:members,channel:id,name:CrewdConversationProjection.title(channel))
            }
        }
    }
    #endif
    private func conversationSubtitle(_ channel: CrewdValue) -> String {
        let summary = workspace.channelSummaries.first { $0["channel"]["id"] == channel["id"] }
        let mentions = summary?["humanMentions"].int ?? 0
        let unread = summary?["unreadForHuman"].int ?? 0
        if mentions > 0 { return "\(mentions) 条消息 @你" }
        if unread > 0 { return "\(unread) 条未读消息" }
        return channel["purpose"].string ?? (channel["kind"].string == "dm" ? "群内单聊" : "机组群聊")
    }
    #if os(macOS)
    private var conversationList: some View {
        VStack(spacing: 0) {
            HStack {
                Text("会话").font(.system(size: 11, weight: .medium))
                Spacer()
                messageFolderMenu
                Button{showingMembers=true}label:{Image(systemName:"person.2")}.buttonStyle(CrewdToolButtonStyle()).help("本群成员与工作")
                Button{conversationListVisible=false}label:{Image(systemName:"sidebar.left")}
                    .buttonStyle(CrewdToolButtonStyle()).help("收起会话列表").accessibilityLabel("收起会话列表")
                Button { workspace.originalSection = "消息"; runner.composeNew(); runner.viewingTerminal = true; openedSession = true; showConversationPopover = false } label: { Image(systemName: "plus") }
                    .buttonStyle(.plain).help("新建成员会话")
            }.foregroundStyle(CrewdChatStyle.muted(scheme)).padding(.horizontal, 12).frame(height: 36)
            ScrollView {
                VStack(spacing: 3) {
                    ForEach(workspace.channels.compactMap { $0["id"].string }, id: \.self) { id in
                        if let row = workspace.channels.first(where: { $0["id"].string == id }) {
                            CrewdConversationRow(title: CrewdConversationProjection.title(row), subtitle: conversationSubtitle(row),
                                selected: !visibleSession && workspace.channelID == id,
                                action: { workspace.selectConversation(id); runner.viewingTerminal = false; showConversationPopover = false }) {
                                CrewdAttentionAvatar(group:workspace.channelWork,crew:workspace.crewID,channel:id){
                                    CrewdGroupAvatar(members: workspace.channelSummaries.first(where: { $0["channel"]["id"].string == id })?["members"].array ?? [], channel: id, name: CrewdConversationProjection.title(row))
                                }
                            }.help(CrewdConversationProjection.context(row))
                        }
                    }
                    if !workspace.sessions.isEmpty {
                        HStack { Text("Agent 会话").font(.system(size: 10, weight: .medium)); Spacer() }
                            .foregroundStyle(CrewdChatStyle.muted(scheme)).padding(.horizontal, 10).padding(.top, 14).padding(.bottom, 3)
                    }
                    ForEach(workspace.sessions.compactMap { $0["id"].string }, id: \.self) { id in
                        if let row = workspace.sessions.first(where: { $0["id"].string == id }) {
                            CrewdConversationRow(title: row["displayName"].string ?? row["title"].string ?? row["role"].string ?? "成员",
                                subtitle: (row["executor"].string ?? "") + " · 成员会话",
                                selected: showingSession && workspace.sessionID == id && !runner.isComposingNew,
                                action: {
                                    guard let run = runner.runs.first(where: { $0.sessionId == id && $0.crewId == workspace.crewID }) else { return }
                                    workspace.originalSection = "消息"; runner.openSession(run.runID); openedSession = true; showConversationPopover = false
                                }) { CrewdMemberAvatar(seed: id, colorSeed: workspace.crewID) }
                                .disabled(!runner.runs.contains { $0.sessionId == id && $0.crewId == workspace.crewID })
                        }
                    }
                }.padding(.horizontal, 5).padding(.top, 3)
            }
        }.background(CrewdChatStyle.canvas(scheme))
    }
    #endif
}
#if os(macOS)
/// Reset only this presentation window's keyboard focus when the visible route
/// changes. Hidden, retained composers must not consume the next keystroke.
private struct CrewdConversationFocusReset: NSViewRepresentable {
    let key: String
    final class Coordinator { var key: String? }
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> NSView { NSView(frame: .zero) }
    func updateNSView(_ view: NSView, context: Context) {
        defer { context.coordinator.key = key }
        if let old = context.coordinator.key, old != key { view.window?.makeFirstResponder(nil) }
    }
}
#endif
#if os(iOS)
private struct CrewdConversationFocusReset: UIViewRepresentable {
    let key: String
    final class Coordinator { var key: String? }
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeUIView(context: Context) -> UIView { UIView(frame: .zero) }
    func updateUIView(_ view: UIView, context: Context) {
        defer { context.coordinator.key = key }
        if let old = context.coordinator.key, old != key { view.window?.endEditing(true) }
    }
}
#endif
