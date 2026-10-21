import Foundation
import Security
import Dispatch

/// One process-wide, non-main serial executor owns every credential store.
/// Synchronous Security calls may wait for the user without blocking AppKit/UI.
@globalActor actor CrewdCredentialActor {
    static let shared = CrewdCredentialActor()
    private init() {}
    nonisolated private let queue = DispatchSerialQueue(label: "pendingcrew.crewd.credentials", qos: .userInitiated)
    nonisolated var unownedExecutor: UnownedSerialExecutor { queue.asUnownedSerialExecutor() }
}

/// No Debug conformance: this envelope includes the device's private credential.
struct CrewdRemoteStoredPair: Codable {
    let id: UUID
    let invitation: CrewdRemoteEnvelope
    let response: CrewdRemoteEnvelope
    let privateKey: Data
    let clientCertificate: Data
    var receipt: CrewdRemoteEnvelope?
}
@CrewdCredentialActor protocol CrewdRemoteCredentialStore {
    func records() throws -> [UUID]
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair
    func save(_ record: CrewdRemoteStoredPair) throws
    func remove(_ id: UUID) throws
}
/// Candidate credentials remain process-local: no Keychain API or persistent
/// private key file is consulted, including before a computer has been selected.
/// Relaunch deliberately requires pairing again; this is not durable recovery.
@CrewdCredentialActor final class CrewdRemoteMemoryCredentials: CrewdRemoteCredentialStore {
    private var values: [UUID: CrewdRemoteStoredPair] = [:]
    func records() throws -> [UUID] {
        return values.keys.sorted { $0.uuidString < $1.uuidString }
    }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        guard let value = values[id] else { throw CrewdRemoteError.unavailableCredential }
        return value
    }
    func save(_ record: CrewdRemoteStoredPair) throws {
        guard try JSONEncoder().encode(record).count <= 524288 else { throw CrewdRemoteError.storageFailure }
        values[record.id] = record
    }
    func remove(_ id: UUID) throws {
        values.removeValue(forKey: id)
    }
}
/// One factory covers original AppModel and the retained migration entry points.
/// Candidate builds cannot fall back to the user's system Keychain.
@CrewdCredentialActor enum CrewdRemoteCredentials {
    private static let isolated = CrewdRemoteMemoryCredentials()
    static func application() -> CrewdRemoteCredentialStore {
        #if PENDINGCREW_LOCAL_MAC && !PENDINGCREW_REMOTE_PRODUCT
        // The default local build never opens its vault. The explicit production
        // remote overlay uses the existing persistent path below.
        return CrewdRefusedCredentials()
        #elseif PENDINGCREW_KNOWLEDGE_VALIDATION
        return isolated
        #elseif PENDINGCREW_DELIVERY_VALIDATION
        guard let scope = try? CrewdDeliveryValidation.qualified() else { return CrewdRefusedCredentials() }
        return CrewdValidationCredentials(scope: scope)
        #elseif PENDINGCREW_NATIVE_CANDIDATE
        return isolated
        #else
        // Explicit isolated fixture opt-in is separate from HOME or data paths.
        if ProcessInfo.processInfo.environment["PENDINGCREW_ISOLATED_CREDENTIALS"] == "memory" { return isolated }
        guard let service = keychainService(bundleIdentifier: Bundle.main.bundleIdentifier) else {
            return CrewdRefusedCredentials()
        }
        return CrewdRemoteKeychain(service: service)
        #endif
    }
    /// Each actual App identity owns its persistent credentials. Missing or
    /// malformed bundle metadata must never open the production App's vault.
    nonisolated static func keychainService(bundleIdentifier: String?) -> String? {
        guard let identifier = bundleIdentifier, !identifier.isEmpty,
              identifier.utf8.count <= 255, identifier.contains("."),
              !identifier.hasPrefix("."), !identifier.hasSuffix("."),
              !identifier.contains("..") else { return nil }
        for byte in identifier.utf8 {
            guard (48...57).contains(byte) || (65...90).contains(byte) ||
                  (97...122).contains(byte) || byte == 45 || byte == 46 else { return nil }
        }
        return identifier + ".crewd.remote.v1"
    }
}
/// A new, non-synchronizing Keychain namespace. Legacy PSK entries are never read.
@CrewdCredentialActor final class CrewdRemoteKeychain: CrewdRemoteCredentialStore {
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
    nonisolated static func canonicalAccount(_ id: UUID) -> String { id.uuidString.lowercased() }
    nonisolated static func matchesReturnedAccount(_ result: Any?, id: UUID) -> Bool {
        guard let attributes = result as? [String: Any] else { return false }
        return attributes[kSecAttrAccount as String] as? String == canonicalAccount(id)
    }
    private func query(_ id: UUID? = nil) -> [String: Any] {
        var query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrSynchronizable as String: false]
        if let id { query[kSecAttrAccount as String] = Self.canonicalAccount(id) }
        return query
    }
    /// Exact approved accounts only; never a service-wide credential enumeration.
    func boundedRecords(_ ids: [UUID]) throws -> [UUID] {
        var present: [UUID] = []
        for id in ids {
            var request = query(id); request[kSecReturnAttributes as String] = true; request[kSecMatchLimit as String] = kSecMatchLimitOne
            var result: CFTypeRef?
            let status = SecItemCopyMatching(request as CFDictionary, &result)
            if status == errSecItemNotFound { continue }
            guard status == errSecSuccess, Self.matchesReturnedAccount(result, id: id) else { throw CrewdRemoteError.unavailableCredential }
            present.append(id)
        }
        return present
    }
    func records() throws -> [UUID] {
        var request = query(); request[kSecReturnAttributes as String] = true; request[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else {
            NSLog("PendingCrew credentials records failed: OSStatus=%d", status)
            throw CrewdRemoteError.unavailableCredential
        }
        guard let attributes = result as? [[String: Any]] else {
            NSLog("PendingCrew credentials records failed: stage=response-attributes")
            throw CrewdRemoteError.unavailableCredential
        }
        return attributes.compactMap { ($0[kSecAttrAccount as String] as? String).flatMap(UUID.init(uuidString:)) }.sorted { $0.uuidString < $1.uuidString }
    }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        var request = query(id); request[kSecReturnData as String] = true; request[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &result)
        guard status == errSecSuccess else {
            NSLog("PendingCrew credentials read failed: OSStatus=%d", status)
            throw CrewdRemoteError.unavailableCredential
        }
        guard let data = result as? Data else {
            NSLog("PendingCrew credentials read failed: stage=response-data")
            throw CrewdRemoteError.unavailableCredential
        }
        guard data.count <= 524288 else {
            NSLog("PendingCrew credentials read failed: stage=data-size")
            throw CrewdRemoteError.unavailableCredential
        }
        guard let record = try? JSONDecoder().decode(CrewdRemoteStoredPair.self, from: data) else {
            NSLog("PendingCrew credentials read failed: stage=record-decode")
            throw CrewdRemoteError.unavailableCredential
        }
        guard record.id == id else {
            NSLog("PendingCrew credentials read failed: stage=record-identity")
            throw CrewdRemoteError.unavailableCredential
        }
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
