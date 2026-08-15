import SwiftUI

/// 同发布者「确认登录」卡：检测到本机 PendingBot 已登录时显示。
/// 头像 + 账号名 + 「继续使用此账号」（→ 家族 SSO mint）+ 「换其它账号」（→ 登录页）。
struct AccountConfirmCard: View {
    let identity: CrewLoginIdentity
    var isBusy: Bool
    var onContinue: () -> Void
    var onUseOther: () -> Void

    var body: some View {
        VStack(spacing: 20) {
            BotAvatar(seed: identity.avatarSeed, size: 72)
            VStack(spacing: 4) {
                Text(identity.title)
                    .font(Theme.Fonts.system(size: 18, weight: .semibold))
                    .foregroundStyle(Theme.Palette.ink)
                Text("已在本机登录 PendingBot")
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
            Button(action: onContinue) {
                HStack(spacing: 8) {
                    if isBusy { ProgressView().controlSize(.small) }
                    Text(isBusy ? "正在登录…" : "继续使用此账号")
                }
                .frame(maxWidth: .infinity).frame(height: 50)
            }
            .buttonStyle(.borderedProminent)
            .disabled(isBusy)

            Button("换其它账号", action: onUseOther)
                .buttonStyle(.plain)
                .font(Theme.Fonts.footnote)
                .foregroundStyle(Theme.Palette.accent)
                .disabled(isBusy)
        }
        .padding(28)
        .frame(maxWidth: 360)
    }
}
