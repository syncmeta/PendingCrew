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
/// Candidate credentials remain process-local: no Keychain API or persistent
/// private key file is consulted, including before a computer has been selected.
/// Relaunch deliberately requires pairing again; this is not durable recovery.
final class CrewdRemoteMemoryCredentials: CrewdRemoteCredentialStore {
    private let lock = NSLock()
    private var values: [UUID: CrewdRemoteStoredPair] = [:]
    func records() throws -> [UUID] {
        lock.lock(); defer { lock.unlock() }
        return values.keys.sorted { $0.uuidString < $1.uuidString }
    }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        lock.lock(); defer { lock.unlock() }
        guard let value = values[id] else { throw CrewdRemoteError.unavailableCredential }
        return value
    }
    func save(_ record: CrewdRemoteStoredPair) throws {
        guard try JSONEncoder().encode(record).count <= 524288 else { throw CrewdRemoteError.storageFailure }
        lock.lock(); defer { lock.unlock() }; values[record.id] = record
    }
    func remove(_ id: UUID) throws {
        lock.lock(); defer { lock.unlock() }; values.removeValue(forKey: id)
    }
}
/// One factory covers original AppModel and the retained migration entry points.
/// Candidate builds cannot fall back to the user's system Keychain.
enum CrewdRemoteCredentials {
    private static let isolated = CrewdRemoteMemoryCredentials()
    static func application() -> CrewdRemoteCredentialStore {
        #if PENDINGCREW_NATIVE_CANDIDATE
        return isolated
        #else
        // Explicit isolated fixture opt-in is separate from HOME or data paths.
        if ProcessInfo.processInfo.environment["PENDINGCREW_ISOLATED_CREDENTIALS"] == "memory" { return isolated }
        return CrewdRemoteKeychain()
        #endif
    }
}
/// A new, non-synchronizing Keychain namespace. Legacy PSK entries are never read.
final class CrewdRemoteKeychain: CrewdRemoteCredentialStore {
    private let service: String
    init(service: String? = nil) {
        if let service { self.service = service }
        else {
            #if PENDINGCREW_NATIVE_CANDIDATE
            let ref = Bundle.main.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String
            let qualified = ref.flatMap { $0.count == 40 && $0.allSatisfy({ "0123456789abcdef".contains($0) }) ? $0 : nil }
            self.service = "com.pendingname.pendingcrew.native-candidate." + (qualified ?? "unqualified") + ".crewd.remote.v1"
            #else
            self.service = "com.pendingname.pendingcrew.crewd.remote.v1"
            #endif
        }
    }
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
