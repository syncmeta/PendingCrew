import Foundation
import SwiftUI
import Combine

/// Original frontend state with one selected crewd authority. Ordinary startup
/// uses the same Core seam; failure cannot fall back to a legacy writer.
@MainActor
final class AppModel: ObservableObject {
    let usesCrewdAuthority = true
    @Published private(set) var crewdBackend: CrewdPendingCrewBackend?
    lazy var crewdWindow = CrewdRemoteWindow()
    private var authorityObservation: AnyCancellable?
    @Published private(set) var connectingAuthority = false

    var crewdDirectory: URL? { CrewdFrontendConfiguration.directory }
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
            crewdWindow.error = "当前后台数据目录配置不可用，连接未建立。"
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
    var backend: PendingCrewBackend? { crewdBackend }
}
