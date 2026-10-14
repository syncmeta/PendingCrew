import Foundation
import CryptoKit
import X509
import SwiftASN1
import Security

/// Uses Apple's X509/ASN1 implementation and CryptoKit; no custom DER encoder.
enum CrewdRemoteCertificate {
    enum Role { case server, client }
    static func validate(_ data: Data, role: Role, now: Date = Date()) throws -> (Certificate, String) {
        guard !data.isEmpty, data.count <= 65536 else { throw CrewdRemoteError.wrongIdentity }
        let certificate = try Certificate(derEncoded: Array(data))
        guard certificate.version == .v3, certificate.subject == certificate.issuer,
              certificate.notValidBefore <= now, now <= certificate.notValidAfter,
              certificate.publicKey.isValidSignature(certificate.signature, for: certificate),
              P256.Signing.PublicKey(certificate.publicKey) != nil,
              let usages = try certificate.extensions.extendedKeyUsage,
              usages.contains(role == .server ? .serverAuth : .clientAuth) else { throw CrewdRemoteError.wrongIdentity }
        if let basic = try certificate.extensions.basicConstraints, basic != .notCertificateAuthority { throw CrewdRemoteError.wrongIdentity }
        // X509 validates the structure first. Preserve the exact DER SPKI node,
        // including algorithm and curve identifiers, rather than EC point bytes.
        let root = try DER.parse(Array(certificate.tbsCertificateBytes))
        guard case .constructed(let children) = root.content else { throw CrewdRemoteError.wrongIdentity }
        let fields = Array(children)
        guard fields.count >= 7, fields[0].identifier.tagClass == .contextSpecific else { throw CrewdRemoteError.wrongIdentity }
        let pin = SHA256.hash(data: Data(fields[6].encodedBytes)).map { String(format: "%02x", $0) }.joined()
        return (certificate, pin)
    }
    static func decode(_ value: String) throws -> Data {
        guard value.utf8.count <= 87384, let data = Data(base64Encoded: value), data.count <= 65536 else { throw CrewdRemoteError.wrongIdentity }
        return data
    }
    static func generate() throws -> (privateKey: Data, certificate: Data) {
        let key = P256.Signing.PrivateKey()
        let name = try DistinguishedName { CommonName("PendingCrew device") }
        let serial = withUnsafeBytes(of: UUID().uuid) { Array($0) }
        let certificate = try Certificate(version: .v3, serialNumber: .init(bytes: serial), publicKey: .init(key.publicKey), notValidBefore: Date().addingTimeInterval(-60), notValidAfter: Date().addingTimeInterval(365 * 24 * 3600), issuer: name, subject: name, extensions: Certificate.Extensions {
            Critical(KeyUsage(digitalSignature: true))
            try ExtendedKeyUsage([.clientAuth])
            Critical(BasicConstraints.notCertificateAuthority)
        }, issuerPrivateKey: .init(key))
        let der = Data(try certificate.serializeAsPEM().derBytes)
        _ = try validate(der, role: .client)
        return (key.derRepresentation, der)
    }
    static func sign<T: Encodable>(_ payload: T, privateKey: Data) throws -> CrewdRemoteEnvelope {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        let bytes = try encoder.encode(payload)
        guard bytes.count <= 65536 else { throw CrewdRemoteError.invalidExchange }
        let signature = try P256.Signing.PrivateKey(derRepresentation: privateKey).signature(for: bytes).derRepresentation
        return CrewdRemoteEnvelope(format: 1, payload: bytes.base64EncodedString(), signature: signature.base64EncodedString())
    }
    static func verify<T: Decodable>(_ envelope: CrewdRemoteEnvelope, certificate: Data, role: Role, expectedPin: String, as: T.Type) throws -> T {
        guard envelope.bounded, let payload = Data(base64Encoded: envelope.payload), payload.count <= 65536,
              let signature = Data(base64Encoded: envelope.signature), signature.count <= 128 else { throw CrewdRemoteError.invalidExchange }
        let (certificate, pin) = try validate(certificate, role: role)
        guard pin == expectedPin, let key = P256.Signing.PublicKey(certificate.publicKey),
              key.isValidSignature(try P256.Signing.ECDSASignature(derRepresentation: signature), for: payload) else { throw CrewdRemoteError.wrongIdentity }
        return try JSONDecoder().decode(T.self, from: payload)
    }
    static func securityIdentity(privateKey: Data, certificate: Data) throws -> SecIdentity {
        let key = try P256.Signing.PrivateKey(derRepresentation: privateKey)
        let (parsed, _) = try validate(certificate, role: .client)
        guard parsed.publicKey == Certificate.PublicKey(key.publicKey), let cert = SecCertificateCreateWithData(nil, certificate as CFData) else { throw CrewdRemoteError.wrongIdentity }
        let attributes = [kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom, kSecAttrKeyClass: kSecAttrKeyClassPrivate, kSecAttrKeySizeInBits: 256] as CFDictionary
        var error: Unmanaged<CFError>?
        guard let secKey = SecKeyCreateWithData(key.x963Representation as CFData, attributes, &error),
              let identity = SecIdentityCreate(nil, cert, secKey) else { throw CrewdRemoteError.unavailableCredential }
        return identity
    }
}
