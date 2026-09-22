#if os(macOS)
import CryptoKit
import Foundation
import Network
import Security

// MARK: - Persistent pairing identity

/// Long-lived identity for this installation.
///
/// The signing key is the stable public identity used by pairing.  The first transport batch uses
/// a unique 256-bit PSK per peer for TLS authentication; retaining the signing key here lets a later
/// QR/Bonjour pairing flow authenticate key exchange without changing the on-disk identity model.
struct PairingDeviceIdentity: Codable, Equatable {
    let privateSigningKey: Data

    var publicSigningKey: Data {
        // Construction/decoding validates the raw key.  Keep this property non-throwing so callers
        // cannot accidentally substitute a generated identity when a stored identity is corrupt.
        (try? Curve25519.Signing.PrivateKey(rawRepresentation: privateSigningKey)
            .publicKey.rawRepresentation) ?? Data()
    }

    var id: String { Self.deviceID(for: publicSigningKey) }

    static func generate() -> PairingDeviceIdentity {
        PairingDeviceIdentity(
            uncheckedPrivateSigningKey: Curve25519.Signing.PrivateKey().rawRepresentation)
    }

    static func deviceID(for publicKey: Data) -> String {
        SHA256.hash(data: publicKey).map { String(format: "%02x", $0) }.joined()
    }

    private init(uncheckedPrivateSigningKey: Data) {
        privateSigningKey = uncheckedPrivateSigningKey
    }

    private enum CodingKeys: String, CodingKey { case privateSigningKey }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let raw = try container.decode(Data.self, forKey: .privateSigningKey)
        guard (try? Curve25519.Signing.PrivateKey(rawRepresentation: raw)) != nil else {
            throw PairingStoreError.invalidLocalIdentity
        }
        privateSigningKey = raw
    }
}

enum PairingStoreError: Error, Equatable, CustomStringConvertible {
    case invalidLocalIdentity
    case invalidTrustRecord(backendID: String)
    case duplicateBackendID(String)
    case keychain(OSStatus)

    var description: String {
        switch self {
        case .invalidLocalIdentity:
            return "本机设备身份损坏；已拒绝换一个身份继续连接"
        case let .invalidTrustRecord(id):
            return "对端信任记录无效：\(id)"
        case let .duplicateBackendID(id):
            return "同一后端有多份信任记录：\(id)"
        case let .keychain(status):
            let detail = SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)"
            return "设备身份钥匙串操作失败：\(detail)"
        }
    }
}

enum DeviceIdentityStore {
    private static let keychainService = "com.pendingname.pendingcrew.pairing"
    private static let keychainAccount = "device-signing-key-v1"

    /// Production identity store.  The private key is device-local and non-synchronizing; a
    /// corrupt item fails closed instead of being replaced and silently invalidating every peer.
    static func loadOrCreate() throws -> PairingDeviceIdentity {
        if let identity = try loadFromKeychain() { return identity }

        let identity = PairingDeviceIdentity.generate()
        let encoded = try JSONEncoder().encode(identity)
        let add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: keychainAccount,
            kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: encoded,
        ]
        let status = SecItemAdd(add as CFDictionary, nil)
        if status == errSecDuplicateItem {
            // App and daemon can race on first launch.  The winner is authoritative; the loser
            // reloads it instead of overwriting it with a second identity.
            guard let stored = try loadFromKeychain() else {
                throw PairingStoreError.keychain(status)
            }
            return stored
        }
        guard status == errSecSuccess else { throw PairingStoreError.keychain(status) }
        return identity
    }

    private static func loadFromKeychain() throws -> PairingDeviceIdentity? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: keychainAccount,
            kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
            kSecReturnData as String: kCFBooleanTrue as Any,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else {
            throw PairingStoreError.keychain(status)
        }
        return try JSONDecoder().decode(PairingDeviceIdentity.self, from: data)
    }

    /// Injectable file-backed store for deterministic tests and explicit migration tooling.
    /// Production callers use the Keychain overload above.
    static func loadOrCreate(at url: URL) throws -> PairingDeviceIdentity {
        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        chmod(directory.path, 0o700)

        // App and daemon may both be the first process after install.  Serialize the read/create
        // decision; otherwise each can persist a different identity and immediately invalidate the
        // other's pairing records.
        let lockURL = url.appendingPathExtension("lock")
        let fd = open(lockURL.path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        defer { flock(fd, LOCK_UN); Darwin.close(fd) }
        guard flock(fd, LOCK_EX) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        chmod(lockURL.path, 0o600)

        if FileManager.default.fileExists(atPath: url.path) {
            let identity = try JSONDecoder().decode(
                PairingDeviceIdentity.self, from: Data(contentsOf: url))
            guard identity.privateSigningKey.count == 32,
                  identity.publicSigningKey.count == 32 else {
                throw PairingStoreError.invalidLocalIdentity
            }
            chmod(url.path, 0o600)
            return identity
        }

        let identity = PairingDeviceIdentity.generate()
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try MultiProcessJSONStore.writeStaged(encoder.encode(identity), to: url)
        guard chmod(url.path, 0o600) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        return identity
    }
}

/// One explicit peer grant.  The PSK is unique to this pair; possession authenticates both ends of
/// TLS 1.2, while the public key and derived device ID give the pairing UI a stable human-verifiable
/// identity independent of address changes.
struct PeerTrustRecord: Codable, Equatable {
    var backendID: String
    var peerDeviceID: String
    var peerPublicSigningKey: Data
    var preSharedKey: Data

    init(backendID: String, peerDeviceID: String, peerPublicSigningKey: Data,
         preSharedKey: Data) {
        self.backendID = backendID
        self.peerDeviceID = peerDeviceID
        self.peerPublicSigningKey = peerPublicSigningKey
        self.preSharedKey = preSharedKey
    }

    var isValid: Bool {
        !backendID.isEmpty
            && peerPublicSigningKey.count == 32
            && preSharedKey.count == 32
            && peerDeviceID == PairingDeviceIdentity.deviceID(for: peerPublicSigningKey)
    }

    static func randomPreSharedKey() -> Data {
        var generator = SystemRandomNumberGenerator()
        return Data((0..<32).map { _ in UInt8.random(in: .min ... .max, using: &generator) })
    }
}

enum PeerTrustStore {
    static func load(from url: URL) throws -> [PeerTrustRecord] {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch let error as NSError
            where error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError {
            return []
        }
        let records = try JSONDecoder().decode([PeerTrustRecord].self, from: data)
        try validate(records)
        return records
    }

    static func save(_ records: [PeerTrustRecord], to url: URL) throws {
        try validate(records)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try MultiProcessJSONStore.writeStaged(encoder.encode(records), to: url)
        chmod(url.deletingLastPathComponent().path, 0o700)
        guard chmod(url.path, 0o600) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
    }

    private static func validate(_ records: [PeerTrustRecord]) throws {
        var ids = Set<String>()
        for record in records {
            guard record.isValid else {
                throw PairingStoreError.invalidTrustRecord(backendID: record.backendID)
            }
            guard ids.insert(record.backendID).inserted else {
                throw PairingStoreError.duplicateBackendID(record.backendID)
            }
        }
    }
}

enum DevicePairingPaths {
    static var directory: URL { PendingCrewDataRoot.subdirectory("pairing") }
    static var trustedPeers: URL { directory.appendingPathComponent("trusted-peers.json") }
}

// MARK: - TLS connection

struct SecureConnectionParameters: Equatable {
    var host: String
    var port: UInt16
    var localIdentity: PairingDeviceIdentity
    var peer: PeerTrustRecord
}

enum SecureTransportError: Error, Equatable, CustomStringConvertible {
    case invalidHost
    case invalidPort
    case invalidLocalIdentity
    case invalidPeerTrust
    case authenticationFailed
    case dnsFailure(String)
    case networkFailure(String)
    case listenerFailure(String)

    var description: String {
        switch self {
        case .invalidHost: return "远程主机为空"
        case .invalidPort: return "远程端口无效"
        case .invalidLocalIdentity: return "本机设备身份无效"
        case .invalidPeerTrust: return "对端信任记录无效"
        case .authenticationFailed: return "TLS 配对身份认证失败"
        case let .dnsFailure(text): return "域名解析失败：\(text)"
        case let .networkFailure(text): return "网络连接失败：\(text)"
        case let .listenerFailure(text): return "安全监听失败：\(text)"
        }
    }

    fileprivate static func classify(_ error: NWError) -> SecureTransportError {
        switch error {
        case .tls:
            // This transport has no certificate fallback: the only TLS credential is the pairing
            // PSK.  A TLS alert therefore means the paired credential did not authenticate.
            return .authenticationFailed
        case let .dns(value): return .dnsFailure(String(describing: value))
        case let .posix(value): return .networkFailure(String(cString: strerror(value.rawValue)))
        case let .wifiAware(value): return .networkFailure(String(describing: value))
        @unknown default: return .networkFailure(String(describing: error))
        }
    }
}

private enum SecureTLSOptions {
    static let applicationProtocol = "pendingcrew-session/1"
    static let cipherSuite = tls_ciphersuite_t(
        rawValue: TLS_ECDHE_PSK_WITH_CHACHA20_POLY1305_SHA256)!

    static func parameters(keys: [(identity: String, key: Data)]) -> NWParameters {
        let tls = NWProtocolTLS.Options()
        let options = tls.securityProtocolOptions
        // Apple's Network.framework PSK support is intentionally limited to TLS 1.2.  Pin both
        // ends to that supported mode: asking for TLS 1.3 silently leaves the server on the
        // certificate path (`NO_CERTIFICATE_SET`) instead of negotiating the configured PSK.
        sec_protocol_options_set_min_tls_protocol_version(options, .TLSv12)
        sec_protocol_options_set_max_tls_protocol_version(options, .TLSv12)
        // Do not accept the non-forward-secret PSK suites.  This is the Apple-supported TLS 1.2
        // ECDHE-PSK AEAD suite (RFC 7905): ephemeral key agreement + ChaCha20-Poly1305.
        sec_protocol_options_append_tls_ciphersuite(options, cipherSuite)
        sec_protocol_options_set_tls_resumption_enabled(options, false)
        sec_protocol_options_set_tls_tickets_enabled(options, false)
        sec_protocol_options_set_peer_authentication_required(options, false)
        sec_protocol_options_add_tls_application_protocol(options, applicationProtocol)
        for entry in keys {
            let key = dispatchData(entry.key)
            let identity = dispatchData(Data(entry.identity.utf8))
            sec_protocol_options_add_pre_shared_key(options, key, identity)
        }

        let tcp = NWProtocolTCP.Options()
        tcp.enableKeepalive = true
        tcp.keepaliveIdle = 10
        tcp.keepaliveInterval = 5
        tcp.keepaliveCount = 3
        tcp.noDelay = true
        return NWParameters(tls: tls, tcp: tcp)
    }

    private static func dispatchData(_ data: Data) -> dispatch_data_t {
        let value = data.withUnsafeBytes { DispatchData(bytes: $0) }
        return value._bridgeToObjectiveC()
    }
}

/// A connected TLS 1.2 PSK byte stream backed by Network.framework.
///
/// No plaintext or local-socket fallback exists in this type.  Failed DNS, TCP, or PSK negotiation
/// remains a failure and is surfaced through `failure`.
@MainActor
final class SecureTCPTransport: SessionMessageLink {
    var onReceive: ((Data) -> Void)?
    var onClose: (() -> Void)?
    private(set) var isOpen = false
    let isSynchronous = false
    private(set) var pendingWriteBytes = 0
    private(set) var failure: SecureTransportError?
    private(set) var negotiatedCipherSuite: UInt16?
    var terminalErrorDescription: String? { failure?.description }

    private let connection: NWConnection
    private let queue: DispatchQueue
    private var locallyClosed = false
    private var terminalDelivered = false
    fileprivate var onReady: (() -> Void)?
    fileprivate var onTerminal: (() -> Void)?

    private init(connection: NWConnection, label: String) {
        self.connection = connection
        queue = DispatchQueue(label: "pendingcrew.secure-tcp." + label)
    }

    static func connect(parameters: SecureConnectionParameters) throws -> SecureTCPTransport {
        let host = parameters.host.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !host.isEmpty else { throw SecureTransportError.invalidHost }
        guard parameters.port != 0,
              let port = NWEndpoint.Port(rawValue: parameters.port) else {
            throw SecureTransportError.invalidPort
        }
        guard parameters.localIdentity.privateSigningKey.count == 32,
              parameters.localIdentity.publicSigningKey.count == 32 else {
            throw SecureTransportError.invalidLocalIdentity
        }
        guard parameters.peer.isValid else { throw SecureTransportError.invalidPeerTrust }

        let networkParameters = SecureTLSOptions.parameters(keys: [
            (parameters.localIdentity.id, parameters.peer.preSharedKey)
        ])
        let connection = NWConnection(host: NWEndpoint.Host(host), port: port,
                                      using: networkParameters)
        let link = SecureTCPTransport(connection: connection, label: "client")
        link.installHandlersAndStart()
        return link
    }

    fileprivate static func accepted(_ connection: NWConnection) -> SecureTCPTransport {
        SecureTCPTransport(connection: connection, label: "server")
    }

    func send(_ framed: Data) {
        guard !locallyClosed, failure == nil, !framed.isEmpty else { return }
        pendingWriteBytes += framed.count
        let byteCount = framed.count
        connection.send(content: framed, completion: .contentProcessed { [weak self] error in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.pendingWriteBytes = max(0, self.pendingWriteBytes - byteCount)
                    if let error { self.finish(with: .classify(error), notifyPeerClose: true) }
                }
            }
        })
    }

    /// Active close follows `SessionMessageLink`: it does not invoke this side's `onClose`.
    func close() {
        guard !locallyClosed else { return }
        locallyClosed = true
        isOpen = false
        connection.cancel()
        if !terminalDelivered {
            terminalDelivered = true
            onTerminal?()
        }
    }

    fileprivate func installHandlersAndStart() {
        connection.stateUpdateHandler = { [weak self] state in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.handle(state) }
            }
        }
        connection.start(queue: queue)
        receiveNext()
    }

    private func receiveNext() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 256 * 1024) {
            [weak self] data, _, isComplete, error in
            if let data, !data.isEmpty {
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        guard let self, !self.locallyClosed, self.failure == nil else { return }
                        self.onReceive?(data)
                    }
                }
            }
            if let error {
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        self?.finish(with: .classify(error), notifyPeerClose: true)
                    }
                }
            } else if isComplete {
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        self?.finish(with: nil, notifyPeerClose: true)
                    }
                }
            } else {
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { self?.receiveNext() }
                }
            }
        }
    }

    private func handle(_ state: NWConnection.State) {
        guard !locallyClosed else { return }
        switch state {
        case .ready:
            if let metadata = connection.metadata(definition: NWProtocolTLS.definition)
                as? NWProtocolTLS.Metadata {
                negotiatedCipherSuite = sec_protocol_metadata_get_negotiated_tls_ciphersuite(
                    metadata.securityProtocolMetadata).rawValue
            }
            isOpen = true
            onReady?()
        case let .failed(error):
            // A TLS authentication failure happens before `.ready`, but it is still a terminal link
            // failure that the viewer must see and retry.  Suppressing `onClose` while `isOpen` is
            // false would leave the production viewer stuck forever in "connecting".
            finish(with: .classify(error), notifyPeerClose: true)
        case .cancelled:
            finish(with: nil, notifyPeerClose: isOpen)
        default:
            break
        }
    }

    private func finish(with error: SecureTransportError?, notifyPeerClose: Bool) {
        guard !terminalDelivered, !locallyClosed else { return }
        terminalDelivered = true
        if let error { failure = error }
        let wasOpen = isOpen
        isOpen = false
        connection.cancel()
        if notifyPeerClose || wasOpen { onClose?() }
        onTerminal?()
    }
}

/// TLS listener for the daemon side.  Only explicitly trusted device IDs are installed as PSKs;
/// unknown devices cannot complete the handshake and never reach `onAccept`.
@MainActor
final class SecureTCPListener {
    var onAccept: ((SecureTCPTransport) -> Void)?
    var onReady: ((UInt16) -> Void)?
    var onFailure: ((SecureTransportError) -> Void)?
    private(set) var port: UInt16?
    private(set) var failure: SecureTransportError?
    private(set) var lastConnectionFailure: SecureTransportError?

    private let listener: NWListener
    private let queue = DispatchQueue(label: "pendingcrew.secure-tcp.listener")
    private var pending: [ObjectIdentifier: SecureTCPTransport] = [:]
    private var started = false

    init(localIdentity: PairingDeviceIdentity, trustedPeers: [PeerTrustRecord], port: UInt16) throws {
        guard localIdentity.privateSigningKey.count == 32,
              localIdentity.publicSigningKey.count == 32 else {
            throw SecureTransportError.invalidLocalIdentity
        }
        guard !trustedPeers.isEmpty, trustedPeers.allSatisfy(\.isValid) else {
            throw SecureTransportError.invalidPeerTrust
        }
        let identities = Set(trustedPeers.map(\.peerDeviceID))
        guard identities.count == trustedPeers.count else {
            throw SecureTransportError.invalidPeerTrust
        }
        let parameters = SecureTLSOptions.parameters(keys: trustedPeers.map {
            ($0.peerDeviceID, $0.preSharedKey)
        })
        guard let endpointPort = NWEndpoint.Port(rawValue: port) else {
            throw SecureTransportError.invalidPort
        }
        do {
            listener = try NWListener(using: parameters, on: endpointPort)
        } catch {
            throw SecureTransportError.listenerFailure(String(describing: error))
        }
        installHandlers()
    }

    func start() {
        guard !started else { return }
        started = true
        listener.start(queue: queue)
    }

    func close() {
        listener.cancel()
        let links = Array(pending.values)
        pending.removeAll()
        links.forEach { $0.close() }
        port = nil
    }

    private func installHandlers() {
        listener.stateUpdateHandler = { [weak self] state in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    switch state {
                    case .ready:
                        self.port = self.listener.port?.rawValue
                        if let port = self.port { self.onReady?(port) }
                    case let .failed(error):
                        self.failure = .listenerFailure(String(describing: error))
                        self.port = nil
                        if let failure = self.failure { self.onFailure?(failure) }
                    case .cancelled:
                        self.port = nil
                    default:
                        break
                    }
                }
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { connection.cancel(); return }
                    let link = SecureTCPTransport.accepted(connection)
                    let id = ObjectIdentifier(link)
                    self.pending[id] = link
                    link.onReady = { [weak self, weak link] in
                        guard let self, let link else { return }
                        self.onAccept?(link)
                    }
                    link.onTerminal = { [weak self, weak link] in
                        guard let self else { return }
                        if let failure = link?.failure { self.lastConnectionFailure = failure }
                        self.pending.removeValue(forKey: id)
                    }
                    // Install ownership and callbacks before the connection can become ready or
                    // fail.  A loopback handshake may complete within one main-runloop turn.
                    link.installHandlersAndStart()
                }
            }
        }
    }
}
#endif
