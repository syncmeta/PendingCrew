import Foundation
#if os(macOS) && PENDINGCREW_LOCAL_DEMO
import Darwin
#endif

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
        #if os(macOS) && PENDINGCREW_LOCAL_DEMO
        guard let authority = CrewdFrontendConfiguration.demoDirectory else {
            fputs("PendingCrew Demo: isolated seed is missing or unsafe; refusing startup.\n", stderr)
            exit(78)
        }
        let legacy = authority.deletingLastPathComponent().appendingPathComponent("legacy", isDirectory: true)
        var info = stat()
        guard lstat(legacy.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR,
              info.st_uid == getuid(), info.st_mode & 0o077 == 0 else {
            fputs("PendingCrew Demo: isolated legacy root is unsafe; refusing startup.\n", stderr)
            exit(78)
        }
        setenv("PENDINGCREW_LEGACY_DATA_DIR", legacy.path, 1)
        setenv("PENDINGCREW_DATA_DIR", legacy.path, 1)
        setenv("PENDINGCREW_CREWD_DATA_DIR", authority.path, 1)
        setenv("PENDINGCREW_CODEX_PROGRAM", "/usr/bin/false", 1)
        setenv("PENDINGCREW_CLAUDE_PROGRAM", "/usr/bin/false", 1)
        MainActor.assumeIsolated {
            do { try prepareAuthority(directory: authority) }
            catch {
                fputs("PendingCrew Demo: isolated frontend is unsafe; refusing startup.\n", stderr)
                exit(78)
            }
        }
        #else
        let env = ProcessInfo.processInfo.environment
        #if os(macOS)
        let defaultLegacy = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew")
        #else
        // iOS supplies the application sandbox home, never a server/local Mac path.
        let defaultLegacy = URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true).appendingPathComponent("Library/Application Support/PendingCrew")
        #endif
        let old = env["PENDINGCREW_DATA_DIR"] ?? defaultLegacy.path
        setenv("PENDINGCREW_LEGACY_DATA_DIR", old, 1)
        let authority = CrewdFrontendConfiguration.directory ??
            FileManager.default.temporaryDirectory.appendingPathComponent("pcw-invalid-frontend-" + UUID().uuidString)
        // Point away from daily data even if private preparation is refused.
        // No existing directory is chmodded or adopted on failure.
        setenv("PENDINGCREW_DATA_DIR", authority.appendingPathComponent("frontend-presentation/ui-cache").path, 1)
        MainActor.assumeIsolated { try? prepareAuthority(directory: authority) }
        #endif
    }
}
