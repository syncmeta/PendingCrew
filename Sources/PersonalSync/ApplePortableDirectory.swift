import Foundation
import Darwin

/// Closed allowlist; never decode StoredPair or arbitrary metadata into this type.
struct ApplePortableHost: Codable, Equatable, Sendable {
    let displayName: String
    let endpoint: URL
    let transportAuthorityID: String
    let databaseID: String
    let verifiedSPKI: String
    init(displayName: String, endpoint: URL, transportAuthorityID: String, databaseID: String, verifiedSPKI: String) throws {
        guard let c = URLComponents(url: endpoint, resolvingAgainstBaseURL: false), c.scheme == "tls",
              let port=c.port,(1...65535).contains(port),
              c.user == nil, c.password == nil, c.query == nil, c.fragment == nil,
              c.path.isEmpty || c.path == "/", let raw = c.host else { throw AppleSyncFailure.invalid }
        let host = raw.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        guard Self.portableHost(host),
              !displayName.isEmpty, displayName.utf8.count <= 128,
              !transportAuthorityID.isEmpty, transportAuthorityID.utf8.count <= 512,
              !databaseID.isEmpty, databaseID.utf8.count <= 512,
              verifiedSPKI.count == 64, verifiedSPKI.allSatisfy({ "0123456789abcdef".contains($0) }) else { throw AppleSyncFailure.invalid }
        self.displayName = displayName; self.endpoint = endpoint; self.transportAuthorityID = transportAuthorityID
        self.databaseID = databaseID; self.verifiedSPKI = verifiedSPKI
    }
    private static func portableHost(_ host: String) -> Bool {
        guard !host.isEmpty, host != "localhost", !host.hasSuffix(".localhost"), !host.hasSuffix("%"), !host.contains("%") else { return false }
        var ipv4 = in_addr()
        if inet_pton(AF_INET, host, &ipv4) == 1 {
            let address = UInt32(bigEndian: ipv4.s_addr)
            return address != 0 && address >> 24 != 127 && address >> 28 < 14
        }
        var ipv6 = in6_addr()
        if inet_pton(AF_INET6, host, &ipv6) == 1 {
            let bytes = withUnsafeBytes(of: ipv6) { Array($0) }
            if bytes.allSatisfy({ $0 == 0 }) || (bytes.prefix(15).allSatisfy({ $0 == 0 }) && bytes[15] == 1) || bytes[0] == 0xff { return false }
            if bytes.prefix(10).allSatisfy({ $0 == 0 }) && bytes[10] == 0xff && bytes[11] == 0xff {
                return bytes[12] != 127 && !bytes.suffix(4).allSatisfy({ $0 == 0 }) && bytes[12] < 224
            }
            return true
        }
        // DNS/LAN names are hints only; enrollment validates actual reachability and TLS/SPKI.
        guard !host.contains(":"), !host.hasSuffix("."),
              !host.split(separator: ".").allSatisfy({ Int($0) != nil || ($0.hasPrefix("0x") && UInt64($0.dropFirst(2), radix: 16) != nil) }) else { return false }
        return host.split(separator: ".", omittingEmptySubsequences: false).allSatisfy { label in
            !label.isEmpty && label.count <= 63 && label.first != "-" && label.last != "-" &&
            label.utf8.allSatisfy { (97...122).contains($0) || (48...57).contains($0) || $0 == 45 }
        }
    }
    private enum CodingKeys: String, CodingKey { case displayName, endpoint, transportAuthorityID, databaseID, verifiedSPKI }
    private struct AnyKey: CodingKey { let stringValue: String; var intValue: Int? { nil }; init?(stringValue: String) { self.stringValue = stringValue }; init?(intValue: Int) { return nil } }
    init(from decoder: Decoder) throws {
        let all = try decoder.container(keyedBy: AnyKey.self)
        let allowed: Set<String> = ["displayName", "endpoint", "transportAuthorityID", "databaseID", "verifiedSPKI"]
        guard all.allKeys.allSatisfy({ allowed.contains($0.stringValue) }) else { throw AppleSyncFailure.invalid }
        let c = try decoder.container(keyedBy: CodingKeys.self)
        try self.init(displayName: c.decode(String.self, forKey: .displayName), endpoint: c.decode(URL.self, forKey: .endpoint), transportAuthorityID: c.decode(String.self, forKey: .transportAuthorityID), databaseID: c.decode(String.self, forKey: .databaseID), verifiedSPKI: c.decode(String.self, forKey: .verifiedSPKI))
    }
}
/// Explicit tiny presentation preferences. KVS is optional, never draft storage.
@MainActor struct AppleLightPreferences {
    enum Theme: String { case system, light, dark }
    let store: NSUbiquitousKeyValueStore
    var theme: Theme {
        get { Theme(rawValue: store.string(forKey: "personal.theme") ?? "") ?? .system }
        nonmutating set { store.set(newValue.rawValue, forKey: "personal.theme") }
    }
    var compactRows: Bool {
        get { store.bool(forKey: "personal.compactRows") }
        nonmutating set { store.set(newValue, forKey: "personal.compactRows") }
    }
}
