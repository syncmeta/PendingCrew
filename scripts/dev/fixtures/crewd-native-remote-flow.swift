import Foundation

// Actual production Apple pairing/transport. Never instantiate the Keychain vault.
final class QAMemoryVault: CrewdRemoteCredentialStore {
    private var values: [UUID: CrewdRemoteStoredPair] = [:]
    func records() throws -> [UUID] { Array(values.keys) }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        guard let value = values[id] else { throw CrewdRemoteError.unavailableCredential }
        return value
    }
    func save(_ record: CrewdRemoteStoredPair) throws { values[record.id] = record }
    func remove(_ id: UUID) throws { values.removeValue(forKey: id) }
}

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
        let vault = QAMemoryVault(), pairing = CrewdRemotePairing(credentials: vault)
        let invitationEnvelope = try envelope(invite)
        stage("public-invitation-decoded")
        _ = try pairing.invitation(invitationEnvelope)
        stage("public-invitation-verified")
        let id = try pairing.respond(invitationEnvelope, deviceLabel: "Independent native loopback QA")
        stage("native-response-signed")
        let recordCount = try vault.records().count
        precondition(recordCount == 1)
        let responseEnvelope = try CrewdRemoteText.decode(pairing.responseText(id))
        let response = try JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(responseEnvelope))
        let acceptParams = CrewdValue.object(["requestId": .string(UUID().uuidString.lowercased()), "responseEnvelope": response])
        let accepted = try await owner.call("remote.accept", acceptParams)
        stage("public-accept-complete")
        let signedReceipt = try envelope(accepted["receipt"])
        let invalidReceipt = CrewdRemoteEnvelope(format: signedReceipt.format, payload: signedReceipt.payload, signature: Data(repeating: 0, count: 64).base64EncodedString())
        do {
            try pairing.confirm(invalidReceipt, id: id)
            fatalError("tampered native receipt signature accepted")
        } catch { }
        let unconfirmed = try vault.read(id)
        precondition(unconfirmed.receipt == nil, "invalid signature changed pairing confirmation")
        try pairing.confirm(signedReceipt, id: id)
        stage("native-receipt-confirmed")
        let (_, invitation, receipt) = try pairing.paired(id)
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
