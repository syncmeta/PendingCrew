#if os(macOS)
import Foundation

/// Old frontend preferences/caches are isolated from both the daily legacy data
/// and the authoritative crewd database. They never become a second work ledger.
enum CrewdFrontendIsolation {
    static func prepare() {
        let env = ProcessInfo.processInfo.environment
        let old = env["PENDINGCREW_DATA_DIR"] ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew").path
        setenv("PENDINGCREW_LEGACY_DATA_DIR", old, 1)
        let authority: URL
        if let explicit = env["PENDINGCREW_CREWD_DATA_DIR"], !explicit.isEmpty { authority = URL(fileURLWithPath: explicit) }
        else if let ref = Bundle.main.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String,
                ref.count == 40, ref.allSatisfy({ "0123456789abcdef".contains($0) }) {
            authority = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrewNativeCandidates").appendingPathComponent(ref)
        } else {
            // Invalid candidate metadata remains disconnected, including its caches.
            authority = FileManager.default.temporaryDirectory.appendingPathComponent("pcw-invalid-frontend-" + UUID().uuidString)
        }
        setenv("PENDINGCREW_DATA_DIR", authority.appendingPathComponent("frontend-presentation").path, 1)
    }
}
#endif
