import Foundation

struct CrewdRemotePairSummary: Identifiable, Equatable {
    let id: UUID
    let name: String
    let endpoint: CrewdRemoteEndpoint
    let pin: String
    let paired: Bool
}
/// Out-of-band exchange only; this coordinator never sends an enrollment request.
final class CrewdRemotePairing {
    static let changed = Notification.Name("PendingCrew.CrewdRemotePairsChanged.v1")
    private let credentials: CrewdRemoteCredentialStore
    init(credentials: CrewdRemoteCredentialStore) { self.credentials = credentials }
    func invitation(_ envelope: CrewdRemoteEnvelope, requireFresh: Bool = true) throws -> CrewdRemoteInvitation {
        guard envelope.bounded, let payload = Data(base64Encoded: envelope.payload), payload.count <= 65536 else { throw CrewdRemoteError.invalidExchange }
        let preview = try JSONDecoder().decode(CrewdRemoteInvitation.self, from: payload)
        let certificate = try CrewdRemoteCertificate.decode(preview.server.certificateDerBase64)
        let invite = try CrewdRemoteCertificate.verify(envelope, certificate: certificate, role: .server, expectedPin: preview.server.spkiSha256, as: CrewdRemoteInvitation.self)
        guard CrewdRemoteText.uuid(invite.invitationId), CrewdRemoteText.uuid(invite.server.authorityId), CrewdRemoteText.uuid(invite.server.dbId),
              invite.endpoint.valid, CrewdRemoteText.pin(invite.server.spkiSha256), !invite.challenge.isEmpty, invite.challenge.utf8.count <= 256,
              !invite.computerLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, invite.computerLabel.utf8.count <= 256,
              let expires = CrewdRemoteText.date(invite.expiresAt) else { throw CrewdRemoteError.invalidExchange }
        if requireFresh && (expires <= Date() || expires.timeIntervalSinceNow > 10 * 60) { throw CrewdRemoteError.expiredInvitation }
        return invite
    }
    /// Call only after the human confirms the computer identity shown in the UI.
    func respond(_ envelope: CrewdRemoteEnvelope, deviceLabel: String) throws -> UUID {
        let invite = try invitation(envelope)
        let label = deviceLabel.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty, label.utf8.count <= 256 else { throw CrewdRemoteError.invalidExchange }
        let generated = try CrewdRemoteCertificate.generate()
        let (_, pin) = try CrewdRemoteCertificate.validate(generated.certificate, role: .client)
        let response = try CrewdRemoteCertificate.sign(CrewdRemoteResponse(invitationId: invite.invitationId, authorityId: invite.server.authorityId, dbId: invite.server.dbId, challenge: invite.challenge, clientCertificateDerBase64: generated.certificate.base64EncodedString(), clientSpkiSha256: pin, deviceLabel: label), privateKey: generated.privateKey)
        let id = UUID()
        try credentials.save(CrewdRemoteStoredPair(id: id, invitation: envelope, response: response, privateKey: generated.privateKey, clientCertificate: generated.certificate))
        NotificationCenter.default.post(name:Self.changed,object:nil)
        return id
    }
    func summaries() throws -> [CrewdRemotePairSummary] {
        try credentials.records().map { id in
            let pair = try credentials.read(id)
            let invite = try invitation(pair.invitation, requireFresh: false)
            return CrewdRemotePairSummary(id: id, name: invite.computerLabel, endpoint: invite.endpoint, pin: invite.server.spkiSha256, paired: pair.receipt != nil)
        }
    }
    func responseText(_ id: UUID) throws -> String { try CrewdRemoteText.encode(credentials.read(id).response) }
    func confirm(_ envelope: CrewdRemoteEnvelope, id: UUID) throws {
        var pair = try credentials.read(id)
        _ = try validatedReceipt(envelope, pair: pair)
        pair.receipt = envelope
        try credentials.save(pair)
        NotificationCenter.default.post(name:Self.changed,object:nil)
    }
    func paired(_ id: UUID) throws -> (CrewdRemoteStoredPair, CrewdRemoteInvitation, CrewdRemoteReceipt) {
        let pair = try credentials.read(id)
        guard let receipt = pair.receipt else { throw CrewdRemoteError.unavailableCredential }
        return (pair, try invitation(pair.invitation, requireFresh: false), try validatedReceipt(receipt, pair: pair))
    }
    func forget(_ id: UUID) throws { try credentials.remove(id); NotificationCenter.default.post(name:Self.changed,object:nil) }
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
