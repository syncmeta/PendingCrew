#if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
import Foundation
import Inject

extension PendingCrewHotReload {
    /// Reuse the real App tree and existing personal-store DI. All state remains
    /// under the explicitly selected developer root; no new Keychain identity.
    @MainActor static func configureRuntime(environment: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
        guard let resources = environment["PENDINGCREW_HOTRELOAD_INJECTION_RESOURCES"],
              resources.hasPrefix("/"),
              FileManager.default.fileExists(atPath: resources + "/macOSInjection.bundle"),
              let rawID = environment["PENDINGCREW_HOTRELOAD_DEVICE_ID"],
              let deviceID = UUID(uuidString: rawID) else {
            fputs("Hot reload requires the verified InjectionNext resources and a developer device UUID.\n", stderr)
            return false
        }
        InjectConfiguration.bundlePath = resources.hasSuffix("/") ? resources : resources + "/"
        setenv("PENDINGCREW_ISOLATED_CREDENTIALS", "memory", 1)
        _ = CrewPersonalDraftService.owned(
            root: PendingCrewDataRoot.url.appendingPathComponent("personal-input", isDirectory: true),
            deviceIdentity: { deviceID.uuidString }, configuration: nil)
        return true
    }
}
#endif
