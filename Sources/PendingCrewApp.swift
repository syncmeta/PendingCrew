import SwiftUI
#if os(macOS)
import AppKit
#endif

enum PendingCrewLinks {
    static let helpDocumentation = URL(string: "https://docs.pendingname.com/pendingcrew/")!
}

/// GUI 入口。`@main` 挪到 `PendingCrewEntry` —— 它先看 argv：带 `--mcp-serve` /
/// `--mcp-hook` 时本进程当 crew-comms helper 跑（re-exec self，spec local-first
/// chunk 4：app 二进制兼当 MCP server/hook，最自包含、免 embed），否则起 GUI。
struct PendingCrewApp: App {
    @StateObject private var model: AppModel
    @StateObject private var crewStore: CrewStore
    #if os(macOS)
    /// 长期职责的唯一所有者（spec §6）。**必须挂在 App 上而不是任何视图上** ——
    /// 挂视图上就会随视图生灭，那正是我们要修的病。
    @StateObject private var sessionHost = SessionHost()
    /// 菜单栏那个数字（P5b·B）。挂在 App 上而不是任何视图上 —— 它要在主窗口
    /// 关着的时候仍然在数，那正是这个功能存在的理由。
    @StateObject private var menuBarAttention = MenuBarAttentionModel()
    #endif
    /// Captain 模板池(BYOK 模式的"本机 captain 池",spec v2 §5.2)。
    /// 登录态下也注入但 UI 不消费 —— 登录态走真 bot 库(后续 task)。
    @StateObject private var captainTemplates = CaptainTemplateStore.shared
    /// App-wide appearance override (跟随系统/浅/深)，设置里改。
    /// `.system` → nil → 跟随 OS。对齐 PendingBot #299。
    @AppStorage(AppearanceMode.storageKey) private var appearanceRaw = AppearanceMode.default.rawValue

    init() {
        let appModel = AppModel()
        _model = StateObject(wrappedValue: appModel)
        _crewStore = StateObject(wrappedValue: CrewStore(appModel: appModel))
        #if os(macOS)
        // 启动即挂 Sparkle 定时检查，不等登录 —— 此前只在 MacRootView.onAppear
        // 才首次触达 AppUpdater.shared，未登录的装机永远不会跑后台检查
        // （PendingCrew 未登录也是常态：本地为家）。未配置更新源时自动 no-op。
        // 对齐 PendingBotApp.swift 的同款 init 接线。
        MainActor.assumeIsolated {
            _ = AppUpdater.shared
        }
        #endif
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                .environmentObject(crewStore)
                .environmentObject(captainTemplates)
                #if os(macOS)
                .environmentObject(sessionHost)
                #endif
                // 外观跟随设置(跟随系统/浅/深)。`.system` → nil → 跟随 OS。
                // 同时覆盖 macOS + iOS/iPadOS。对齐 PendingBot #299。
                .preferredColorScheme((AppearanceMode(rawValue: appearanceRaw) ?? .default).colorScheme)
                #if os(macOS)
                .frame(minWidth: 1040, minHeight: 680)
                #endif
        }
        #if os(macOS)
        // macOS 系统设置场景：⌘, 打开。独立窗口，单独绑同一个外观 key，
        // 这样在设置窗口改外观时它本身也即时重绘。对齐 PendingBot #299。
        Settings {
            CrewSettingsView()
                .preferredColorScheme((AppearanceMode(rawValue: appearanceRaw) ?? .default).colorScheme)
        }
        .commands {
            PendingCrewUpdateCommands()
            CommandGroup(replacing: .help) {
                Button("PendingCrew 帮助") {
                    NSWorkspace.shared.open(PendingCrewLinks.helpDocumentation)
                }
                .keyboardShortcut("?", modifiers: .command)
            }
        }
        // 菜单栏常驻入口（P5b·B）：不开主窗口也看得到有几件事在等人拍板。
        // **图标常在、数字只在有事时出现** —— 图标是「点一下进去」的入口，
        // 消失了人就没地方点；而常年挂一个 0 会训练人忽略它。
        MenuBarExtra {
            MenuBarPanel(attention: menuBarAttention)
                .environmentObject(model)
                .environmentObject(crewStore)
                .preferredColorScheme((AppearanceMode(rawValue: appearanceRaw) ?? .default).colorScheme)
        } label: {
            // 品牌符号 + 可选数字。有事时加粗，安静时常规 ——
            // 一眼扫过去不用读数字就知道要不要停下来。
            //
            // 图标用的是我们自己的 `PendingCrewSymbol`（Assets 里的 .symbolset，
            // 一份规范的 SF Symbol 模板：带 Guides / 各权重的 Baseline·Capline·margin，
            // `fill="none"`、无渐变无位图 ⇒ **矢量单色**，菜单栏会按系统外观自动反色）。
            //
            // **两态为什么用权重而不是空心/实心**：空心↔实心那一对是 SF Symbol
            // 自带的 `.fill` 变体，自定义 symbol 没有第二个字形可换。而这个符号的
            // SVG 里本来就定义了 Ultralight→Black 各档的 margin，**它是照多权重设计的**，
            // 所以按权重分档是顺着它的设计走，不是将就。
            //
            // 换图标的时候别把这个两态一起丢了 —— 它不是装饰：
            // 常年挂一个数字会训练人忽略它，而「粗细变了」是不用读数字就能扫到的信号。
            Label {
                if let badge = menuBarAttention.count.badge { Text(badge) }
            } icon: {
                Image("PendingCrewSymbol")
                    .fontWeight(menuBarAttention.count.isQuiet ? .regular : .bold)
            }
            .accessibilityLabel(menuBarAttention.count.summary)
        }
        .menuBarExtraStyle(.window)
        .onChange(of: crewStore.crews.count, initial: true) { _, _ in
            menuBarAttention.start(crewStore: crewStore)
        }
        #endif
    }
}

#if os(macOS)
/// 放在 App 菜单「关于 PendingCrew」下方；观察 updater 才能在 Sparkle 启动完成后
/// 把初始禁用的菜单项实时变为可点。
private struct PendingCrewUpdateCommands: Commands {
    @ObservedObject private var updater = AppUpdater.shared

    var body: some Commands {
        CommandGroup(after: .appInfo) {
            Button("检查更新…") { updater.checkForUpdates() }
                .disabled(!updater.canCheckForUpdates)
        }
    }
}
#endif

/// 启动路由。只有一种顶层态:**直接进主界面** —— Mac 走 `MacThreePaneView`,
/// iPad/iOS 走 `IPadShell`。
///
/// #63:PendingCrew 不再登录到任何地方,登录页整块删掉,原来那条按
/// 「是否已配置」在主界面 / 登录页之间分叉的路由一并去掉。
struct RootView: View {
    @EnvironmentObject private var crewStore: CrewStore

    var body: some View {
        Group {
            #if os(iOS)
            IPadShell()
            #else
            MacThreePaneView()
            #endif
        }
        // 进入主界面时确保机器列表已就绪。macOS backend 恒本地（至少一台本机）。
        .task {
            await crewStore.refreshMachines()
        }
    }
}

// MacThreePaneView 实现挪到 Mac/Views/MacRootView.swift。
