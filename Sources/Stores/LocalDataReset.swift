import Foundation
#if os(macOS)
import AppKit
#endif

/// PendingCrew「清除本机所有数据」协调器。仅本地清除;清完重启 app 以达到真·全新安装。
@MainActor
enum LocalDataReset {
    /// 执行清除。
    ///
    /// #63 第二期之前这里还清两样凭据（本 app 的 device grant、与 PendingBot 共享
    /// 的家族凭据），确认弹窗因此是「保留/一并清除」二选一。凭据层随跨端遥控整层
    /// 删除，本 app 已不再往 Keychain 写任何东西，弹窗也收成一个按钮。
    static func performReset() {
        // 1. UserDefaults 整域(首启免责声明标志 / deviceId / appearance 等)
        if let bundleId = Bundle.main.bundleIdentifier {
            UserDefaults.standard.removePersistentDomain(forName: bundleId)
        }
        // 2. 本地 crew 数据目录(local-crews / captain-templates / whiteboards / approvals 全包)
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        if let dir = support?.appendingPathComponent("PendingCrew", isDirectory: true) {
            try? FileManager.default.removeItem(at: dir)
        }
        // 3. macOS 重启 app（绕开内存单例/@State/disclosure 缓存）。
        //    iOS 不能自重启，清完 defaults 后交给现有 app 状态流。
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
