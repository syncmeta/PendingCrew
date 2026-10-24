import SwiftUI
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif

/// Only the sidebar owns its navigation path. Returning never changes the work route.
private struct CrewdOpenSidebarConversationsKey: EnvironmentKey { static let defaultValue: ((String) -> Void)? = nil }
extension EnvironmentValues {
    var crewdOpenSidebarConversations: ((String) -> Void)? {
        get { self[CrewdOpenSidebarConversationsKey.self] }
        set { self[CrewdOpenSidebarConversationsKey.self] = newValue }
    }
}

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
    #if os(macOS)
    var control:AnyView? = nil
    #endif
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
            #if os(macOS)
            sectionContent
            #else
            CrewdOriginalMessages(workspace:workspace,header:nil,openSession:openSession){sectionContent}
            #endif
        }
    }
    private var sectionHeader:some View {
        VStack(spacing:0){
            HStack(spacing:12){
                CrewdGlassWorkspaceTabs(selection:$workspace.originalSection)
                    .frame(maxWidth:.infinity,alignment:.leading)
                if let searchControl { searchControl }
            }.padding(.horizontal,14).padding(.vertical,8)
        }
    }
    private var sectionContent: some View {
        VStack(spacing: 0) {
            ZStack {
                // Keep the original chat instance mounted: drafts, anchors and
                // its existing pagination/identity adapter survive other panels.
                Group {
                    #if os(macOS)
                    CrewdOriginalMessages(workspace:workspace,header:nil,openSession:openSession){
                        CrewdOriginalMessageSurface(workspace:workspace){content()}
                            .environment(\.crewdOriginalChannelID, workspace.channelID).id(workspace.channelID)
                    }
                    #else
                    CrewdOriginalMessageSurface(workspace:workspace){content()}
                        .environment(\.crewdOriginalChannelID, workspace.channelID).id(workspace.channelID)
                    #endif
                }
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
        var presentation = CrewdMessageFolderPresentation(filter:filter,clear:{selection.wrappedValue="all"},classify:{id in target = .init(id:id,crew:resources.crew,channel:resources.channel)})
        #if os(macOS)
        presentation.control = AnyView(CrewdFolderControls(store:resources,drafts:$workspace.resourceDrafts,
            selection:selection,view:.messages,compact:true).fixedSize(horizontal:true,vertical:false))
        #endif
        return presentation
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
        Group {
            #if os(macOS)
            macOverview
            #else
            standardOverview
            #endif
        }
        .onAppear { group.setActive(true, owner: owner) }
        .onDisappear { group.setActive(false, owner: owner) }
        .task(id: group.crew + group.channel + group.capabilities.sorted().joined(separator: ",") + group.activationEpoch.uuidString) {
            if group.capabilities.contains("channelWork"), !group.channel.isEmpty { try? await group.refresh() }
        }
    }
    #if os(macOS)
    /// Original CrewTodoPanel geometry, fed only by the current channel work
    /// projection. No legacy LocalTodoStore reader or detail action is used.
    private var macOverview: some View {
        VStack(alignment:.leading,spacing:0) {
            HStack(spacing:6) {
                Text("待办工作").font(Theme.Fonts.headline.weight(.semibold))
                CrewTodoLedgerPills(ledger:Binding(get:{human ? .human:.agent},set:{human = $0 == .human}),contextHelp:{ledger in
                    ledger == .human ? "查看当前群需要人处理的关联工作":"查看当前群 AI 的关联工作"
                })
                Spacer(minLength:8)
                Button(TodoListPresentation.overviewLayout.detailButtonTitle){showDetails(human)}
                    .buttonStyle(.bordered).controlSize(.small).font(Theme.Fonts.caption).tint(Theme.Palette.accent)
            }.padding(.horizontal,14).padding(.vertical,10)
            if group.crew != currentCrew || group.loading {
                ProgressView("正在读取当前群").controlSize(.small).font(Theme.Fonts.caption).padding(.horizontal,14)
            } else if let failure=group.failure {
                CrewdInlineNotice(title:"待办工作暂未更新",detail:failure.localizedDescription,symbol:"exclamationmark.triangle",warning:true).padding(.horizontal,14)
            } else if !group.capabilities.contains("channelWork") {
                Text("当前后台尚未开放群工作引用。").font(Theme.Fonts.caption).foregroundStyle(.secondary).padding(.horizontal,14)
            } else if !group.loaded {
                Text("尚未读取当前会话事项").font(Theme.Fonts.caption).foregroundStyle(.secondary).padding(.horizontal,14)
            } else if items(human).isEmpty {
                Text(human ? "暂无待处理的人工待办":"暂无未完成的 AI 待办")
                    .font(Theme.Fonts.caption).foregroundStyle(.secondary).padding(.horizontal,14).padding(.bottom,6)
            } else {
                let rows=items(human)
                LazyVStack(alignment:.leading,spacing:8) {
                    ForEach(rows.indices,id:\.self){index in macTodoRow(rows[index])}
                }.padding(.horizontal,12).padding(.bottom,8)
            }
        }.frame(maxWidth:.infinity,alignment:.leading).background(CrewTodoPagePalette.background)
    }
    private func macTodoRow(_ row:CrewdValue)->some View {
        let layout=TodoListPresentation.overviewLayout
        let corners=layout.cardCorners
        let shape=UnevenRoundedRectangle(cornerRadii:.init(topLeading:CGFloat(corners.topLeading),bottomLeading:CGFloat(corners.bottomLeading),bottomTrailing:CGFloat(corners.bottomTrailing),topTrailing:CGFloat(corners.topTrailing)),style:.continuous)
        return VStack(alignment:.leading,spacing:7) {
            HStack(spacing:6) {
                CrewTodoStatusCircle(status:row["status"].string ?? "",size:15)
                Text(CrewdWorkStatusLabel.display(row["status"].string ?? ""))
                    .font(Theme.Fonts.footnote.weight(.semibold)).foregroundStyle(Theme.Palette.accent)
            }
            MarkdownText(text:TodoListPresentation.cardMarkdown(row["label"].string ?? "关联工作",lineBudget:layout.bodyLineLimit),variant:.todo)
                .lineLimit(layout.bodyLineLimit).textSelection(.enabled)
                .frame(maxHeight:CGFloat(layout.previewMaxHeight),alignment:.top).clipped()
            Button(layout.rowDetailTitle){showDetails(human)}
                .buttonStyle(.plain).font(Theme.Fonts.caption.weight(.semibold)).foregroundStyle(Theme.Palette.accent)
        }.padding(.vertical,10).padding(.horizontal,12)
            .frame(maxWidth:.infinity,alignment:.leading)
            .frame(maxHeight:CGFloat(layout.cardMaxHeight),alignment:.top).clipped()
            .background(shape.fill(CrewTodoPagePalette.card))
            .overlay(shape.strokeBorder(Theme.Palette.hairline,lineWidth:0.5))
            .padding(.vertical,4).contentShape(Rectangle()).onTapGesture{showDetails(human)}
    }
    #endif
    private var standardOverview: some View {
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
    @State private var showingMobileInspector = false
    #endif
    #if os(macOS)
    @EnvironmentObject private var runner: CrewSessionRunner
    private var showingSession: Bool { runner.viewingTerminal }
    #endif
    var body: some View {
        #if os(macOS)
        VStack(spacing: 0) {
            if let header { header }
            GeometryReader { available in
                // The pre-rewrite work area kept group chat and the member/
                // terminal pane side by side. The new shared tabs stay above
                // both; bound the old minimums to the actual available width.
                HSplitView {
                    // Split sizing belongs to the pane, not the ideal width of
                    // its current chat, roster or terminal contents. In particular,
                    // entering the terminal must not grow the entire split view.
                    GeometryReader { pane in
                        content().frame(width:pane.size.width,height:pane.size.height)
                    }
                    .frame(minWidth:min(360,max(0,(available.size.width-1)/2)),maxWidth:.infinity,maxHeight:.infinity)
                    GeometryReader { pane in
                        CrewSessionWindowView(conversationVisible:workspace.originalSection == "消息")
                            .frame(width:pane.size.width,height:pane.size.height)
                    }
                    .frame(minWidth:min(320,max(0,(available.size.width-1)/2)),idealWidth:400,maxWidth:560,maxHeight:.infinity)
                }
                .frame(maxWidth:.infinity,maxHeight:.infinity)
            }
        }.frame(minWidth:0,maxWidth:.infinity,maxHeight:.infinity)
        .background(CrewdConversationFocusReset(key: workspace.originalSection + workspace.crewID + workspace.channelID + (showingSession ? runner.selectedRunId?.uuidString ?? "new" : "channel")))
        .task(id: workspace.originalSection + resources.activationEpoch.uuidString) {
            if workspace.originalSection == "消息" { await resources.activate(.messages, force: true) }
        }
        #else
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button { showingMobileInspector = true } label: { Label("待办工作与成员", systemImage: "sidebar.right") }
                    .buttonStyle(CrewdToolButtonStyle()).accessibilityLabel("查看当前群的待办工作与成员")
            }.font(CrewdType.label).frame(minHeight: 44).padding(.horizontal, 14)
            if let header { header }
            content().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(CrewdConversationFocusReset(key: workspace.originalSection + workspace.crewID + workspace.channelID))
        .opacity(showingMobileInspector ? 0:1).allowsHitTesting(!showingMobileInspector).accessibilityHidden(showingMobileInspector)
        .overlay {
            if showingMobileInspector {
                VStack(spacing:0){
                    HStack{Button("返回会话",systemImage:"chevron.left"){showingMobileInspector=false};Spacer();Text("本群成员")}.padding(12)
                    ScrollView{VStack(alignment:.leading,spacing:18){
                        CrewdOriginalTodoSummary(group:workspace.channelWork,currentCrew:workspace.crewID,showDetails:{human in workspace.originalSection=human ? "真人Todo":"Todo";showingMobileInspector=false})
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
        .onChange(of:messageSearchTarget,initial:true){_,id in if id != nil{showingMobileInspector=false}}
        .onChange(of: workspace.crewID) { _, _ in showingMobileInspector=false }
        .onChange(of: workspace.knowledgeNavigation) { _, _ in
            workspace.originalSection = "消息"
        }
        #endif
    }

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

/// The single conversation directory lives inside the existing sidebar stack.
/// Its actions only select existing projections; execution still needs explicit controls.
struct CrewdSidebarConversations: View {
    @ObservedObject var workspace: CrewdWorkspace
    let crewID: String
    var openSession: ((String) -> Void)? = nil
    var showContent: () -> Void = {}
    var filtersChannelFolders = false
    var back: (() -> Void)? = nil
    @Environment(\.colorScheme) private var scheme
    #if os(macOS)
    @EnvironmentObject private var runner: CrewSessionRunner
    #else
    @State private var showingNewMember = false
    #endif
    private struct Item: Identifiable {
        let id: String
        let value: CrewdValue
        let channel: Bool
        let order: Int
        var date: String { value["updatedAt"].string ?? value["createdAt"].string ?? "" }
    }
    private var items: [Item] {
        let visibleChannels = workspace.channels.filter { channel in
            !filtersChannelFolders || CrewdFolderFilter.includes(
                CrewdFolderFilter.object("channel", channel["id"].string ?? ""),
                selection: workspace.resourceDrafts.selection(crewID, .messages), snapshot: workspace.resources.folders[.messages])
        }
        let channels = visibleChannels.enumerated().compactMap { index, row in
            row["id"].string.map { Item(id: "channel/" + $0, value: row, channel: true, order: index) }
        }
        let members = workspace.sessions.enumerated().compactMap { index, row in
            row["id"].string.map { Item(id: "session/" + $0, value: row, channel: false, order: channels.count + index) }
        }
        return (channels + members).sorted { $0.date == $1.date ? $0.order < $1.order : $0.date > $1.date }
    }
    var body: some View {
        VStack(spacing: 0) {
            #if os(macOS)
            HStack(spacing: 6) {
                if let back {
                    Button("机组", systemImage: "chevron.left", action: back)
                        .buttonStyle(.borderless).accessibilityLabel("返回机组选择")
                } else { Text("会话").font(.headline) }
                Spacer(minLength: 0)
                directoryTools
            }.controlSize(.small).frame(minHeight: 36).padding(.horizontal, 10)
            #endif
            directory.frame(maxWidth: .infinity, maxHeight: .infinity)
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
        #if os(iOS)
        .navigationTitle("会话")
        .toolbar { ToolbarItem(placement: .primaryAction) { directoryTools } }
        .sheet(isPresented: $showingNewMember) {
            NavigationStack {
                ScrollView { CrewdSessionStartView(workspace: workspace, drafts: $workspace.messageDrafts).padding(20) }
                    .navigationTitle("新成员会话").navigationBarTitleDisplayMode(.inline)
                    .toolbar { ToolbarItem(placement: .cancellationAction) { Button("关闭") { showingNewMember = false } } }
            }
        }
        #endif
    }
    private var directory: some View {
        Group {
            if workspace.crewID == crewID {
                #if os(macOS)
                // Use the same native sidebar List container as the crew root, so
                // both pages inherit identical outer row margins from the host.
                List { directoryRows }
                    .listStyle(.sidebar)
                    .scrollContentBackground(.hidden)
                    .background(Color.clear)
                #else
                ScrollView {
                    LazyVStack(spacing: 0) { directoryRows }
                        .padding(.horizontal, 5).padding(.vertical, 8)
                }
                #endif
            } else { ProgressView("正在读取会话…").frame(maxWidth: .infinity, maxHeight: .infinity) }
        }
    }
    @ViewBuilder
    private var directoryRows: some View {
        if filtersChannelFolders {
            CrewdFolderControls(store: workspace.resources, drafts: $workspace.resourceDrafts,
                selection: Binding(get: { workspace.resourceDrafts.selection(crewID, .messages) },
                                   set: { workspace.resourceDrafts.select(crewID, .messages, $0) }), view: .messages)
        }
        ForEach(items) { item in
            if item.channel { channelRow(item.value) }
            else { sessionRow(item.value) }
        }
    }
    private var directoryTools: some View {
        HStack(spacing: 6) {
            CrewdFolderControls(store: workspace.channelResources, drafts: $workspace.resourceDrafts,
                selection: Binding(get: { workspace.resourceDrafts.selection(crewID, .messages, channel: workspace.channelID) },
                                   set: { workspace.resourceDrafts.select(crewID, .messages, $0, channel: workspace.channelID) }),
                view: .messages, compact: true, iconOnly: true)
            Button("新建成员会话", systemImage: "plus") {
                guard workspace.crewID == crewID else { return }
                #if os(macOS)
                workspace.originalSection = "消息"; runner.composeNew(); runner.viewingTerminal = true
                #else
                showingNewMember = true
                #endif
                showContent()
            }.labelStyle(.iconOnly).disabled(workspace.crewID != crewID)
        }
    }
    private func subtitle(_ channel: CrewdValue) -> String {
        let summary = workspace.channelSummaries.first { $0["channel"]["id"] == channel["id"] }
        let mentions = summary?["humanMentions"].int ?? 0, unread = summary?["unreadForHuman"].int ?? 0
        if mentions > 0 { return "\(mentions) 条消息 @你" }
        if unread > 0 { return "\(unread) 条未读消息" }
        return channel["purpose"].string ?? (channel["kind"].string == "dm" ? "群内单聊" : "")
    }
    private func channelRow(_ channel: CrewdValue) -> some View {
        let id = channel["id"].string ?? ""
        let members = workspace.channelSummaries.first { $0["channel"]["id"].string == id }?["members"].array ?? []
        #if os(macOS)
        let selected = !runner.viewingTerminal && workspace.channelID == id
        #else
        let selected = workspace.channelID == id
        #endif
        return CrewdConversationRow(title: CrewdConversationProjection.title(channel), subtitle: subtitle(channel), selected: selected, sidebarStyle: true, activityDate: (channel["updatedAt"].string ?? channel["createdAt"].string).flatMap { CrewTimestamp.parse($0) }, action: {
            guard workspace.crewID == crewID else { return }
            if workspace.channelID != id { workspace.selectConversation(id) }
            #if os(macOS)
            runner.viewingTerminal = false
            #endif
            showContent()
        }) {
            if channel["kind"].string == "dm", let peer = members.compactMap(CrewdAvatarIdentity.seed).first(where: { $0 != "human" }) {
                CrewdMemberAvatar(seed: peer, colorSeed: id)
            } else {
                CrewdAttentionAvatar(group: workspace.channelWork, crew: crewID, channel: id) {
                    CrewdGroupAvatar(members: members, channel: id, name: CrewdConversationProjection.title(channel))
                }
            }
        }.help(CrewdConversationProjection.context(channel))
    }
    private func sessionRow(_ session: CrewdValue) -> some View {
        let id = session["id"].string ?? ""
        #if os(macOS)
        let run = runner.runs.first { $0.sessionId == id && $0.crewId == crewID }
        let selected = runner.viewingTerminal && workspace.sessionID == id && !runner.isComposingNew
        let enabled = run != nil
        #else
        let selected = workspace.sessionID == id
        let enabled = openSession != nil
        #endif
        return CrewdConversationRow(title: session["displayName"].string ?? session["title"].string ?? session["role"].string ?? "成员", subtitle: (session["executor"].string ?? "") + " · 成员会话", selected: selected, sidebarStyle: true, activityDate: (session["updatedAt"].string ?? session["createdAt"].string).flatMap { CrewTimestamp.parse($0) }, action: {
            guard workspace.crewID == crewID else { return }
            #if os(macOS)
            guard let run else { return }
            runner.openSession(run.runID)
            #else
            workspace.selectSession(id); openSession?(id)
            #endif
            workspace.originalSection = "消息"; showContent()
        }) { CrewdMemberAvatar(seed: id, colorSeed: crewID) }.disabled(!enabled)
    }
}
