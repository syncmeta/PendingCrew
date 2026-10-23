import Foundation
import Network

// The runner inserts the production failure types and exact begin/commit bodies.
// Stand-ins below only replace credentials, wire values and the call boundary;
// no socket, key generation, real vault or pairing operation is performed.
/* FAILURE_TYPES */
enum CrewdRemoteError: Error { case invalidExchange, expiredInvitation, wrongIdentity }
enum CrewdRemoteText { static func date(_ text: String) -> Date? { ISO8601DateFormatter().date(from: text) } }
struct CrewdEnrollmentLink: Sendable, Equatable { var expiresAt: String; var enrollmentId = "fixture-enrollment"; var token = "synthetic-not-a-token" }
struct CrewdEnrollmentBegin: Encodable { let enrollmentId, requestId, token, clientCertificateDerBase64, clientSpkiSha256, deviceLabel: String }
struct CrewdEnrollmentCommit: Encodable { let enrollmentId, requestId: String; let responseEnvelope: Data }
struct CrewdEnrollmentBegan: Decodable { let requestId: String; let invitation: Data }
struct CrewdRemoteAcceptResult: Decodable { let grant: Int }
typealias CrewdEnrollmentAttempt = Attempt
struct Attempt {
    enum Phase { case prepared, beginUnknown, challengeReady, commitUnknown, accepted }
    let id: UUID
    let link: CrewdEnrollmentLink
    let deviceLabel = "synthetic device"
    let privateKey = Data([1, 2, 3])
    let clientCertificate = Data([4, 5, 6])
    let clientPin = "synthetic pin"
    var phase: Phase
    var invitation: Data? = Data([7])
    var response: Data? = Data([8])
    var accepted: CrewdRemoteAcceptResult?
    var commitAttempted = false
    var requestId: String { id.uuidString.lowercased() }
}
@MainActor final class Vault {
    enum Record { case recipient(Attempt) }
    var value: Attempt
    var calls = 0
    init(_ value: Attempt) { self.value = value }
    func save(_ record: Record) throws { if case .recipient(let value) = record { self.value = value } }
}
@MainActor final class Harness {
    typealias RecoveryScope = @Sendable () async -> Bool
    let vault: Vault
    var running = Set<UUID>()
    let call: @Sendable (String, Data, CrewdEnrollmentLink) async throws -> Data
    init(_ vault: Vault, call: @escaping @Sendable (String, Data, CrewdEnrollmentLink) async throws -> Data) { self.vault = vault; self.call = call }
    private func read(_ id: UUID) throws -> Attempt { guard vault.value.id == id else { throw CrewdRemoteError.wrongIdentity }; return vault.value }
    private func adoptChallenge(_ data: Data, into attempt: inout Attempt) throws { attempt.phase = .challengeReady }
    private func accept(_ value: CrewdRemoteAcceptResult, into attempt: inout Attempt, currentGrant: Int) throws { attempt.phase = .accepted }
    /* PRODUCTION_METHODS */
}
@main struct EnrollmentLifecycleFixture {
    @MainActor static func main() async {
        var failed: [String] = []; var assertions = 0
        func check(_ value: Bool, _ label: String) { assertions += 1; if !value { failed.append(label) } }
        func attempt(_ phase: Attempt.Phase) -> Attempt { .init(id: UUID(), link: .init(expiresAt: ISO8601DateFormatter().string(from: Date().addingTimeInterval(300))), phase: phase) }
        for commit in [false, true] {
            for sent in [false, true] {
                for scope in [false, true] {
                    let original = attempt(commit ? .challengeReady : .prepared), vault = Vault(original)
                    let failure = CrewdEnrollmentTransportFailure(request: commit ? .commit : .begin, stage: sent ? .handedToSend : .connecting, cause: .timeout)
                    let client = Harness(vault) { _, _, _ in await MainActor.run { vault.calls += 1 }; throw failure }
                    do {
                        if commit { _ = try await client.commit(original.id /* CAN_RESTORE */) }
                        else { _ = try await client.begin(original.id /* CAN_RESTORE */) }
                    } catch {}
                    let recover = !sent && scope
                    check(vault.value.phase == (recover ? original.phase : commit ? .commitUnknown : .beginUnknown), "\(commit ? "commit" : "begin") sent=\(sent) scope=\(scope) phase")
                    check(vault.value.commitAttempted == (commit && !recover), "commit flag follows proven boundary")
                    check(vault.calls == 1 && vault.value.id == original.id && vault.value.privateKey == original.privateKey && vault.value.clientCertificate == original.clientCertificate, "same request/key, no automatic retry")
                }
            }
        }
        for commit in [false, true] {
            let original = attempt(commit ? .commitUnknown : .beginUnknown), vault = Vault(original)
            let client = Harness(vault) { _, _, _ in await MainActor.run { vault.calls += 1 }; throw CancellationError() }
            let scope = true
            do {
                if commit { _ = try await client.commit(original.id /* CAN_RESTORE */) }
                else { _ = try await client.begin(original.id /* CAN_RESTORE */) }
            } catch {}
            check(vault.calls == 0 && vault.value.phase == original.phase, "old Unknown is not replayed or cleared")
        }
        let original = attempt(.prepared), vault = Vault(original), scope = true
        let client = Harness(vault) { _, _, _ in await MainActor.run { vault.calls += 1 }; throw CancellationError() }
        do { _ = try await client.begin(original.id /* CAN_RESTORE */) } catch {}
        check(vault.value.phase == .beginUnknown && vault.calls == 1, "cancellation alone is not proof of no send")
        let changed = Vault(attempt(.prepared))
        let stale = Harness(changed) { _, _, _ in
            await MainActor.run { changed.value.phase = .accepted }
            throw CrewdEnrollmentTransportFailure(request: .begin, stage: .connecting, cause: .timeout)
        }
        do { _ = try await stale.begin(changed.value.id /* CAN_RESTORE */) } catch {}
        check(changed.value.phase == .accepted, "late failure cannot replace newer attempt state")
        let mismatch = Vault(attempt(.prepared))
        let mismatchedCall = Harness(mismatch) { _, _, _ in
            throw CrewdEnrollmentTransportFailure(request: .lookup, stage: .connecting, cause: .timeout)
        }
        do { _ = try await mismatchedCall.begin(mismatch.value.id /* CAN_RESTORE */) } catch {}
        check(mismatch.value.phase == .beginUnknown, "different operation proof cannot restore begin")
        let replaced = Vault(attempt(.challengeReady))
        let replacedCall = Harness(replaced) { _, _, _ in
            await MainActor.run { replaced.value.response = Data([99]) }
            throw CrewdEnrollmentTransportFailure(request: .commit, stage: .connecting, cause: .timeout)
        }
        do { _ = try await replacedCall.commit(replaced.value.id /* CAN_RESTORE */) } catch {}
        check(replaced.value.phase == .commitUnknown && replaced.value.response == Data([99]), "changed original challenge cannot be overwritten by late failure")
        #if PRODUCTION_ENROLLMENT_FAILURE
        var boundary = CrewdEnrollmentTransmission(request: .begin)
        check(!boundary.failure(.timeout).requestWasHandedToSend, "before-send proof")
        boundary.handedToSend()
        check(boundary.failure(.cancelled).requestWasHandedToSend, "handoff makes cancellation uncertain")
        let denied = CrewdEnrollmentTransportFailure(request: .lookup, stage: .connecting, cause: .localNetworkDenied)
        check(denied.errorDescription?.contains("本次查询") == true && denied.errorDescription?.contains("原配对结果") == true, "failed lookup does not relabel the original request")
        #endif
        print("\(failed.isEmpty ? "PASS" : "FAIL") \(assertions) production lifecycle assertions; failures: \(failed)")
        exit(failed.isEmpty ? 0 : 1)
    }
}
