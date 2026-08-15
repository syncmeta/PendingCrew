import Foundation

/// Bridges a local `CrewSessionRun` to its server-side `crew_sessions` row so
/// cross-device viewers can see a session exists + when it ends. Logged mode
/// only; BYOK runs pass `nil` and stay purely local.
///
/// **Phase 1 scope**: just the row lifecycle close (`finish`). The structured
/// per-event transcript mirror + the realtime SessionProxy socket were ripped
/// out with the one-shot runner — Phase 2 rebuilds live cross-device control as
/// a raw byte tunnel over the embedded terminal's PTY, not as parsed events.
///
/// **Best-effort**: the server call is `try?`'d — a failure (network blip,
/// scope gap, evicted lease) is logged but never disrupts the local agent
/// process. The local run is always the source of truth for the user.
///
/// Lives in `Sources/Remote/` (three-platform) rather than next to the PTY
/// runner: it only touches `PendingCrewAPI`. The macOS-only convenience that
/// maps a `CrewSessionRun.Status` onto `finish(status:)` stays behind the
/// fence in `Mac/Services/CrewSessionRunner.swift`, because that enum belongs
/// to the PTY run.
struct CrewSessionServerLink: Sendable {
    let api: PendingCrewAPI
    let runnerHostId: String
    let sessionId: String

    /// Close out the server session row. `status` is the server's
    /// completed/failed/cancelled enum.
    func finish(status: String, summary: String?) async {
        do {
            try await api.finishSession(
                runnerHostId: runnerHostId,
                sessionId: sessionId,
                status: status,
                summary: summary
            )
        } catch {
            print("[crew-session] finish failed: \(error)")
        }
    }
}
