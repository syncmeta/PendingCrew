import SwiftUI

/// 跨平台 crew 列表 —— iPad sidebar / macOS reuse 用。
///
/// 不含 macOS-only 的 session 列、toolbar、identity footer 等，那些逻辑
/// 留在 `CrewSidebarView`（`#if os(macOS)`）。这里只是纯列表。
///
/// 选中态通过 `$selection` 双向绑定传入，由外层 NavigationSplitView 或
/// macOS List(selection:) 统一持有。
struct CrewListView: View {
    @EnvironmentObject private var crewStore: CrewStore
    #if os(iOS)
    @EnvironmentObject private var appModel: AppModel
    var connectComputer: (() -> Void)? = nil
    var hasPairedComputer = false
    var readingComputers = false
    #endif
    /// 当前选中的 crew id；nil = 无选中。
    @Binding var selection: String?

    var body: some View {
        // 根 crew 标注一次算完再分发给行（`rootTitlesByCrew` 的注释说明了为什么不
        // 在行里各算各的）。喂全量 `crewStore.crews`：父边可以跨机器。
        Group {
            #if os(iOS)
            if let workspace = crewStore.crewdWorkspace {
                CrewMobileDirectory(workspace: workspace, crews: crewStore.crews, selection: $selection)
                    .id(workspace.presentationNamespace)
            } else { flatList }
            #else
            flatList
            #endif
        }
        .overlay {
            if crewStore.crews.isEmpty {
                emptyOverlay
            }
        }
        .task { await crewStore.refreshList() }
    }

    private var flatList: some View {
        let rootTitles = CrewRootLineage.rootTitlesByCrew(in: crewStore.crews)
        return List(crewStore.crews, selection: $selection) { crew in
            CrewListRow(crew: crew, rootTitles: rootTitles[crew.id] ?? []).tag(crew.id)
        }
    }

    // MARK: - Empty state

    @ViewBuilder
    private var emptyOverlay: some View {
        if awaitingList {
            ProgressView()
        } else {
            #if os(iOS)
            if let connectComputer {
                MobileComputerIntroduction(hasPairedComputer: hasPairedComputer,
                                           connect: connectComputer)
            } else {
                unavailableOrEmpty
            }
            #else
            unavailableOrEmpty
            #endif
        }
    }

    private var awaitingList: Bool {
        #if os(iOS)
        if readingComputers { return true }
        #endif
        return crewStore.loadingList
    }

    private var connectionFailure: String? {
        #if os(iOS)
        appModel.crewdWindow.error ?? appModel.crewdWindow.workspace?.error ?? crewStore.error
        #else
        crewStore.error
        #endif
    }

    @ViewBuilder
    private var unavailableOrEmpty: some View {
        if let error = connectionFailure {
            ContentUnavailableView {
                Label("远端连接不可用", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("重试") { Task { await crewStore.refreshList() } }
            }
        } else {
            ContentUnavailableView {
                #if os(iOS)
                Label("这里还没有机组", systemImage: "person.2.slash")
                #else
                Label("这里还没有 crew", systemImage: "person.2.slash")
                #endif
            } description: {
                #if os(iOS)
                Text("这台电脑还没有可显示的机组。")
                #else
                Text("远端 Mac 当前没有可显示的 crew。")
                #endif
            }
        }
    }

}

#if os(iOS)
private struct CrewMobileDirectory: View {
    @ObservedObject var workspace: CrewdWorkspace
    let crews: [CrewSummary]
    @Binding var selection: String?
    @State private var mode = CrewdNavigation.Sidebar.contacts
    @State private var collapsed: Set<String> = []
    private var visible: [CrewdValue] {
        let ids = Set(crews.map(\.id))
        return workspace.crews.filter { ids.contains($0["id"].string ?? "") }
    }
    var body: some View {
        VStack(spacing: 0) {
            Picker("侧栏", selection: $mode) {
                ForEach(CrewdNavigation.Sidebar.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }.pickerStyle(.segmented).padding(.horizontal, 12).padding(.vertical, 10)
            List(selection: $selection) {
                if mode == .contacts {
                    ForEach(CrewdNavigation.directory(visible, collapsed: collapsed)) { row in
                        if let crew = crews.first(where: { $0.id == row.object }) {
                            HStack(spacing: 6) {
                                if row.expandable {
                                    Button {
                                        if !collapsed.insert(row.id).inserted { collapsed.remove(row.id) }
                                    } label: { Image(systemName: collapsed.contains(row.id) ? "chevron.right" : "chevron.down") }
                                        .buttonStyle(.plain).font(.caption).accessibilityLabel("展开或折叠" + crew.title)
                                }
                                directoryRow(crew)
                            }.padding(.leading, CGFloat(row.depth) * 14).tag(crew.id)
                        }
                    }
                } else {
                    ForEach(CrewdNavigation.timeline(visible, activity: workspace.crewActivity).compactMap { $0["id"].string }, id: \.self) { id in
                        if let crew = crews.first(where: { $0.id == id }) { directoryRow(crew).tag(crew.id) }
                    }
                }
            }.listStyle(.sidebar)
        }
    }
    private func directoryRow(_ crew: CrewSummary) -> some View {
        HStack(spacing: 10) {
            CrewdGroupAvatar(members: workspace.crewAvatarMembers[crew.id] ?? [], channel: crew.id, name: crew.title)
            VStack(alignment: .leading, spacing: 4) {
                Text(crew.title).font(.system(size: 14, weight: .medium)).lineLimit(2)
                if mode == .messages {
                    Text(workspace.crewActivity[crew.id]?["text"].string ?? (workspace.activityRead.contains(crew.id) ? "暂无消息" : "消息尚未读取"))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
        }.padding(.vertical, 5)
    }
}
#endif

// MARK: - Row

private struct CrewListRow: View {
    let crew: CrewSummary
    /// 本行 crew 挂在哪些根 crew 之下（空 = 自己就是根，不画标注）。
    let rootTitles: [String]

    var body: some View {
        HStack(spacing: 10) {
            BotAvatar(emojiSeed: crew.id, colorSeed: crew.id, size: 28)
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    CrewTitleRootBadge(
                        title: crew.title,
                        titleFont: Theme.Fonts.body,
                        titleColor: Theme.Palette.ink,
                        rootTitles: rootTitles,
                        badgeSize: 13
                    )
                    Spacer(minLength: 0)
                }
                subtitleText
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .lineLimit(1)
            }
        }
        .padding(.vertical, 2)
    }

    @ViewBuilder
    private var subtitleText: some View {
        let location = crew.runtimeLocationKind?.shortLabel ?? crew.runtimeLocation
        Text(location)
    }
}
