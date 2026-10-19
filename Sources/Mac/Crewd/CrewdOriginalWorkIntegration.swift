import SwiftUI
#if os(macOS)
import AppKit
#endif

private struct CrewdOriginalChannelKey: EnvironmentKey { static let defaultValue = "" }
extension EnvironmentValues {
    var crewdOriginalChannelID: String {
        get { self[CrewdOriginalChannelKey.self] }
        set { self[CrewdOriginalChannelKey.self] = newValue }
    }
}

struct CrewdOriginalWorkIntegration: ViewModifier {
    let workspace: CrewdWorkspace?
    func body(content: Content) -> some View {
        if let workspace { CrewdOriginalCrewContent(workspace: workspace) { content } }
        else { content }
    }
}
private struct CrewdOriginalCrewContent<Content: View>: View {
    @ObservedObject var workspace: CrewdWorkspace
    @EnvironmentObject private var crewStore: CrewStore
    @Environment(\.colorScheme) private var scheme
    @ViewBuilder let content: () -> Content
    var body: some View {
        VStack(spacing: 0) {
            Picker("机组视图", selection: $workspace.originalSection) {
                ForEach(["消息", "Todo", "文档", "进展"], id: \.self) { Text($0).tag($0) }
            }.pickerStyle(.segmented).labelsHidden().frame(maxWidth: 330)
                .padding(.horizontal, 16).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(CrewdChatStyle.sidebar(scheme).opacity(0.35))
            Divider().overlay(CrewdChatStyle.line(scheme))
            ZStack {
                // Keep the original chat instance mounted: drafts, anchors and
                // its existing pagination/identity adapter survive other panels.
                CrewdOriginalMessages(workspace: workspace) { content() }
                    .opacity(workspace.originalSection == "消息" ? 1 : 0)
                    .allowsHitTesting(workspace.originalSection == "消息")
                    .disabled(workspace.originalSection != "消息")
                    .accessibilityHidden(workspace.originalSection != "消息")
                if workspace.originalSection == "Todo" {
                    VStack {
                        CrewdChannelTodoView(group: workspace.channelWork, work: workspace.workStore,
                            linkDrafts: $workspace.channelLinkDrafts, requirementDrafts: $workspace.requirementDrafts,
                            planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                            members: workspace.sessions, crews: workspace.crews,
                            openCrew: { crewStore.selectCrew($0) })
                        Divider()

                    CrewdWorkPanels(store: workspace.workStore, resources: workspace.resources,
                        resourceDrafts: $workspace.resourceDrafts, drafts: $workspace.requirementDrafts,
                        planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                        members: workspace.sessions, crews: workspace.crews).padding(10)
                    }
                } else if workspace.originalSection == "文档" {
                    CrewdKnowledgeDocumentsEntry(workspace: workspace)
                } else if workspace.originalSection == "进展" {
                    CrewdWorkPanels(store: workspace.workStore, resources: workspace.resources,
                        resourceDrafts: $workspace.resourceDrafts, drafts: $workspace.requirementDrafts,
                        planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                        members: workspace.sessions, crews: workspace.crews, progressOnly: true).padding(10)
                }
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
            showDetails: { workspace.originalSection = "Todo" })
    }
}
private struct CrewdOriginalTodoSummary: View {
    @ObservedObject var group: CrewdChannelWorkStore
    @Environment(\.colorScheme) private var scheme
    let currentCrew: String
    let showDetails: () -> Void
    @State private var owner = UUID()
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("本群 Todo").font(.system(size: 11, weight: .semibold))
                Spacer()
                Button("详细", action: showDetails).buttonStyle(.plain).font(.system(size: 11))
            }.foregroundStyle(CrewdChatStyle.muted(scheme))
            Text("同一账本的相关事项").font(.system(size: 10)).foregroundStyle(CrewdChatStyle.muted(scheme))
            if group.crew != currentCrew || group.loading {
                ProgressView("正在读取").controlSize(.small).font(.caption)
            } else if let failure = group.failure {
                Text(failure.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
            } else if !group.capabilities.contains("channelWork") {
                Text("当前后台尚未开放群工作引用。").font(.caption).foregroundStyle(CrewdChatStyle.muted(scheme))
            } else if !group.loaded {
                Text("尚未读取当前会话事项").font(.caption).foregroundStyle(CrewdChatStyle.muted(scheme))
            } else if group.items.isEmpty {
                Text("当前会话暂无关联事项").font(.caption).foregroundStyle(CrewdChatStyle.muted(scheme)).padding(.vertical, 12)
            } else {
                ForEach(group.items.indices, id: \.self) { index in
                    let row = group.items[index]
                    VStack(alignment: .leading, spacing: 7) {
                        HStack(spacing: 7) {
                            Image(systemName: row["status"].string == "done" ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(CrewdChatStyle.accent(scheme))
                            Text(row["object"]["id"].string ?? "事项").font(.system(size: 11, weight: .medium))
                        }
                        Text(row["label"].string ?? "关联工作").font(.system(size: 12)).lineLimit(4)
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

/// Message folders classify the backend's channels, not individual message
/// bodies, Todo parents or a second local communication ledger.
private struct CrewdOriginalMessages<Content: View>: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var resources: CrewdResources
    @ViewBuilder let content: () -> Content
    init(workspace: CrewdWorkspace, @ViewBuilder content: @escaping () -> Content) {
        self.workspace = workspace; self.resources = workspace.resources; self.content = content
    }
    private var folder: Binding<String> {
        Binding(get: { workspace.resourceDrafts.selection(workspace.crewID, .messages) },
                set: { workspace.resourceDrafts.select(workspace.crewID, .messages, $0) })
    }
    private var visibleChannels: [CrewdValue] {
        workspace.channels.filter { row in
            guard let id = row["id"].string else { return false }
            return CrewdFolderFilter.includes(CrewdFolderFilter.object("channel", id),
                selection: folder.wrappedValue, snapshot: resources.folders[.messages])
        }
    }
    #if os(macOS)
    @EnvironmentObject private var runner: CrewSessionRunner
    @Environment(\.colorScheme) private var scheme
    @State private var openedSession = false
    @State private var showFolders = false
    @State private var showConversationPopover = false
    private var showingSession: Bool { runner.viewingTerminal }
    private var visibleChat: Bool { !showingSession && workspace.originalSection == "消息" && visibleChannels.contains { $0["id"].string == workspace.channelID } }
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
                            ForEach(visibleChannels.compactMap { $0["id"].string }, id: \.self) { id in
                                if let row = visibleChannels.first(where: { $0["id"].string == id }) {
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
            ZStack {
                content().environment(\.crewdOriginalChannelID, workspace.channelID).id(workspace.channelID)
                    .opacity(visibleChat ? 1 : 0).allowsHitTesting(visibleChat).disabled(!visibleChat).accessibilityHidden(!visibleChat)
                if openedSession || showingSession {
                    CrewSessionWindowView(presentation: .conversation)
                        .opacity(showingSession ? 1 : 0).allowsHitTesting(showingSession)
                        .disabled(!showingSession).accessibilityHidden(!showingSession)
                }
                if !showingSession && !visibleChat {
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
        VStack(spacing: 0) {
            if workspace.originalSection == "消息" {
                CrewdFolderControls(store: resources, drafts: $workspace.resourceDrafts,
                    selection: folder, view: .messages,
                    object: workspace.channelID.isEmpty ? nil : CrewdFolderFilter.object("channel", workspace.channelID)).padding(8)
                ScrollView(.horizontal) {
                    HStack {
                        ForEach(visibleChannels.compactMap { $0["id"].string }, id: \.self) { id in
                            if let row = visibleChannels.first(where: { $0["id"].string == id }) {
                                Button { workspace.selectConversation(id) } label: {
                                    Text(CrewdConversationProjection.title(row))
                                        .fontWeight(id == workspace.channelID ? .semibold : .regular)
                                }.buttonStyle(.bordered)
                            }
                        }
                    }.padding(.horizontal, 8)
                }
            }
            ZStack {
                content().environment(\.crewdOriginalChannelID, workspace.channelID)
                    .id(workspace.channelID)
                    .opacity(visibleChannels.contains(where: { $0["id"].string == workspace.channelID }) ? 1 : 0)
                    .allowsHitTesting(visibleChannels.contains(where: { $0["id"].string == workspace.channelID }))
                if workspace.originalSection == "消息", !visibleChannels.contains(where: { $0["id"].string == workspace.channelID }) {
                    Text("当前文件夹没有选中的会话；请选择上方会话，或切换到全部。")
                        .font(.caption).foregroundStyle(.secondary).padding()
                }
            }
        }.task(id: workspace.originalSection + resources.activationEpoch.uuidString) {
            if workspace.originalSection == "消息" { await resources.activate(.messages, force: true) }
        }
        #endif
    }
    #if os(macOS)
    private var conversationList: some View {
        VStack(spacing: 0) {
            HStack {
                Text("会话").font(.system(size: 11, weight: .medium))
                Spacer()
                Button { runner.composeNew(); runner.viewingTerminal = true; openedSession = true; showConversationPopover = false } label: { Image(systemName: "plus") }
                    .buttonStyle(.plain).help("新建成员会话")
            }.foregroundStyle(CrewdChatStyle.muted(scheme)).padding(.horizontal, 12).frame(height: 36)
            ScrollView {
                VStack(spacing: 3) {
                    ForEach(visibleChannels.compactMap { $0["id"].string }, id: \.self) { id in
                        if let row = visibleChannels.first(where: { $0["id"].string == id }) {
                            CrewdConversationRow(title: CrewdConversationProjection.title(row), subtitle: row["kind"].string == "dm" ? "群内单聊" : "机组群聊",
                                selected: !showingSession && workspace.channelID == id,
                                action: { workspace.selectConversation(id); runner.viewingTerminal = false; showConversationPopover = false }) {
                                CrewdGroupAvatar(members: workspace.channelSummaries.first(where: { $0["channel"]["id"].string == id })?["members"].array ?? [], channel: id, name: CrewdConversationProjection.title(row))
                            }
                        }
                    }
                    ForEach(workspace.sessions.compactMap { $0["id"].string }, id: \.self) { id in
                        if let row = workspace.sessions.first(where: { $0["id"].string == id }) {
                            CrewdConversationRow(title: row["displayName"].string ?? row["title"].string ?? row["role"].string ?? "成员",
                                subtitle: (row["executor"].string ?? "") + " · 成员会话",
                                selected: showingSession && workspace.sessionID == id && !runner.isComposingNew,
                                action: {
                                    guard let run = runner.runs.first(where: { $0.sessionId == id && $0.crewId == workspace.crewID }) else { return }
                                    runner.openSession(run.runID); openedSession = true; showConversationPopover = false
                                }) { CrewdMemberAvatar(seed: id, colorSeed: workspace.crewID) }
                                .disabled(!runner.runs.contains { $0.sessionId == id && $0.crewId == workspace.crewID })
                        }
                    }
                }.padding(.horizontal, 5).padding(.top, 3)
            }
            Divider()
            Button { showFolders.toggle() } label: { Label("会话文件夹", systemImage: "folder").font(.system(size: 11)) }
                .buttonStyle(.plain).padding(10)
                .popover(isPresented: $showFolders) {
                    CrewdFolderControls(store: resources, drafts: $workspace.resourceDrafts, selection: folder, view: .messages,
                        object: workspace.channelID.isEmpty ? nil : CrewdFolderFilter.object("channel", workspace.channelID))
                        .padding(14).frame(width: 280)
                }
        }.background(CrewdChatStyle.canvas(scheme))
        .onChange(of: workspace.crewID) { _, _ in showFolders = false }
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
