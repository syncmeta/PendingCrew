import Foundation
#if os(macOS)
import AppKit
#endif

/// PendingCrew「清除本机所有数据」协调器。仅本地清除;清完重启 app 以达到真·全新安装。
@MainActor
enum LocalDataReset {
    /// 与 PendingBot 共享的登录状态是否存在 —— 决定确认弹窗是否给"保留/一并清除"二选一。
    static var sharedLoginPresent: Bool { FamilyCredentialStore.get() != nil }

    /// 执行清除。`clearSharedLogin` 为 true 时连"与 PendingBot 共享的登录状态"一并删。
    static func performReset(clearSharedLogin: Bool) {
        // 1. 本 app 登录 token
        KeychainStore.delete(account: KeychainAccount.deviceGrant)
        // 2. 与 PendingBot 共享的登录状态(按用户选择)
        if clearSharedLogin { FamilyCredentialStore.clear() }
        // 3. UserDefaults 整域(apiBaseURL / 首启免责声明标志 / deviceId / appearance 等)
        if let bundleId = Bundle.main.bundleIdentifier {
            UserDefaults.standard.removePersistentDomain(forName: bundleId)
        }
        // 4. 本地 crew 数据目录(local-crews / captain-templates / whiteboards / approvals 全包)
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        if let dir = support?.appendingPathComponent("PendingCrew", isDirectory: true) {
            try? FileManager.default.removeItem(at: dir)
        }
        // 5. macOS 重启 app（绕开 isConfigured 恒 true + 内存单例/@State/disclosure 缓存）。
        //    iOS 不能自重启，清完凭据和 defaults 后交给现有 app 状态流回到未配置态。
        #if os(macOS)
        relaunch()
        #endif
    }

    #if os(macOS)
    private static func relaunch() {
        let url = Bundle.main.bundleURL
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: url, configuration: config) { _, _ in
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }
    #endif
}
