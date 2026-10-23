#if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
import Foundation
import Darwin

/// The optional developer build uses the original App/Root/View tree. Preparing
/// or compiling it does not qualify a private runtime or authorize Core startup.
enum PendingCrewHotReload {
    private static func canonicalDirectory(_ path: String) -> Bool {
        guard let resolved = realpath(path, nil) else { return false }
        defer { free(resolved) }
        var metadata = stat()
        return String(cString: resolved) == path && stat(path, &metadata) == 0 &&
            (metadata.st_mode & S_IFMT) == S_IFDIR
    }
    static func allowEntry(environment: [String: String] = ProcessInfo.processInfo.environment,
                           bundleID: String? = Bundle.main.bundleIdentifier) -> Bool {
        guard bundleID == "com.pendingname.pendingcrew.hotreload",
              environment["PENDINGCREW_HOTRELOAD_PRIVATE_RUNTIME_REVIEWED"] == "1" else {
            fputs("Hot reload source is prepared; private App/Core runtime has not been enabled. See docs/development/swiftui-hot-reload.md.\n", stderr)
            return false
        }
        // Explicit private roots are necessary, but are not a network/Keychain
        // sandbox. The runtime review must cover those side effects separately.
        for key in ["PENDINGCREW_DATA_DIR", "PENDINGCREW_CREWD_DATA_DIR"] {
            guard let raw = environment[key], raw.hasPrefix("/private/tmp/"),
                  canonicalDirectory(raw) else {
                fputs("Hot reload requires explicit canonical private temporary roots.\n", stderr)
                return false
            }
        }
        return true
    }
}
#endif
