import SwiftUI

/// PendingCrew 登录入口(确认卡 ↔ 直接登录页)的容器。
///
/// 两条进入路径:
/// - **iPad**:`RootView` 在未配置(无 device grant)时渲染本页。
/// - **macOS**:本地为家,RootView 直进主界面;本页改由侧栏的 `CrewLoginSheet`
///   按需弹出(登录是可选能力叠加)。
///
/// 逻辑:检测到本机已登录(共享 keychain 有家族凭据)→ 先展示确认卡;用户确认 →
/// `model.tryFamilySSO()` 静默 mint;失败/无凭据/点「换其它账号」→ 落到平台对应
/// 的直接登录页(`CrewMacWelcomeView` / `CrewWelcomeView`,内含 Apple/Google/邮箱/扫码)。
struct WelcomeView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showingLogin = false
    @State private var minting = false
    @State private var mintError: String?

    var body: some View {
        Group {
            if let identity = CrewLoginIdentity.current(), !showingLogin {
                VStack(spacing: 16) {
                    AccountConfirmCard(
                        identity: identity,
                        isBusy: minting,
                        onContinue: {
                            minting = true
                            mintError = nil
                            Task {
                                let ok = await model.tryFamilySSO()
                                minting = false
                                if !ok {
                                    // 凭据可能失效 —— 落到登录页让用户重新登录。
                                    mintError = "本机登录凭据可能已失效，请重新登录。"
                                    showingLogin = true
                                }
                            }
                        },
                        onUseOther: { showingLogin = true }
                    )
                    if let mintError {
                        Text(mintError)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
            } else {
                #if os(macOS)
                CrewMacWelcomeView()
                #else
                CrewWelcomeView()
                #endif
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
