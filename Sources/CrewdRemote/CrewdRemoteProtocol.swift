import Foundation

/// Public wire DTOs. Private credentials are deliberately absent from this model.
struct CrewdRemoteEnvelope: Codable, Equatable, Sendable {
    let format: UInt32
    let payload: String
    let signature: String
    var bounded: Bool { format == 1 && !payload.isEmpty && payload.utf8.count <= 131072 && !signature.isEmpty && signature.utf8.count <= 256 }
}
struct CrewdRemotePublicIdentity: Codable, Equatable, Sendable {
    let authorityId: String
    let dbId: String
    let certificateDerBase64: String
    let spkiSha256: String
}
struct CrewdRemoteEndpoint: Codable, Equatable, Sendable {
    let host: String
    let port: UInt16
    var valid: Bool { port != 0 && !host.isEmpty && host.utf8.count <= 255 && host == host.trimmingCharacters(in: .whitespacesAndNewlines) && !CrewdRemoteText.containsControlScalar(host) }
}
struct CrewdRemoteInvitation: Codable, Equatable, Sendable {
    let invitationId: String
    let server: CrewdRemotePublicIdentity
    let endpoint: CrewdRemoteEndpoint
    let challenge: String
    let expiresAt: String
    let computerLabel: String
}
struct CrewdRemoteResponse: Codable, Equatable, Sendable {
    let invitationId: String
    let authorityId: String
    let dbId: String
    let challenge: String
    let clientCertificateDerBase64: String
    let clientSpkiSha256: String
    let deviceLabel: String
}
struct CrewdRemoteReceipt: Codable, Equatable, Sendable {
    let invitationId: String
    let grantId: String
    let authorityId: String
    let dbId: String
    let serverSpkiSha256: String
    let clientSpkiSha256: String
    let grantRevision: String
    let acceptedAt: String
}
enum CrewdRemoteError: LocalizedError {
    case invalidExchange, expiredInvitation, wrongIdentity, unavailableCredential, storageFailure
    var errorDescription: String? {
        switch self {
        case .invalidExchange: return "连接码无效，请重新从电脑复制。"
        case .expiredInvitation: return "配对邀请已过期，请在电脑上重新生成。"
        case .wrongIdentity: return "连接码与这台电脑或设备不一致，请重新配对。"
        case .unavailableCredential: return "设备凭据无法读取，请解锁设备或重新配对。"
        case .storageFailure: return "无法保存设备连接，请检查设备状态后重试。"
        }
    }
}
enum CrewdRemoteText {
    private static let prefix = "pendingcrew-remote-v1:"
    static func encode(_ envelope: CrewdRemoteEnvelope) throws -> String {
        guard envelope.bounded else { throw CrewdRemoteError.invalidExchange }
        return prefix + (try JSONEncoder().encode(envelope)).base64EncodedString()
    }
    static func decode(_ text: String) throws -> CrewdRemoteEnvelope {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.utf8.count <= 262144, text.hasPrefix(prefix),
              let data = Data(base64Encoded: String(text.dropFirst(prefix.count))), data.count <= 196608,
              let envelope = try? JSONDecoder().decode(CrewdRemoteEnvelope.self, from: data), envelope.bounded else { throw CrewdRemoteError.invalidExchange }
        return envelope
    }
    static func date(_ value: String) -> Date? {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return parser.date(from: value) ?? ISO8601DateFormatter().date(from: value)
    }
    static func uuid(_ value: String) -> Bool { UUID(uuidString: value)?.uuidString.lowercased() == value }
    // Store generates lower(hex(randomblob(16))); database identity is opaque
    // and existing meta.dbId is preserved, rather than reformatted as UUID.
    enum DatabaseIDFailure { case empty, utf8Bound, trimChanged, controlScalar }
    enum DatabaseIDValidation {
        case accepted(String)
        case rejected(DatabaseIDFailure)
    }
    // Keep this shared check as an explicit loop. The optimized full App has
    // exhibited different bound-method contains/allSatisfy specialization from
    // the isolated qualifier; direct membership preserves the same character
    // set without depending on that higher-order specialization.
    static func containsControlScalar(_ value: String) -> Bool {
        for scalar in value.unicodeScalars {
            if CharacterSet.controlCharacters.contains(scalar) { return true }
        }
        return false
    }
    /// Same opaque identity policy and short-circuit order as databaseID.
    /// The accepted value is the original string, never a normalized identity.
    /// No raw value, length, scalar or error description is logged here.
    static func validateDatabaseID(_ value: String) -> DatabaseIDValidation {
        guard !value.isEmpty else { return .rejected(.empty) }
        guard value.utf8.count <= 256 else { return .rejected(.utf8Bound) }
        guard value == value.trimmingCharacters(in: .whitespacesAndNewlines) else { return .rejected(.trimChanged) }
        guard !containsControlScalar(value) else { return .rejected(.controlScalar) }
        return .accepted(value)
    }
    static func databaseID(_ value: String) -> Bool {
        if case .accepted = validateDatabaseID(value) { return true }
        return false
    }
    static func revision(_ value: String) -> Bool { Int64(value).map { $0 > 0 && String($0) == value } ?? false }
    static func pin(_ value: String) -> Bool { value.utf8.count == 64 && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) } }
}

struct CrewdRemoteConfiguration: Codable, Equatable {
    let revision: String
    let enabled: Bool
    let listenHost: String
    let port: UInt16
}
struct CrewdRemoteStatus: Codable {
    let configuration: CrewdRemoteConfiguration
    let listening: Bool
    let endpoint: CrewdRemoteEndpoint?
    let identity: CrewdRemotePublicIdentity?
    let failure: String?
}
struct CrewdRemoteGrant: Codable, Identifiable {
    let id: String
    let scope: String
    let deviceLabel: String
    let clientSpkiSha256: String
    let revision: String
    let createdAt: String
    let revokedAt: String?
    let revokeNote: String?
}
struct CrewdRemoteAcceptResult: Codable {
    let requestId: String
    let reused: Bool
    let receipt: CrewdRemoteEnvelope
    let grant: CrewdRemoteGrant
}
