// VENDORED from PendingBot apps/pendingbot/Sources/Components/Theme.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// One place to change the look. Mirrors the design language used across
/// the PendingBot reference + the web `tokens.css`: deep-jade accent on
/// warm-ink neutrals, editorial serif for titles, rounded sans for chrome.
///
/// Dual-mode theme — the brand is two anchor colors (#044735 + #fdfcfa) in
/// light; the dark variant (2026-06-11 spec) swaps the light source rather than
/// inverting. Every `Palette` token is `Color.adaptive(light:dark:)`, so the
/// whole app repaints from this one definition when the appearance flips.
///
/// **Cross-platform:** the palette, the `Font`-based typography, and the
/// spacing/radii are pure SwiftUI and compile on both iOS and the native
/// macOS build (`Sources/Mac/*`), so the Mac UI stays visually identical to
/// iOS. Only the `UIFont`-returning helpers (used by UIKit-backed renderers
/// like MarkdownUI / SwiftMath) are gated behind `canImport(UIKit)`.
enum Theme {

    // ── Palette ─────────────────────────────────────────────────────────────

    /// Every token is appearance-adaptive (`Color.adaptive(light:dark:)`).
    /// Light values are the original warm-cream palette; dark values are the
    /// 2026-06-11 dark spec (`docs/superpowers/specs/2026-06-11-dark-mode-palette.md`).
    ///
    /// Dark-mode design intent (see spec): dark is a *light-source swap*, not an
    /// inversion. The brand green flips from "ink" (deep forest on cream) to
    /// "light" (bright jade glowing on warm near-black); color temperature stays
    /// warm throughout; the surface ladder keeps the same "closer-to-user =
    /// brighter" direction; the three tag pairs (green/amber/plum) hold one
    /// brightness register in both modes.
    enum Palette {
        /// Brand accent — deep forest green / bright jade.
        static let accent      = Color.adaptive(light: 0x044735, dark: 0x57B690)
        /// Hover/pressed — slightly lighter so it reads as "lit up".
        static let accentHover = Color.adaptive(light: 0x0A6049, dark: 0x6CC6A1)
        /// Soft accent fill — used for the user bubble + selected pills.
        /// Light: tinted from accent, washed toward cream. Dark: green-dyed
        /// deep surface so the bubble still reads as "belongs to green".
        static let accentBg    = Color.adaptive(light: 0xE3EEEA, dark: 0x1C3A2F)

        /// Warm amber — public-bot tag fg. Sits in the same brightness
        /// register as the deep accent green so the two pills feel paired.
        static let amber       = Color.adaptive(light: 0x6F5A0B, dark: 0xE0BD55)
        /// Public-bot tag bg, washed from `amber` (fg/bg swap direction in dark).
        static let amberBg     = Color.adaptive(light: 0xFAEDC2, dark: 0x3B3110)

        /// Plum — private-bot tag fg. Same brightness register as the
        /// accent green and amber so all three tag pills feel paired.
        static let plum        = Color.adaptive(light: 0x5B3D8A, dark: 0xB9A0E3)
        /// Private-bot tag bg, washed from `plum`.
        static let plumBg      = Color.adaptive(light: 0xECE3F7, dark: 0x322546)

        /// Page canvas — pure white / warm near-black.
        static let canvas        = Color.adaptive(light: 0xFFFFFF, dark: 0x161512)
        /// Cards / sheets — sits a hair above canvas (brighter in both modes).
        static let surface       = Color.adaptive(light: 0xFFFFFF, dark: 0x201E1A)
        /// Subtler than surface — chips / pills / muted bg fills.
        static let surfaceMuted  = Color.adaptive(light: 0xEFEEE9, dark: 0x2A2823)

        /// Primary ink — warm near-black / warm white.
        static let ink       = Color.adaptive(light: 0x1B1A14, dark: 0xECE9E0)
        /// Secondary — labels, timestamps, descriptions (≈6:1 either way).
        static let inkMuted  = Color.adaptive(light: 0x6E6A5C, dark: 0xA8A294)
        /// Hairline borders — barely-there separators.
        static let hairline  = Color.adaptive(light: 0xE2E0D7, dark: 0x383530)

        /// User-message bubble — same as accentBg, named for clarity.
        static let userBubble = accentBg
        /// Optimistic user bubble while the send request is in flight (no
        /// HTTP response yet). Washed toward canvas so the bubble reads as
        /// "not yet acknowledged"; flips to `userBubble` the moment the
        /// server returns 200.
        static let userBubbleSending = Color.adaptive(light: 0xF1F6F3, dark: 0x182A23)

        /// Text/icon sitting ON an `accent` fill (发送按钮、selected pill)。
        /// Light: white on deep forest. Dark: the accent flips to bright jade
        /// (a *light* source), so the ink on it flips to deep green — plain
        /// white would wash out against jade.
        static let onAccent = Color.adaptive(light: 0xFFFFFF, dark: 0x0E2A20)

        /// Sign-in-with-Apple pill fill. Apple's branding rule: black button
        /// on light UI, white button on dark UI (a black pill would sink into
        /// the near-black canvas).
        static let applePill   = Color.adaptive(light: 0x000000, dark: 0xFFFFFF)
        /// Logo + label sitting ON `applePill` — the inverse of the fill.
        static let onApplePill = Color.adaptive(light: 0xFFFFFF, dark: 0x000000)
        /// "Google" label on the glass pill. Light: Google's dark-grey text;
        /// dark: their official dark-theme text grey (#E3E3E3).
        static let googleInk   = Color.adaptive(light: 0x2E2E33, dark: 0xE3E3E3)

        /// Destructive / error fg — terracotta. Was 12 scattered `0xB14B3C`
        /// literals; centralised so dark mode lifts them in one place.
        static let danger   = Color.adaptive(light: 0xB14B3C, dark: 0xE08D7A)
        /// Washed terracotta fill behind danger content (tool-trace error chip).
        static let dangerBg = Color.adaptive(light: 0xF4DDD6, dark: 0x3C241E)
        /// Positive-delta / success fg(钱包入账、群钱包动作成功提示).
        static let success  = Color.adaptive(light: 0x2E7D5B, dark: 0x6FBF99)
        /// Wallet-tier gold(钱包档位/预警色,亮度寄存同 amber 但更金).
        static let gold     = Color.adaptive(light: 0xB8862B, dark: 0xD9B45F)
    }

    // ── Typography ──────────────────────────────────────────────────────────

    enum Fonts {
        /// Per-platform multiplier (1.0 on iOS, ~0.88 on Mac / Catalyst).
        /// Applied to every size below so the desktop UI doesn't read as
        /// the iOS layout zoomed in.
        private static let scale: CGFloat = Platform.isMac ? Platform.macFontScale : 1.0
        private static func sized(_ pt: CGFloat) -> CGFloat { pt * scale }

        /// Exposed for callers that need a raw scaled point size — e.g.
        /// MarkdownUI's `FontSize(...)` which takes a CGFloat, so the
        /// bot's markdown body can match the user bubble's
        /// `Theme.Fonts.body` size on Mac (otherwise the bot reads at
        /// 16pt while the user reads at ~14pt and the two sides look
        /// mismatched).
        static func scaled(_ pt: CGFloat) -> CGFloat { sized(pt) }

        static func system(
            size: CGFloat,
            weight: Font.Weight = .regular,
            design: Font.Design = .default
        ) -> Font {
            .system(size: sized(size), weight: weight, design: design)
        }

        static func monospaced(size: CGFloat, weight: Font.Weight = .regular) -> Font {
            system(size: size, weight: weight, design: .monospaced)
        }

        /// Unscaled font-sized glyphs: SF Symbols, emoji avatars, and icon-like
        /// marks whose size is tied to a fixed visual container rather than text.
        static func glyph(
            size: CGFloat,
            weight: Font.Weight = .regular,
            design: Font.Design = .default
        ) -> Font {
            .system(size: size, weight: weight, design: design)
        }

        #if canImport(UIKit)
        static func uiSystem(size: CGFloat, weight: UIFont.Weight = .regular) -> UIFont {
            .systemFont(ofSize: sized(size), weight: weight)
        }

        static func uiMonospaced(size: CGFloat, weight: UIFont.Weight = .regular) -> UIFont {
            .monospacedSystemFont(ofSize: sized(size), weight: weight)
        }

        /// UIKit font from an already-resolved point size. Use when the
        /// caller receives a size from another scaled renderer, such as math.
        static func uiResolvedMonospaced(size: CGFloat, weight: UIFont.Weight = .regular) -> UIFont {
            .monospacedSystemFont(ofSize: size, weight: weight)
        }
        #endif

        /// Serif display (system New York). Used for titles, header marks,
        /// avatar initials. Feels editorial vs the regular SF Pro UI.
        static func serif(size: CGFloat, weight: Font.Weight = .semibold) -> Font {
            system(size: size, weight: weight, design: .serif)
        }

        /// Rounded sans — softer than SF Pro, used for buttons, chip labels,
        /// nav, counters. Matches the warm palette.
        static func rounded(size: CGFloat, weight: Font.Weight = .regular) -> Font {
            system(size: size, weight: weight, design: .rounded)
        }

        static let body            = system(size: 16)
        static let bodyEmphasized  = system(size: 16, weight: .medium)
        static let subheadline     = system(size: 15)
        static let headline        = system(size: 17, weight: .semibold)
        static let footnote        = system(size: 13)
        static let caption         = system(size: 12)
        static let caption2        = system(size: 11)
        static let title           = serif(size: 28, weight: .semibold)
        static let title2          = system(size: 22)
        static let title3          = system(size: 20)
        static let sectionTitle    = serif(size: 20, weight: .semibold)
        static let monoSmall       = monospaced(size: 12)
    }

    // ── Spacing / radii ─────────────────────────────────────────────────────

    enum Metrics {
        static let gutter: CGFloat = 16
        static let rowVPad: CGFloat = 10
        static let bubbleRadius: CGFloat = 18
        static let cardRadius: CGFloat = 14
        static let pillRadius: CGFloat = 999

        /// Cap on a column of long-form content (chat bubbles, list rows).
        /// Above this width — iPad landscape, Mac windows — letting the
        /// column stretch produces 1500pt-wide bubbles that read poorly.
        /// 760 keeps lines around 70–80 chars which matches reading research.
        static let readableColumn: CGFloat = 760
    }
}

// ── Wide-screen reading column ──────────────────────────────────────────────

extension View {
    /// Center the view inside a column capped at `Theme.Metrics.readableColumn`.
    /// On iPhone portrait this is a no-op (the screen is narrower than the cap);
    /// on iPad landscape it keeps content from spreading across the window.
    ///
    /// **Mac:** the cap is dropped and the inner frame becomes leading-aligned.
    /// Mac windows are usually narrower than iPad landscape, and the
    /// vertical tab rail + NavigationSplitView already chop the content
    /// area into reasonable columns — leaving a centered 760pt island
    /// inside that produces big empty gutters on both sides which read as
    /// "this app doesn't know it's on Mac."
    func readableColumnWidth(
        _ maxWidth: CGFloat = Theme.Metrics.readableColumn
    ) -> some View {
        Group {
            if Platform.isMac {
                self.frame(maxWidth: .infinity, alignment: .leading)
            } else {
                self.frame(maxWidth: maxWidth)
                    .frame(maxWidth: .infinity, alignment: .center)
            }
        }
    }
}

// `Color(hex:)` lives in the cross-platform `ColorHex.swift` so macOS shares it.
