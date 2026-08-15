import SwiftUI

/// 顶部 roster 头像条：一排成员头像（session 带状态点），保留 crew 特有的
/// "一眼看 captain/session 运行状态" 价值，视觉对齐 iOS。
///
/// 头像改用 `CrewAvatarBadges(sender:size:)` — 与气泡行里的头像来自同一组件，
/// 保证星标角标 / 状态点 / terminal 头像在 roster 和对话里完全一致。
struct CrewRosterBar: View {
    let members: [CrewMember]
    let captainBotId: String?

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 14) {
                ForEach(members) { m in
                    let sender = CrewSenderNaming.groupSender(for: m, captainBotId: captainBotId)
                    VStack(spacing: 3) {
                        CrewAvatarBadges(sender: sender, size: 28)
                        Text(sender.displayName)
                            .font(Theme.Fonts.caption2)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .lineLimit(1)
                            .frame(maxWidth: 76)
                    }
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
        }
        .background(Theme.Palette.surfaceMuted.opacity(0.5))
    }
}
