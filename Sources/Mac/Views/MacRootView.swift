#if os(macOS)
import SwiftUI
import AppKit

/// Mac 主视图：**原生三栏 `NavigationSplitView`**（crew 列表 | 群聊 | 成员/终端）。
///
/// - 左栏 CrewSidebarView ── crew 列表 + 新建 / 刷新（原生 sidebar：圆角 + 材质 + 折叠开关）
/// - 中栏 CrewCenterView   ── 选中 crew 的群聊会话页（白板 + 沟通渠道）
/// - 右栏 CrewSessionWindowView ── 成员列表 / session 终端 + composer + 审批卡片，**常驻**
///
/// 用原生三栏（而非「2 栏 + detail 内塞 HSplitView」）的两条理由：① 保留原生 sidebar 质感
///（圆角 / 半透明材质 / 原生折叠开关）—— 自绘 HSplitView 给不了；② 列宽由系统协商，窗口
/// 窄了自动收某列，不会像旧 hybrid 那样按 ideal 把窗口撑超屏、把 sidebar 推出屏幕外裁掉
///（docs/tech-debt.md「inspector 被切」那条回归）。
///
/// Sidebar / Center / SessionWindow 都用 @EnvironmentObject 拿 CrewStore + AppModel；
/// CrewSessionRunner 归 app 级的 `SessionHost` 持有（前后端分离 P0）——这里只观察，不创建。
struct MacThreePaneView: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    /// 长期职责的唯一所有者（spec §6）—— app 级持有，视图只观察不创建。
    @EnvironmentObject private var sessionHost: SessionHost
    private var sessionRunner: CrewSessionRunner { sessionHost.runner }
    /// 左 sidebar 可见性 —— 由原生 sidebar 折叠开关控制（默认 `.all`：三栏全开）。
    @State private var columnVisibility: NavigationSplitViewVisibility = .all

    var body: some View {
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
            NavigationSplitView(columnVisibility: $columnVisibility) {
                CrewSidebarView()
                    .environmentObject(sessionRunner) // 侧栏头像状态点要看 runs
                    .navigationSplitViewColumnWidth(min: 240, ideal: 280, max: 340)
            } content: {
                CrewCenterView()
                    .environmentObject(sessionRunner)
                    .navigationSplitViewColumnWidth(min: 360, ideal: 520)
            } detail: {
                CrewSessionWindowView()
                    .environmentObject(sessionRunner)
                    .navigationSplitViewColumnWidth(min: 320, ideal: 400, max: 560)
            }
            if sessionRunner.showingCockpit {
                CockpitOverlay(runner: sessionRunner)
                    .environmentObject(crewStore)
                    .environmentObject(model)
                    .transition(.opacity)
                    .zIndex(1)
            }
        }
        .animation(.easeOut(duration: 0.16), value: sessionRunner.showingCockpit)
        // 每个 crew 记住自己右栏打开的 session（#481）：切 crew 时把选中态存回旧
        // crew、恢复新 crew 记住的那份 —— 切回来还是原来打开的 session，不串。
        .onChange(of: crewStore.selectedCrewId) { old, new in
            sessionRunner.switchCrew(from: old, to: new)
        }
        // 去掉 toolbar 底部那条灰色分隔线（详见 WindowSeparatorRemover）。tick 传
        // selectedCrewId —— 切 crew 时借 updateNSView 重设，抢在 SwiftUI 把它重置回去之后。
        .background(WindowSeparatorRemover(tick: crewStore.selectedCrewId ?? ""))
        .task {
            // 长期职责（编排器/中继/唤醒器/两个轮询中心/用量监视 + 那一串编排
            // 订阅）全在 SessionHost 里起，这里只招呼一声。幂等。
            sessionHost.start(model: model, crewStore: crewStore)
            // 首次进入时把列表 + subjects 都拉一遍 —— subjects 用于创建
            // crew sheet 的 picker，提前 prefetch 避免 sheet 打开时空。
            await crewStore.refreshList()
            await crewStore.refreshSubjects()
            // 机器清单（侧栏按机器分组的数据源）由 `RootView.task` 的
            // registerSelfMachine + refreshMachines 负责（它包着 MacThreePaneView，
            // 首屏即跑）—— 这里不重复。
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
    @ObservedObject var runner: CrewSessionRunner

    var body: some View {
        ZStack {
            Color.black.opacity(0.22)
                .ignoresSafeArea()
                .contentShape(Rectangle())
                .onTapGesture { runner.showingCockpit = false }
            CockpitView(runner: runner)
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
        .onExitCommand { runner.showingCockpit = false }
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
        window.titlebarSeparatorStyle = .none
        window.titlebarAppearsTransparent = true
        if !window.styleMask.contains(.fullSizeContentView) {
            window.styleMask.insert(.fullSizeContentView)
        }
    }
}
#endif
