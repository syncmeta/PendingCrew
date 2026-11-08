import Foundation
import Combine

/// Each window keeps its old authority until the explicitly chosen candidate has
/// passed the original credential, hello and durable-original-history gates.
@MainActor final class CrewdRemoteWindow: ObservableObject {
    enum ComputerListState: Equatable { case notLoaded, loaded, failed(String) }
    enum SelectionPhase: Equatable {
        case computers, credential, connection, originalHistory
        var label: String {
            switch self {
            case .computers: return "正在读取已存电脑…"
            case .credential: return "正在确认这台电脑…"
            case .connection: return "正在连接所选电脑…"
            case .originalHistory: return "正在核对原连接的提交记录…"
            }
        }
    }
    struct SelectionAttempt: Equatable {
        let id: UUID
        let target: UUID?
        var phase: SelectionPhase
        var deadline: Date
    }
    /// Factories are a narrow offline QA seam. Readiness, history qualification,
    /// deadlines, publication and retirement always run the production path.
    struct Factories {
        var remote: @MainActor (UUID, CrewdRemotePairing, CrewdRemotePairing.Selection) -> CrewdWorkspace = { id, pairing, selection in
            CrewdWorkspace(authorityRecordID: id, connect: CrewdRemoteTransport.preparedConnector(id: id, pairing: pairing, selection: selection))
        }
        #if os(macOS)
        var local: @MainActor (URL, UUID) -> CrewdWorkspace = { CrewdWorkspace(dataDirectory: $0, authorityRecordID: $1) }
        #endif
        var rememberHost: @MainActor (ApplePortableHost, @escaping @MainActor () -> Bool) async -> Void = {
            await CrewPersonalDraftService.shared.rememberHost($0, stillCurrent: $1)
        }
    }
    @Published private(set) var computers: [CrewdRemotePairSummary] = []
    @Published private(set) var computerListState: ComputerListState = .notLoaded
    @Published private(set) var workspace: CrewdWorkspace?
    @Published private(set) var selected: UUID?
    @Published private(set) var retainedWorkspaces: [CrewdWorkspace] = []
    @Published private(set) var credentialBusy = false
    @Published private(set) var attempt: SelectionAttempt?
    @Published private(set) var selectionFailure: String?
    @Published private(set) var selectionNotice: String?
    @Published var showingConnectionSettings = false
    @Published var error: String?
    var isSelecting: Bool { attempt != nil }
    var authorityEpoch: UUID { selectionEpoch }
    var inputReviewWorkspaces: [CrewdWorkspace] { Self.inputReviewWorkspaces(retained: retainedWorkspaces, current: workspace) }
    static func inputReviewWorkspaces(retained: [CrewdWorkspace], current: CrewdWorkspace?) -> [CrewdWorkspace] {
        var scopes = retained
        if let current, !current.client.readyForWork { scopes.append(current) }
        var seen = Set<ObjectIdentifier>()
        return scopes.filter { seen.insert(ObjectIdentifier($0)).inserted }
    }
    private var refreshTask: Task<Bool, Never>?
    private var refreshID: UUID?
    @Published private(set) var computerObservation: UUID?
    var observingComputers: Bool { computerObservation != nil }
    /// Synchronous invalidation reaches the backend before any deferred refresh.
    let authorityInvalidated = PassthroughSubject<CrewdWorkspace, Never>()
    private struct CredentialRecheck {
        let revision: UUID
        let sequence: UInt64
        let recordID: UUID?
    }
    private var pendingCredentialRecheck: CredentialRecheck?
    private var allowAutomaticCredentialRecheck = true
    private var credentialRecheckTask: Task<Void, Never>?
    private var closed = false
    private var lastRemovedAuthority: (recordID: UUID, authority: UUID)?
    private var changes: AnyCancellable?
    private var busyChanges: AnyCancellable?
    private var recheckCompletions: AnyCancellable?
    private var workspaceChanges: AnyCancellable?
    private var selectionTask: Task<Void, Never>?
    private var candidate: CrewdWorkspace?
    private var selectionEpoch = UUID()
    private var credentialEpoch = UUID()
    private var attemptCredentialRevision: UUID?
    private var attemptCredentialSequence: UInt64?
    private var selectedCredentialSequence: UInt64?
    private var computerListSequence: UInt64 = 0
    private var selectedReceipt: CrewdRemoteReceipt?
    private var hintObservation: Task<Void, Never>?
    private var hintEpoch = UUID()
    private let broker: CrewdCredentialReadBroker
    private let limits: CrewdConnectionDeadlines
    private let factories: Factories
    #if DEBUG
    /// Diagnostic observation only; records delivery before existing guards.
    /// No state transition, suspension or credential operation is injected.
    var credentialChangeObservedForQA: (@MainActor () -> Void)?
    var validatedComputerSequenceForQA: UInt64 { computerListSequence }
    #endif
    let pairing: CrewdRemotePairing
    init(pairing: CrewdRemotePairing = .shared, broker: CrewdCredentialReadBroker? = nil,
         deadlines: CrewdConnectionDeadlines = .init(), factories: Factories = .init()) {
        self.pairing = pairing; self.broker = broker ?? .shared; self.limits = deadlines; self.factories = factories
        busyChanges = self.broker.$busy.sink { [weak self] busy in
            guard let self else { return }
            self.credentialBusy = busy
            if !busy { self.scheduleCredentialRecheck() }
        }
        recheckCompletions = self.broker.changeCompletions.sink { [weak self] completion in
            self?.consumeCredentialCompletion(completion)
        }
        #if !PENDINGCREW_LOCAL_MAC || PENDINGCREW_REMOTE_PRODUCT
        changes = NotificationCenter.default.publisher(for: CrewdRemotePairing.changed).sink { [weak self] notification in
            let change = notification.object as? CrewdRemotePairing.Change
            Task { @MainActor in self?.credentialChanged(change) }
        }
        #endif
    }
    private func credentialChanged(_ change: CrewdRemotePairing.Change?) {
        #if DEBUG
        credentialChangeObservedForQA?()
        #endif
        guard !closed, change == nil || change?.pairingID == pairing.observationID else { return }
        // Unloaded windows still do no implicit credential access. A connected
        // window or active explicit choice must never swallow a known change.
        guard computerListState == .loaded || selected != nil || attempt != nil else { return }
        let affectedSelection = selected != nil && (change == nil || (change?.recordID == selected && (change?.sequence ?? 0) > (selectedCredentialSequence ?? 0)))
        let affectedAttempt = attempt?.target != nil && (change == nil || (change?.recordID == attempt?.target && (change?.sequence ?? 0) > (attemptCredentialSequence ?? 0)))
        // A notification may arrive after a newer original read/selection. It
        // cannot revoke that freshly qualified lease or manufacture a reread.
        if let change, !affectedSelection, !affectedAttempt, change.sequence <= computerListSequence { return }
        allowAutomaticCredentialRecheck = true
        let recheckID = affectedSelection ? selected : (affectedAttempt ? attempt?.target : change?.recordID)
        let stamp = CrewdRemotePairing.credentialRevision.snapshot
        pendingCredentialRecheck = .init(revision: stamp.id, sequence: stamp.sequence,
            recordID: affectedSelection || affectedAttempt ? recheckID : (pendingCredentialRecheck?.recordID ?? recheckID))
        if let change, change.kind == .removed, change.sequence > computerListSequence,
           !(selected == change.recordID && change.sequence <= (selectedCredentialSequence ?? 0)) { computers.removeAll { $0.id == change.recordID } }
        if affectedAttempt {
            cancelSelection()
            let message = "已存电脑记录在连接期间发生变化，电脑尚未切换；请核对后重新选择。"
            selectionFailure = message; error = message
        }
        if affectedSelection {
            // No credential read or permission prompt is needed to disable a
            // locally removed/replaced scope. Retirement keeps Unknown/drafts,
            // releases this client only and never stops/resends business work.
            if change?.kind == .removed, let id = selected { lastRemovedAuthority = (id, selectionEpoch) }
            withdrawComputerListObservation()
            retireSelected()
            error = "这台电脑的已存记录已变化，原连接已保留供核对；请重新明确选择。"
        }
        scheduleCredentialRecheck()
    }
    private func consumeCredentialCompletion(_ completion: CrewdCredentialReadBroker.ChangeCompletion) {
        guard !closed, completion.pairingID == pairing.observationID, let pending = pendingCredentialRecheck,
              pending.revision == completion.revision, pending.recordID == completion.recordID else { return }
        pendingCredentialRecheck = nil
        switch completion.result {
        case .success(let snapshot):
            guard snapshot.credentialRevision == CrewdRemotePairing.credentialRevision.current else { return }
            computers = snapshot.computers; computerListSequence = snapshot.credentialSequence; computerListState = .loaded
        case .failure(let failure):
            computerListState = .failed(failure.localizedDescription)
            if !isSelecting, error == nil { error = failure.localizedDescription }
        }
        // A pending marker has no authority to install a connection, clear a
        // selection failure, or publish another window's backend/history/hint.
    }
    private func scheduleCredentialRecheck() {
        guard !closed, allowAutomaticCredentialRecheck, !isSelecting, credentialRecheckTask == nil, let pending = pendingCredentialRecheck else { return }
        let key = CrewdCredentialReadBroker.Key.changedComputers(pairing.observationID, pending.recordID)
        // Joining the same post-write recheck is allowed. An unrelated live
        // worker (including a timed-out writer) is never queued behind.
        guard broker.canObserve(key) else { return }
        credentialRecheckTask = Task { [weak self] in
            guard let self else { return }
            defer { self.credentialRecheckTask = nil; self.scheduleCredentialRecheck() }
            guard !self.closed, !Task.isCancelled, self.allowAutomaticCredentialRecheck, !self.isSelecting,
                  self.pendingCredentialRecheck?.revision == pending.revision, self.broker.canObserve(key) else { return }
            let epoch = self.credentialEpoch, authority = self.selectionEpoch, observation = UUID()
            self.computerObservation = observation
            defer { if self.computerObservation == observation { self.computerObservation = nil } }
            self.pendingCredentialRecheck = nil // one bounded observation for this known change
            do {
                let value = try await self.broker.observe(key, timeout: self.limits.credential) { [pairing = self.pairing] in
                    .computers(try pairing.changedComputerSnapshot(pending.recordID))
                }
                try Task.checkCancellation()
                guard !self.closed, self.credentialEpoch == epoch, self.selectionEpoch == authority,
                      self.pendingCredentialRecheck == nil,
                      case .computers(let snapshot) = value,
                      snapshot.credentialRevision == CrewdRemotePairing.credentialRevision.current else { return }
                self.computers = snapshot.computers; self.computerListSequence = snapshot.credentialSequence; self.computerListState = .loaded
                // This is a list/qualification refresh, never an authority install.
                // In particular it cannot erase the explicit selection failure.
            } catch {
                if let observed = error as? CrewdConnectionObservationError, case .busy = observed {
                    if self.pendingCredentialRecheck == nil { self.pendingCredentialRecheck = pending }
                    return
                }
                guard !self.closed, self.credentialEpoch == epoch, self.selectionEpoch == authority,
                      self.pendingCredentialRecheck == nil, !Task.isCancelled else { return }
                self.computerListState = .failed(error.localizedDescription)
                if self.error == nil { self.error = error.localizedDescription }
            }
        }
    }
    @discardableResult func refresh() async -> Bool {
        guard !closed else { return false }
        if let task = refreshTask { return await task.value }
        guard !isSelecting else { return false }
        // An explicit read may resume a cancelled pending notification batch,
        // and shares an already running qualification instead of another key.
        allowAutomaticCredentialRecheck = true
        if pendingCredentialRecheck != nil || credentialRecheckTask != nil {
            let sequence = pendingCredentialRecheck?.sequence ?? CrewdRemotePairing.credentialRevision.snapshot.sequence
            scheduleCredentialRecheck()
            if let task = credentialRecheckTask { await task.value }
            guard !closed, !isSelecting, pendingCredentialRecheck == nil, computerListSequence >= sequence else { return false }
            if case .loaded = computerListState { return true }; return false
        }
        let token = UUID(), epoch = credentialEpoch, authority = selectionEpoch, originalSelected = selected
        let task = Task { [self] in
            #if PENDINGCREW_LOCAL_MAC && !PENDINGCREW_REMOTE_PRODUCT
            computers = []; computerListState = .notLoaded; return false
            #else
            do {
                let until = Date().addingTimeInterval(limits.credential)
                // A real in-process write can finish after the atomic snapshot
                // but before its MainActor delivery. Only that known revision
                // change permits one fresh snapshot after the old op terminated;
                // a prompt, timeout or unknown error never retries a read.
                for pass in 0..<2 {
                    let value = try await broker.observe(.computers(pairing.observationID, originalSelected), timeout: max(0.001, until.timeIntervalSinceNow)) { [pairing] in
                        .computers(try pairing.computerSnapshot(selected: originalSelected))
                    }
                    try Task.checkCancellation()
                    guard credentialEpoch == epoch, selectionEpoch == authority, selected == originalSelected,
                          case .computers(let snapshot) = value else { return false }
                    guard snapshot.credentialRevision == CrewdRemotePairing.credentialRevision.current else {
                        if pass == 0, until.timeIntervalSinceNow > 0 { continue }
                        throw CrewdConnectionObservationError.changed
                    }
                    computers = snapshot.computers; computerListSequence = snapshot.credentialSequence; computerListState = .loaded; error = nil
                    if selected != nil, snapshot.selectedReceipt != selectedReceipt { retireSelected() }
                    return true
                }
                return false
            } catch {
                guard credentialEpoch == epoch, selectionEpoch == authority, !Task.isCancelled else { return false }
                self.error = error.localizedDescription; computerListState = .failed(error.localizedDescription)
                // Observer timeout/busy is not a credential rejection. Preserve
                // the original selected authority; an actual read failure keeps
                // the original fail-closed retirement behavior.
                if !(error is CrewdConnectionObservationError), !(error is CancellationError), selected != nil { retireSelected() }
                return false
            }
            #endif
        }
        refreshID = token; refreshTask = task; computerObservation = token
        let result = await task.value
        if refreshID == token { refreshTask = nil; refreshID = nil }
        if computerObservation == token { computerObservation = nil }
        return result
    }
    private func retireCurrent() {
        selectionEpoch = UUID(); hintEpoch = UUID(); hintObservation?.cancel(); hintObservation = nil
        workspaceChanges?.cancel(); workspaceChanges = nil
        if let old = workspace { old.retire(); authorityInvalidated.send(old); retainedWorkspaces.append(old) }
    }
    private func install(_ next: CrewdWorkspace) {
        workspace = next
        workspaceChanges = Publishers.Merge(next.objectWillChange, next.client.objectWillChange).sink { [weak self, weak next] _ in
            guard let self, let next, self.workspace === next else { return }; self.objectWillChange.send()
        }
    }
    private func retireSelected() { retireCurrent(); workspace = nil; selected = nil; selectedReceipt = nil; selectedCredentialSequence = nil }
    /// This ends observation only. A pending synchronous credential operation
    /// still owns the process broker, and cannot be repeated or redirected.
    func cancelComputerListObservation() {
        allowAutomaticCredentialRecheck = false
        withdrawComputerListObservation()
    }
    private func withdrawComputerListObservation() {
        credentialEpoch = UUID(); refreshTask?.cancel(); refreshTask = nil; refreshID = nil
        credentialRecheckTask?.cancel(); computerObservation = nil
    }
    func cancelSelection() {
        credentialEpoch = UUID(); selectionTask?.cancel(); selectionTask = nil
        refreshTask?.cancel(); refreshTask = nil; refreshID = nil
        if computerObservation != nil, credentialRecheckTask == nil { computerObservation = nil }
        if let candidate, candidate !== workspace { candidate.retire() }
        candidate = nil; attempt = nil; attemptCredentialRevision = nil; attemptCredentialSequence = nil
        scheduleCredentialRecheck()
    }
    func dismissSelectionFailure() { selectionFailure = nil }
    func close() {
        closed = true; pendingCredentialRecheck = nil; credentialRecheckTask?.cancel(); credentialRecheckTask = nil
        cancelSelection(); hintEpoch = UUID(); hintObservation?.cancel(); hintObservation = nil
        retireSelected()
    }
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

    private func begin(_ target: UUID?, phase: SelectionPhase, duration: TimeInterval) -> UUID {
        cancelSelection(); selectionFailure = nil; selectionNotice = nil; error = nil
        let token = UUID(); let stamp = CrewdRemotePairing.credentialRevision.snapshot; attemptCredentialRevision = target == nil ? nil : stamp.id; attemptCredentialSequence = target == nil ? nil : stamp.sequence; attempt = .init(id: token, target: target, phase: phase, deadline: Date().addingTimeInterval(duration))
        return token
    }
    private func advance(_ token: UUID, _ phase: SelectionPhase, duration: TimeInterval) throws {
        try requireAttempt(token)
        attempt?.phase = phase; attempt?.deadline = Date().addingTimeInterval(duration)
    }
    private func requireAttempt(_ token: UUID) throws {
        guard !Task.isCancelled, attempt?.id == token else { throw CancellationError() }
        if let revision = attemptCredentialRevision, revision != CrewdRemotePairing.credentialRevision.current { throw CrewdRemoteError.wrongIdentity }
    }
    private func finish(_ token: UUID) {
        guard attempt?.id == token else { return }
        candidate = nil; attempt = nil; attemptCredentialRevision = nil; attemptCredentialSequence = nil; selectionTask = nil
        scheduleCredentialRecheck()
    }
    private func fail(_ token: UUID, _ failure: Error) {
        guard attempt?.id == token else { return }
        if let candidate, candidate !== workspace { candidate.retire() }
        if !(failure is CancellationError) {
            var message = failure.localizedDescription
            if let observed = failure as? CrewdConnectionObservationError, case .timedOut = observed {
                if attempt?.phase == .connection { message = "本次连接等待已结束，电脑尚未切换；原连接和输入已保留。" }
                if attempt?.phase == .originalHistory { message = "原连接的提交记录仍未就绪，电脑尚未切换；原输入已保留。" }
            }
            error = message; selectionFailure = message
        }
        finish(token)
    }
    private func qualify(_ next: CrewdWorkspace, root: URL, token: UUID) async throws {
        candidate = next; next.originalJournalRequired = true
        if !next.client.readyForWork {
            try advance(token, .connection, duration: limits.connection)
            let observation = CrewdBoundedConnectionObservation<Bool>()
            _ = try await observation.value(timeout: limits.connection) { await next.connect(); return next.client.readyForWork }
        }
        try requireAttempt(token)
        guard next.client.readyForWork, !next.isRetired else {
            throw CrewdClientError(message: next.error ?? next.client.lastError ?? "这台电脑的身份或连接尚未确认，原连接已保留。", kind: .notSubmitted)
        }
        try advance(token, .originalHistory, duration: limits.journal)
        let generation = next.observationGeneration, connection = next.client.connectionEpoch
        let observation = CrewdBoundedConnectionObservation<Bool>()
        _ = try await observation.value(timeout: limits.journal) { await next.prepareOriginalJournal(root: root); return next.originalJournalReady }
        try requireAttempt(token)
        guard next.acceptsObservation(generation), next.client.connectionEpoch == connection, next.originalJournalReady else {
            throw CrewdClientError(message: next.error ?? "原连接的提交记录尚未就绪，电脑尚未切换。", kind: .notSubmitted)
        }
    }
    func select(_ id: UUID, journalRoot: URL? = nil) async {
        guard !closed else { return }
        let token = begin(id, phase: .credential, duration: limits.credential)
        let task = Task { [self] in
            do {
                guard let root = journalRoot ?? CrewdFrontendConfiguration.directory?.appendingPathComponent("frontend-presentation/unknown-submissions") else {
                    throw CrewdClientError(message: "当前后台数据目录配置不可用，连接未建立。", kind: .notSubmitted)
                }
                if computerListState != .loaded {
                    try advance(token, .computers, duration: limits.credential)
                    let oldSelected = selected
                    let value = try await broker.observe(.computers(pairing.observationID, oldSelected), timeout: limits.credential) { [pairing] in
                        .computers(try pairing.computerSnapshot(selected: oldSelected))
                    }
                    try requireAttempt(token)
                    guard case .computers(let snapshot) = value else { throw CrewdRemoteError.wrongIdentity }
                    computers = snapshot.computers; computerListSequence = snapshot.credentialSequence; computerListState = .loaded
                    // Detect a real changed original receipt before admitting a
                    // new candidate; it is not authorized by the list label.
                    if oldSelected != nil, snapshot.selectedReceipt != selectedReceipt { retireSelected() }
                }
                guard let computer = computers.first(where: { $0.id == id && $0.paired }) else {
                    throw CrewdClientError(message: "原电脑记录已失效或尚未完成配对，请核对原请求；不会连接另一条记录。", kind: .notSubmitted)
                }
                try advance(token, .credential, duration: limits.credential)
                let value = try await broker.observe(.selection(pairing.observationID, computer), timeout: limits.credential) { [pairing] in
                    .selection(try pairing.selection(computer))
                }
                try requireAttempt(token)
                guard case .selection(let qualified) = value else { throw CrewdRemoteError.wrongIdentity }
                let receipt = qualified.connection.receipt
                let next: CrewdWorkspace
                if selected == id, selectedReceipt == receipt, let current = workspace, !current.isRetired, current.client.readyForWork, current.originalJournalReady { next = current }
                else { next = factories.remote(id, pairing, qualified) }
                guard next.authorityRecordID == id, next.dataDirectory == nil else { next.retire(); throw CrewdRemoteError.wrongIdentity }
                try await qualify(next, root: root, token: token)
                try requireAttempt(token)
                if workspace !== next { retireCurrent(); selected = id; selectedReceipt = receipt; install(next) }
                selectedCredentialSequence = qualified.credentialSequence
                finish(token)
                rememberHost(qualified.computer, receipt: receipt, workspace: next)
            } catch { fail(token, error) }
        }
        selectionTask = task
        await withTaskCancellationHandler(operation: { await task.value }, onCancel: {
            Task { @MainActor [weak self] in if self?.attempt?.id == token { self?.cancelSelection() } }
        })
    }
    #if os(macOS)
    func selectLocal(directory: URL, journalRoot: URL? = nil) async {
        guard !closed else { return }
        let token = begin(nil, phase: .connection, duration: limits.connection)
        let task = Task { [self] in
            do {
                let next: CrewdWorkspace
                if selected == nil, let current = workspace, current.dataDirectory == directory, !current.isRetired, current.client.readyForWork, current.originalJournalReady { next = current }
                else {
                    let namespace = try CrewdPresentationJournal.localNamespace(directory: directory)
                    next = factories.local(directory, namespace)
                    guard next.authorityRecordID == namespace, next.dataDirectory == directory else { next.retire(); throw CrewdRemoteError.wrongIdentity }
                }
                try await qualify(next, root: journalRoot ?? directory.appendingPathComponent("frontend-presentation/unknown-submissions"), token: token)
                try requireAttempt(token)
                if workspace !== next { retireCurrent(); selected = nil; selectedReceipt = nil; selectedCredentialSequence = nil; install(next) }
                finish(token)
            } catch { fail(token, error) }
        }
        selectionTask = task
        await withTaskCancellationHandler(operation: { await task.value }, onCancel: {
            Task { @MainActor [weak self] in if self?.attempt?.id == token { self?.cancelSelection() } }
        })
    }
    #endif
    /// Confirmation binds the exact displayed record. Timeout ends observation,
    /// never permission to repeat a delete that might already have completed.
    @discardableResult func forget(_ computer: CrewdRemotePairSummary) async -> Bool {
        guard !isSelecting, computers.contains(computer) else { return false }
        let epoch = credentialEpoch, authority = selectionEpoch
        do {
            _ = try await broker.observe(.forget(pairing.observationID, computer), timeout: limits.credential) { [pairing] in
                try pairing.forget(computer); return .forgotten
            }
            guard !closed, !Task.isCancelled else { return false }
            guard epoch == credentialEpoch, authority == selectionEpoch else {
                // The real completed notification may already have retired this
                // very scope. Report that known original delete, never mutate a
                // replacement selection or call an additional credential read.
                return lastRemovedAuthority?.recordID == computer.id && lastRemovedAuthority?.authority == authority
            }
            if selected == computer.id { retireSelected() }
            // The original completed-change notification owns the single
            // deferred qualification read, including cross-window coalescing.
            return true
        } catch {
            if epoch == credentialEpoch, authority == selectionEpoch, !Task.isCancelled { self.error = error.localizedDescription }
            return false
        }
    }
    /// A directory hint is neither an authority grant nor a connection gate.
    /// One process-wide writer prevents abandoned observers from queueing writes.
    private func rememberHost(_ computer: CrewdRemotePairSummary, receipt: CrewdRemoteReceipt, workspace next: CrewdWorkspace) {
        var address = URLComponents(); address.scheme = "tls"; address.host = computer.endpoint.host; address.port = Int(computer.endpoint.port)
        guard let url = address.url, let host = try? ApplePortableHost(displayName: computer.name, endpoint: url,
            transportAuthorityID: receipt.authorityId, databaseID: receipt.dbId, verifiedSPKI: receipt.serverSpkiSha256) else { return }
        hintEpoch = UUID(); let hint = hintEpoch, authority = selectionEpoch
        let current: @MainActor () -> Bool = { [weak self, weak next] in
            guard let self, let next else { return false }
            return self.hintEpoch == hint && self.selectionEpoch == authority && self.workspace === next && !next.isRetired
        }
        let save = factories.rememberHost
        guard let worker = CrewdHostHintWriter.shared.start(operation: { await save(host, current) }) else {
            selectionNotice = "连接已建立，电脑目录提示仍在保存。"; return
        }
        hintObservation = Task { [weak self] in
            do {
                let observation = CrewdBoundedConnectionObservation<Bool>()
                _ = try await observation.value(timeout: self?.limits.hostHint ?? 5) { await worker.value; return true }
            } catch {
                guard let self, current() else { return }
                self.hintEpoch = UUID()
                if !(error is CancellationError) { self.selectionNotice = "连接已建立，电脑目录提示暂未保存。" }
            }
        }
    }
}

@MainActor private final class CrewdHostHintWriter {
    static let shared = CrewdHostHintWriter()
    private var worker: Task<Void, Never>?
    func start(operation: @escaping @MainActor () async -> Void) -> Task<Void, Never>? {
        guard worker == nil else { return nil }
        let task = Task { [self] in await operation(); worker = nil }
        worker = task; return task
    }
}
