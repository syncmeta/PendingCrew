#if os(macOS)
import SwiftUI
import AppKit

/// Daily native shell: crew sidebar, work surface and optional member inspector.
/// Narrow windows present that same inspector as a sheet, preserving the chat.
struct MacThreePaneView: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    /// 长期职责的唯一所有者（spec §6）—— app 级持有，视图只观察不创建。
    @EnvironmentObject private var sessionHost: SessionHost
    private var sessionRunner: CrewSessionRunner { sessionHost.runner }
    /// 左 sidebar 可见性 —— 由原生 sidebar 折叠开关控制（默认 `.all`：三栏全开）。
    @State private var columnVisibility: NavigationSplitViewVisibility = .all
    @State private var inspectorVisible = true
    @State private var compactInspector = false
    @State private var showingUnavailableStatus=false
    /// 驾驶舱开关位（人类 Todo #96）。**用 `@State` 持有，不是 `@StateObject`** ——
    /// `@State` 只替我们保管这个引用，**不订阅**它的 `objectWillChange`。于是开关
    /// 驾驶舱不会把本视图（连着整棵三栏）作废；只有下面那层 `CockpitLayer` 观察它。
    /// 换成 `@StateObject` 会静默地把整棵树重新拉回失效范围，编译器一句话都不会说。
    @State private var cockpit = CockpitPresentation()

    var body: some View {
        GeometryReader { geometry in
        ZStack {
            // 原生三栏：列宽交系统协商。两侧 cap 住、中栏弹性吃 slack（免得右栏像旧版那样霸占
            // 大半屏留白）；窗口窄了系统自动收某列，不会按 ideal 撑窗超屏裁掉 sidebar。
            // detail（成员/终端）常驻 —— 切到某 session 时由 CrewSessionWindowView 自己在
            // 「成员列表 ↔ 终端」间切（viewingTerminal），不再需要单独开关收/放一栏。
            //
            // **无论驾驶舱开没开，这一整棵树始终挂着**（#542）：旧版在 showingCockpit 时
            // 整片换成另一个 NavigationSplitView，群聊那栏被卸载重建 —— 回来时 composer
            // 草稿和滚动位置全被冲掉。驾驶舱改成叠在上面的临时窗口后，群聊视图常驻，
            // 关掉驾驶舱看到的就是离开前那一屏。
            VStack(spacing: 0) {
            if !model.usesCrewdAuthority {OrchestrationNoticeBar()}
            NavigationSplitView(columnVisibility: $columnVisibility) {
                CrewSidebarView()
                    .environmentObject(sessionRunner)
                    .navigationSplitViewColumnWidth(min:200,ideal:238,max:310)
            } detail: {
                CrewCenterView().environmentObject(sessionRunner)
                    .frame(minWidth:340,maxWidth:.infinity,maxHeight:.infinity)
                    .toolbar {
                        if model.usesCrewdAuthority,model.crewdBackend?.workspace == nil {
                            ToolbarItem{
                                Button{showingUnavailableStatus=true}label:{Label("连接状态",systemImage:"wifi.exclamationmark")}
                                    .popover(isPresented:$showingUnavailableStatus){
                                        VStack(alignment:.leading,spacing:12){
                                            Text(model.connectingAuthority ? "正在连接后台":model.backendConfigurationError ?? "后台尚未连接").textSelection(.enabled)
                                            Button("重试连接"){Task{await model.connectCrewdAuthority()}}.disabled(model.connectingAuthority)
                                        }.padding(16).frame(width:300)
                                    }
                            }
                        }
                        ToolbarItem(placement:.primaryAction) {
                            Button {
                                if geometry.size.width<1100 {compactInspector=true}
                                else {inspectorVisible.toggle()}
                            }label:{Label("待办工作与成员",systemImage:"sidebar.right")}
                            .disabled(crewStore.selectedDetail == nil)
                            .help("显示或收起当前群的待办工作与成员")
                        }
                    }
            }
            .inspector(isPresented:Binding(get:{inspectorVisible && geometry.size.width>=1100 && crewStore.selectedDetail != nil},set:{inspectorVisible=$0})) {
                CrewSessionWindowView(presentation:model.usesCrewdAuthority ? .inspector:.automatic)
                    .environmentObject(sessionRunner)
                    .inspectorColumnWidth(min:220,ideal:254,max:340)
            }
            .sheet(isPresented:$compactInspector) {
                VStack(spacing:0){
                    HStack{Text("待办工作与成员").font(.headline);Spacer();Button("完成"){compactInspector=false}}
                        .padding(16)
                    Divider()
                    CrewSessionWindowView(presentation:model.usesCrewdAuthority ? .inspector:.automatic)
                        .environmentObject(sessionRunner)
                }.frame(minWidth:360,idealWidth:430,minHeight:480)
            }
            .navigationSplitViewStyle(.balanced).tint(Theme.Palette.accent)
            // 入口（中栏 toolbar 那颗按钮）只需要**写**这个开关位。给的是普通
            // environment 值而不是 `.environmentObject` —— 后者会让整条中栏订阅它，
            // 等于把 #96 刚拆掉的广播接回来一半。
            .environment(\.cockpitPresentation, cockpit)
            .environment(\.crewdShowInspector, {
                if geometry.size.width<1100 {compactInspector=true}
                else {inspectorVisible=true}
            })
            }

            if !model.usesCrewdAuthority {
                CockpitLayer(presentation: cockpit)
                    .environmentObject(crewStore).environmentObject(model).zIndex(1)
            }
        }.frame(width: geometry.size.width, height: geometry.size.height)
        }
        // ⚠️ **这里不许再挂 `.animation(_:value:)`**（人类 Todo #96）。
        // 它的作用域是整个子树，包括整棵 NavigationSplitView；开关驾驶舱那一下的
        // 失效会被裹进一个 0.16s 的动画事务里逐帧重算，而不是一次算完。
        // 淡入淡出已挪进 `CockpitLayer`，只包驾驶舱自己。
        //
        // 每个 crew 记住自己右栏打开的 session（#481）：切 crew 时把选中态存回旧
        // crew、恢复新 crew 记住的那份 —— 切回来还是原来打开的 session，不串。
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
    }
}

/// 驾驶舱的**临时窗口**外壳（#542）—— 一块浮在群聊之上的卡片 + 一层压暗的背景。
///
/// 为什么是叠上去而不是换一屏：群聊视图必须常驻，关掉驾驶舱回来时 composer 草稿和
/// 滚动位置得原样在（见 `MacThreePaneView` 里那段注释）。
///
/// 卡片四周留边（顶上留得多一点，让开窗口的红绿灯），所以它看起来就是一块临时浮起来的
/// 面板，而不是又一个主界面。关闭是卡片自己左上角那颗圆形叉（在 `CockpitView` 的头部）——
/// 位置对齐 Mac 红点的心智；点背景或按 Esc 同样关掉。
private struct CockpitOverlay: View {
    @ObservedObject var presentation: CockpitPresentation

    var body: some View {
        ZStack {
            Color.black.opacity(0.22)
                .ignoresSafeArea()
                .contentShape(Rectangle())
                .onTapGesture { presentation.close() }
            CockpitView(onClose: { presentation.close() }, planFocus: presentation.planFocus)
                .clipShape(RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius)
                        .stroke(Theme.Palette.hairline, lineWidth: 1))
                .shadow(color: .black.opacity(0.28), radius: 26, y: 10)
                .padding(.top, 44)      // 让开标题栏红绿灯
                .padding(.horizontal, 28)
                .padding(.bottom, 28)
        }
        // Esc 关掉（临时窗口该有的手感）。
        .onExitCommand { presentation.close() }
    }
}

/// 驾驶舱**这一层**，也是这个开关唯一惊动到的地方（人类 Todo #96）。
///
/// 三件事都收在这里，一件都不许再往上挂：
/// 1. **观察开关位** —— 只有本视图订阅 `CockpitPresentation`。开/关驾驶舱时，
///    侧栏、群聊、终端一个都不重算（以前它们全都要，因为开关位挂在 runner 上）。
/// 2. **动画只包这一层** —— `.animation(_:value:)` 的作用域是整个子树。挂在外面那个
///    ZStack 上时，它包住的是整棵 `NavigationSplitView`，于是那次全树失效被摊成
///    0.16s 里的逐帧重算。挂在这儿，它只作用于驾驶舱自己的淡入淡出。
/// 3. **关着的时候是空的** —— 空 ZStack 不占尺寸、不吃点击，外层那个 ZStack 照旧只有
///    三栏在参与布局。
private struct CockpitLayer: View {
    @ObservedObject var presentation: CockpitPresentation

    var body: some View {
        ZStack {
            if presentation.isOpen {
                CockpitOverlay(presentation: presentation)
                    .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.16), value: presentation.isOpen)
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
