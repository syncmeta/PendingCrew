// VENDORED from PendingBot apps/pendingbot/Sources/Components/ColorHex.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
import SwiftUI
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

extension Color {
    /// Build a Color from a 24-bit sRGB hex literal. Example: `Color(hex: 0x044735)`.
    ///
    /// Cross-platform brand-color helper — moved out of the iOS-only `Theme.swift`
    /// so macOS views (`Sources/Mac/*`) can reuse the same brand palette hex values
    /// and stay visually aligned with iOS.
    init(hex: UInt32, alpha: Double = 1) {
        self = Color(
            .sRGB,
            red:   Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >>  8) & 0xFF) / 255,
            blue:  Double( hex        & 0xFF) / 255,
            opacity: alpha
        )
    }

    /// Appearance-adaptive sRGB color: resolves to `light` in light mode, `dark`
    /// in dark mode. This is the single seam dark mode flows through — every
    /// `Theme.Palette` token is built from it, so flipping the system appearance
    /// (or the in-app override) repaints the whole app from one definition with
    /// no per-call-site work.
    ///
    /// Backed by the platform's dynamic color (`UIColor`/`NSColor` provider), so
    /// resolution happens at render time against the ambient trait collection /
    /// appearance — `.preferredColorScheme(...)` on an ancestor (or the system)
    /// drives it. While the app still forces `.light`, these stay on `light`.
    static func adaptive(light: UInt32, dark: UInt32, alpha: Double = 1) -> Color {
        #if canImport(UIKit)
        return Color(UIColor { traits in
            UIColor(srgbHex: traits.userInterfaceStyle == .dark ? dark : light, alpha: alpha)
        })
        #elseif canImport(AppKit)
        return Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return NSColor(srgbHex: isDark ? dark : light, alpha: alpha)
        })
        #else
        return Color(hex: light, alpha: alpha)
        #endif
    }
}

#if canImport(UIKit)
private extension UIColor {
    convenience init(srgbHex hex: UInt32, alpha: Double) {
        self.init(
            red:   CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >>  8) & 0xFF) / 255,
            blue:  CGFloat( hex        & 0xFF) / 255,
            alpha: CGFloat(alpha)
        )
    }
}
#elseif canImport(AppKit)
private extension NSColor {
    convenience init(srgbHex hex: UInt32, alpha: Double) {
        self.init(
            srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
            green:   CGFloat((hex >>  8) & 0xFF) / 255,
            blue:    CGFloat( hex        & 0xFF) / 255,
            alpha:   CGFloat(alpha)
        )
    }
}
#endif
