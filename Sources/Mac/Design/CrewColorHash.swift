#if os(macOS)
import SwiftUI

/// id → 稳定的柔色底 + emoji + 首字。算法搬自 PendingBot iOS ColorHash
/// (apps/pendingbot/Sources/Components/ColorHash.swift)，保证同一 bot/user 在
/// 两端头像配色一致。常量以 PendingBot 为准（视觉源真值）：
/// - hue：Int32 `&* 31 &+` (Java-style 串哈希)，归一化到 0..<1.0
/// - softBackground：saturation 0.22 / brightness 0.94
/// - emoji：与 hue 同款哈希，64 条策展列表
enum CrewColorHash {
    /// Hash → 0..<1.0 hue，跨会话/跨平台稳定。
    static func hue(for seed: String) -> Double {
        var h: Int32 = 0
        for scalar in seed.unicodeScalars {
            h = h &* 31 &+ Int32(bitPattern: scalar.value)
        }
        return Double(abs(Int(h)) % 360) / 360.0
    }

    /// 柔色底（低饱和高明度的暖色洗）。
    static func softBackground(for seed: String) -> Color {
        Color(hue: hue(for: seed), saturation: 0.22, brightness: 0.94)
    }

    static func initial(for name: String) -> String {
        guard let first = name.trimmingCharacters(in: .whitespaces).first else { return "?" }
        return String(first).uppercased()
    }

    /// 头像 emoji 池 —— 与 PendingBot 一字不差（同 seed 两端同 emoji）。
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

    /// 与 `hue(for:)` 同款哈希，让色相与 emoji 不漂移。
    static func emoji(for seed: String) -> String {
        var h: Int32 = 0
        for scalar in seed.unicodeScalars {
            h = h &* 31 &+ Int32(bitPattern: scalar.value)
        }
        let idx = abs(Int(h)) % avatarEmoji.count
        return avatarEmoji[idx]
    }
}
#endif
