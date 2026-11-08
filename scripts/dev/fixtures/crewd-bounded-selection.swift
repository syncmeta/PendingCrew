// Calls the actual built App module; only the vault, socket peer and catalog
// writer are controlled offline dependencies. Never opens an application vault.
@testable import PendingCrew
import Foundation
import CryptoKit
import X509

enum CheckFailure: Error { case failed(String), releasedFailure }
func check(_ value: @autoclosure () throws -> Bool, _ message: String) throws {
    if try !value() { throw CheckFailure.failed(message) }
}

/// No sleep or self-timeout in the synchronous read: it never returns until the
/// test controller explicitly releases it, even after every observer has left.
final class BlockingVaultGate: @unchecked Sendable {
    private let condition = NSCondition()
    private var operation: String?
    private var released = true, fail = false, entered = false
    private var counts: [String: Int] = [:]
    var waiting: Bool { condition.lock(); defer { condition.unlock() }; return entered && !released }
    func count(_ operation: String) -> Int { condition.lock(); defer { condition.unlock() }; return counts[operation, default: 0] }
    func arm(_ operation: String, fail: Bool = false) {
        condition.lock(); defer { condition.unlock() }
        self.operation = operation; self.fail = fail; released = false; entered = false
    }
    func release() { condition.lock(); released = true; condition.broadcast(); condition.unlock() }
    func enter(_ value: String) throws {
        precondition(!Thread.isMainThread)
        condition.lock(); defer { condition.unlock() }
        counts[value, default: 0] += 1
        guard value == operation else { return }
        entered = true; condition.broadcast()
        while !released { condition.wait() }
        if fail { throw CheckFailure.releasedFailure }
    }
}
@CrewdCredentialActor final class BlockingMemoryVault: CrewdRemoteCredentialStore {
    nonisolated let gate: BlockingVaultGate
    private var values: [UUID: CrewdRemoteStoredPair] = [:]
    init(gate: BlockingVaultGate) { self.gate = gate }
    func records() throws -> [UUID] { try gate.enter("records"); return Array(values.keys) }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        try gate.enter(id.uuidString)
        guard let value = values[id] else { throw CrewdRemoteError.unavailableCredential }; return value
    }
    func save(_ record: CrewdRemoteStoredPair) throws { values[record.id] = record }
    func remove(_ id: UUID) throws { values.removeValue(forKey: id) }
}
@CrewdCredentialActor final class CredentialFixture {
    let pairing: CrewdRemotePairing
    let ids: [UUID]
    let bindings: [[String: String]]
    private let originals: [UUID: CrewdRemoteStoredPair]
    init(gate: BlockingVaultGate) throws {
        let vault = BlockingMemoryVault(gate: gate)
        let coordinator = CrewdRemotePairing(credentials: vault)
        let signing = P256.Signing.PrivateKey()
        let name = try DistinguishedName { CommonName("Bounded selection offline fixture") }
        let cert = try Certificate(version: .v3, serialNumber: .init(bytes: [1]), publicKey: .init(signing.publicKey), notValidBefore: Date().addingTimeInterval(-60), notValidAfter: Date().addingTimeInterval(3600), issuer: name, subject: name, extensions: Certificate.Extensions {
            Critical(KeyUsage(digitalSignature: true)); try ExtendedKeyUsage([.serverAuth]); Critical(BasicConstraints.notCertificateAuthority)
        }, issuerPrivateKey: .init(signing))
        let der = Data(try cert.serializeAsPEM().derBytes)
        let pin = try CrewdRemoteCertificate.validate(der, role: .server).1
        var ids: [UUID] = [], bindings: [[String: String]] = [], originals: [UUID: CrewdRemoteStoredPair] = [:]
        for label in ["A", "B"] {
            let authority = UUID().uuidString.lowercased(), database = "bounded-offline-" + label
            let invite = CrewdRemoteInvitation(invitationId: UUID().uuidString.lowercased(), server: .init(authorityId: authority, dbId: database, certificateDerBase64: der.base64EncodedString(), spkiSha256: pin), endpoint: .init(host: "127.0.0.1", port: 1), challenge: "offline-challenge-" + label, expiresAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(300)), computerLabel: "Offline " + label)
            let envelope = try CrewdRemoteCertificate.sign(invite, privateKey: signing.derRepresentation)
            let response = try coordinator.respondWithText(envelope, deviceLabel: "Offline QA")
            let pair = try vault.read(response.id)
            let decoded = try JSONDecoder().decode(CrewdRemoteResponse.self, from: Data(base64Encoded: pair.response.payload)!)
            let grant = UUID().uuidString.lowercased()
            let receipt = CrewdRemoteReceipt(invitationId: invite.invitationId, grantId: grant, authorityId: authority, dbId: database, serverSpkiSha256: pin, clientSpkiSha256: decoded.clientSpkiSha256, grantRevision: "1", acceptedAt: ISO8601DateFormatter().string(from: Date()))
            _ = try coordinator.confirmReceipt(CrewdRemoteCertificate.sign(receipt, privateKey: signing.derRepresentation), id: response.id)
            originals[response.id] = try vault.read(response.id)
            ids.append(response.id); bindings.append(["authorityId": authority, "dbId": database, "grantId": grant, "grantRevision": "1"])
        }
        self.pairing = coordinator; self.ids = ids; self.bindings = bindings; self.originals = originals
    }
    func barrier() {}
    /// Real production writes in the in-memory vault: A → absent → exactly A.
    func credentialABA(_ summary: CrewdRemotePairSummary) throws {
        guard let pair = originals[summary.id], let envelope = pair.receipt,
              let bytes = Data(base64Encoded: envelope.payload) else { throw CheckFailure.failed("missing original") }
        let receipt = try JSONDecoder().decode(CrewdRemoteReceipt.self, from: bytes)
        try pairing.forget(summary)
        _ = try pairing.adoptEnrollment(pair, grantId: receipt.grantId, grantRevision: receipt.grantRevision)
    }
}
@MainActor final class HintGate {
    var started = false, finished = false, applied = false
    private var continuation: CheckedContinuation<Void, Never>?
    func save(_ host: ApplePortableHost, current: @escaping @MainActor () -> Bool) async {
        started = true
        await withCheckedContinuation { continuation = $0 }
        applied = current(); finished = true
    }
    func release() { continuation?.resume(); continuation = nil }
}
@MainActor final class WindowFactory {
    let root: URL
    var local: CrewdWorkspace
    var made = 0
    let ids: [UUID]
    var hint: HintGate?
    init(root: URL, local: CrewdWorkspace, ids: [UUID]) { self.root = root; self.local = local; self.ids = ids }
    func factories() -> CrewdRemoteWindow.Factories {
        var value = CrewdRemoteWindow.Factories()
        value.local = { [self] _, _ in local }
        value.remote = { [self] id, _, _ in
            made += 1
            let name = id == ids[0] ? "remote-a.sock" : "remote-b.sock"
            return CrewdWorkspace(authorityRecordID: id, connect: { [root] client in try await client.connect(path: root.appendingPathComponent(name).path) })
        }
        value.rememberHost = { [self] host, current in if let hint { await hint.save(host, current: current) } }
        return value
    }
}
@main struct BoundedSelectionQA {
    @MainActor static func until(_ predicate: () -> Bool, _ message: String, seconds: TimeInterval = 3) async throws {
        let stop = Date().addingTimeInterval(seconds)
        while !predicate(), Date() < stop { try await Task.sleep(nanoseconds: 1_000_000) }
        try check(predicate(), message)
    }
    @MainActor static func main() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1]), scenario = CommandLine.arguments[2]
        setenv("PENDINGCREW_CREWD_DATA_DIR", root.appendingPathComponent("local").path, 1)
        let gate = BlockingVaultGate(), broker = CrewdCredentialReadBroker(), hint = HintGate()
        let fixture = try await CredentialFixture(gate: gate)
        let ids = await fixture.ids, pairing = fixture.pairing, bindings = await fixture.bindings
        let publicBindings = ["remote-a.sock": bindings[0], "remote-b.sock": bindings[1]]
        try JSONSerialization.data(withJSONObject: publicBindings).write(to: root.appendingPathComponent("public-bindings.json"))
        let localRoot = root.appendingPathComponent("local")
        let namespace = try CrewdPresentationJournal.localNamespace(directory: localRoot)
        let local = CrewdWorkspace(dataDirectory: localRoot, authorityRecordID: namespace)
        try await local.client.connect(path: localRoot.appendingPathComponent("crewd.sock").path)
        let factory = WindowFactory(root: root, local: local, ids: ids)
        var limits = CrewdConnectionDeadlines(); limits.credential = 0.12; limits.connection = 1.5; limits.journal = 0.15; limits.hostHint = 0.04
        let window = CrewdRemoteWindow(pairing: pairing, broker: broker, deadlines: limits, factories: factory.factories())
        let model = AppModel(window: window, directory: { localRoot }, bindDrafts: { _ in })
        var other: CrewdRemoteWindow?
        var writerGate: BlockingVaultGate?
        var controlledWriter: Task<CrewdCredentialReadBroker.Value, Error>?
        let rootJournal = localRoot.appendingPathComponent("frontend-presentation/unknown-submissions")
        var result: Error?
        do {
            await model.connectCrewdAuthority()
            try await until({ model.crewdBackend?.workspace === local }, "Local backend did not qualify")
            try check(local.client.readyForWork && local.originalJournalReady, "Local skipped actual hello/history gates")
            let draft = CrewdDrafts.Context.message(crew: "existing-crew", channel: "existing-channel", session: "")
            local.messageDrafts.set("UNSENT ORIGINAL", for: draft)
            let journal = try CrewdPresentationJournal(root: rootJournal, scope: local.presentationScope!)
            let unknown = try journal.prepare(method: "session.resume", params: .object(["sessionId": .string("existing-session")]), crew: "existing-crew")
            func preserved() throws {
                try check(local.messageDrafts.text(for: draft) == "UNSENT ORIGINAL", "original draft changed")
                try check(try journal.unknownRecords().contains { $0.id == unknown.id }, "original unknown receipt lost")
            }
            if scenario == "refresh-coalesced" || scenario == "close-pending" {
                gate.arm("records"); let before = gate.count("records")
                let a = Task { await window.refresh() }, b = Task { await window.refresh() }
                try await until({ gate.waiting }, "synchronous vault never entered")
                if scenario == "close-pending" { window.close() }
                else {
                    let aResult = await a.value, bResult = await b.value
                    try check(!aResult && !bResult && broker.busy && gate.waiting, "observer timeout claimed underlying completion")
                    try check(window.workspace === local && local.client.readyForWork, "refresh timeout retired original Local")
                }
                try check(gate.count("records") == before + 1, "coalesced refresh issued multiple underlying reads")
                gate.release(); _ = await a.value; _ = await b.value
                try await until({ !broker.busy }, "released read never reached terminal")
                await fixture.barrier()
                try check(gate.count("records") == before + 1, "duplicate refresh was queued behind the hung read")
                if scenario == "close-pending" { try check(window.workspace == nil && window.computerListState == .notLoaded, "late read reopened closed window") }
                try preserved()
            } else {
                let loaded = await window.refresh(); try check(loaded, "initial in-memory list failed")
                if scenario == "cross-window-credential-forget" {
                    // A owns the actual production broker/Pairing.forget call.
                    // Only the final synchronous return is controlled; B uses
                    // its real notification sink, Window, AppModel and journal.
                    let second = CrewdRemoteWindow(pairing: pairing, broker: broker, deadlines: limits, factories: factory.factories())
                    other = second
                    let secondModel = AppModel(window: second, directory: { localRoot }, bindDrafts: { _ in })
                    let secondLoaded = await second.refresh()
                    try check(secondLoaded, "second original computer list failed")
                    await secondModel.connectCrewdAuthority(computerID: ids[0])
                    try await until({ second.selected == ids[0] && secondModel.crewdBackend?.workspace === second.workspace }, "B did not commit the original remote authority")
                    guard let original = second.workspace, let scope = original.presentationScope else { throw CheckFailure.failed("B original workspace/scope absent") }
                    try check(original.client.readyForWork && original.originalJournalReady, "B skipped real connection or journal")
                    let originalSummary = window.computers.first { $0.id == ids[0] }!
                    let remoteDraft = CrewdDrafts.Context.message(crew: "existing-crew", channel: "existing-channel", session: "")
                    original.messageDrafts.set("UNSENT REMOTE ORIGINAL", for: remoteDraft)
                    let remoteJournal = try CrewdPresentationJournal(root: rootJournal, scope: scope)
                    let remoteUnknown = try remoteJournal.prepare(method: "session.resume", params: .object(["sessionId": .string("existing-remote-session")]), crew: "existing-crew")
                    var delivered = 0
                    second.credentialChangeObservedForQA = { delivered += 1 }
                    let completionGate = BlockingVaultGate(); writerGate = completionGate
                    completionGate.arm("after-original-forget-notification")
                    controlledWriter = Task {
                        try await broker.observe(.forget(pairing.observationID, originalSummary), timeout: 10) {
                            try pairing.forget(originalSummary)
                            // NotificationCenter.post has returned, so every
                            // real sink has been scheduled. Keep the original
                            // actor operation outstanding until the controller
                            // observes B's actual sink entrance, not a sleep.
                            try completionGate.enter("after-original-forget-notification")
                            return .forgotten
                        }
                    }
                    try await until({ completionGate.waiting }, "writer never reached its post-notification gate")
                    try await until({ delivered > 0 }, "B actual notification sink was not observed")
                    try check(broker.busy && completionGate.waiting, "original writer finished before B notification delivery")
                    try check(delivered == 1, "unexpected extra credential notification")
                    try check(original.isRetired || secondModel.crewdBackend == nil, "known removed credential left B's old write authority available while broker busy")
                    try check(original.messageDrafts.text(for: remoteDraft) == "UNSENT REMOTE ORIGINAL", "B original draft lost on credential invalidation")
                    try check(try remoteJournal.unknownRecords().contains { $0.id == remoteUnknown.id }, "B original Unknown was removed")
                    completionGate.release()
                    if let controlledWriter { _ = try await controlledWriter.value }
                    try await until({ !broker.busy }, "original forget operation did not terminate")
                    await fixture.barrier()
                    try await until({ second.workspace == nil && second.selected == nil && secondModel.crewdBackend == nil }, "B did not retire removed original pairing after writer completion")
                    try check(second.retainedWorkspaces.contains { $0 === original }, "removed credential discarded original B input scope")
                    try check(original.messageDrafts.text(for: remoteDraft) == "UNSENT REMOTE ORIGINAL", "B draft changed after completion")
                    try check(try remoteJournal.unknownRecords().contains { $0.id == remoteUnknown.id }, "B original Unknown was replayed or removed after completion")
                    try preserved()
                } else if scenario == "coalesced-two-windows" {
                    let second = CrewdRemoteWindow(pairing: pairing, broker: broker, deadlines: limits, factories: factory.factories()); other = second
                    let secondLoaded = await second.refresh(); try check(secondLoaded, "second list failed")
                    let before = gate.count(ids[0].uuidString); gate.arm(ids[0].uuidString)
                    let a = Task { await window.select(ids[0], journalRoot: rootJournal) }
                    try await until({ gate.waiting }, "original target read never entered")
                    let b = Task { await second.select(ids[0], journalRoot: rootJournal) }
                    try await Task.sleep(nanoseconds: 15_000_000)
                    try check(gate.count(ids[0].uuidString) == before + 1, "same original read was repeated")
                    gate.release(); await a.value; await b.value
                    await fixture.barrier()
                    try check(gate.count(ids[0].uuidString) == before + 1, "coalesced read was duplicated after release")
                    try check(window.selected == ids[0] && second.selected == ids[0], "shared credential read did not complete both independent windows")
                    try check(window.workspace !== second.workspace && window.workspace?.originalJournalReady == true && second.workspace?.originalJournalReady == true, "windows shared workspace authority/cache or skipped history")
                    try preserved()
                } else if scenario == "credential-epoch-aba" {
                    try Data().write(to: root.appendingPathComponent("remote-a.sock.hold"))
                    let original = window.computers.first { $0.id == ids[0] }!
                    let a = Task { await model.connectCrewdAuthority(computerID: ids[0]) }
                    try await until({ window.attempt?.phase == .connection && factory.made == 1 }, "A did not reach transport")
                    try await fixture.credentialABA(original)
                    try await until({ !window.isSelecting }, "credential A-absent-A did not invalidate old plan")
                    try Data().write(to: root.appendingPathComponent("remote-a.sock.release")); await a.value
                    try check(window.workspace === local && model.crewdBackend?.workspace === local && window.selectionFailure != nil, "same public A after ABA admitted stale credential plan")
                    try preserved()
                } else if scenario == "late-connection-new-attempt" {
                    for name in ["remote-a.sock", "remote-b.sock"] { try Data().write(to: root.appendingPathComponent(name + ".hold")) }
                    let a = Task { await model.connectCrewdAuthority(computerID: ids[0]) }
                    try await until({ window.attempt?.phase == .connection && factory.made == 1 }, "A never reached actual connection")
                    model.cancelCrewdAuthoritySelection(); await model.connectCrewdAuthority()
                    let b = Task { await model.connectCrewdAuthority(computerID: ids[1]) }
                    try await until({ window.attempt?.target == ids[1] && window.attempt?.phase == .connection }, "B did not own next attempt")
                    try Data().write(to: root.appendingPathComponent("remote-a.sock.release"))
                    await a.value; try await Task.sleep(nanoseconds: 30_000_000)
                    try check(window.attempt?.target == ids[1] && model.connectingAuthority && model.crewdBackend?.workspace === local, "late A cleared B busy/backend")
                    try Data().write(to: root.appendingPathComponent("remote-b.sock.release")); await b.value
                    try await until({ model.crewdBackend?.workspace === window.workspace && window.selected == ids[1] }, "B never committed qualified backend")
                    try check(window.selectionFailure == nil, "late A error leaked into B"); try preserved()
                } else if scenario == "connection-timeout" {
                    try Data().write(to: root.appendingPathComponent("remote-a.sock.hold"))
                    await model.connectCrewdAuthority(computerID: ids[0])
                    try check(!model.connectingAuthority && window.workspace === local && window.selectionFailure != nil, "connection deadline replaced old authority or stayed busy")
                    try Data().write(to: root.appendingPathComponent("remote-a.sock.release"))
                    try await Task.sleep(nanoseconds: 30_000_000)
                    try check(window.workspace === local && local.client.readyForWork, "late connection committed after deadline")
                    try preserved()
                } else if scenario == "journal-timeout" {
                    let secondRoot = root.appendingPathComponent("second-local")
                    let ns = try CrewdPresentationJournal.localNamespace(directory: secondRoot)
                    let second = CrewdWorkspace(dataDirectory: secondRoot, authorityRecordID: ns)
                    try await second.client.connect(path: secondRoot.appendingPathComponent("crewd.sock").path)
                    factory.local = second
                    try Data().write(to: root.appendingPathComponent("second-local-crewd.sock.history-hold"))
                    await window.selectLocal(directory: secondRoot, journalRoot: rootJournal)
                    try check(!window.isSelecting && window.workspace === local && window.selectionFailure != nil && second.isRetired, "history deadline admitted unqualified candidate")
                    try Data().write(to: root.appendingPathComponent("second-local-crewd.sock.history-release"))
                    try await Task.sleep(nanoseconds: 30_000_000)
                    try check(window.workspace === local && local.originalJournalReady, "late history replaced original journal/backend")
                    try preserved()
                } else if scenario == "journal-failure" {
                    let file = root.appendingPathComponent("not-a-journal-directory"); try Data([1]).write(to: file)
                    await window.select(ids[0], journalRoot: file)
                    try check(window.workspace === local && window.selected == nil && window.selectionFailure != nil, "unqualified journal replaced original authority")
                    try preserved()
                } else if scenario == "hint-blocked" || scenario == "hint-scope-cancel" {
                    factory.hint = hint
                    await model.connectCrewdAuthority(computerID: ids[0])
                    try await until({ hint.started && model.crewdBackend?.workspace === window.workspace }, "catalog observer held qualified backend")
                    try check(!model.connectingAuthority && window.selected == ids[0], "hint became an authority gate")
                    try await until({ window.selectionNotice != nil }, "hint observer did not expire")
                    try check(window.workspace?.client.readyForWork == true && window.workspace?.originalJournalReady == true && !hint.finished, "catalog timeout disconnected qualified authority")
                    if scenario == "hint-scope-cancel" { window.close() }
                    hint.release(); try await until({ hint.finished }, "hint fake was not drained")
                    try check(!hint.applied, "late catalog result published after observer/scope expired")
                    try preserved()
                } else {
                    let failure = scenario == "credential-cancel-local-late-failure"
                    let before = gate.count(ids[0].uuidString); gate.arm(ids[0].uuidString, fail: failure)
                    let a = Task { await model.connectCrewdAuthority(computerID: ids[0]) }
                    try await until({ gate.waiting }, "synchronous target vault never entered")
                    try check(model.connectingAuthority && model.crewdBackend?.workspace === local && local.client.readyForWork, "candidate progress falsified current Local readiness/backend")
                    if scenario == "credential-timeout" {
                        await a.value
                        try check(!model.connectingAuthority && window.selectionFailure != nil && broker.busy && gate.waiting, "deadline did not leave original synchronous operation outstanding")
                    } else { model.cancelCrewdAuthoritySelection() }
                    let started = Date(); await model.connectCrewdAuthority()
                    try check(Date().timeIntervalSince(started) < 0.3 && window.workspace === local && model.crewdBackend?.workspace === local, "Local waited for hung credential domain")
                    // A different target must not queue another underlying read.
                    let beforeB = gate.count(ids[1].uuidString)
                    await model.connectCrewdAuthority(computerID: ids[1])
                    let newerError = window.selectionFailure
                    try check(newerError != nil && gate.count(ids[1].uuidString) == beforeB, "missing busy result")
                    let bReads = gate.count(ids[1].uuidString)
                    try check(gate.count(ids[0].uuidString) == before + 1 && factory.made == 0, "abandoned read repeated or connected")
                    gate.release(); await a.value
                    try await until({ !broker.busy }, "released synchronous operation did not terminate")
                    await fixture.barrier()
                    try check(gate.count(ids[0].uuidString) == before + 1, "duplicate original read was queued after cancellation")
                    try await Task.sleep(nanoseconds: 15_000_000)
                    try check(window.selectionFailure == newerError && gate.count(ids[1].uuidString) == bReads && window.workspace === local, "late old credential result changed new scope/error or queued B")
                    try preserved()
                }
            }
        } catch { result = error }
        // Even a failed assertion drains the truly blocking fake before exit.
        gate.release(); writerGate?.release(); hint.release(); window.close(); other?.close(); local.retire()
        for name in ["remote-a.sock", "remote-b.sock"] { try? Data().write(to: root.appendingPathComponent(name + ".release")) }
        try? Data().write(to: root.appendingPathComponent("second-local-crewd.sock.history-release"))
        try await until({ !broker.busy }, "cleanup left synchronous fake outstanding")
        if let controlledWriter { _ = try? await controlledWriter.value }
        await fixture.barrier()
        if hint.started { try await until({ hint.finished }, "cleanup left catalog fake outstanding") }
        if let result { throw result }
        print("PASS actual production bounded-selection: " + scenario)
    }
}
