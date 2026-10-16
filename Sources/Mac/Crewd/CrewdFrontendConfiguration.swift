import Foundation
import Darwin

/// Production storage is stable across source versions. Candidate storage and
/// credentials stay isolated; selecting a directory never imports legacy data.
enum CrewdFrontendConfiguration {
    /// Stable across candidate rebuilds. Explicit validation roots stay outside daily storage.
    private static var candidateDirectory: URL? {
        let env = ProcessInfo.processInfo.environment
        if let path = env["PENDINGCREW_CREWD_DATA_DIR"], path.hasPrefix("/") {
            return candidateDirectory(path: path)
        }
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            .flatMap { candidateDirectory(path: $0.appendingPathComponent("PendingCrewCandidates/SQLite-Mac").path) }
    }
    /// Resolve only an existing ancestor; missing leaves remain subject to the
    /// journal's owner/mode checks. Foundation standardization rewrites Darwin's
    /// /private/tmp to /tmp, which is unsuitable for the journal's exact identity.
    static func candidateDirectory(path: String) -> URL? {
        guard path.hasPrefix("/"), !path.contains("pcw-delivery-validation.f401"),
              let canonical = canonicalCandidatePath(path),
              !canonical.contains("pcw-delivery-validation.f401") else { return nil }
        let daily = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew").path
        guard let canonicalDaily = canonicalCandidatePath(daily),
              canonical != canonicalDaily, !canonical.hasPrefix(canonicalDaily + "/") else { return nil }
        return URL(fileURLWithPath: canonical, isDirectory: true)
    }
    private static func canonicalCandidatePath(_ path: String) -> String? {
        var components = [String]()
        for part in path.split(separator: "/") {
            if part == "." { continue }
            if part == ".." { if !components.isEmpty { components.removeLast() }; continue }
            components.append(String(part))
        }
        var existing = "/" + components.joined(separator: "/"), missing = [String]()
        var info = stat()
        while lstat(existing, &info) != 0 {
            guard errno == ENOENT, existing != "/" else { return nil }
            let url = URL(fileURLWithPath: existing)
            missing.insert(url.lastPathComponent, at: 0)
            existing = url.deletingLastPathComponent().path
        }
        // A selected existing leaf must be an entity directory, never a symlink.
        guard !missing.isEmpty || (info.st_mode & S_IFMT) == S_IFDIR,
              let pointer = realpath(existing, nil) else { return nil }
        defer { free(pointer) }
        let parent = String(cString: pointer)
        guard stat(parent, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR else { return nil }
        return missing.reduce(parent) { $0 == "/" ? "/" + $1 : $0 + "/" + $1 }
    }
    static var directory: URL? {
        #if PENDINGCREW_KNOWLEDGE_VALIDATION
        return candidateDirectory
        #elseif PENDINGCREW_DELIVERY_VALIDATION
        return try? CrewdDeliveryValidation.qualified().data
        #else
        #if PENDINGCREW_NATIVE_CANDIDATE
        guard let ref = Bundle.main.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String,
              ref.count == 40, ref.allSatisfy({ "0123456789abcdef".contains($0) }) else { return nil }
        #endif
        if let explicit = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"],
           !explicit.isEmpty {
            guard explicit.hasPrefix("/") else { return nil }
            return URL(fileURLWithPath: explicit).standardizedFileURL
        }
        #if PENDINGCREW_NATIVE_CANDIDATE
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewNativeCandidates")
            .appendingPathComponent(ref)
        #else
        // A sibling of the legacy PendingCrew root, never an automatic import
        // source. Strict owner/mode/alias validation happens before any store.
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewCore").standardizedFileURL
        #endif
        #endif
    }
}
