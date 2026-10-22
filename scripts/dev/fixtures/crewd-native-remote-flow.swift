import Foundation
import CryptoKit
import X509

// Actual production Apple pairing/transport. Never instantiate the Keychain vault.
@CrewdCredentialActor final class QAMemoryVault: CrewdRemoteCredentialStore {
    var delay: TimeInterval = 0
    var failSave = false
    private(set) var saves = 0, removes = 0, reads = 0
    private static var active = 0, maximumActive = 0
    var maximumActive: Int { Self.maximumActive }
    private func enter() {
        precondition(!Thread.isMainThread, "credential work ran on main thread")
        Self.active += 1; Self.maximumActive = max(Self.maximumActive, Self.active)
        if delay > 0 { Thread.sleep(forTimeInterval: delay) }
    }
    private func leave() { Self.active -= 1 }
    private var values: [UUID: CrewdRemoteStoredPair] = [:]
    func records() throws -> [UUID] { enter(); defer { leave() }; return Array(values.keys) }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        enter(); defer { leave() }; reads += 1
        guard let value = values[id] else { throw CrewdRemoteError.unavailableCredential }
        return value
    }
    func save(_ record: CrewdRemoteStoredPair) throws { enter(); defer { leave() }; saves += 1; if failSave { throw CrewdRemoteError.storageFailure }; values[record.id] = record }
    func remove(_ id: UUID) throws { enter(); defer { leave() }; removes += 1; values.removeValue(forKey: id) }
}

#if QA_CREDENTIAL_SERIAL || QA_REMOTE_CODE_FILE
@CrewdCredentialActor private final class SerialScope {
    let vault = QAMemoryVault()
    let pairing: CrewdRemotePairing
    let invitation: CrewdRemoteEnvelope
    private let key: Data
    init() throws {
        pairing = CrewdRemotePairing(credentials: vault)
        let signing = P256.Signing.PrivateKey()
        key = signing.derRepresentation
        let name = try DistinguishedName { CommonName("Credential serial in-memory case") }
        let certificate = try Certificate(version: .v3, serialNumber: .init(bytes: [1]), publicKey: .init(signing.publicKey), notValidBefore: Date().addingTimeInterval(-60), notValidAfter: Date().addingTimeInterval(3600), issuer: name, subject: name, extensions: Certificate.Extensions {
            Critical(KeyUsage(digitalSignature: true)); try ExtendedKeyUsage([.serverAuth]); Critical(BasicConstraints.notCertificateAuthority)
        }, issuerPrivateKey: .init(signing))
        let der = Data(try certificate.serializeAsPEM().derBytes)
        let pin = try CrewdRemoteCertificate.validate(der, role: .server).1
        let formatter = ISO8601DateFormatter()
        invitation = try CrewdRemoteCertificate.sign(CrewdRemoteInvitation(invitationId: UUID().uuidString.lowercased(), server: .init(authorityId: UUID().uuidString.lowercased(), dbId: "serial-memory-database", certificateDerBase64: der.base64EncodedString(), spkiSha256: pin), endpoint: .init(host: "127.0.0.1", port: 1), challenge: "serial-memory-challenge", expiresAt: formatter.string(from: Date().addingTimeInterval(300)), computerLabel: "Serial memory computer"), privateKey: key)
        vault.delay = 0.12
    }
    func receipt(_ id: UUID, revision: String = "1") throws -> CrewdRemoteEnvelope {
        let pair = try vault.read(id)
        let invite = try pairing.invitation(pair.invitation)
        let response = try JSONDecoder().decode(CrewdRemoteResponse.self, from: Data(base64Encoded: pair.response.payload)!)
        return try CrewdRemoteCertificate.sign(CrewdRemoteReceipt(invitationId: invite.invitationId, grantId: UUID().uuidString.lowercased(), authorityId: invite.server.authorityId, dbId: invite.server.dbId, serverSpkiSha256: invite.server.spkiSha256, clientSpkiSha256: response.clientSpkiSha256, grantRevision: revision, acceptedAt: ISO8601DateFormatter().string(from: Date())), privateKey: key)
    }
    func stats() -> [Int] { [vault.saves, vault.removes, vault.reads, vault.maximumActive] }
    func failWrites() { vault.failSave = true }
}
#if QA_REMOTE_CODE_FILE
@main struct NativeRemoteFlow {
    @MainActor static func main() async throws {
        let scope = try await SerialScope()
        let invitation = try CrewdRemoteText.encode(scope.invitation)
        let pairing = scope.pairing
        let before = await scope.stats()
        let raw = "\n" + invitation + "\n"
        let imported = try CrewdRemoteCodeFile.text(Data(raw.utf8), stage: .invitation)
        precondition(imported == raw)
        let afterParse = await scope.stats()
        precondition(afterParse == before, "file parsing accessed credentials")
        let response = try await pairing.respondWithText(scope.invitation, deviceLabel: "File memory device")
        let receipt = try CrewdRemoteText.encode(try await scope.receipt(response.id))
        let codes: [(CrewdRemoteCodeStage, String)] = [(.invitation, raw), (.response, response.text), (.receipt, receipt)]
        var checked = 0
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("pcw-code-file-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        for (stage, code) in codes {
            let bytes = Data(code.utf8)
            let document = try CrewdRemoteCodeDocument(text: code, stage: stage)
            precondition(document.bytes == bytes, "export changed original signed text")
            let file = root.appendingPathComponent(stage.filename)
            try bytes.write(to: file)
            let read = try CrewdRemoteCodeFile.read(file, stage: stage)
            precondition(read == code)
            for other in codes where other.0 != stage {
                do { _ = try CrewdRemoteCodeFile.text(bytes, stage: other.0); fatalError("wrong phase accepted") }
                catch CrewdRemoteCodeFileError.wrongStage { checked += 1 }
            }
        }
        for (bytes, expected) in [(Data(repeating: 32, count: CrewdRemoteCodeFile.maximumBytes + 1), 1), (Data([0xff, 0xfe]), 2), (Data("pendingcrew-remote-v1:broken".utf8), 3)] {
            do { _ = try CrewdRemoteCodeFile.text(bytes, stage: .invitation); fatalError("malformed file accepted") }
            catch CrewdRemoteCodeFileError.tooLarge { precondition(expected == 1) }
            catch CrewdRemoteCodeFileError.invalidUTF8 { precondition(expected == 2) }
            catch CrewdRemoteCodeFileError.wrongStage { precondition(expected == 3) }
        }
        let huge = root.appendingPathComponent("oversized.txt")
        try Data(repeating: 32, count: CrewdRemoteCodeFile.maximumBytes + 1).write(to: huge)
        do { _ = try CrewdRemoteCodeFile.read(huge, stage: .invitation); fatalError("oversize file read accepted") }
        catch CrewdRemoteCodeFileError.tooLarge {}
        do { _ = try CrewdRemoteCodeFile.read(root, stage: .invitation); fatalError("directory read accepted") }
        catch CrewdRemoteCodeFileError.notRegularFile {}
        let gate = CrewdRemoteCodeImport()
        var field = "original input"
        let first = gate.prepare(); gate.invalidate() // edit A→B→A still invalidates the request.
        gate.complete(first, result: .success(raw), apply: { field = $0 })
        precondition(field == "original input")
        let closed = gate.prepare(); gate.invalidate() // original form disappeared or became busy.
        let replacement = gate.prepare()
        gate.complete(closed, result: .success(raw), apply: { field = $0 })
        precondition(field == "original input")
        gate.complete(replacement, result: .failure(CrewdRemoteCodeFileError.wrongStage), apply: { field = $0 })
        precondition(field == "original input" && gate.message != nil)
        let fileURL = root.appendingPathComponent(CrewdRemoteCodeStage.invitation.filename)
        let late = gate.prepare()
        gate.read(fileURL, stage: .invitation, token: late) { _ in fatalError("late actual file read applied") }
        gate.invalidate()
        let current = gate.prepare()
        gate.complete(current, result: .success("new edit"), apply: { field = $0 })
        try await Task.sleep(nanoseconds: 30_000_000)
        precondition(field == "new edit", "late file result overwrote new input")
        let valid = gate.prepare()
        let afterSetup = await scope.stats()
        gate.read(root.appendingPathComponent(CrewdRemoteCodeStage.invitation.filename), stage: .invitation, token: valid) { field = $0 }
        while gate.reading { try await Task.sleep(nanoseconds: 1_000_000) }
        precondition(field == raw && gate.message == "已导入，请继续核对。")
        let afterFiles = await scope.stats()
        precondition(afterFiles == afterSetup, "import/export touched credentials")
        precondition(checked == 6)
        print("PASS production file exchange: three stages, exact original export/import bytes, six wrong-stage refusals, bounded UTF8/file reads, no credential access, stale/edit/closed guards, retained input on errors")
    }
}
#else
@main struct NativeRemoteFlow {
    @MainActor static func main() async throws {
        let scope = try await SerialScope()
        let pairing = scope.pairing, invitation = scope.invitation
        let other = try await SerialScope()
        let otherPairing = other.pairing, otherInvite = other.invitation
        let operation = CrewdRemoteOperation()
        var applied = false, failed = false
        let first = operation.run({ try await pairing.respondWithText(invitation, deviceLabel: "Memory device") }, apply: { _ in applied = true }, failed: { _ in failed = true })!
        let independent = Task { try await otherPairing.respondWithText(otherInvite, deviceLabel: "Other memory device") }
        let duplicate = operation.run({ try await pairing.respondWithText(invitation, deviceLabel: "Memory device") }, apply: { _ in fatalError("single-flight admitted duplicate") }, failed: { _ in fatalError("duplicate executed") })
        precondition(duplicate == nil)
        try await Task.sleep(nanoseconds: 30_000_000)
        precondition(operation.busy, "MainActor did not run while credential write was waiting")
        operation.invalidate(); first.cancel()
        await first.value
        _ = try await independent.value
        precondition(!applied && !failed && !operation.busy, "closed presentation applied a late result")
        async let a = pairing.respondWithText(invitation, deviceLabel: "Memory device")
        async let b = pairing.respondWithText(invitation, deviceLabel: "Memory device")
        let (one, two) = try await (a, b)
        precondition(one.id == two.id && one.text == two.text)
        var stats = await scope.stats()
        precondition(stats[0] == 1 && stats[2] == 0, "response performed duplicate write or write-then-read")
        let signed = try await scope.receipt(one.id)
        async let c = pairing.confirmReceipt(signed, id: one.id)
        async let d = pairing.confirmReceipt(signed, id: one.id)
        let (receipt, repeated) = try await (c, d)
        precondition(receipt == repeated)
        stats = await scope.stats(); precondition(stats[0] == 2 && stats[2] == 3, "confirmation performed write-then-read")
        let summary = try await pairing.summaries().first!
        let stale = CrewdRemotePairSummary(id: summary.id, name: "Changed row", endpoint: summary.endpoint, pin: summary.pin, paired: summary.paired, response: summary.response, receipt: summary.receipt)
        do { try await pairing.forget(stale); fatalError("stale summary deleted credential") } catch CrewdRemoteError.wrongIdentity {}
        let changedReceipt = try await scope.receipt(one.id, revision: "2")
        _ = try await pairing.confirmReceipt(changedReceipt, id: one.id)
        do { try await pairing.forget(summary); fatalError("stale receipt with unchanged labels deleted credential") } catch CrewdRemoteError.wrongIdentity {}
        let current = try await pairing.summaries().first!
        try await pairing.forget(current)
        do { try await pairing.forget(current); fatalError("duplicate delete admitted") } catch CrewdRemoteError.unavailableCredential {}
        stats = await scope.stats(); precondition(stats[1] == 1 && stats[3] == 1)
        let failing = try await SerialScope(); await failing.failWrites()
        let failingPairing = failing.pairing, failingInvite = failing.invitation
        var errors = 0
        let failedTask = operation.run({ try await failingPairing.respondWithText(failingInvite, deviceLabel: "Memory device") }, apply: { _ in fatalError("failed save reported success") }, failed: { _ in errors += 1 })!
        await failedTask.value
        let failureStats = await failing.stats()
        precondition(errors == 1 && failureStats[0] == 1 && failureStats[1] == 0)
        print("PASS production credential serial: main responsive, single-flight, atomic response/confirm, single delete, stale refusal, late result discarded, no retry")
    }
}
#endif
#else
@main struct NativeRemoteFlow {
    @MainActor static func main() async throws {
        guard CommandLine.arguments.count == 2 else { fatalError("explicit isolated service root required") }
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        func stage(_ value: String) { FileHandle.standardOutput.write(Data(("NATIVE_STAGE " + value + "\n").utf8)) }
        stage("begin")
        let owner = CrewdClient()
        try await owner.connect(path: root.appendingPathComponent("crewd.sock").path)
        stage("local-qualified")
        let status = try await owner.call("remote.status")
        precondition(status["listening"] == .bool(true), "harness must explicitly configure loopback first")
        precondition(status["configuration"]["listenHost"] == .string("127.0.0.1"))
        func envelope(_ value: CrewdValue) throws -> CrewdRemoteEnvelope {
            try JSONDecoder().decode(CrewdRemoteEnvelope.self, from: JSONEncoder().encode(value))
        }
        let invite = try await owner.call("remote.invite", .object(["requestId": .string(UUID().uuidString.lowercased())]))
        let vault = await QAMemoryVault()
        let pairing = await CrewdRemotePairing(credentials: vault)
        let invitationEnvelope = try envelope(invite)
        stage("public-invitation-decoded")
        _ = try await pairing.invitation(invitationEnvelope)
        stage("public-invitation-verified")
        let id = try await pairing.respond(invitationEnvelope, deviceLabel: "Independent native loopback QA")
        stage("native-response-signed")
        let recordCount = try await vault.records().count
        precondition(recordCount == 1)
        let responseEnvelope = try CrewdRemoteText.decode(try await pairing.responseText(id))
        let response = try JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(responseEnvelope))
        let acceptParams = CrewdValue.object(["requestId": .string(UUID().uuidString.lowercased()), "responseEnvelope": response])
        let accepted = try await owner.call("remote.accept", acceptParams)
        stage("public-accept-complete")
        let signedReceipt = try envelope(accepted["receipt"])
        let invalidReceipt = CrewdRemoteEnvelope(format: signedReceipt.format, payload: signedReceipt.payload, signature: Data(repeating: 0, count: 64).base64EncodedString())
        do {
            try await pairing.confirm(invalidReceipt, id: id)
            fatalError("tampered native receipt signature accepted")
        } catch { }
        let unconfirmed = try await unconfirmedReceipt(vault, id: id)
        precondition(unconfirmed, "invalid signature changed pairing confirmation")
        try await pairing.confirm(signedReceipt, id: id)
        stage("native-receipt-confirmed")
        let receipt = try await pairing.receipt(id)
        let invitation = try await pairing.invitation(invitationEnvelope, requireFresh: false)
        precondition(invitation.server.dbId == receipt.dbId && invitation.server.authorityId == receipt.authorityId)
        let remote = CrewdClient()
        try await CrewdRemoteTransport.connector(id: id, pairing: pairing)(remote)
        precondition(remote.connected)
        stage("native-tls-and-hello-qualified")
        let work = root.appendingPathComponent("native-work")
        try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        let crew = try await remote.call("crew.create", .object(["title": .string("Native TLS QA crew"), "workingDirectory": .string(work.path)]))
        guard let crewID = crew["id"].string else { fatalError("actual crew receipt missing") }
        let requirement = try await remote.call("todo.create", .object(["requestId": .string(UUID().uuidString.lowercased()), "crewId": .string(crewID), "text": .string("Native Apple TLS requirement")]))
        let current = try await remote.call("todo.get", .object(["id": requirement["requirementId"]]))
        precondition(current["text"] == .string("Native Apple TLS requirement"))
        var events: [CrewdValue] = []
        remote.onEvent = { name, value in if name == "document.changed" { events.append(value) } }
        _ = try await remote.call("events.subscribe", .object(["topics": .array([.string("documents")])]))
        let document = try await owner.call("document.create", .object(["requestId": .string(UUID().uuidString.lowercased()), "crewId": .string(crewID), "title": .string("Native subscribed event"), "body": .string("No user data")]))
        let deadline = Date().addingTimeInterval(10)
        while !events.contains(where: { $0["id"] == document["document"]["id"] }) {
            precondition(Date() < deadline, "actual native remote event not received")
            try await Task.sleep(for: .milliseconds(20))
        }
        _ = try await owner.call("remote.revoke", .object(["grantId": accepted["grant"]["id"], "expectedRevision": accepted["grant"]["revision"], "note": .string("Native live grant revoke")]))
        do {
            _ = try await remote.call("todo.get", .object(["id": requirement["requirementId"]]))
            fatalError("revoked native connection still read work")
        } catch { }
        stage("live-revoke-denied-old-read")
        remote.disconnect()
        let next = CrewdClient()
        do {
            try await CrewdRemoteTransport.connector(id: id, pairing: pairing)(next)
            fatalError("old native certificate regained qualified connection")
        } catch { }
        next.disconnect()
        stage("old-cert-new-connection-denied")
        let confirmation = try await owner.call("remote.accept", acceptParams)
        precondition(confirmation["reused"] == .bool(true) && confirmation["grant"]["revokedAt"] != .null)
        owner.disconnect()
        try Data("Native production pairing + Network TLS13 + Core work + event + revoke PASS\n".utf8).write(to: root.appendingPathComponent("native-result.txt"))
        print("Native loopback acceptance PASS; temporary memory vault only")
    }
}

@CrewdCredentialActor private func unconfirmedReceipt(_ vault: QAMemoryVault, id: UUID) throws -> Bool { try vault.read(id).receipt == nil }
#endif
