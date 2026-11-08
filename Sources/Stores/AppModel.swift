import Foundation
import SwiftUI
import Combine

/// The selected backend always belongs to the committed authority, including
/// while another explicitly chosen computer is still being qualified.
@MainActor final class AppModel: ObservableObject {
    let usesCrewdAuthority = true
    @Published private(set) var crewdBackend: CrewdPendingCrewBackend?
    let crewdWindow: CrewdRemoteWindow
    private var authorityObservation: AnyCancellable?
    private var backendRefresh: Task<Void, Never>?
    private var backendRefreshAgain = false
    private var authorityInitializationAttempted = false
    private let bindDrafts: @MainActor (CrewdWorkspace?) -> Void
    private let directoryProvider: @MainActor () -> URL?
    var connectingAuthority: Bool { crewdWindow.isSelecting }
    var computerSelectionError: String? {
        get { crewdWindow.selectionFailure }
        set { if newValue == nil { crewdWindow.dismissSelectionFailure() } }
    }
    var crewdDirectory: URL? { directoryProvider() }
    var backendConfigurationError: String? { crewdWindow.error ?? crewdBackend?.workspace.error ?? "尚未连接已配对的电脑。" }
    init(window: CrewdRemoteWindow? = nil, directory: @escaping @MainActor () -> URL? = { CrewdFrontendConfiguration.directory }, bindDrafts: @escaping @MainActor (CrewdWorkspace?) -> Void = { CrewPersonalDraftService.shared.bindCore($0) }) {
        crewdWindow = window ?? CrewdRemoteWindow(); self.bindDrafts = bindDrafts; self.directoryProvider = directory
        #if os(macOS)
        AppQuitLifecycle.shared.crewdExit = { [weak self] mode in try await self?.crewdWindow.exit(mode: mode) }
        AppQuitLifecycle.shared.crewdMayForceStop = { [weak self] in self?.crewdWindow.canForceManagedStop == true }
        #endif
        authorityObservation = crewdWindow.objectWillChange.sink { [weak self] _ in
            self?.objectWillChange.send()
            self?.scheduleBackendRefresh()
        }
    }
    private func scheduleBackendRefresh() {
        backendRefreshAgain = true
        guard backendRefresh == nil else { return }
        backendRefresh = Task { [weak self] in
            guard let self else { return }
            repeat {
                self.backendRefreshAgain = false
                await self.refreshCommittedBackend()
            } while self.backendRefreshAgain && !Task.isCancelled
            self.backendRefresh = nil
        }
    }
    private func refreshCommittedBackend() async {
        let selected = crewdWindow.workspace, authority = crewdWindow.authorityEpoch, selectionAttempt = crewdWindow.attempt?.id
        guard let selected else {
            bindDrafts(nil)
            if crewdBackend != nil { crewdBackend = nil }; return
        }
        selected.originalJournalRequired = true
        bindDrafts(selected)
        guard selected.client.readyForWork else { if crewdBackend != nil { crewdBackend = nil }; return }
        let epoch = selected.observationGeneration, connection = selected.client.connectionEpoch
        if !selected.originalJournalReady {
            guard let directory = crewdDirectory else {
                crewdBackend = nil; let message = "当前后台数据目录配置不可用，连接未建立。"; if crewdWindow.attempt?.id == selectionAttempt, crewdWindow.error != message { crewdWindow.error = message }; return
            }
            do {
                let observation = CrewdBoundedConnectionObservation<Bool>()
                _ = try await observation.value(timeout: CrewdConnectionDeadlines().journal) {
                    await selected.prepareOriginalJournal(root: directory.appendingPathComponent("frontend-presentation/unknown-submissions"))
                    return selected.originalJournalReady
                }
            } catch {
                guard crewdWindow.authorityEpoch == authority, crewdWindow.workspace === selected,
                      selected.acceptsObservation(epoch), selected.client.connectionEpoch == connection else { return }
                crewdBackend = nil; if crewdWindow.attempt?.id == selectionAttempt, crewdWindow.error != error.localizedDescription { crewdWindow.error = error.localizedDescription }; return
            }
        }
        guard crewdWindow.authorityEpoch == authority, crewdWindow.workspace === selected,
              selected.acceptsObservation(epoch), selected.client.connectionEpoch == connection else { return }
        guard selected.originalJournalReady else {
            crewdBackend = nil; if let message = selected.error, crewdWindow.attempt?.id == selectionAttempt, crewdWindow.error != message { crewdWindow.error = message }; return
        }
        if crewdBackend?.workspace !== selected { crewdBackend = CrewdPendingCrewBackend(workspace: selected) }
    }
    /// Remounting never chooses a second automatic default.
    func initializeCrewdAuthority() async {
        guard !authorityInitializationAttempted else { return }
        authorityInitializationAttempted = true
        guard crewdWindow.workspace == nil else { return }
        await connectCrewdAuthority()
    }
    func cancelCrewdAuthoritySelection() { crewdWindow.cancelSelection() }
    func connectCrewdAuthority(computerID: UUID? = nil) async {
        guard usesCrewdAuthority else { return }
        authorityInitializationAttempted = true
        // An explicit Local choice can always end a remote observation. No old
        // defer or late remote result owns this new choice's busy/error state.
        crewdWindow.cancelSelection()
        guard let directory = crewdDirectory else {
            crewdWindow.error = "当前后台数据目录配置不可用，连接未建立。"; return
        }
        do { try CrewdFrontendIsolation.prepareAuthority(directory: directory) }
        catch { crewdWindow.error = error.localizedDescription; return }
        let root = directory.appendingPathComponent("frontend-presentation/unknown-submissions")
        if let computerID { await crewdWindow.select(computerID, journalRoot: root) }
        else {
            #if os(macOS)
            await crewdWindow.selectLocal(directory: directory, journalRoot: root)
            #else
            _ = await crewdWindow.refresh()
            #endif
        }
        // Read the committed authority afresh; an older caller cannot publish
        // its abandoned target or clear the newer attempt's progress.
        scheduleBackendRefresh()
    }
    var backend: PendingCrewBackend? { crewdBackend }
}
