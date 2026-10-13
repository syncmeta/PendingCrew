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
    private var selectedReceipt:CrewdRemoteReceipt?
    let pairing: CrewdRemotePairing
    init(pairing: CrewdRemotePairing = CrewdRemotePairing(credentials: CrewdRemoteKeychain())) {
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
    private func retireSelected() {
        if let old=workspace {old.retire();retainedWorkspaces.append(old)}
        workspace=nil;selected=nil;selectedReceipt=nil
    }
    func select(_ id: UUID) async {
        guard selected != id else {await workspace?.connect();return}
        do { selectedReceipt = try pairing.paired(id).2 }
        catch { self.error = error.localizedDescription; return }
        if let old = workspace { old.retire(); retainedWorkspaces.append(old) }
        selected = id
        let next = CrewdWorkspace(authorityRecordID: id, connect: CrewdRemoteTransport.connector(id: id, pairing: pairing))
        workspace = next
        await next.connect()
    }
    #if os(macOS)
    func selectLocal(directory: URL) async {
        guard selected != nil || workspace?.dataDirectory != directory else { return }
        if let old = workspace { old.retire(); retainedWorkspaces.append(old) }
        selected = nil; selectedReceipt = nil
        let next = CrewdWorkspace(dataDirectory: directory)
        workspace = next
        await next.connect()
    }
    #endif
    func forget(_ id: UUID) {
        do {
            try pairing.forget(id)
            if selected == id {
                if let old = workspace { old.retire(); retainedWorkspaces.append(old) }
                workspace = nil; selected = nil; selectedReceipt = nil
            }
            refresh()
        } catch { self.error = error.localizedDescription }
    }
}
