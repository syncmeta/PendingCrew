import Foundation
import Combine
import Network
import Security

struct CrewdRemotePairSummary: Identifiable, Equatable, Sendable {
    let id: UUID
    let name: String
    let endpoint: CrewdRemoteEndpoint
    let pin: String
    let paired: Bool
    // Public signed snapshots bind deletion even when visible labels stay equal.
    let response: CrewdRemoteEnvelope
    let receipt: CrewdRemoteEnvelope?
}
/// Out-of-band exchange only; this coordinator never sends an enrollment request.
@CrewdCredentialActor final class CrewdRemotePairing {
    nonisolated static let shared = CrewdRemotePairing()
    nonisolated static let changed = Notification.Name("PendingCrew.CrewdRemotePairsChanged.v1")
    private var injected: CrewdRemoteCredentialStore?
    private var credentials: CrewdRemoteCredentialStore {
        if let injected { return injected }
        let store = CrewdRemoteCredentials.application(); injected = store; return store
    }
    // No store construction or credential access crosses into the UI initializer.
    nonisolated init() {}
    init(credentials: CrewdRemoteCredentialStore) { injected = credentials }
    private var responses: [String: (UUID, String)] = [:]
    struct Response: Sendable { let id: UUID; let text: String }
    /// Save and capture the response in one non-suspending transaction. A repeated
    /// invocation in this process returns the original result, never a second key.
    func respondWithText(_ envelope: CrewdRemoteEnvelope, deviceLabel: String) throws -> Response {
        let key = envelope.payload + ":" + envelope.signature
        if let previous = responses[key] { return Response(id: previous.0, text: previous.1) }
        let result = try makeResponse(envelope, deviceLabel: deviceLabel)
        responses[key] = (result.id, result.text)
        return result
    }
    func confirmReceipt(_ envelope: CrewdRemoteEnvelope, id: UUID) throws -> CrewdRemoteReceipt {
        var pair = try credentials.read(id)
        let receipt = try validatedReceipt(envelope, pair: pair)
        if pair.receipt != envelope {
            pair.receipt = envelope
            try credentials.save(pair)
            NotificationCenter.default.post(name: Self.changed, object: nil)
        }
        return receipt
    }
    /// Adopt only the already persisted enrollment identity; never generate a
    /// second response/key when the final receipt or UI acknowledgement is lost.
    func adoptEnrollment(_ pair: CrewdRemoteStoredPair, grantId:String, grantRevision:String) throws -> CrewdRemoteReceipt {
        let receipt=try verifyEnrollmentReceipt(pair,grantId:grantId,grantRevision:grantRevision)
        if try credentials.records().contains(pair.id) {
            let old=try credentials.read(pair.id)
            guard old.invitation==pair.invitation,old.response==pair.response,
                  old.privateKey==pair.privateKey,old.clientCertificate==pair.clientCertificate,
                  old.receipt==nil || old.receipt==pair.receipt else{throw CrewdRemoteError.wrongIdentity}
        }
        try credentials.save(pair)
        NotificationCenter.default.post(name:Self.changed,object:nil)
        return receipt
    }
    func verifyEnrollmentReceipt(_ pair:CrewdRemoteStoredPair,grantId:String,grantRevision:String)throws->CrewdRemoteReceipt {
        guard let envelope=pair.receipt else{throw CrewdRemoteError.invalidExchange}
        let receipt=try validatedReceipt(envelope,pair:pair)
        guard receipt.grantId==grantId,receipt.grantRevision==grantRevision else{throw CrewdRemoteError.wrongIdentity}
        return receipt
    }
    func receipt(_ id: UUID) throws -> CrewdRemoteReceipt { try paired(id).2 }
    func forget(_ summary: CrewdRemotePairSummary) throws {
        let pair = try credentials.read(summary.id)
        let invite = try invitation(pair.invitation, requireFresh: false)
        let current = CrewdRemotePairSummary(id: pair.id, name: invite.computerLabel, endpoint: invite.endpoint, pin: invite.server.spkiSha256, paired: pair.receipt != nil, response: pair.response, receipt: pair.receipt)
        guard current == summary else { throw CrewdRemoteError.wrongIdentity }
        try forget(summary.id)
    }
    func invitation(_ envelope: CrewdRemoteEnvelope, requireFresh: Bool = true) throws -> CrewdRemoteInvitation {
        guard envelope.bounded, let payload = Data(base64Encoded: envelope.payload), payload.count <= 65536 else { throw CrewdRemoteError.invalidExchange }
        let preview = try JSONDecoder().decode(CrewdRemoteInvitation.self, from: payload)
        let certificate = try CrewdRemoteCertificate.decode(preview.server.certificateDerBase64)
        let invite = try CrewdRemoteCertificate.verify(envelope, certificate: certificate, role: .server, expectedPin: preview.server.spkiSha256, as: CrewdRemoteInvitation.self)
        guard CrewdRemoteText.uuid(invite.invitationId), CrewdRemoteText.uuid(invite.server.authorityId), CrewdRemoteText.databaseID(invite.server.dbId),
              invite.endpoint.valid, CrewdRemoteText.pin(invite.server.spkiSha256), !invite.challenge.isEmpty, invite.challenge.utf8.count <= 256,
              !invite.computerLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, invite.computerLabel.utf8.count <= 256,
              let expires = CrewdRemoteText.date(invite.expiresAt) else { throw CrewdRemoteError.invalidExchange }
        if requireFresh && (expires <= Date() || expires.timeIntervalSinceNow > 10 * 60) { throw CrewdRemoteError.expiredInvitation }
        #if PENDINGCREW_KNOWLEDGE_VALIDATION
        throw CrewdRemoteError.invalidExchange // First Mac candidate is local-only.
        #elseif PENDINGCREW_DELIVERY_VALIDATION
        try CrewdDeliveryValidation.validateInvitation(invite)
        #endif
        return invite
    }
    /// Call only after the human confirms the computer identity shown in the UI.
    func respond(_ envelope: CrewdRemoteEnvelope, deviceLabel: String) throws -> UUID {
        try respondWithText(envelope, deviceLabel: deviceLabel).id
    }
    private func makeResponse(_ envelope: CrewdRemoteEnvelope, deviceLabel: String) throws -> Response {
        let invite = try invitation(envelope)
        let label = deviceLabel.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty, label.utf8.count <= 256 else { throw CrewdRemoteError.invalidExchange }
        let generated = try CrewdRemoteCertificate.generate()
        let (_, pin) = try CrewdRemoteCertificate.validate(generated.certificate, role: .client)
        let response = try CrewdRemoteCertificate.sign(CrewdRemoteResponse(invitationId: invite.invitationId, authorityId: invite.server.authorityId, dbId: invite.server.dbId, challenge: invite.challenge, clientCertificateDerBase64: generated.certificate.base64EncodedString(), clientSpkiSha256: pin, deviceLabel: label), privateKey: generated.privateKey)
        #if PENDINGCREW_KNOWLEDGE_VALIDATION
        let id = UUID()
        #elseif PENDINGCREW_DELIVERY_VALIDATION
        let id = try CrewdDeliveryValidation.qualified().account
        #else
        let id = UUID()
        #endif
        let text = try CrewdRemoteText.encode(response)
        try credentials.save(CrewdRemoteStoredPair(id: id, invitation: envelope, response: response, privateKey: generated.privateKey, clientCertificate: generated.certificate))
        NotificationCenter.default.post(name:Self.changed,object:nil)
        return Response(id: id, text: text)
    }
    func summaries() throws -> [CrewdRemotePairSummary] {
        try credentials.records().map { id in
            let pair = try credentials.read(id)
            let invite = try invitation(pair.invitation, requireFresh: false)
            return CrewdRemotePairSummary(id: id, name: invite.computerLabel, endpoint: invite.endpoint, pin: invite.server.spkiSha256, paired: pair.receipt != nil, response: pair.response, receipt: pair.receipt)
        }
    }
    func responseText(_ id: UUID) throws -> String { try CrewdRemoteText.encode(credentials.read(id).response) }
    func confirm(_ envelope: CrewdRemoteEnvelope, id: UUID) throws {
        _ = try confirmReceipt(envelope, id: id)
    }
    private func paired(_ id: UUID) throws -> (CrewdRemoteStoredPair, CrewdRemoteInvitation, CrewdRemoteReceipt) {
        let pair = try credentials.read(id)
        guard let receipt = pair.receipt else { throw CrewdRemoteError.unavailableCredential }
        return (pair, try invitation(pair.invitation, requireFresh: false), try validatedReceipt(receipt, pair: pair))
    }
    private func forget(_ id: UUID) throws {
        let pair=try credentials.read(id)
        if let enrollmentId=pair.enrollmentId {try CrewdEnrollment.shared.removePairAttempt(pair,enrollmentId:enrollmentId)}
        try credentials.remove(id)
        responses = responses.filter { $0.value.0 != id }
        NotificationCenter.default.post(name:Self.changed,object:nil)
    }
    struct Connection: Sendable {
        let invitation: CrewdRemoteInvitation
        let receipt: CrewdRemoteReceipt
        let parameters: NWParameters
    }
    func connection(_ id: UUID) throws -> Connection {
        let (pair, invitation, receipt) = try paired(id)
        let identity = try CrewdRemoteCertificate.securityIdentity(privateKey: pair.privateKey, certificate: pair.clientCertificate)
        guard let protocolIdentity = sec_identity_create(identity) else { throw CrewdRemoteError.unavailableCredential }
        let options = NWProtocolTLS.Options()
        sec_protocol_options_set_min_tls_protocol_version(options.securityProtocolOptions, .TLSv13)
        sec_protocol_options_set_max_tls_protocol_version(options.securityProtocolOptions, .TLSv13)
        sec_protocol_options_set_tls_tickets_enabled(options.securityProtocolOptions, false)
        sec_protocol_options_set_tls_resumption_enabled(options.securityProtocolOptions, false)
        sec_protocol_options_set_peer_authentication_required(options.securityProtocolOptions, true)
        sec_protocol_options_set_local_identity(options.securityProtocolOptions, protocolIdentity)
        let expectedPin = receipt.serverSpkiSha256
        sec_protocol_options_set_verify_block(options.securityProtocolOptions, { _, trust, complete in
            let actualTrust = sec_trust_copy_ref(trust).takeRetainedValue()
            guard let chain = SecTrustCopyCertificateChain(actualTrust) as? [SecCertificate], chain.count == 1 else { complete(false); return }
            do {
                let (_, pin) = try CrewdRemoteCertificate.validate(SecCertificateCopyData(chain[0]) as Data, role: .server)
                complete(pin == expectedPin)
            } catch { complete(false) }
        }, DispatchQueue(label: "pendingcrew.crewd.remote.certificate"))
        let parameters = NWParameters(tls: options, tcp: NWProtocolTCP.Options())
        parameters.allowLocalEndpointReuse = false
        return Connection(invitation: invitation, receipt: receipt, parameters: parameters)
    }

    private func validatedReceipt(_ envelope: CrewdRemoteEnvelope, pair: CrewdRemoteStoredPair) throws -> CrewdRemoteReceipt {
        let invite = try invitation(pair.invitation, requireFresh: false)
        let certificate = try CrewdRemoteCertificate.decode(invite.server.certificateDerBase64)
        let receipt = try CrewdRemoteCertificate.verify(envelope, certificate: certificate, role: .server, expectedPin: invite.server.spkiSha256, as: CrewdRemoteReceipt.self)
        let (_, clientPin) = try CrewdRemoteCertificate.validate(pair.clientCertificate, role: .client)
        _ = try CrewdRemoteCertificate.securityIdentity(privateKey: pair.privateKey, certificate: pair.clientCertificate)
        let response = try CrewdRemoteCertificate.verify(pair.response, certificate: pair.clientCertificate, role: .client, expectedPin: clientPin, as: CrewdRemoteResponse.self)
        guard receipt.invitationId == invite.invitationId, receipt.authorityId == invite.server.authorityId,
              receipt.dbId == invite.server.dbId, receipt.serverSpkiSha256 == invite.server.spkiSha256,
              receipt.clientSpkiSha256 == clientPin, CrewdRemoteText.uuid(receipt.grantId), CrewdRemoteText.revision(receipt.grantRevision),
              CrewdRemoteText.date(receipt.acceptedAt) != nil,
              response.invitationId == invite.invitationId, response.authorityId == invite.server.authorityId, response.dbId == invite.server.dbId,
              response.challenge == invite.challenge, response.clientSpkiSha256 == clientPin,
              response.clientCertificateDerBase64 == pair.clientCertificate.base64EncodedString() else { throw CrewdRemoteError.wrongIdentity }
        return receipt
    }
}

/// Presentation lifetime only: invalidating a view never cancels or repeats an
/// in-flight credential operation. Its result remains in the credential domain.
@MainActor final class CrewdRemoteOperation: ObservableObject {
    @Published private(set) var busy = false
    private var epoch = UUID()
    func invalidate() { epoch = UUID() }
    @discardableResult func run<Value: Sendable>(
        _ operation: @escaping @MainActor () async throws -> Value,
        apply: @escaping @MainActor (Value) -> Void,
        failed: @escaping @MainActor (Error) -> Void
    ) -> Task<Void, Never>? {
        guard !busy else { return nil }
        busy = true
        let token = epoch
        return Task { @MainActor in
            do {
                let value = try await operation()
                busy = false
                guard epoch == token else { return }
                apply(value)
            } catch {
                busy = false
                guard epoch == token else { return }
                failed(error)
            }
        }
    }
}
