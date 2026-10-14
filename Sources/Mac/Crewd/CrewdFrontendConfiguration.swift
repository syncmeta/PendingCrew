import Foundation

/// Production storage is stable across source versions. Candidate storage and
/// credentials stay isolated; selecting a directory never imports legacy data.
enum CrewdFrontendConfiguration {
    static var directory: URL? {
        #if PENDINGCREW_DELIVERY_VALIDATION
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
