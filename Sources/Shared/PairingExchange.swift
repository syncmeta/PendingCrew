import Foundation

/// Cross-platform wire format for the manual two-Mac pairing exchange.
///
/// The next iOS backend batch can reuse these Codable envelopes and text codec without importing
/// AppKit, Keychain storage, or the macOS daemon.  Long-lived private keys are deliberately absent:
/// invitations carry the inviter's public key and one pair-scoped PSK; responses carry only public
/// binding data and a signature.
struct ManualPairingInvitationPayload: Codable, Equatable {
    static let currentSchemaVersion = 1

    var schemaVersion: Int
    var invitationID: String
    var inviterDeviceID: String
    var inviterPublicSigningKey: Data
    var displayName: String
    var remoteURL: String
    var issuedAt: Date
    var expiresAt: Date
    var preSharedKey: Data
}

struct ManualPairingInvitationEnvelope: Codable, Equatable {
    var payload: ManualPairingInvitationPayload
    var signature: Data
}

struct ManualPairingResponsePayload: Codable, Equatable {
    static let currentSchemaVersion = 1

    var schemaVersion: Int
    var invitationID: String
    /// SHA-256 of the signed invitation envelope.  This binds the response to one exact bearer
    /// invitation rather than merely to a user-visible ID.
    var invitationDigest: Data
    var targetDeviceID: String
    var joinerDeviceID: String
    var joinerPublicSigningKey: Data
}

struct ManualPairingResponseEnvelope: Codable, Equatable {
    var payload: ManualPairingResponsePayload
    var signature: Data
}

enum ManualPairingTextKind: Equatable {
    case invitation(ManualPairingInvitationEnvelope)
    case response(ManualPairingResponseEnvelope)
}

enum ManualPairingError: Error, Equatable, CustomStringConvertible {
    case malformedText
    case unsupportedVersion(Int)
    case invalidInvitation
    case invalidResponse
    case invalidSignature
    case invalidEndpoint(String)
    case expiredInvitation(String)
    case replayedInvitation(String)
    case replayedResponse(String)
    case wrongTarget(expected: String, actual: String)
    case responseNotPending(String)
    case trustConflict(String)
    case storage(String)

    var description: String {
        switch self {
        case .malformedText: return "配对文本解不开或类型不对"
        case let .unsupportedVersion(version): return "不支持的配对格式版本：\(version)"
        case .invalidInvitation: return "配对邀请字段无效"
        case .invalidResponse: return "配对回应字段无效"
        case .invalidSignature: return "配对签名校验失败"
        case let .invalidEndpoint(value): return "配对地址无效：\(value)"
        case let .expiredInvitation(id): return "配对邀请已过期：\(id)"
        case let .replayedInvitation(id): return "配对邀请已经用过：\(id)"
        case let .replayedResponse(id): return "配对回应已经用过：\(id)"
        case let .wrongTarget(expected, actual):
            return "配对回应发给了别的设备（应为 \(expected)，实际 \(actual)）"
        case let .responseNotPending(id): return "本机没有等待这条配对回应：\(id)"
        case let .trustConflict(id): return "设备 \(id) 已有不同的信任记录，拒绝覆盖"
        case let .storage(reason): return "配对资料无法安全落盘：\(reason)"
        }
    }
}

enum ManualPairingTextCodec {
    static let invitationPrefix = "pendingcrew-pair-invite-v1:"
    static let responsePrefix = "pendingcrew-pair-response-v1:"

    static func encodeInvitation(_ value: ManualPairingInvitationEnvelope) throws -> String {
        invitationPrefix + (try canonicalData(value)).base64EncodedString()
    }

    static func encodeResponse(_ value: ManualPairingResponseEnvelope) throws -> String {
        responsePrefix + (try canonicalData(value)).base64EncodedString()
    }

    static func decodeInvitation(_ text: String) throws -> ManualPairingInvitationEnvelope {
        try decode(text, prefix: invitationPrefix, as: ManualPairingInvitationEnvelope.self)
    }

    static func decodeResponse(_ text: String) throws -> ManualPairingResponseEnvelope {
        try decode(text, prefix: responsePrefix, as: ManualPairingResponseEnvelope.self)
    }

    static func decodeKind(_ text: String) throws -> ManualPairingTextKind {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix(invitationPrefix) {
            return .invitation(try decodeInvitation(trimmed))
        }
        if trimmed.hasPrefix(responsePrefix) {
            return .response(try decodeResponse(trimmed))
        }
        throw ManualPairingError.malformedText
    }

    static func canonicalData<T: Encodable>(_ value: T) throws -> Data {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        encoder.outputFormatting = [.sortedKeys]
        return try encoder.encode(value)
    }

    private static func decode<T: Decodable>(
        _ text: String, prefix: String, as type: T.Type
    ) throws -> T {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix(prefix),
              let data = Data(base64Encoded: String(trimmed.dropFirst(prefix.count))) else {
            throw ManualPairingError.malformedText
        }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        do { return try decoder.decode(type, from: data) }
        catch { throw ManualPairingError.malformedText }
    }
}
