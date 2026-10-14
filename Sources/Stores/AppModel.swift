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
                if let selected {
                    selected.originalJournalRequired = true
                    guard let directory = self.crewdDirectory else {
                        self.crewdBackend = nil
                        let message = "当前后台数据目录配置不可用，连接未建立。"
                        if self.crewdWindow.error != message { self.crewdWindow.error = message }
                        self.objectWillChange.send()
                        return
                    }
                    let epoch = selected.observationGeneration, connection = selected.client.connectionEpoch
                    await selected.prepareOriginalJournal(root: directory.appendingPathComponent("frontend-presentation/unknown-submissions"))
                    guard self.crewdWindow.workspace === selected, selected.acceptsObservation(epoch), selected.client.connectionEpoch == connection else { return }
                    guard selected.originalJournalReady else {
                        self.crewdBackend = nil
                        let message = selected.error ?? "原提交历史尚未就绪；连接仅供只读核对。"
                        if self.crewdWindow.error != message { self.crewdWindow.error = message }
                        self.objectWillChange.send()
                        return
                    }
                }
                guard self.crewdWindow.workspace === selected else { return }
                if self.crewdBackend?.workspace !== selected { self.crewdBackend = selected.map(CrewdPendingCrewBackend.init) }
                self.objectWillChange.send()
            }
        }
    }
    func connectCrewdAuthority() async {
        guard usesCrewdAuthority, !connectingAuthority else { return }
        connectingAuthority = true; defer { connectingAuthority = false }
        guard let directory = crewdDirectory else {
            crewdWindow.error = "当前后台数据目录配置不可用，连接未建立。"
            return
        }
        do { try CrewdFrontendIsolation.prepareAuthority(directory: directory) }
        catch { crewdWindow.error = error.localizedDescription; return }
        crewdWindow.error = nil
        #if os(macOS)
        await crewdWindow.selectLocal(directory: directory)
        #else
        crewdWindow.refresh()
        if crewdWindow.workspace == nil, let known = crewdWindow.computers.first(where: \.paired) { await crewdWindow.select(known.id) }
        #endif
        if let workspace = crewdWindow.workspace {
            workspace.originalJournalRequired = true
            let epoch = workspace.observationGeneration, connection = workspace.client.connectionEpoch
            await workspace.prepareOriginalJournal(root: directory.appendingPathComponent("frontend-presentation/unknown-submissions"))
            guard crewdWindow.workspace === workspace, workspace.acceptsObservation(epoch), workspace.client.connectionEpoch == connection else { return }
            guard workspace.originalJournalReady else {
                crewdBackend = nil
                crewdWindow.error = workspace.error ?? "原提交历史尚未就绪；连接仅供只读核对。"
                return
            }
            if crewdBackend?.workspace !== workspace { crewdBackend = CrewdPendingCrewBackend(workspace: workspace) }
        }
    }
    var backend: PendingCrewBackend? { crewdBackend }
}
