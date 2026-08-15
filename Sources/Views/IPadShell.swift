#if os(iOS)
import SwiftUI

/// iPhone / iPad 认证后的根视图 —— NavigationSplitView：
///   sidebar = CrewListView（crew 列表）
///   detail  = CrewChatView（所选群聊）
///
/// **iPhone（compact）不另起一套 shell**：NavigationSplitView 在 compact
/// 下会自己塌成一个 NavigationStack —— sidebar 当根，选中 crew 就把 detail
/// 推上去，返回键由系统给。所以「列表 → 群聊 → 返回」是原生行为，不用手糊
/// NavigationStack + navigationDestination。这里只补 compact 才需要的一件事：
/// 群聊页的标题（regular 下 detail 是并排的一列，加标题反而多一条空导航条，
/// 所以按 size class 分）。返回时的选中态复位由塌陷后的 NavigationStack 自己管。
///
/// 依赖注入清单（runtime crash 防护）：
///   - CrewListView 需要 @EnvironmentObject var crewStore: CrewStore
///   - CrewChatView 需要 @EnvironmentObject var appModel: AppModel
///     （sessionRunner 是 macOS-only，iOS 不需要）
/// 两者均由 WindowGroup → RootView 链路上的 .environmentObject(...) 注入，
/// IPadShell 自身声明同样的 @EnvironmentObject 让 SwiftUI 知道它用到了它们。
struct IPadShell: View {
    @EnvironmentObject private var crewStore: CrewStore
    @EnvironmentObject private var appModel: AppModel
    /// compact = iPhone 竖屏 / 大多数 iPhone 形态；regular = iPad 与
    /// iPhone Max 横屏（那时两栏并排，行为与 iPad 一致）。
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass

    @State private var selectedCrewId: String?

    private var isCompact: Bool { horizontalSizeClass == .compact }

    var body: some View {
        NavigationSplitView {
            CrewListView(selection: $selectedCrewId)
                .navigationTitle("机组")
        } detail: {
            detail
        }
        .onChange(of: selectedCrewId) { _, id in
            crewStore.selectCrew(id) // selectCrew 是同步方法，接受 String?
        }
    }

    @ViewBuilder
    private var detail: some View {
        if let id = selectedCrewId, let title = crewTitle(for: id) {
            CrewChatView(crewId: id, crewTitle: title)
                .id(id) // 切换 crew 时强制重建 chat view（清空旧 state）
                // compact 下这一屏是被推上来的，得有标题；regular 下留空
                // 标题 = 与改动前一致（别动 iPad）。不碰 .toolbar 可见性 ——
                // iPad detail 列那条导航条上挂着系统的侧栏开关，隐了就没了。
                .navigationTitle(isCompact ? title : "")
                .navigationBarTitleDisplayMode(.inline)
        } else {
            PendingCrewPlaceholderIcon(size: 64)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// 从 crewStore.crews 中找到对应 crew 的 title，供 CrewChatView 初始化用。
    private func crewTitle(for id: String) -> String? {
        crewStore.crews.first(where: { $0.id == id })?.title
    }
}
#endif
