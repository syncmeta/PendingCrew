// VENDORED from PendingBot apps/pendingbot/Sources/Components/BlinkingCursor.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
import SwiftUI

/// Streaming-output caret modeled on a terminal cursor / ChatGPT's tail
/// indicator. Three observable states drive its appearance:
///
///   • streaming + tokens flowing → solid block
///   • streaming + paused (no token in a beat) → blinks like a TTY caret
///   • stream finished → caller removes the view; pair with `.transition(.opacity)`
///     so SwiftUI fades it out instead of yanking it.
///
/// The blink uses TimelineView so it doesn't churn other state; the host
/// view only re-publishes when `paused` flips.
struct BlinkingCursor: View {
    /// True when the producer hasn't emitted a token in a short window —
    /// switches from solid to TTY-style blink so the user knows we're
    /// alive but waiting on the upstream.
    var paused: Bool

    /// 530 ms full cycle ≈ standard terminal blink rate (Cocoa text caret
    /// is similar). Slow enough to read as "waiting", fast enough not to
    /// feel sluggish.
    private static let blinkPeriod: TimeInterval = 1.06

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.05)) { ctx in
            let visible: Bool = {
                if !paused { return true }
                let t = ctx.date.timeIntervalSinceReferenceDate
                let phase = t.truncatingRemainder(dividingBy: Self.blinkPeriod)
                return phase < (Self.blinkPeriod / 2)
            }()
            RoundedRectangle(cornerRadius: 1.5, style: .continuous)
                .fill(Theme.Palette.ink)
                .frame(width: 8, height: 16)
                .opacity(visible ? 0.85 : 0)
                // Soften the on/off edges so the blink reads as a "pulse"
                // rather than a hard square wave.
                .animation(.easeOut(duration: 0.14), value: visible)
        }
        .frame(width: 8, height: 16)
        .accessibilityHidden(true)
    }
}
