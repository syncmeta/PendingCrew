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
            // 品牌符号 + 可选数字。**图标恒定一个样，数字有无就是唯一的信号** ——
            // 没事没有数字，有事才有。
            //
            // 图标用的是我们自己的 `PendingCrewSymbolFill`（Assets 里的 .symbolset，
            // 一份规范的 SF Symbol 模板：带 Guides / 各权重的 Baseline·Capline·margin，
            // 无渐变无位图 ⇒ **矢量单色**，菜单栏会按系统外观自动反色）。
            //
            // **实心**是人类 2026-09-08 装上 0.1.28 看过菜单栏之后点名要的（Todo #127）。
            // 它跟 `PendingCrewSymbol`（描边版）是同一份路径、只差 `fill` 与 `stroke`
            // 那一处 —— 描边版仍在用：空群占位图标要的是轻，那儿用淡色 `.tertiary`，
            // 实心在那个位置会压得太重。**两个变体各有去处，别合并成一个。**
            //
            // ⚠️ **别再给图标本身加第二种状态**（加粗、换字形、变色都算）。
            // 2026-09-08 人类当面拍的，原话：「没事不用加粗。没事就没有数字
            // 有事就有数字」。在此之前这里试过两版：空心↔实心、常规↔加粗 ——
            // 两版都是**跟「有没有数字」重复的第二个信号**，而不是补充。
            // 同一件事说两遍不会让它更醒目，只会让图标一直在动。
            Label {
                if let badge = menuBarAttention.count.badge { Text(badge) }
            } icon: {
                Image("PendingCrewSymbolFill")
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
