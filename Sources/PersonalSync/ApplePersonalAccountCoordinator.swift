import Foundation
import CloudKit

protocol AppleAccountTransport: AppleBranchTransport {
    func accountIdentity() async throws -> AppleSyncAccount
}
extension AppleCloudKitTransport: AppleAccountTransport {}

/// This is supplied only by a qualified product overlay, never guessed at startup.
struct ApplePersonalCloudConfiguration: Equatable, Sendable {
    let containerIdentifier: String
    let environment: AppleSyncEnvironment
    let draftsCollection: String
    let hostsCollection: String
    /// Only a separately qualified signed overlay may supply the actual KVS entitlement.
    let keyValueStoreIdentifier:String?
    init(containerIdentifier:String,environment:AppleSyncEnvironment,draftsCollection:String,hostsCollection:String,keyValueStoreIdentifier:String?=nil){
        self.containerIdentifier=containerIdentifier;self.environment=environment;self.draftsCollection=draftsCollection;self.hostsCollection=hostsCollection;self.keyValueStoreIdentifier=keyValueStoreIdentifier
    }
    func accepts(_ account: AppleSyncAccount) -> Bool {
        account.container == containerIdentifier && account.environment == environment && !account.userRecordID.isEmpty
    }
}
struct ApplePersonalNamespaceID: Hashable, Sendable {
    let account: AppleSyncAccount?
    let draftsCollection: String
    let hostsCollection: String
    static let local = Self(account: nil, draftsCollection: "drafts", hostsCollection: "hosts")
    var stableName: String {
        guard let account else { return "local" }
        // A structured tuple separates both account and qualification collections.
        return appleSyncDigest((try? appleSyncEncode([account.container, account.environment.rawValue, account.userRecordID, draftsCollection, hostsCollection])) ?? Data())
    }
    func directory(in originalRoot: URL) -> URL {
        account == nil ? originalRoot : originalRoot.appendingPathComponent("account-" + stableName, isDirectory: true)
    }
    var latestNamespace: String { account == nil ? "personal" : "personal/" + stableName }
}

/// Wraps the existing synchronous CrewComposerDraftStore. It owns no persistence.
@MainActor struct ApplePersonalLatestAccess<Payload: Codable & Equatable & Sendable> {
    let readAll: () -> [AppleDraftKey: Payload]
    let initializationError: String?
    /// nil means the existing writer durably saved this exact original.
    let save: (AppleDraftKey, Payload) -> String?
}

/// One whole UI/storage namespace. Retain old bundles, including failed originals,
/// until process exit; the app's save/export exit guard must inspect every bundle.
@MainActor final class ApplePersonalAccountNamespace<Payload: Codable & Equatable & Sendable> {
    typealias DraftStore = AppleBranchStore<Payload>
    typealias HostStore = AppleBranchStore<ApplePortableHost>
    struct Stores: Sendable { let namespace: ApplePersonalNamespaceID; let deviceID: String; let drafts: DraftStore; let hosts: HostStore }
    struct Ticket {
        let namespace: ApplePersonalNamespaceID
        let selection: UUID
        let key: AppleDraftKey
        let revision: UUID
        let payload: Payload
    }
    struct HostTicket { let namespace: ApplePersonalNamespaceID; let host: ApplePortableHost }
    let id: ApplePersonalNamespaceID
    let latest: ApplePersonalLatestAccess<Payload>
    private let factory: () async throws -> Stores
    private var storesTask: Task<Stores, Error>?
    private var selected: () -> Bool = { false }
    private var selection = UUID()
    private(set) var latestMemory: [AppleDraftKey: Payload] = [:]
    private(set) var revisions: [AppleDraftKey: UUID] = [:]
    private(set) var catalog: Set<AppleDraftKey> = []
    private(set) var versions: [AppleDraftKey: [DraftStore.Value]] = [:]
    private(set) var recovered: [UUID: Payload] = [:]
    private(set) var hosts: [ApplePortableHost] = []
    private(set) var errors: [AppleDraftKey: String] = [:]
    private(set) var unsavedLatest: Set<AppleDraftKey> = []
    var didChange: () -> Void = {}
    var didCheckpoint: () -> Void = {}
    var savingKeys:Set<AppleDraftKey> { Set(writers.keys) }
    private var pendingCheckpoints: Set<AppleDraftKey> = []
    private var writers: [AppleDraftKey: Task<Void, Never>] = [:]
    private struct SyncOperation { let id: UUID; let selection: UUID; let task: Task<Void, Error> }
    private var inflight: SyncOperation?
    init(id: ApplePersonalNamespaceID, latest: ApplePersonalLatestAccess<Payload>, stores: @escaping () async throws -> Stores) {
        self.id = id; self.latest = latest; factory = stores
        latestMemory = latest.readAll(); catalog = Set(latestMemory.keys)
    }
    fileprivate func activate(selected: @escaping () -> Bool) { self.selected = selected; selection = UUID() }
    fileprivate func deactivate() {
        selected = { false }; selection = UUID()
        writers.values.forEach { $0.cancel() }; writers.removeAll()
        inflight?.task.cancel(); inflight = nil
        // Latest/memory/revisions/catalog/errors/unsaved inputs remain in THIS bundle.
    }
    var isSelected: Bool { selected() }
    var initializationError: String? { latest.initializationError }
    private func stores() async throws -> Stores {
        if let storesTask { return try await storesTask.value }
        let expected = id, factory = self.factory
        let task = Task {
            let result = try await factory()
            guard result.namespace == expected else { throw AppleSyncFailure.invalid }
            return result
        }
        storesTask = task
        do { return try await task.value } catch { storesTask = nil; throw error }
    }
    func localDraft(_ key: AppleDraftKey) -> Payload? { latestMemory[key] }
    /// Synchronous source save before any async work. No cloud operation here.
    @discardableResult func checkpoint(_ payload: Payload, key: AppleDraftKey) throws -> Ticket {
        guard selected() else { throw AppleSyncFailure.accountChanged }
        latestMemory[key] = payload; revisions[key] = UUID(); catalog.insert(key); pendingCheckpoints.insert(key)
        if let error = latest.save(key, payload) { unsavedLatest.insert(key); errors[key] = error }
        else { unsavedLatest.remove(key); errors[key] = nil }
        let ticket = ticket(key: key)!
        didChange()
        let previous = writers[key]; previous?.cancel()
        writers[key] = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250)); await previous?.value
            guard let self, !Task.isCancelled, self.isCurrent(ticket) else { return }
            do {
                let stores = try await self.stores()
                guard self.isCurrent(ticket), !Task.isCancelled else { return }
                _ = try await stores.drafts.checkpoint(key: key, payload: payload)
                guard self.isCurrent(ticket) else { return }
                self.unsavedLatest.remove(key); self.pendingCheckpoints.remove(key)
                self.errors[key] = nil
            } catch { if self.isCurrent(ticket) { self.errors[key] = "本机版本保存失败；原文仍保留：\(error)" } }
            if self.isCurrent(ticket) { self.writers[key] = nil; self.didChange(); self.didCheckpoint() }
        }
        return ticket
    }
    /// Preserve input arriving between the synchronous account fence and its
    /// MainActor projection. It stays in the old local file, never uploads.
    func retainUnselected(_ payload:Payload,key:AppleDraftKey) {
        latestMemory[key]=payload;revisions[key]=UUID();catalog.insert(key)
        if let error=latest.save(key,payload){unsavedLatest.insert(key);errors[key]=error}
        else{unsavedLatest.remove(key)}
        didChange()
    }
    func snapshot(key:AppleDraftKey,payload:Payload,empty:Payload)->Ticket? {
        guard selected(),(latestMemory[key] ?? empty)==payload else{return nil}
        if latestMemory[key] == nil { latestMemory[key]=payload }
        return ticket(key:key)
    }
    @discardableResult func clearIfUnchanged(_ expected:Ticket,empty:Payload)throws->Ticket {
        try validate(expected)
        return try checkpoint(empty,key:expected.key)
    }
    /// Original merge semantics remain in this one namespace/writer owner.
    func resolve(key:AppleDraftKey,payload:Payload?,empty:Payload,retiring:Set<UUID>,expected:Ticket)async throws->Ticket {
        guard expected.key==key else{throw AppleSyncFailure.invalid}
        try validate(expected)
        if let previous=writers[key]{await previous.value}
        try validate(expected)
        let stores=try await stores()
        try validate(expected)
        let replacement=payload ?? empty
        latestMemory[key]=replacement;revisions[key]=UUID();catalog.insert(key)
        let error=latest.save(key,replacement)
        if let error{unsavedLatest.insert(key);errors[key]=error}else{unsavedLatest.remove(key);errors[key]=nil}
        let applied=ticket(key:key)!
        didChange()
        do{_ = try await stores.drafts.resolve(key:key,payload:payload,retiring:retiring)}
        catch{if isCurrent(applied){errors[key]=error.localizedDescription;didChange()};throw error}
        try validate(applied)
        unsavedLatest.remove(key);pendingCheckpoints.remove(key)
        errors[key]=error == nil ? nil:"版本已保存在本机；快捷原文保存失败，可重试。"
        try await refresh()
        try validate(applied)
        didChange()
        return applied
    }
    func retainedTicket(key:AppleDraftKey)->Ticket? {
        guard let payload=latestMemory[key] else{return nil}
        if revisions[key] == nil{revisions[key]=UUID()}
        return Ticket(namespace:id,selection:selection,key:key,revision:revisions[key]!,payload:payload)
    }
    func ticket(key: AppleDraftKey) -> Ticket? {
        guard selected(), let payload = latestMemory[key] else { return nil }
        if revisions[key] == nil { revisions[key] = UUID() }
        return Ticket(namespace: id, selection: selection, key: key, revision: revisions[key]!, payload: payload)
    }
    func isCurrent(_ ticket: Ticket) -> Bool {
        selected() && ticket.selection == selection && sameOriginal(ticket)
    }
    /// For explicit export acknowledgement of a retained old account only.
    func sameOriginal(_ ticket: Ticket) -> Bool {
        ticket.namespace == id && revisions[ticket.key] == ticket.revision && latestMemory[ticket.key] == ticket.payload
    }
    func acknowledgeExport(_ ticket: Ticket) throws {
        guard sameOriginal(ticket) else { throw AppleSyncFailure.invalid }
        unsavedLatest.remove(ticket.key)
        errors[ticket.key] = "该原文已明确导出；本机保存仍需重试。"
        didChange()
    }
    /// Existing clear/merge UI uses this to fence its confirmation across awaits.
    func validate(_ ticket: Ticket) throws { guard isCurrent(ticket) else { throw AppleSyncFailure.accountChanged } }
    func refresh() async throws {
        guard selected() else { throw AppleSyncFailure.accountChanged }
        let stamp = selection, stores = try await stores()
        let keys = await stores.drafts.keys()
        var values: [AppleDraftKey: [DraftStore.Value]] = [:]
        for key in keys { values[key] = await stores.drafts.active(key: key) }
        let recovered = await stores.drafts.recoveredOriginals()
        var hosts: [ApplePortableHost] = []
        for key in await stores.hosts.keys() {
            for value in await stores.hosts.active(key: key) where !hosts.contains(value.payload) { hosts.append(value.payload) }
        }
        guard selected(), selection == stamp else { throw AppleSyncFailure.accountChanged }
        catalog.formUnion(keys); versions = values; self.recovered = recovered; self.hosts = hosts
        didChange()
    }
    func hostTicket(_ host: ApplePortableHost) -> HostTicket? {
        hosts.contains(host) ? HostTicket(namespace: id, host: host) : nil
    }
    fileprivate func sameHost(_ ticket: HostTicket) -> Bool { ticket.namespace == id && hosts.contains(ticket.host) }
    func rememberHost(_ host: ApplePortableHost) async throws {
        guard selected() else { throw AppleSyncFailure.accountChanged }
        let stamp = selection, stores = try await stores()
        guard selected(), selection == stamp else { throw AppleSyncFailure.accountChanged }
        let key = AppleDraftKey(authorityID: host.transportAuthorityID, databaseID: host.databaseID, crewID: "directory", destination: .originalSession(host.verifiedSPKI), kind: "host")
        _ = try await stores.hosts.checkpoint(key: key, payload: host)
        guard selected(), selection == stamp else { throw AppleSyncFailure.accountChanged }
        if !hosts.contains(host) { hosts.append(host) }
    }
    /// Only the current namespace's already-selected data can be exchanged.
    fileprivate func synchronize(using transport: any AppleAccountTransport) async throws -> Bool {
        guard selected(), id.account != nil, inflight == nil else { throw AppleSyncFailure.accountChanged }
        let stamp = selection, revisions = self.revisions, operation = UUID()
        let task = Task {
            let stores = try await self.stores()
            guard self.selected(), self.selection == stamp else { throw AppleSyncFailure.accountChanged }
            for writer in self.writers.values { await writer.value }
            try Task.checkCancellation()
            for key in self.pendingCheckpoints {
                guard self.selected(), self.selection == stamp, let payload = self.latestMemory[key] else { throw AppleSyncFailure.accountChanged }
                let revision = self.revisions[key]
                _ = try await stores.drafts.checkpoint(key: key, payload: payload)
                if self.revisions[key] == revision { self.pendingCheckpoints.remove(key); self.unsavedLatest.remove(key) }
            }
            guard self.selected(), self.selection == stamp else { throw AppleSyncFailure.accountChanged }
            try Task.checkCancellation()
            try await stores.drafts.synchronize(using: transport)
            guard self.selected(), self.selection == stamp else { throw AppleSyncFailure.accountChanged }
            try await stores.hosts.synchronize(using: transport)
        }
        inflight = SyncOperation(id: operation, selection: stamp, task: task)
        defer { if inflight?.id == operation { inflight = nil } }
        func ownsOperation() -> Bool { selected() && selection == stamp && inflight?.id == operation && inflight?.selection == stamp }
        do {
            try await task.value
            guard ownsOperation() else { throw AppleSyncFailure.accountChanged }
            try await refresh()
            let stores = try await stores()
            let represented = latestMemory.allSatisfy { key, payload in versions[key]?.contains(where: { $0.deviceID == stores.deviceID && $0.payload == payload }) == true }
            let draftStatus = await stores.drafts.status, hostStatus = await stores.hosts.status
            guard ownsOperation() else { throw AppleSyncFailure.accountChanged }
            try Task.checkCancellation()
            return self.revisions == revisions && unsavedLatest.isEmpty && initializationError == nil && represented && draftStatus == .synced && hostStatus == .synced
        } catch { throw error }
    }
}

/// A synchronous notification fence rejects old tickets before MainActor delivery.
private final class ApplePersonalAccountFence: @unchecked Sendable {
    private let lock = NSLock(); private var value: UInt64 = 0
    func bump() -> UInt64 { lock.withLock { value &+= 1; return value } }
    func current() -> UInt64 { lock.withLock { value } }
    func isCurrent(_ stamp: UInt64) -> Bool { lock.withLock { value == stamp } }
}

private final class ApplePersonalAccountObservation {
    let token: NSObjectProtocol
    init(_ callback: @escaping @Sendable (Notification) -> Void) {
        token = NotificationCenter.default.addObserver(forName: .CKAccountChanged, object: nil, queue: nil, using: callback)
    }
    deinit { NotificationCenter.default.removeObserver(token) }
}

@MainActor final class ApplePersonalAccountCoordinator<Payload: Codable & Equatable & Sendable> {
    typealias Namespace = ApplePersonalAccountNamespace<Payload>
    enum Status: Equatable {
        case localOnly, notConfigured, discovering, ready(AppleSyncAccount), pending(AppleSyncAccount)
        case synchronized(AppleSyncAccount), accountRefreshRequired, unavailable(String)
    }
    let configuration: ApplePersonalCloudConfiguration?
    private let factory: (ApplePersonalNamespaceID) throws -> Namespace
    private let makeTransport: (ApplePersonalCloudConfiguration) throws -> any AppleAccountTransport
    private let fence = ApplePersonalAccountFence()
    private var transport: (any AppleAccountTransport)?
    private var observer: ApplePersonalAccountObservation?
    private var syncOperation: UUID?
    private(set) var retained: [ApplePersonalNamespaceID: Namespace] = [:]
    private(set) var current: Namespace
    private(set) var optedIn = false
    var didChange: () -> Void = {}
    private var statusValue: Status = .localOnly
    private var synchronizedRevisions: [AppleDraftKey: UUID]?
    private(set) var status: Status {
        get {
            if optedIn && !current.isSelected { return .accountRefreshRequired }
            if case .synchronized(let account) = statusValue, current.revisions != synchronizedRevisions || !current.unsavedLatest.isEmpty { return .pending(account) }
            return statusValue
        }
        set {
            statusValue = newValue
            if case .synchronized = newValue { synchronizedRevisions = current.revisions } else { synchronizedRevisions = nil }
            didChange()
        }
    }
    init(local: Namespace, configuration: ApplePersonalCloudConfiguration?, namespaceFactory: @escaping (ApplePersonalNamespaceID) throws -> Namespace, transportFactory: @escaping (ApplePersonalCloudConfiguration) throws -> any AppleAccountTransport) {
        precondition(local.id == .local)
        self.configuration = configuration; factory = namespaceFactory; makeTransport = transportFactory
        current = local; retained[.local] = local
        let stamp = fence.bump(), fence = self.fence
        local.activate { fence.isCurrent(stamp) }
        // No CKContainer, account lookup, observer or KVS construction at startup.
    }
    var hasUnsavedOriginals: Bool { retained.values.contains { !$0.unsavedLatest.isEmpty } }
    var unsavedExitToken: String? {
        let rows = retained.values.flatMap { owner in owner.unsavedLatest.map { key in owner.id.stableName + "/" + appleSyncDigest((try? appleSyncEncode(key)) ?? Data()) + "/" + (owner.revisions[key]?.uuidString ?? "unknown") } }.sorted()
        return rows.isEmpty ? nil : appleSyncDigest(Data(rows.joined(separator: "\n").utf8))
    }
    private func select(_ next: Namespace, stamp: UInt64) {
        syncOperation = nil
        current.deactivate(); current = next
        let fence = self.fence; next.activate { fence.isCurrent(stamp) }
    }
    /// Invoke only for the explicit opt-in / account refresh button.
    func enable() async throws {
        guard let configuration else { status = .notConfigured; throw AppleSyncFailure.invalid }
        guard configuration.containerIdentifier.hasPrefix("iCloud."), !configuration.draftsCollection.isEmpty,
              !configuration.hostsCollection.isEmpty, configuration.draftsCollection != configuration.hostsCollection else { throw AppleSyncFailure.invalid }
        optedIn = true
        let stamp = fence.bump(), previous = transport; transport = nil
        select(retained[.local]!, stamp: stamp); status = .discovering
        await previous?.cancel()
        guard fence.isCurrent(stamp), optedIn else { throw AppleSyncFailure.accountChanged }
        var built: (any AppleAccountTransport)?
        do {
            let adapter = try makeTransport(configuration); built = adapter; transport = adapter
            if observer == nil {
                let fence = self.fence
                observer = ApplePersonalAccountObservation { [weak self] _ in
                    let notificationStamp = fence.bump()
                    Task { @MainActor [weak self] in await self?.accountChanged(stamp: notificationStamp) }
                }
            }
            let account = try await adapter.accountIdentity()
            guard fence.isCurrent(stamp), optedIn, configuration.accepts(account) else { throw AppleSyncFailure.accountChanged }
            let id = ApplePersonalNamespaceID(account: account, draftsCollection: configuration.draftsCollection, hostsCollection: configuration.hostsCollection)
            let owner = try retained[id] ?? factory(id)
            guard owner.id == id else { throw AppleSyncFailure.invalid }
            retained[id] = owner; select(owner, stamp: stamp); status = .ready(account)
            // No checkpoint/import/exchange here. Old local/account content stays put.
        } catch {
            await built?.cancel()
            if fence.isCurrent(stamp) { transport = nil; status = .unavailable(String(describing: error)) }
            throw error
        }
    }
    func disable() async {
        optedIn = false; let stamp = fence.bump(), previous = transport; transport = nil
        observer = nil
        select(retained[.local]!, stamp: stamp); status = .localOnly
        await previous?.cancel()
    }
    private func accountChanged(stamp: UInt64) async {
        guard optedIn, fence.isCurrent(stamp) else { return }
        let previous = transport; transport = nil
        select(retained[.local]!, stamp: stamp); status = .accountRefreshRequired
        await previous?.cancel()
        // The user can explicitly refresh identity; this event never uploads or migrates.
    }
    func synchronize() async throws {
        guard optedIn, current.isSelected, syncOperation == nil, let transport, let account = current.id.account else { throw AppleSyncFailure.accountChanged }
        let owner = current, stamp = fence.current(), operation = UUID()
        syncOperation = operation
        defer { if syncOperation == operation { syncOperation = nil } }
        status = .pending(account)
        do {
            let clean = try await owner.synchronize(using: transport)
            guard fence.isCurrent(stamp), current === owner, optedIn, syncOperation == operation else { throw AppleSyncFailure.accountChanged }
            status = clean ? .synchronized(account) : .pending(account)
        } catch {
            if fence.isCurrent(stamp), current === owner, syncOperation == operation { status = .unavailable(String(describing: error)) }
            throw error
        }
    }
    func copySelectedHosts(_ tickets: [Namespace.HostTicket], from source: Namespace) async throws {
        guard optedIn, current.id.account != nil, retained[source.id] === source, source !== current,
              tickets.allSatisfy({ source.sameHost($0) }) else { throw AppleSyncFailure.invalid }
        let owner = current, stamp = fence.current()
        for ticket in tickets {
            guard fence.isCurrent(stamp), current === owner else { throw AppleSyncFailure.accountChanged }
            try await owner.rememberHost(ticket.host)
        }
    }
    /// A reviewed, explicit selection copies only these contents; source stays intact.
    func copySelected(_ tickets: [Namespace.Ticket], from source: Namespace, replacing targets: [AppleDraftKey: Namespace.Ticket] = [:]) throws {
        guard optedIn, current.id.account != nil, retained[source.id] === source, source !== current else { throw AppleSyncFailure.invalid }
        guard Set(tickets.map(\.key)).count == tickets.count, tickets.allSatisfy({ source.sameOriginal($0) }) else { throw AppleSyncFailure.invalid }
        for ticket in tickets where current.localDraft(ticket.key) != nil {
            guard let target = targets[ticket.key], target.key == ticket.key, current.isCurrent(target) else { throw AppleSyncFailure.invalid }
        }
        for ticket in tickets { _ = try current.checkpoint(ticket.payload, key: ticket.key) }
    }
}
