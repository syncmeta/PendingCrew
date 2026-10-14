import Foundation

/// Old frontend preferences/caches are isolated from both the daily legacy data
/// and the authoritative crewd database. They never become a second work ledger.
enum CrewdFrontendIsolation {
    @MainActor private(set) static var preparationError: String?

    @MainActor static func prepareAuthority(directory: URL) throws {
        do {
            let cache = try CrewdPresentationJournal.prepareUICache(directory: directory)
            setenv("PENDINGCREW_DATA_DIR", cache.path, 1)
            preparationError = nil
        } catch {
            preparationError = error.localizedDescription
            throw error
        }
    }
    static func prepare() {
        let env = ProcessInfo.processInfo.environment
        let old = env["PENDINGCREW_DATA_DIR"] ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew").path
        setenv("PENDINGCREW_LEGACY_DATA_DIR", old, 1)
        let authority = CrewdFrontendConfiguration.directory ??
            FileManager.default.temporaryDirectory.appendingPathComponent("pcw-invalid-frontend-" + UUID().uuidString)
        // Point away from daily data even if private preparation is refused.
        // No existing directory is chmodded or adopted on failure.
        setenv("PENDINGCREW_DATA_DIR", authority.appendingPathComponent("frontend-presentation/ui-cache").path, 1)
        MainActor.assumeIsolated { try? prepareAuthority(directory: authority) }
    }
}
