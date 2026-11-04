import Foundation

// CrewdRemoteTransport's production dependency on the workspace is this typealias.
enum CrewdWorkspace {
    typealias Connector = @MainActor (CrewdClient) async throws -> Void
}

private actor EnrollmentTrace {
    enum Failure: Error { case lostCommitReply }
    private(set) var methods: [String] = []

    func call(_ method: String, _ bytes: Data, _ link: CrewdEnrollmentLink) async throws -> Data {
        methods.append(method)
        let result = try await CrewdEnrollmentTransport.call(method, bytes, link: link)
        if method == "enrollment.commit" { throw Failure.lostCommitReply }
        return result
    }
}

@main private struct NativeEnrollmentPatrol {
    @MainActor static func main() async throws {
        guard CommandLine.arguments.count == 2 else { fatalError("owned temporary Core directory required") }
        // The runner creates this canonical private directory. Foundation's
        // symlink resolver may display the same directory under /tmp instead.
        let directory = URL(fileURLWithPath: CommandLine.arguments[1])
        precondition(directory.path.hasPrefix("/private/tmp/pcw-enrollment-patrol.") && directory.lastPathComponent == "data")
        let owner = CrewdClient()
        try await owner.connect(path: directory.appendingPathComponent("crewd.sock").path)
        defer { owner.disconnect() }
        let status = try decode(CrewdRemoteStatus.self, try await owner.call("remote.status"))
        guard status.listening, status.configuration.listenHost == "127.0.0.1", let server = status.identity else {
            fatalError("temporary loopback host is not qualified")
        }
        stage("owner loopback qualified")
        let pairVault = await CrewdRemoteMemoryCredentials()
        let pairing = await CrewdRemotePairing(credentials: pairVault)
        let issuerVault = await CrewdEnrollmentVault(service: nil, memory: true)
        let issuer = await CrewdEnrollment(vault: issuerVault, pairing: pairing, call: { _, _, _ in
            fatalError("local issuer entered recipient transport")
        })
        let issued = try await issuer.createHostLink(server: server, endpoint: .init(host: "127.0.0.1", port: status.configuration.port), label: "Isolated native host") { method, bytes in
            let params = try JSONDecoder().decode(CrewdValue.self, from: bytes)
            return try JSONEncoder().encode(await owner.call(method, params))
        }
        let preview = try CrewdEnrollment.preview(issued.link!)
        precondition(preview.enrollmentId == issued.enrollmentId && preview.server == server && preview.endpoint.host == "127.0.0.1")

        let recipientVault = await CrewdEnrollmentVault(service: nil, memory: true)
        let trace = EnrollmentTrace()
        let recipient = await CrewdEnrollment(vault: recipientVault, pairing: pairing, call: { try await trace.call($0, $1, $2) })
        let prepared = try await recipient.prepare(preview, deviceLabel: "Isolated Apple recipient")
        let reopened = try await recipient.prepare(preview, deviceLabel: "Original recipient remains")
        precondition(prepared.id == reopened.id && prepared.clientPin == reopened.clientPin && prepared.phase == .prepared)
        let challenged = try await recipient.begin(prepared.id)
        let beforeCommit = try await pairVault.records()
        precondition(challenged.id == prepared.id && challenged.phase == .challengeReady && beforeCommit.isEmpty)
        stage("original challenge verified before explicit commit")
        do {
            _ = try await recipient.commit(prepared.id)
            fatalError("fixture must hide the already committed response")
        } catch EnrollmentTrace.Failure.lostCommitReply {}
        let pending = try await recipient.attempts()
        precondition(pending.count == 1 && pending[0].id == prepared.id && pending[0].phase == .commitUnknown)
        let recovered = try await recipient.lookup(prepared.id)
        let grant = recovered.accepted!.grant
        let receipt = try await pairing.receipt(prepared.id)
        let records = try await pairVault.records()
        let stages = await trace.methods
        precondition(recovered.id == prepared.id && recovered.phase == .accepted && records == [prepared.id])
        precondition(stages == ["enrollment.begin", "enrollment.commit", "enrollment.lookup"])
        precondition(receipt.grantId == grant.id && receipt.dbId == server.dbId && receipt.authorityId == server.authorityId)
        stage("original enrollment recovered")

        let crew = try await owner.call("crew.create", .object(["title": .string("Isolated native patrol"), "workingDirectory": .string(directory.deletingLastPathComponent().appendingPathComponent("work").path)]))
        let crewID = crew["id"]
        precondition(crewID.string?.isEmpty == false)
        let connect = CrewdRemoteTransport.connector(id: prepared.id, pairing: pairing, expectedReceipt: receipt)
        let first = CrewdClient()
        try await connect(first)
        precondition(first.capabilities.contains("work.patrolStart"))
        let runtime = first.runtimeIdentity["runtimeBindingId"]
        let requestID = CrewdValue.string(UUID().uuidString.lowercased())
        let databaseID = CrewdValue.string(receipt.dbId)
        let command = CrewdValue.object(["operation": .string("work.patrolStart"), "params": .object(["crewId": crewID])])
        let submitted = try await first.call("command.submit", .object(["requestId": requestID, "dbId": databaseID, "runtimeBindingId": runtime, "command": command]))
        precondition(submitted["requestId"] == requestID)
        first.disconnect()

        let second = CrewdClient()
        try await connect(second)
        defer { second.disconnect() }
        precondition(second.runtimeIdentity["runtimeBindingId"] == runtime)
        let lookup = CrewdValue.object(["requestId": requestID, "dbId": databaseID])
        let original = try await second.call("command.lookup", lookup)
        precondition(original["requestId"] == requestID && original["dbId"] == databaseID && original["runtimeBindingId"] == runtime)
        precondition(original["command"] == command && original["phase"] == .string("committed") && original["historical"] == .bool(false))
        let result = original["result"], runID = result["runId"]
        precondition(result["crewId"] == crewID && result["disposition"] == .string("started"))
        precondition(runID.string.flatMap(UUID.init(uuidString:)) != nil)
        let patrol = try await second.call("work.patrolStatus", .object(["crewId": crewID]))
        precondition(patrol["crewId"] == crewID && patrol["manualRun"]["runId"] == runID)
        stage("original patrol verified after reconnect")

        _ = try await owner.call("remote.revoke", .object(["grantId": .string(grant.id), "expectedRevision": .string(grant.revision), "note": .string("Isolated native live revoke")]))
        do {
            _ = try await second.call("command.lookup", lookup)
            fatalError("revoked live transport accepted a read")
        } catch {}
        let next = CrewdClient()
        do {
            try await connect(next)
            fatalError("revoked grant regained a qualified connection")
        } catch {}
        precondition(!next.connected)
        let revoked = try await recipient.lookup(prepared.id)
        precondition(revoked.id == prepared.id && revoked.phase == .revoked)
        let finalStages = await trace.methods
        precondition(finalStages == stages + ["enrollment.lookup"])
        let evidence = CrewdValue.object([
            "enrollmentRequestId": .string(prepared.requestId), "enrollmentStages": .array(finalStages.map(CrewdValue.string)),
            "patrolRequestId": requestID, "patrolRunId": runID, "crewId": crewID,
            "verifiedOnFreshTransport": .bool(true), "liveAndFreshTransportRevoked": .bool(true),
        ])
        try JSONEncoder().encode(evidence).write(to: directory.deletingLastPathComponent().appendingPathComponent("native-result.json"), options: [.withoutOverwriting])
        print("PASS native short-link enrollment, original patrol receipt on fresh mTLS transport, live and fresh revoke")
    }

    private static func decode<T: Decodable>(_ type: T.Type, _ value: CrewdValue) throws -> T {
        try JSONDecoder().decode(type, from: JSONEncoder().encode(value))
    }

    private static func stage(_ value: String) {
        FileHandle.standardOutput.write(Data((value + "\n").utf8))
    }
}
