import SwiftUI

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
                content().opacity(workspace.originalSection == "消息" ? 1 : 0).allowsHitTesting(workspace.originalSection == "消息")
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
