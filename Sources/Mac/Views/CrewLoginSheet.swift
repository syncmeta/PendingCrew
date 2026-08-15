#if os(macOS)
import SwiftUI

/// macOS 侧栏「登录」入口的 sheet 容器：直接复用自管理的 WelcomeView
/// （确认卡 ↔ 直接登录页 CrewMacWelcomeView），登录成功（isAuthenticated 翻 true）即自动关闭。
struct CrewLoginSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        WelcomeView()
            .onChange(of: model.isAuthenticated) { _, authed in
                if authed { dismiss() }
            }
            .overlay(alignment: .topTrailing) {
                // 与驾驶舱等浮层同一颗玻璃白关闭件（Todo #22）。
                GlassCloseButton(action: { dismiss() })
                    .padding(12)
            }
    }
}
#endif
