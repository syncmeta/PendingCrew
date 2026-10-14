import Foundation
import Combine

/// Each window owns its authority selection and all workspace caches/drafts.
@MainActor final class CrewdRemoteWindow: ObservableObject {
    @Published private(set) var computers: [CrewdRemotePairSummary] = []
    @Published private(set) var workspace: CrewdWorkspace?
    @Published private(set) var selected: UUID?
    @Published private(set) var retainedWorkspaces: [CrewdWorkspace] = []
    @Published var error: String?
    private var changes:AnyCancellable?
    private var workspaceChanges:AnyCancellable?
    private var connectionTask:Task<Void,Never>?
    private var selectionEpoch=UUID()
    private var selectedReceipt:CrewdRemoteReceipt?
    let pairing: CrewdRemotePairing
    init(pairing: CrewdRemotePairing = CrewdRemotePairing(credentials: CrewdRemoteCredentials.application())) {
        self.pairing = pairing
        changes=NotificationCenter.default.publisher(for:CrewdRemotePairing.changed).sink { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
    }
    func refresh() {
        do {
            computers = try pairing.summaries(); error = nil
            if let id=selected, (try? pairing.paired(id).2) != selectedReceipt { retireSelected() }
        }
        catch {
            self.error = error.localizedDescription
            if let id=selected, (try? pairing.paired(id).2) != selectedReceipt {retireSelected()}
        }
    }
    private func retireCurrent() {
        selectionEpoch=UUID();connectionTask?.cancel();connectionTask=nil
        workspaceChanges?.cancel();workspaceChanges=nil
        if let old=workspace {old.retire();retainedWorkspaces.append(old)}
    }
    private func install(_ next:CrewdWorkspace) {
        workspace=next
        // The toolbar belongs to this window, while connection/state events
        // originate from the selected workspace and its client.
        workspaceChanges=Publishers.Merge(next.objectWillChange,next.client.objectWillChange).sink { [weak self,weak next] _ in
            guard let self,let next,self.workspace === next else{return}
            self.objectWillChange.send()
        }
    }
    private func connectSelected(_ next:CrewdWorkspace) async {
        guard workspace === next else{return}
        if let task=connectionTask {await task.value;return}
        if next.client.connected {return}
        let epoch=UUID();selectionEpoch=epoch
        let task=Task {await next.connect()};connectionTask=task
        await task.value
        if selectionEpoch==epoch {connectionTask=nil}
    }
    private func retireSelected() {
        retireCurrent();workspace=nil;selected=nil;selectedReceipt=nil
    }
    func close() {retireSelected()}
    #if os(macOS)
    func quitOwnedLocalHelpers() async throws {
        let scopes = retainedWorkspaces + (workspace.map { [$0] } ?? [])
        for scope in scopes { try await scope.quitOwnedLocalHelper() }
        close() // Remote scopes only release this client's connection.
    }
    #endif
    func select(_ id: UUID) async {
        let receipt:CrewdRemoteReceipt
        do {receipt=try pairing.paired(id).2}
        catch {self.error=error.localizedDescription;return}
        if selected==id,selectedReceipt==receipt,let current=workspace {
            await connectSelected(current);return
        }
        retireCurrent();selected=id;selectedReceipt=receipt
        let next=CrewdWorkspace(authorityRecordID:id,connect:CrewdRemoteTransport.connector(id:id,pairing:pairing))
        install(next);await connectSelected(next)
    }
    #if os(macOS)
    func selectLocal(directory: URL) async {
        if selected==nil,let current=workspace,current.dataDirectory==directory {
            await connectSelected(current);return
        }
        retireCurrent();selected=nil;selectedReceipt=nil
        let namespace: UUID
        do { namespace = try CrewdPresentationJournal.localNamespace(directory: directory) }
        catch { self.error = error.localizedDescription; workspace = nil; return }
        let next=CrewdWorkspace(dataDirectory:directory, authorityRecordID: namespace)
        install(next);await connectSelected(next)
    }
    #endif
    func forget(_ id: UUID) {
        do {
            try pairing.forget(id)
            if selected == id {
                retireSelected()
            }
            refresh()
        } catch { self.error = error.localizedDescription }
    }
}
