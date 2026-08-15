#if os(macOS)
import SwiftUI

/// 设置窗口「更新」区，PendingBot / PendingCrew 共用一份（apps/shared）。
/// 放在 Form 里使用；自带 Section 外壳。
struct UpdateSettingsSection: View {
    @ObservedObject private var updater = AppUpdater.shared

    var body: some View {
        Section {
            LabeledContent("当前版本") {
                Text(AppBuildStamp.current)
                    .textSelection(.enabled)
                    .foregroundStyle(.secondary)
            }
            Button("检查更新") { updater.checkForUpdates() }
                .disabled(!updater.canCheckForUpdates)
        } header: {
            Text("更新")
        } footer: {
            if updater.isConfigured {
                Text("自动检查新版本；发现新版会弹窗展示更新内容，安装时机由你决定。")
            } else {
                Text("此构建未配置更新源（开发构建或缺公钥），检查更新不可用。")
            }
        }
    }
}
#endif
