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
        if let explicit = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"], !explicit.isEmpty {
            #if os(macOS)
            return candidateDirectory(path: explicit)
            #else
            guard explicit.hasPrefix("/") else { return nil }
            return URL(fileURLWithPath: explicit).standardizedFileURL
            #endif
        }
        #if PENDINGCREW_NATIVE_CANDIDATE
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewNativeCandidates").appendingPathComponent(ref)
        #else
        #if os(macOS)
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            .flatMap { candidateDirectory(path: $0.appendingPathComponent("PendingCrewLocal").path) }
        #else
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewCore").standardizedFileURL
        #endif
        #endif
        #endif
    }
}

#if os(macOS)
/// Local file selection only. Never probes a CLI, login state or credentials.
enum CrewdCodexConfiguration {
    static let key = "pendingcrew.crewd.codexProgram.v1"
    static func program(environment: [String: String] = ProcessInfo.processInfo.environment,
                        defaults: UserDefaults = .standard, home: URL = FileManager.default.homeDirectoryForCurrentUser) -> String? {
        if let explicit = environment["PENDINGCREW_CODEX_PROGRAM"], !explicit.isEmpty { return explicit }
        if let selected = defaults.string(forKey: key), !selected.isEmpty { return selected }
        let paths = [home.appendingPathComponent(".local/bin/codex").path, "/opt/homebrew/bin/codex", "/usr/local/bin/codex"] +
            (environment["PATH"] ?? "").split(separator: ":").filter { $0.hasPrefix("/") }.map { String($0) + "/codex" }
        return paths.first { executable($0) }
    }
    static func launchArguments(directory: URL, program: String?) -> [String] {
        var arguments = ["--data-dir", directory.path, "serve", "--claude", "/usr/bin/false"]
        #if PENDINGCREW_LOCAL_MAC
        arguments.append("--managed-parent")
        #endif
        return arguments + (program.flatMap { executable($0) ? $0 : nil }.map { ["--codex", $0] } ?? [])
    }
    static func executable(_ path: String) -> Bool {
        var directory: ObjCBool = false
        return path.hasPrefix("/") && FileManager.default.fileExists(atPath: path, isDirectory: &directory) &&
            !directory.boolValue && FileManager.default.isExecutableFile(atPath: path)
    }
}
#endif
