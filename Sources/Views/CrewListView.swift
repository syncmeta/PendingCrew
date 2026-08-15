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
    /// 当前选中的 crew id；nil = 无选中。
    @Binding var selection: String?

    var body: some View {
        // 根 crew 标注一次算完再分发给行（`rootTitlesByCrew` 的注释说明了为什么不
        // 在行里各算各的）。喂全量 `crewStore.crews`：父边可以跨机器。
        let rootTitles = CrewRootLineage.rootTitlesByCrew(in: crewStore.crews)
        List(crewStore.crews, selection: $selection) { crew in
            CrewListRow(crew: crew, rootTitles: rootTitles[crew.id] ?? [])
                .tag(crew.id)
        }
        .overlay {
            if crewStore.crews.isEmpty {
                emptyOverlay
            }
        }
        .task { await crewStore.refreshList() }
    }

    // MARK: - Empty state

    /// 空态文案要诚实。旧文案是「还没有 crew / 新建一个来开始」——在 iPhone
    /// 上这两句都不成立：iPhone/iPad 上根本没有「新建」入口（`CreateCrewSheet`
    /// 是 macOS-only），而列表空的真实原因通常不是「没有 crew」，是
    /// **Mac 上那些 crew 存在本机、没接进云端**（`AppModel.backend` 在 macOS
    /// 上是 LocalBackend、iOS 上是 EdgeBackend，本机 crew 从不上云）。
    /// 照旧文案念，人会一直在手机上找那个不存在的「+」。
    ///
    /// 这里只改文案。手机上「把 Mac 的 crew 接进来」的入口是下一批的事
    /// （当前唯一入口 `Mac/Views/CrewDetailInspector.swift` 是 macOS-only）。
    @ViewBuilder
    private var emptyOverlay: some View {
        if crewStore.loadingList {
            ProgressView()
        } else {
            ContentUnavailableView {
                Label("这里还没有 crew", systemImage: "person.2.slash")
            } description: {
                Text("这份列表只显示**已接入云端**的 crew。在 Mac 上新建的 crew 默认只存在那台 Mac 上，要在这儿看到它，先去 Mac 的 crew 详情里把它接入。")
            }
        }
    }
}

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
