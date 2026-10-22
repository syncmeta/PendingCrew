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

struct CrewdOriginalWorkIntegration: ViewModifier {
    let workspace: CrewdWorkspace?
    var openSession: ((String) -> Void)? = nil
    func body(content: Content) -> some View {
        if let workspace { CrewdOriginalCrewContent(workspace: workspace, openSession: openSession) { content } }
        else { content }
    }
}
private struct CrewdOriginalCrewContent<Content: View>: View {
    @ObservedObject var workspace: CrewdWorkspace
    @EnvironmentObject private var crewStore: CrewStore
    @Environment(\.colorScheme) private var scheme
    @ViewBuilder let content: () -> Content
    var openSession: ((String) -> Void)? = nil
    @State private var showingCrewLedger = false
    @State private var todoQueries: [String: String] = [:]
    @State private var todoTags: [String: String] = [:]
    var body: some View {
        CrewdOriginalMessages(workspace: workspace, header: AnyView(sectionHeader), openSession: openSession) { sectionContent }
    }
    private var sectionHeader: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
            ScrollView(.horizontal, showsIndicators: false) {
                Picker("当前群内容", selection: $workspace.originalSection) {
                    Text("消息").tag("消息")
                    Text("人待办").tag("真人Todo")
                    Text("AI待办").tag("Todo")
                    Text("文档/数据").tag("文档")
                    Text("计划/进展").tag("进展")
                }.pickerStyle(.segmented).labelsHidden().fixedSize()
                    .accessibilityLabel("当前群内容")
            }.frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 0)
            Menu {
                Button("机组工作总账") { showingCrewLedger = true }
            } label: { Image(systemName: "ellipsis") }
                #if os(macOS)
                .menuStyle(.borderlessButton)
                #endif
                .fixedSize().help("机组共享工作与全组织收件箱")
            }
                .padding(.horizontal, 14).padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(CrewdChatStyle.sidebar(scheme).opacity(0.30))
            Divider().overlay(CrewdChatStyle.line(scheme))
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
                    channelTodo(human: workspace.originalSection == "真人Todo").padding(14)
                } else if workspace.originalSection == "文档" {
                    CrewdKnowledgeDocumentsEntry(workspace: workspace)
                } else if workspace.originalSection == "进展" {
                    CrewdOriginalChannelProgress(workspace: workspace)
                }

            }
        }
        .sheet(isPresented: $showingCrewLedger) {
            VStack(spacing: 0) {
                HStack { Text("机组工作总账 · 全组织真人收件箱").font(.headline); Spacer(); Button("完成") { showingCrewLedger = false } }.padding(16)
                Divider()
                CrewdWorkPanels(store: workspace.workStore, resources: workspace.resources,
                    resourceDrafts: $workspace.resourceDrafts, drafts: $workspace.requirementDrafts,
                    planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                    members: workspace.sessions, crews: workspace.crews)
            }
            #if os(macOS)
            .frame(minWidth: 540, minHeight: 480)
            #endif
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
            CrewdFolderControls(store:resources,drafts:$workspace.resourceDrafts,selection:selection,view:.messages).padding(.horizontal,10).padding(.vertical,5)
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
    private var todoItems:[CrewdValue]{group.items.filter{let kind=$0["object"]["kind"].string;return kind == "requirement" || kind == "humanTask"}}
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("本群待办").font(.system(size: 12, weight: .semibold))
                Spacer()
                Button("查看全部") { showDetails(false) }.buttonStyle(.plain).font(.system(size: 11))
                    .foregroundStyle(CrewdChatStyle.accent(scheme))
            }.foregroundStyle(CrewdChatStyle.muted(scheme))
            if group.crew != currentCrew || group.loading {
                ProgressView("正在读取").controlSize(.small).font(.caption)
            } else if let failure = group.failure {
                Text(failure.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
            } else if !group.capabilities.contains("channelWork") {
                Text("当前后台尚未开放群工作引用。").font(.caption).foregroundStyle(CrewdChatStyle.muted(scheme))
            } else if !group.loaded {
                Text("尚未读取当前会话事项").font(.caption).foregroundStyle(CrewdChatStyle.muted(scheme))
            } else if todoItems.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("把要做的事留在这里").font(.system(size: 12, weight: .medium))
                    Button("添加或关联待办") { showDetails(false) }.buttonStyle(.plain)
                        .font(.system(size: 11)).foregroundStyle(CrewdChatStyle.accent(scheme))
                }.padding(.vertical, 12)
            } else {
                ForEach(todoItems.indices, id: \.self) { index in
                    let row = todoItems[index]
                    VStack(alignment: .leading, spacing: 7) {
                        HStack(spacing: 7) {
                            Image(systemName: row["status"].string == "done" ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(CrewdChatStyle.accent(scheme))
                            Text(row["object"]["id"].string ?? "事项").font(.system(size: 10, weight: .medium))
                                .foregroundStyle(CrewdChatStyle.muted(scheme)).lineLimit(1)
                        }
                        Button { showDetails(row["object"]["kind"].string == "humanTask") } label: {
                            Text(row["label"].string ?? "关联工作").font(.system(size: 13, weight: .medium))
                                .multilineTextAlignment(.leading).lineLimit(4)
                        }.buttonStyle(.plain)
                        Text(CrewdWorkStatusLabel.display(row["status"].string ?? ""))
                            .font(.system(size: 10)).foregroundStyle(CrewdChatStyle.muted(scheme))
                    }.padding(11).frame(maxWidth: .infinity, alignment: .leading)
                        .background(CrewdChatStyle.bubble(scheme), in: RoundedRectangle(cornerRadius: 11))
                        .overlay(RoundedRectangle(cornerRadius: 11).stroke(CrewdChatStyle.line(scheme), lineWidth: 0.6))
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
    @State private var mobileChat = false
    #endif
    #if os(macOS)
    @EnvironmentObject private var runner: CrewSessionRunner
    @State private var openedSession = false
    @State private var showConversationPopover = false
    private var showingSession: Bool { runner.viewingTerminal }
    private var visibleChat: Bool { !showingSession && workspace.channels.contains { $0["id"].string == workspace.channelID } }
    private var visibleContent: Bool { workspace.originalSection != "消息" || visibleChat }
    private var visibleSession: Bool { workspace.originalSection == "消息" && showingSession }
    #endif
    var body: some View {
        #if os(macOS)
        GeometryReader { geometry in
        HStack(spacing: 0) {
            if geometry.size.width >= 600 {
                conversationList.frame(width: min(208, geometry.size.width * 0.30))
                Divider()
            }
            VStack(spacing: 0) {
                if geometry.size.width < 600 {
                    HStack {
                        Button { showConversationPopover = true } label: {
                            Label("群聊与会话", systemImage: "bubble.left.and.bubble.right")
                        }.buttonStyle(.plain).font(.system(size: 12))
                        Spacer()
                        if showingSession {
                            Button("返回群聊") { runner.viewingTerminal = false }
                                .buttonStyle(.plain).font(.system(size: 12))
                        }
                    }.padding(.horizontal, 12).frame(height: 32)
                        .popover(isPresented: $showConversationPopover) {
                            conversationList.frame(width: 248, height: 440)
                        }
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
                    if !wide {
                        HStack {
                            Button { mobileChat = false } label: { Label("会话", systemImage: "chevron.left") }
                            Spacer()
                        }.font(.system(size: 12)).padding(.horizontal, 14).padding(.vertical, 8)
                    }
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
        .onChange(of: workspace.crewID) { _, _ in mobileChat = false }
        .onChange(of: workspace.knowledgeNavigation) { _, _ in
            workspace.originalSection = "消息"; mobileChat = true
        }
        #endif
    }
    #if os(iOS)
    private var mobileConversationList: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 3) {
                Text("会话").font(.system(size: 12, weight: .medium))
                    .foregroundStyle(CrewdChatStyle.muted(scheme)).padding(12)
                ForEach(workspace.channels.compactMap { $0["id"].string }, id: \.self) { id in
                    if let row = workspace.channels.first(where: { $0["id"].string == id }) {
                        CrewdConversationRow(title: CrewdConversationProjection.title(row), subtitle: conversationSubtitle(row),
                            selected: workspace.channelID == id,
                            action: { workspace.selectConversation(id); mobileChat = true }) {
                                channelAvatar(row)
                            }
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
        } else { CrewdGroupAvatar(members: members, channel: id, name: CrewdConversationProjection.title(channel)) }
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
                                CrewdGroupAvatar(members: workspace.channelSummaries.first(where: { $0["channel"]["id"].string == id })?["members"].array ?? [], channel: id, name: CrewdConversationProjection.title(row))
                            }
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
