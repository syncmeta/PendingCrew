#if os(macOS)
import Combine
import Foundation
import Sparkle

/// 三 app 统一的 Sparkle 封装（spec 2026-08-06-unified-sparkle-updates）。
/// 自动**检查**开、自动**安装**关 —— 发现新版弹标准窗口，装不装由人点。
///
/// Info.plist 缺 SUFeedURL 或 SUPublicEDKey（P3 密钥生成前的构建都缺钥）
/// 时 `isConfigured == false`：Sparkle 完全不启动，设置区如实报「未配置」。
/// 不带病启动、不静默假装能更新。
@MainActor
final class AppUpdater: NSObject, ObservableObject {
    static let shared = AppUpdater()

    /// 忙判定注入点：PendingCrew 在 MacRootView 注入「有 session 在跑」；
    /// PendingBot 不注入（nil = 永不忙）。见 UpdateCheckGate。
    var isBusy: (() -> Bool)?

    @Published private(set) var canCheckForUpdates = false
    let isConfigured: Bool
    private var controller: SPUStandardUpdaterController?
    private var cancellable: AnyCancellable?

    private override init() {
        let bundle = Bundle.main
        let feed = bundle.object(forInfoDictionaryKey: "SUFeedURL") as? String ?? ""
        let publicKey = bundle.object(forInfoDictionaryKey: "SUPublicEDKey") as? String ?? ""
        isConfigured = URL(string: feed)?.scheme == "https" && !publicKey.isEmpty
        super.init()
        guard isConfigured else { return }
        let controller = SPUStandardUpdaterController(
            startingUpdater: false, updaterDelegate: self, userDriverDelegate: nil)
        self.controller = controller
        cancellable = controller.updater.publisher(for: \.canCheckForUpdates)
            .receive(on: RunLoop.main)
            .sink { [weak self] value in self?.canCheckForUpdates = value }
        controller.startUpdater()
    }

    func checkForUpdates() {
        controller?.checkForUpdates(nil)
    }
}

extension AppUpdater: SPUUpdaterDelegate {
    /// 每次检查前 Sparkle 都来问；抛错 = 这次不查（Sparkle 稍后自动重试）。
    /// `.updates` 是人手点的，永远放行；后台定时检查看忙不忙。
    func updater(_ updater: SPUUpdater, mayPerform updateCheck: SPUUpdateCheck) throws {
        let userInitiated = (updateCheck == .updates)
        guard UpdateCheckGate.allows(userInitiated: userInitiated, busy: isBusy?() ?? false) else {
            throw NSError(domain: "AppUpdater", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "有 session 正在运行，暂缓后台检查更新",
            ])
        }
    }
}
#endif
