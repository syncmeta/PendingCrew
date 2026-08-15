// VENDORED from PendingBot apps/pendingbot/Sources/Components/Haptics.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
#if os(iOS)
import UIKit

/// Centralized haptic feedback. Avoids creating one-shot generators all over
/// the codebase (which is wasteful — they need to be prepared to feel snappy).
enum Haptics {
    private static let light = UIImpactFeedbackGenerator(style: .light)
    private static let soft  = UIImpactFeedbackGenerator(style: .soft)
    private static let rigid = UIImpactFeedbackGenerator(style: .rigid)
    private static let selection = UISelectionFeedbackGenerator()
    private static let notification = UINotificationFeedbackGenerator()

    /// Throttle for `streamTick()` — token reveals happen on every frame
    /// when the buffer is full, but a haptic on every frame would be a
    /// vibrating buzz. ChatGPT-style: a faint pulse roughly twice a
    /// second, regardless of how fast tokens arrive.
    nonisolated(unsafe) private static var lastStreamTickAt: TimeInterval = 0

    /// Call from app launch so the first haptic isn't laggy.
    static func warmUp() {
        light.prepare(); soft.prepare(); rigid.prepare()
        selection.prepare(); notification.prepare()
    }

    /// Sending a message — short and confident.
    @MainActor static func send()    { light.impactOccurred(intensity: 0.7) }
    /// First chunk of a streamed response — gentler.
    @MainActor static func receive() { soft.impactOccurred(intensity: 0.4) }
    /// Per-token tick during a streaming reply — very faint, throttled.
    /// Matches the ChatGPT iOS feel: you can sense the bot "typing" without
    /// the phone buzzing constantly. Caller invokes on every reveal chunk;
    /// the throttle keeps it pleasant.
    @MainActor static func streamTick() {
        let now = CACurrentMediaTime()
        guard now - lastStreamTickAt >= 0.18 else { return }
        lastStreamTickAt = now
        soft.impactOccurred(intensity: 0.22)
    }
    /// Tab switch / list selection.
    @MainActor static func tap()     { selection.selectionChanged() }
    /// Voice call answered — single firm thump, the "you're through" cue.
    @MainActor static func connected() { rigid.impactOccurred(intensity: 0.8) }
    /// Successful action (sent / saved / deleted).
    @MainActor static func success() { notification.notificationOccurred(.success) }
    /// Recoverable failure.
    @MainActor static func warning() { notification.notificationOccurred(.warning) }
    /// Hard failure (auth revoked, server unreachable).
    @MainActor static func error()   { notification.notificationOccurred(.error) }
}
#elseif os(macOS)

/// macOS no-op twin of `Haptics`. The native Mac build shares the same
/// cross-platform logic (e.g. `WelcomeViewModel`) which sprinkles haptic
/// cues at success/failure points; Macs have no Taptic Engine for general
/// feedback, so every call is a silent no-op. Keeping the same API surface
/// means shared code calls `Haptics.tap()` etc. without `#if os` fences.
enum Haptics {
    static func warmUp() {}
    @MainActor static func send() {}
    @MainActor static func receive() {}
    @MainActor static func streamTick() {}
    @MainActor static func tap() {}
    @MainActor static func connected() {}
    @MainActor static func success() {}
    @MainActor static func warning() {}
    @MainActor static func error() {}
}
#endif
