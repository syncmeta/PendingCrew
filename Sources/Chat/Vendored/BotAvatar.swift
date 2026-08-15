// VENDORED from PendingBot apps/pendingbot/Sources/Components/BotAvatar.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
import SwiftUI

/// Soft pastel circle with an emoji glyph.
///
/// `seed` is the single-source case (default): same string drives both
/// hue and emoji. Used for places without a conversation context (the
/// user's own profile avatar, friend rows, etc.).
///
/// `emojiSeed` + `colorSeed` is the split case: emoji stays fixed per
/// bot identity while the background tint varies per conversation, so
/// each chat with the same bot gets its own colour but the face stays
/// recognisable. Pass `emojiSeed: bot.id`, `colorSeed: conv.id`.
struct BotAvatar: View {
    let emojiSeed: String
    let colorSeed: String
    var size: CGFloat = 36

    init(seed: String, size: CGFloat = 36) {
        self.emojiSeed = seed
        self.colorSeed = seed
        self.size = size
    }

    init(emojiSeed: String, colorSeed: String, size: CGFloat = 36) {
        self.emojiSeed = emojiSeed
        self.colorSeed = colorSeed
        self.size = size
    }

    var body: some View {
        ZStack {
            Circle()
                .fill(ColorHash.softBackground(for: colorSeed))
            Text(ColorHash.emoji(for: emojiSeed))
                .font(Theme.Fonts.glyph(size: size * 0.55))
        }
        .frame(width: size, height: size)
        .overlay(
            Circle().strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)
        )
    }
}
