import Foundation
import SwiftUI
import Combine

/// Original frontend state with one selected crewd authority. The legacy local
/// backend remains available only to the ordinary, unconverted Mac configuration.
@MainActor
final class AppModel: ObservableObject {
    let usesCrewdAuthority: Bool = {
        #if os(iOS)
        true
        #elseif PENDINGCREW_NATIVE_CANDIDATE
        true
        #else
        ProcessInfo.processInfo.arguments.contains("--crewd-ui")
        #endif
    }()
    @Published private(set) var crewdBackend: CrewdPendingCrewBackend?
    lazy var crewdWindow = CrewdRemoteWindow()
    private var authorityObservation: AnyCancellable?
    @Published private(set) var connectingAuthority = false
    private lazy var localBackend = LocalBackend(store: .shared, whiteboard: .shared)

    var crewdDirectory: URL? {
        #if os(macOS)
        if let explicit = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"], !explicit.isEmpty { return URL(fileURLWithPath: explicit).standardizedFileURL }
        guard let ref = Bundle.main.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String,
              ref.count == 40, ref.allSatisfy({ "0123456789abcdef".contains($0) }) else { return nil }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrewNativeCandidates").appendingPathComponent(ref)
        #else
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewFrontend").standardizedFileURL.resolvingSymlinksInPath()
        #endif
    }
    var backendConfigurationError: String? { crewdWindow.error ?? crewdBackend?.workspace.error ?? "尚未连接已配对的电脑。" }
    init() {
        guard usesCrewdAuthority else { return }
        #if os(macOS)
        AppQuitLifecycle.shared.crewdQuit = { [weak self] in try await self?.crewdWindow.quitOwnedLocalHelpers() }
        #endif
        authorityObservation = crewdWindow.objectWillChange.sink { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self else { return }
                let selected = self.crewdWindow.workspace
                if self.crewdBackend?.workspace !== selected { self.crewdBackend = selected.map(CrewdPendingCrewBackend.init) }
                if let selected, let directory = self.crewdDirectory {
                    selected.originalJournalRequired = true
                    await selected.prepareOriginalJournal(root: directory.appendingPathComponent("frontend-presentation/unknown-submissions"))
                }
                self.objectWillChange.send()
            }
        }
    }
    func connectCrewdAuthority() async {
        guard usesCrewdAuthority, !connectingAuthority else { return }
        connectingAuthority = true; defer { connectingAuthority = false }
        #if os(macOS)
        guard let directory = crewdDirectory else {
            crewdWindow.error = "这个候选版本缺少来源信息，连接未建立。"
            return
        }
        do { try CrewdFrontendIsolation.prepareAuthority(directory: directory) }
        catch { crewdWindow.error = error.localizedDescription; return }
        crewdWindow.error = nil
        await crewdWindow.selectLocal(directory: directory)
        #else
        crewdWindow.refresh()
        if crewdWindow.workspace == nil, let known = crewdWindow.computers.first(where: \.paired) { await crewdWindow.select(known.id) }
        #endif
        if let workspace = crewdWindow.workspace {
            workspace.originalJournalRequired = true
            if let directory = crewdDirectory { await workspace.prepareOriginalJournal(root: directory.appendingPathComponent("frontend-presentation/unknown-submissions")) }
            if crewdBackend?.workspace !== workspace { crewdBackend = CrewdPendingCrewBackend(workspace: workspace) }
        }
    }
    var backend: PendingCrewBackend? {
        if usesCrewdAuthority { return crewdBackend }
        #if os(macOS)
        return localBackend
        #else
        return nil
        #endif
    }
}
