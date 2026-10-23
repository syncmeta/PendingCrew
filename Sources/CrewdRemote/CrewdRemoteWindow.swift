import Foundation
import Combine

/// Each window owns its authority selection and all workspace caches/drafts.
@MainActor final class CrewdRemoteWindow: ObservableObject {
    enum ComputerListState: Equatable { case notLoaded, loaded, failed(String) }
    @Published private(set) var computers: [CrewdRemotePairSummary] = []
    @Published private(set) var computerListState: ComputerListState = .notLoaded
    @Published private(set) var workspace: CrewdWorkspace?
    @Published private(set) var selected: UUID?
    @Published private(set) var retainedWorkspaces: [CrewdWorkspace] = []
    @Published private(set) var credentialBusy = false
    @Published var showingConnectionSettings = false
    @Published var error: String?
    /// Read-only review follows the original workspace, never the current writer.
    var inputReviewWorkspaces: [CrewdWorkspace] {
        Self.inputReviewWorkspaces(retained: retainedWorkspaces, current: workspace)
    }
    static func inputReviewWorkspaces(retained: [CrewdWorkspace], current: CrewdWorkspace?) -> [CrewdWorkspace] {
        var scopes = retained
        // A disconnected selection still owns its drafts even though AppModel
        // has removed its write backend. Do not retire/reconnect it to read them.
        if let current, !current.client.readyForWork { scopes.append(current) }
        var seen = Set<ObjectIdentifier>()
        return scopes.filter { seen.insert(ObjectIdentifier($0)).inserted }
    }
    private var refreshTask: Task<Bool, Never>?
    private var changes:AnyCancellable?
    private var workspaceChanges:AnyCancellable?
    private var connectionTask:Task<Void,Never>?
    private var selectionEpoch=UUID()
    private var credentialEpoch=UUID()
    private var selectedReceipt:CrewdRemoteReceipt?
    let pairing: CrewdRemotePairing
    init(pairing: CrewdRemotePairing = CrewdRemotePairing.shared) {
        self.pairing = pairing
        #if !PENDINGCREW_LOCAL_MAC || PENDINGCREW_REMOTE_PRODUCT
        changes=NotificationCenter.default.publisher(for:CrewdRemotePairing.changed).sink { [weak self] _ in
            Task { @MainActor in
                guard let self, self.computerListState == .loaded else { return }
                await self.refresh()
            }
        }
        #endif
    }
    @discardableResult func refresh() async -> Bool {
        if let task = refreshTask { return await task.value }
        guard !credentialBusy else { return false }
        let task = Task { await loadComputers() }
        refreshTask = task
        let result = await task.value
        refreshTask = nil
        return result
    }
    private func loadComputers() async -> Bool {
        guard !credentialBusy else { return false }
        credentialBusy = true; defer { credentialBusy = false }
        let epoch = credentialEpoch
        #if PENDINGCREW_LOCAL_MAC && !PENDINGCREW_REMOTE_PRODUCT
        computers = []; computerListState = .notLoaded
        return false
        #else
        do {
            let id = selected
            let values = try await pairing.summaries()
            let receipt: CrewdRemoteReceipt?
            if let id { receipt = try await pairing.receipt(id) } else { receipt = nil }
            guard epoch == credentialEpoch else { return false }
            computers = values; computerListState = .loaded; error = nil
            if selected != nil, receipt != selectedReceipt { retireSelected() }
            return true
        }
        catch {
            guard epoch == credentialEpoch else { return false }
            self.error = error.localizedDescription
            computerListState = .failed(error.localizedDescription)
            // A failed read cannot authorize a second credential read or keep
            // the selected connection active. Retire it and retain its drafts.
            if selected != nil { retireSelected() }
            return false
        }
        #endif
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
    func close() {credentialEpoch=UUID();retireSelected()}
    #if os(macOS)
    var canForceManagedStop: Bool {
        (retainedWorkspaces + (workspace.map { [$0] } ?? [])).contains { $0.canForceManagedStop }
    }
    func exit(mode: CrewdQuitMode) async throws {
        // Historical fixed-background preferences now notify stop; no path
        // releases management and leaves work running on application Quit.
        let stopMode: CrewdQuitMode = mode == .background ? .safePause : mode
        let scopes = retainedWorkspaces + (workspace.map { [$0] } ?? [])
        var stoppedDirectories = Set<URL>()
        for scope in scopes {
            if let directory = scope.dataDirectory, stoppedDirectories.contains(directory) { continue }
            if try await scope.exitOwnedLocalHelper(mode: stopMode), let directory = scope.dataDirectory {
                stoppedDirectories.insert(directory)
            }
        }
        close()
    }
    func quitOwnedLocalHelpers() async throws {
        let scopes = retainedWorkspaces + (workspace.map { [$0] } ?? [])
        for scope in scopes { try await scope.quitOwnedLocalHelper() }
        close() // Remote scopes only release this client's connection.
    }
    #endif
    func select(_ id: UUID) async {
        let epoch = credentialEpoch
        if let task = refreshTask { _ = await task.value }
        guard epoch == credentialEpoch, !credentialBusy else { return }
        credentialBusy = true
        let receipt:CrewdRemoteReceipt
        do {
            receipt = try await pairing.receipt(id)
            credentialBusy = false
            guard credentialEpoch == epoch else { return }
        }
        catch {credentialBusy = false; if credentialEpoch == epoch { self.error=error.localizedDescription };return}
        if selected==id,selectedReceipt==receipt,let current=workspace {
            await connectSelected(current);return
        }
        if let computer=computers.first(where:{$0.id == id}) {
            var address=URLComponents();address.scheme="tls";address.host=computer.endpoint.host;address.port=Int(computer.endpoint.port)
            if let url=address.url,let hint=try? ApplePortableHost(displayName:computer.name,endpoint:url,transportAuthorityID:receipt.authorityId,databaseID:receipt.dbId,verifiedSPKI:receipt.serverSpkiSha256){await CrewPersonalDraftService.shared.rememberHost(hint)}
        }
        retireCurrent();selected=id;selectedReceipt=receipt
        let next=CrewdWorkspace(authorityRecordID:id,connect:CrewdRemoteTransport.connector(id:id,pairing:pairing))
        install(next);await connectSelected(next)
    }
    #if os(macOS)
    func selectLocal(directory: URL) async {
        credentialEpoch=UUID()
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
    /// Confirmation binds the exact displayed saved computer. A changed record
    /// never silently redirects this local credential deletion to another one.
    @discardableResult func forget(_ computer: CrewdRemotePairSummary) async -> Bool {
        let epoch = credentialEpoch
        if let task = refreshTask { _ = await task.value }
        guard epoch == credentialEpoch else { return false }
        guard !credentialBusy, computers.contains(computer) else { return false }
        credentialBusy = true
        do {
            try await pairing.forget(computer)
            credentialBusy = false
            // Closing or selecting another authority does not undo the completed
            // deletion, and a late result must not retire that new authority.
            guard epoch == credentialEpoch else { return false }
            if selected == computer.id { retireSelected() }
            _ = await refresh(); return true
        } catch {
            credentialBusy = false
            if epoch == credentialEpoch { self.error = error.localizedDescription }
            return false
        }
    }
}
