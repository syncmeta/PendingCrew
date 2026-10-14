import SwiftUI

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
    @ViewBuilder let content: () -> Content
    var body: some View {
        VStack(spacing: 0) {
            Picker("机组视图", selection: $workspace.originalSection) {
                ForEach(["消息", "Todo", "文档", "进展"], id: \.self) { Text($0).tag($0) }
            }.pickerStyle(.segmented).padding(8)
            ZStack {
                // Keep the original chat instance mounted: drafts, anchors and
                // its existing pagination/identity adapter survive other panels.
                CrewdOriginalMessages(workspace: workspace) { content() }
                    .opacity(workspace.originalSection == "消息" ? 1 : 0).allowsHitTesting(workspace.originalSection == "消息")
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
                    CrewdDocumentsView(store: workspace.resources, drafts: $workspace.resourceDrafts).padding(10)
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
    @EnvironmentObject private var crewStore: CrewStore
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("本群 Todo").font(.headline)
            Text("需求与人类待决事项由当前后台保存。").font(.caption).foregroundStyle(.secondary)
            if workspace.crewID == crewID {
                CrewdChannelTodoView(group: workspace.channelWork, work: workspace.workStore,
                    linkDrafts: $workspace.channelLinkDrafts, requirementDrafts: $workspace.requirementDrafts,
                    planDrafts: $workspace.planDrafts, humanDrafts: $workspace.humanDrafts,
                    members: workspace.sessions, crews: workspace.crews,
                    openCrew: { crewStore.selectCrew($0) })
            } else { Text("正在核对当前机组的 Todo。") }
        }.padding(12)
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
    var body: some View {
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
                                    Text(row["name"].string ?? row["title"].string ?? id)
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
    }
}
