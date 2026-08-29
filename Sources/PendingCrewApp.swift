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
                .modifier(FirstLaunchDisclosureGate())
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
            CommandGroup(replacing: .help) {
                Button("PendingCrew 帮助") {
                    NSWorkspace.shared.open(PendingCrewLinks.helpDocumentation)
                }
                .keyboardShortcut("?", modifiers: .command)
            }
        }
        #endif
    }
}

#if os(macOS)
/// Spec v2 §8.4 — 把 "本机 agent = 完整用户权限" 的 disclosure modal 挂在
/// RootView 之上。仅每台机第一次启动 PendingCrew 时显示一次,接受后写
/// `UserDefaults` 不再弹。RootView 始终渲染在背后(用户接受前看不到也点不
/// 到主界面 —— sheet 是 modal)。
private struct FirstLaunchDisclosureGate: ViewModifier {
    /// 用 `@State` 而不是常量 —— `markAccepted()` 同步改 UserDefaults 后 SwiftUI
    /// 不会自动重渲(UserDefaults 不是 @Published);我们手动把 sheet 收起来。
    @State private var showing: Bool = !FirstLaunchDisclosure.isAccepted()

    func body(content: Content) -> some View {
        content.sheet(isPresented: $showing) {
            FirstLaunchDisclosureView {
                FirstLaunchDisclosure.markAccepted()
                showing = false
            }
            // `.interactiveDismissDisabled` 防用户 Esc / 点空白处绕过 modal。
            // disclosure 必须主动接受。
            .interactiveDismissDisabled()
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
