import Foundation

/// Production storage is stable across source versions. Candidate storage and
/// credentials stay isolated; selecting a directory never imports legacy data.
enum CrewdFrontendConfiguration {
    /// Stable across candidate rebuilds. Explicit validation roots stay outside daily storage.
    private static var candidateDirectory: URL? {
        let env = ProcessInfo.processInfo.environment
        if let path = env["PENDINGCREW_CREWD_DATA_DIR"], path.hasPrefix("/") {
            let url = URL(fileURLWithPath: path).standardizedFileURL
            let daily = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew").path
            guard url.path != daily, !url.path.hasPrefix(daily + "/"), !url.path.contains("pcw-delivery-validation.f401") else { return nil }
            return url
        }
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewCandidates/SQLite-Mac")
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
