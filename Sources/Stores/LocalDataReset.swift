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
        #if os(macOS)
        // 0. **先停 daemon，再删目录**（前后端分离 §6.1，spec 点名要在 P4 一并处理）。
        //    分家之后 session 和共享账本的写入方都在 daemon 里。不先停它，下面第 2 步
        //    删掉的目录会被它**立刻重新写回来** —— 状态快照、白板、registry 一个接一个
        //    回来，用户看到的是「清了个寂寞」，而且找不出是谁写的。
        //    没有 daemon 在跑时这是 no-op（`inproc` 模式下就是这种情况）。
        if let pid = SessionDaemonControl.runningDaemonPid() {
            let stopped = SessionDaemonControl.stopRunningDaemon()
            NSLog("[LocalDataReset] daemon pid \(pid) 停止\(stopped ? "成功" : "超时——仍继续清除")")
        }
        #endif
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
