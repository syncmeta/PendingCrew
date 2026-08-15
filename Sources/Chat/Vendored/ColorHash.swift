// VENDORED from PendingBot apps/pendingbot/Sources/Components/ColorHash.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
import SwiftUI

/// Stable per-bot visual identity derived from its id. Cross-platform (pure
/// SwiftUI Color, no UIKit) so iOS + macOS avatars share the exact same emoji
/// + tint for a given seed.
///
/// Stable per-bot visual identity derived from its id. Produces a two-stop
/// gradient so avatars feel lively without being fussy. Same shape as the
/// reference impl — keeps the brand identity consistent across both apps.
enum ColorHash {

    /// Hash → 0..<360 hue, stable across sessions and platforms.
    static func hue(for botId: String) -> Double {
        var h: Int32 = 0
        for scalar in botId.unicodeScalars {
            h = h &* 31 &+ Int32(bitPattern: scalar.value)
        }
        return Double(abs(Int(h)) % 360) / 360.0
    }

    /// Two analogous hues, slightly shifted, forming a gentle gradient.
    /// Kept around in case anything still wants the old vivid look.
    static func gradient(for botId: String) -> LinearGradient {
        let h = hue(for: botId)
        let top    = Color(hue: h,
                           saturation: 0.46, brightness: 0.78)
        let bottom = Color(hue: (h + 0.08).truncatingRemainder(dividingBy: 1.0),
                           saturation: 0.58, brightness: 0.62)
        return LinearGradient(colors: [top, bottom],
                              startPoint: .topLeading,
                              endPoint: .bottomTrailing)
    }

    /// Soft pastel fill used by conversation avatars. Low saturation,
    /// high brightness — reads as "warm wash" against the cream canvas
    /// rather than competing with the brand accent.
    static func softBackground(for seed: String) -> Color {
        let h = hue(for: seed)
        return Color(hue: h, saturation: 0.22, brightness: 0.94)
    }

    static func initial(for name: String) -> String {
        guard let first = name.trimmingCharacters(in: .whitespaces).first else { return "?" }
        return String(first).uppercased()
    }

    /// Stable emoji glyph for an avatar — same bot id always yields the
    /// same emoji across launches. Keeps the curated list small enough
    /// (~64 entries) that collisions feel "another bot", not "a glitch".
    /// Mostly faces / animals / objects so anything reads as a "creature".
    private static let avatarEmoji: [String] = [
        "🦊", "🐼", "🐯", "🦁", "🐸", "🐧", "🐳", "🐙",
        "🦉", "🦄", "🐝", "🦋", "🐢", "🐬", "🦒", "🦔",
        "🦕", "🦖", "🐲", "🦚", "🦩", "🐌", "🐞", "🦜",
        "🌵", "🌻", "🌸", "🌙", "⭐", "🔥", "🍄", "🍀",
        "🍓", "🍑", "🍋", "🍇", "🥑", "🌽", "🥨", "🍪",
        "🎈", "🎨", "🎭", "🎪", "🎲", "🧩", "🪁", "🪐",
        "🚀", "⛵", "🏕", "🗿", "🪴", "🪨", "💎", "🧊",
        "🦤", "🦥", "🦦", "🦨", "🐿", "🦫", "🪼", "🪿",
    ]

    /// Pick an emoji deterministically from the bot id. Same algorithm as
    /// `hue(for:)` so the gradient + emoji stay coupled (don't drift apart).
    static func emoji(for botId: String) -> String {
        var h: Int32 = 0
        for scalar in botId.unicodeScalars {
            h = h &* 31 &+ Int32(bitPattern: scalar.value)
        }
        let idx = abs(Int(h)) % avatarEmoji.count
        return avatarEmoji[idx]
    }
}
