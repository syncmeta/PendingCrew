#if os(macOS)
import SwiftUI
#if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
import Inject
#endif
import AppKit

/// Daily native shell: crew sidebar and one work surface. Members and controls
/// live inside the work surface while the original chat stays mounted.
struct MacThreePaneView: View {
    #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
    @ObserveInjection private var injection
    #endif
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    /// 长期职责的唯一所有者（spec §6）—— app 级持有，视图只观察不创建。
    @EnvironmentObject private var sessionHost: SessionHost
    private var sessionRunner: CrewSessionRunner { sessionHost.runner }
    /// 左 sidebar 可见性 —— 由原生 sidebar 折叠开关控制（默认 `.all`：三栏全开）。
    @State private var columnVisibility: NavigationSplitViewVisibility = .all

    var body: some View {
        NavigationSplitView(columnVisibility:$columnVisibility){
            CrewSidebarView().environmentObject(sessionRunner)
                .navigationSplitViewColumnWidth(min:200,ideal:238,max:310)
        }detail:{
            CrewCenterView().environmentObject(sessionRunner)
                .frame(minWidth:340,maxWidth:.infinity,maxHeight:.infinity)
                .toolbar(removing: .sidebarToggle)
                // Keep workspace controls below the native titlebar hit region.
                // The root material still extends behind the transparent chrome.
        }
        .navigationSplitViewStyle(.balanced).tint(Theme.Palette.accent)
        .toolbar(removing: .sidebarToggle)
        .environment(\.crewdToggleSidebar, {
            columnVisibility = columnVisibility == .detailOnly ? .all : .detailOnly
        })
        .environment(\.crewdSidebarCollapsed, columnVisibility == .detailOnly)
        .toolbar {
            if crewStore.selectedDetail == nil {
                ToolbarItem(placement: .navigation) {
                    Button { columnVisibility = columnVisibility == .detailOnly ? .all : .detailOnly } label: {
                        Image(systemName: "sidebar.left")
                    }.labelStyle(.iconOnly).accessibilityLabel("显示或隐藏边栏")
                }
            }
        }
        .onChange(of: crewStore.selectedCrewId) { old, new in
            sessionRunner.switchCrew(from: old, to: new)
            if model.usesCrewdAuthority, let workspace = model.crewdBackend?.workspace, let new, new != crewStore.pendingCrew?.id {
                workspace.selectCrew(new)
            }
        }
        // 去掉 toolbar 底部那条灰色分隔线（详见 WindowSeparatorRemover）。tick 传
        // selectedCrewId —— 切 crew 时借 updateNSView 重设，抢在 SwiftUI 把它重置回去之后。
        .onChange(of: model.crewdBackend?.workspace.presentationNamespace) { _, _ in
            guard model.usesCrewdAuthority else { return }
            crewStore.retireOriginalAuthorityPresentation()
            sessionHost.begin(model: model, crewStore: crewStore)
            Task { await crewStore.refreshList() }
        }
        .modifier(CrewdWindowMaterial())
        .background(WindowSeparatorRemover(tick: crewStore.selectedCrewId ?? ""))
        .modifier(CrewdOriginalConnectionIntegration())
        .task {
            // 长期职责（编排器/中继/唤醒器/两个轮询中心/用量监视 + 那一串编排
            // 订阅）全在 SessionHost 里起，这里只招呼一声。幂等。
            if model.usesCrewdAuthority { await model.connectCrewdAuthority() }
            sessionHost.begin(model: model, crewStore: crewStore)
            // 首次进入时把列表 + subjects 都拉一遍 —— subjects 用于创建
            // crew sheet 的 picker，提前 prefetch 避免 sheet 打开时空。
            await crewStore.refreshList()
            await crewStore.refreshSubjects()
            // 机器清单（侧栏按机器分组的数据源）由 `RootView.task` 的
            // refreshMachines 负责（它包着 MacThreePaneView，首屏即跑）——
            // 这里不重复。
        }
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }
}

/// 让标题栏与内容无缝、且**不盖住 sidebar**。做三件事：
/// 1. `titlebarSeparatorStyle = .none` —— 去掉 hairline。
/// 2. `titlebarAppearsTransparent = true` —— 标题栏背景透明，不再画自己那层比内容浅
///    的材质（那层材质的下边沿就是用户看到的"灰线"）。
/// 3. `fullSizeContentView` —— 内容铺满到窗口顶。这样**每一栏各自的背景透上来**：
///    sidebar 透出它的侧栏材质（不被一条白 toolbar 盖住）、detail 透出白 canvas
///    （和下面群聊无缝、无灰线）。比窗口级 `.toolbarBackground` 刷单一颜色更对——
///    后者会把 sidebar 那半截也刷白。
///
/// **不设** `backgroundColor`（上一版设动态底色，被材质用暗外观取值成近黑，把 composer
/// 材质带黑了）。标题(navigationTitle)照常显示，不碰 titleVisibility。
///
/// 必须在 view 真正挂上 window 后设（`viewDidMoveToWindow`）；SwiftUI 装配 toolbar 后会把
/// separatorStyle 重置回 .automatic，故延迟 0.3s + `tick`(crew 选择)变化时各补设一次。
private struct WindowSeparatorRemover: NSViewRepresentable {
    /// 任意会变的值（这里传 selectedCrewId）—— 变化时 SwiftUI 调 updateNSView，借机重设。
    var tick: String

    func makeNSView(context: Context) -> NSView { SeparatorKillerView() }

    func updateNSView(_ nsView: NSView, context: Context) {
        SeparatorKillerView.applyChrome(nsView.window)
    }
}

private final class SeparatorKillerView: NSView {
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        Self.applyChrome(window)
        // SwiftUI 在 toolbar 装配完后会把 separatorStyle 重置回 .automatic，延迟再设一次。
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            Self.applyChrome(self?.window)
        }
    }

    static func applyChrome(_ window: NSWindow?) {
        guard let window else { return }
        if window.titlebarSeparatorStyle != .none { window.titlebarSeparatorStyle = .none }
        if !window.titlebarAppearsTransparent { window.titlebarAppearsTransparent = true }
        if !window.styleMask.contains(.fullSizeContentView) {
            window.styleMask.insert(.fullSizeContentView)
        }
    }
}
#endif
