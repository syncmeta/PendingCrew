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
    private var authorityInitializationAttempted = false
    /// Only explicit target selection reports here; startup remains unchanged.
    @Published var computerSelectionError:String?

    var crewdDirectory: URL? { CrewdFrontendConfiguration.directory }
    var backendConfigurationError: String? { crewdWindow.error ?? crewdBackend?.workspace.error ?? "尚未连接已配对的电脑。" }
    init() {
        guard usesCrewdAuthority else { return }
        #if os(macOS)
        AppQuitLifecycle.shared.crewdExit = { [weak self] mode in try await self?.crewdWindow.exit(mode: mode) }
        AppQuitLifecycle.shared.crewdMayForceStop = { [weak self] in self?.crewdWindow.canForceManagedStop == true }
        #endif
        authorityObservation = crewdWindow.objectWillChange.sink { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self else { return }
                let selected = self.crewdWindow.workspace
                CrewPersonalDraftService.shared.bindCore(selected)
                if let selected {
                    selected.originalJournalRequired = true
                    guard selected.client.readyForWork else {
                        if self.crewdBackend != nil { self.crewdBackend = nil }
                        return
                    }
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
                        if let message = selected.error, self.crewdWindow.error != message { self.crewdWindow.error = message }
                        self.objectWillChange.send()
                        return
                    }
                }
                guard self.crewdWindow.workspace === selected else { return }
                CrewPersonalDraftService.shared.bindCore(selected)
                if self.crewdBackend?.workspace !== selected { self.crewdBackend = selected.map(CrewdPendingCrewBackend.init) }
                self.objectWillChange.send()
            }
        }
    }
    /// View tasks can run again after remounting. Only the first automatic
    /// initialization may choose a default; subsequent selection is explicit.
    func initializeCrewdAuthority() async {
        guard !authorityInitializationAttempted else { return }
        authorityInitializationAttempted = true
        guard crewdWindow.workspace == nil else { return }
        await connectCrewdAuthority()
    }
    func connectCrewdAuthority(computerID:UUID?=nil) async {
        guard usesCrewdAuthority else{return}
        // An explicit choice made before the view task also owns the selection,
        // including a failed attempt whose error must remain visible.
        authorityInitializationAttempted = true
        guard !connectingAuthority else {
            if computerID != nil{computerSelectionError="另一项电脑连接正在进行，请稍后再明确选择。"}
            return
        }
        if computerID != nil{computerSelectionError=nil}
        connectingAuthority = true
        defer {
            connectingAuthority = false
            if let computerID,crewdWindow.selected != computerID || crewdBackend?.workspace.authorityRecordID != computerID || crewdBackend?.workspace.client.readyForWork != true {
                computerSelectionError=crewdWindow.error ?? crewdWindow.workspace?.error ?? "这台电脑的身份或连接尚未确认；原配对和输入已保留，不会切换到替代电脑。"
            }
        }
        guard let directory = crewdDirectory else {
            crewdWindow.error = "当前后台数据目录配置不可用，连接未建立。"
            return
        }
        do { try CrewdFrontendIsolation.prepareAuthority(directory: directory) }
        catch { crewdWindow.error = error.localizedDescription; return }
        crewdWindow.error = nil
        if let computerID {
            guard await crewdWindow.refresh() else{return}
            guard crewdWindow.computers.contains(where:{$0.id==computerID && $0.paired}) else {
                crewdWindow.error="原电脑记录已失效或尚未完成配对，请核对原请求；不会连接另一条记录。"
                return
            }
            await crewdWindow.select(computerID)
            guard crewdWindow.selected==computerID,crewdWindow.workspace?.authorityRecordID==computerID else {
                if crewdWindow.error==nil{crewdWindow.error="未取得这台电脑的原凭据，连接未切换。"}
                return
            }
        }else {
            #if os(macOS)
            await crewdWindow.selectLocal(directory: directory)
            #else
            // Observation on startup, never an automatic remote selection.
            await crewdWindow.refresh()
            #endif
        }
        if let workspace = crewdWindow.workspace {
            workspace.originalJournalRequired = true
            guard workspace.client.readyForWork else { crewdBackend = nil; return }
            let epoch = workspace.observationGeneration, connection = workspace.client.connectionEpoch
            await workspace.prepareOriginalJournal(root: directory.appendingPathComponent("frontend-presentation/unknown-submissions"))
            guard crewdWindow.workspace === workspace, workspace.acceptsObservation(epoch), workspace.client.connectionEpoch == connection else { return }
            guard workspace.originalJournalReady else {
                crewdBackend = nil
                if let message = workspace.error { crewdWindow.error = message }
                return
            }
            CrewPersonalDraftService.shared.bindCore(workspace)
            if crewdBackend?.workspace !== workspace { crewdBackend = CrewdPendingCrewBackend(workspace: workspace) }
        }
    }
    var backend: PendingCrewBackend? { crewdBackend }
}
