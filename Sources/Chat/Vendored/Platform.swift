// VENDORED from PendingBot apps/pendingbot/Sources/Components/Platform.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// Platform detection shared across iOS (incl. Mac Catalyst) and the native
/// macOS build.
///
/// On iOS the only reliable "is this running on a Mac" signal is the
/// compile-time `targetEnvironment(macCatalyst)` flag —
/// `UIDevice.current.userInterfaceIdiom` returns `.pad` on Catalyst when the
/// binary ships without "Optimize Interface for Mac", which silently dropped
/// MacTabRoot back into the iOS `TabView` branch and the
/// bottom-bar-rendered-at-the-top regression that follows.
///
/// On the native macOS build (`#if os(macOS)`) we're unambiguously on a Mac,
/// so `isMac` is simply `true`. This lets the shared `Theme` font-scale and
/// `readableColumnWidth` logic apply the desktop-density treatment on the
/// real macOS app too, not just Catalyst.
enum Platform {
    #if os(macOS)
    static let isMac: Bool = true
    #elseif targetEnvironment(macCatalyst)
    static let isMac: Bool = true
    #else
    static let isMac: Bool = false
    #endif

    /// Multiplier applied to all `Theme.Fonts.*` sizes on Mac so the UI
    /// reads at desktop density (Mac users sit further from the screen
    /// than phone users; iOS-default 17pt body looks ballooned on a
    /// 27" monitor). Applied centrally in Theme so individual call
    /// sites don't have to know they're on Mac.
    static let macFontScale: CGFloat = 0.88
}
