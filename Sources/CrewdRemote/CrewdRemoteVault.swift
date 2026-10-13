import Foundation
import Security

/// No Debug conformance: this envelope includes the device's private credential.
struct CrewdRemoteStoredPair: Codable {
    let id: UUID
    let invitation: CrewdRemoteEnvelope
    let response: CrewdRemoteEnvelope
    let privateKey: Data
    let clientCertificate: Data
    var receipt: CrewdRemoteEnvelope?
}
protocol CrewdRemoteCredentialStore {
    func records() throws -> [UUID]
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair
    func save(_ record: CrewdRemoteStoredPair) throws
    func remove(_ id: UUID) throws
}
/// A new, non-synchronizing Keychain namespace. Legacy PSK entries are never read.
final class CrewdRemoteKeychain: CrewdRemoteCredentialStore {
    private let service: String
    init(service: String = "com.pendingname.pendingcrew.crewd.remote.v1") { self.service = service }
    private func query(_ id: UUID? = nil) -> [String: Any] {
        var query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrSynchronizable as String: false]
        if let id { query[kSecAttrAccount as String] = id.uuidString.lowercased() }
        return query
    }
    func records() throws -> [UUID] {
        var request = query(); request[kSecReturnAttributes as String] = true; request[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess, let attributes = result as? [[String: Any]] else { throw CrewdRemoteError.unavailableCredential }
        return attributes.compactMap { ($0[kSecAttrAccount as String] as? String).flatMap(UUID.init(uuidString:)) }.sorted { $0.uuidString < $1.uuidString }
    }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        var request = query(id); request[kSecReturnData as String] = true; request[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(request as CFDictionary, &result) == errSecSuccess, let data = result as? Data,
              data.count <= 524288, let record = try? JSONDecoder().decode(CrewdRemoteStoredPair.self, from: data), record.id == id else { throw CrewdRemoteError.unavailableCredential }
        return record
    }
    func save(_ record: CrewdRemoteStoredPair) throws {
        let data = try JSONEncoder().encode(record)
        guard data.count <= 524288 else { throw CrewdRemoteError.storageFailure }
        let values: [String: Any] = [kSecValueData as String: data, kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly]
        let query = query(record.id)
        let update = SecItemUpdate(query as CFDictionary, values as CFDictionary)
        if update == errSecSuccess { return }
        guard update == errSecItemNotFound else { throw CrewdRemoteError.storageFailure }
        guard SecItemAdd(query.merging(values) { _, new in new } as CFDictionary, nil) == errSecSuccess else { throw CrewdRemoteError.storageFailure }
    }
    func remove(_ id: UUID) throws {
        let status = SecItemDelete(query(id) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw CrewdRemoteError.storageFailure }
    }
}
