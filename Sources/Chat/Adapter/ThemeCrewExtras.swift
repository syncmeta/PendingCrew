/// PendingCrew-side extensions to the vendored Theme.
/// These shims add tokens/helpers that PendingCrew needs but that live
/// outside the vendored Theme.swift (which must not be hand-edited).
///
/// **Never edit** `apps/pendingcrew/Sources/Chat/Vendored/Theme.swift` —
/// put all PendingCrew-specific additions here instead.
import SwiftUI

// ── Metrics extras ──────────────────────────────────────────────────────────

extension Theme.Metrics {
    /// Standard crew avatar diameter (30 pt). Used by `CrewAvatar` as the
    /// default `size` parameter; matches the iOS equivalent (28–32 pt range).
    static let avatar: CGFloat = 30
}

// ── Font call-site shims ─────────────────────────────────────────────────────
//
// CrewTheme.Fonts used positional (unlabeled) size parameters, e.g.
//   CrewTheme.Fonts.system(14, weight: .medium)
// The vendored Theme.Fonts uses labeled `size:`:
//   Theme.Fonts.system(size: 14, weight: .medium)
//
// These free functions in the Theme.Fonts namespace let existing call sites
// migrate one-for-one without touching argument labels everywhere.

extension Theme.Fonts {
    /// Convenience overload: positional `size` so macOS call sites that used
    /// `CrewTheme.Fonts.system(N)` / `CrewTheme.Fonts.system(N, weight: .X)`
    /// map directly after rename. Delegates to the canonical `system(size:…)`.
    static func system(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        system(size: size, weight: weight)
    }

    /// Convenience overload: positional `size` for `rounded`. Maps
    /// `CrewTheme.Fonts.rounded(N, weight: .X)` → `Theme.Fonts.rounded(N, weight: .X)`.
    static func rounded(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        rounded(size: size, weight: weight)
    }

    /// `callout` alias (15 pt) — matches `CrewTheme.Fonts.callout`. Internally
    /// delegates to `subheadline` which is the same size in the vendored Theme.
    static let callout = subheadline
}

// ── Palette extras ───────────────────────────────────────────────────────────

extension Theme.Palette {
    /// Claude 官方品牌橙 —— 侧栏额度行的 Claude logomark 着色（浅色主题原色，
    /// 深色主题提亮一档才不沉进近黑画布）。
    static let claudeMark = Color.adaptive(light: 0xC96442, dark: 0xE08265)

    /// OpenAI logomark 着色（浅色近黑 / 深色近白）。**Codex 复用这个标** ——
    /// simple-icons 品牌库、openai/codex 仓库、npm 包、docs favicon 都查过，
    /// 没有独立于 OpenAI 的官方 Codex logomark，人类 2026-07-26 拍板就用这个。
    static let openAIMark = Color.adaptive(light: 0x1C1D22, dark: 0xE6E8EE)

    /// 额度环的底槽（未用掉的那段弧）—— 比 hairline 重一点点，3pt 细环才看得出。
    static let quotaTrack = Color.adaptive(light: 0xE6E6E2, dark: 0x2E3038)
}
