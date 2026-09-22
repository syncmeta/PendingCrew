import CryptoKit
import Darwin
import Foundation

struct ManualPairingPaths: Equatable {
    var trustedPeers: URL
    var backendRegistry: URL
    var exchangeLedger: URL
    var listenerSettings: URL

    static var standard: ManualPairingPaths {
        .init(
            trustedPeers: DevicePairingPaths.trustedPeers,
            backendRegistry: DevicePairingPaths.backendRegistry,
            exchangeLedger: DevicePairingPaths.exchangeLedger,
            listenerSettings: DevicePairingPaths.listenerSettings)
    }
}

struct SessionDaemonSecureListenerSettings: Codable, Equatable {
    static let currentSchemaVersion = 1

    var schemaVersion: Int
    var enabled: Bool
    var port: UInt16

    init(port: UInt16) {
        schemaVersion = Self.currentSchemaVersion
        enabled = true
        self.port = port
    }

    var isValid: Bool {
        schemaVersion == Self.currentSchemaVersion && enabled && port != 0
    }

    static func load(from url: URL = DevicePairingPaths.listenerSettings) throws
        -> SessionDaemonSecureListenerSettings? {
        let data: Data
        do { data = try Data(contentsOf: url) }
        catch let error as NSError
            where error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError {
            return nil
        } catch {
            throw ManualPairingError.storage("监听配置读不出来：\(error.localizedDescription)")
        }
        guard let value = try? JSONDecoder().decode(Self.self, from: data), value.isValid else {
            throw ManualPairingError.storage("监听配置已损坏或版本不受支持")
        }
        return value
    }

    static func save(_ value: SessionDaemonSecureListenerSettings, to url: URL) throws {
        guard value.isValid else {
            throw ManualPairingError.storage("拒绝写入无效监听配置")
        }
        try PairingFileTransaction.withExclusiveFiles([url]) {
            try PairingFileTransaction.commit([
                .init(url: url, data: try encoded(value), mode: 0o600),
            ])
        }
    }

    static func encoded(_ value: SessionDaemonSecureListenerSettings) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try encoder.encode(value)
    }
}

enum ManualPairingImportResult: Equatable {
    case invitationAccepted(response: String, backend: BackendRef)
    case responseAccepted(port: UInt16, peerDeviceID: String)
}

private struct ManualPairingLedger: Codable, Equatable {
    struct PendingInvitation: Codable, Equatable {
        var invitationID: String
        var invitationDigest: Data
        var preSharedKey: Data
        var port: UInt16
        var expiresAt: Date
        var targetDeviceID: String
    }

    var pending: [String: PendingInvitation] = [:]
    var consumedInvitations: [String] = []
    var consumedResponses: [String] = []

    static func load(from url: URL) throws -> ManualPairingLedger {
        let data: Data
        do { data = try Data(contentsOf: url) }
        catch let error as NSError
            where error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError {
            return .init()
        } catch {
            throw ManualPairingError.storage("配对交换账本读不出来：\(error.localizedDescription)")
        }
        do { return try JSONDecoder().decode(Self.self, from: data) }
        catch { throw ManualPairingError.storage("配对交换账本已损坏：\(error)") }
    }

    func encoded() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try encoder.encode(self)
    }
}

struct ManualPairingCoordinator {
    static let invitationLifetime: TimeInterval = 30 * 60

    private let identity: PairingDeviceIdentity
    private let paths: ManualPairingPaths
    private let now: () -> Date
    private let makeInvitationID: () -> String
    private let makePreSharedKey: () -> Data

    init(
        identity: PairingDeviceIdentity,
        paths: ManualPairingPaths,
        now: @escaping () -> Date = Date.init,
        makeInvitationID: @escaping () -> String = { UUID().uuidString.lowercased() },
        makePreSharedKey: @escaping () -> Data = PeerTrustRecord.randomPreSharedKey
    ) {
        self.identity = identity
        self.paths = paths
        self.now = now
        self.makeInvitationID = makeInvitationID
        self.makePreSharedKey = makePreSharedKey
    }

    static func production() throws -> ManualPairingCoordinator {
        ManualPairingCoordinator(
            identity: try DeviceIdentityStore.loadOrCreate(), paths: .standard)
    }

    func createInvitation(displayName: String, remoteURL: String) throws -> String {
        let name = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        let address = remoteURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw ManualPairingError.invalidInvitation }
        let endpoint = try Self.parseEndpoint(address)
        let invitationID = makeInvitationID()
        guard !invitationID.isEmpty else { throw ManualPairingError.invalidInvitation }
        let psk = makePreSharedKey()
        guard psk.count == 32 else { throw ManualPairingError.invalidInvitation }
        let issuedAt = now()
        let payload = ManualPairingInvitationPayload(
            schemaVersion: ManualPairingInvitationPayload.currentSchemaVersion,
            invitationID: invitationID,
            inviterDeviceID: identity.id,
            inviterPublicSigningKey: identity.publicSigningKey,
            displayName: name,
            remoteURL: endpoint.address,
            issuedAt: issuedAt,
            expiresAt: issuedAt.addingTimeInterval(Self.invitationLifetime),
            preSharedKey: psk)
        let envelope = ManualPairingInvitationEnvelope(
            payload: payload, signature: try identity.sign(Self.canonical(payload)))
        let digest = Self.digest(try ManualPairingTextCodec.canonicalData(envelope))

        try PairingFileTransaction.withExclusiveFiles([paths.exchangeLedger]) {
            var ledger = try ManualPairingLedger.load(from: paths.exchangeLedger)
            guard ledger.pending[invitationID] == nil,
                  !ledger.consumedInvitations.contains(invitationID),
                  !ledger.consumedResponses.contains(invitationID) else {
                throw ManualPairingError.replayedInvitation(invitationID)
            }
            ledger.pending[invitationID] = .init(
                invitationID: invitationID, invitationDigest: digest,
                preSharedKey: psk, port: endpoint.port,
                expiresAt: payload.expiresAt, targetDeviceID: identity.id)
            try PairingFileTransaction.commit([
                .init(url: paths.exchangeLedger, data: try ledger.encoded(), mode: 0o600),
            ])
        }
        return try ManualPairingTextCodec.encodeInvitation(envelope)
    }

    func importText(_ text: String) throws -> ManualPairingImportResult {
        switch try ManualPairingTextCodec.decodeKind(text) {
        case let .invitation(envelope): return try acceptInvitation(envelope)
        case let .response(envelope): return try acceptResponse(envelope)
        }
    }

    private func acceptInvitation(
        _ envelope: ManualPairingInvitationEnvelope
    ) throws -> ManualPairingImportResult {
        let payload = envelope.payload
        guard payload.schemaVersion == ManualPairingInvitationPayload.currentSchemaVersion else {
            throw ManualPairingError.unsupportedVersion(payload.schemaVersion)
        }
        guard !payload.invitationID.isEmpty,
              payload.inviterPublicSigningKey.count == 32,
              payload.preSharedKey.count == 32,
              payload.inviterDeviceID == PairingDeviceIdentity.deviceID(
                for: payload.inviterPublicSigningKey),
              payload.inviterDeviceID != identity.id,
              !payload.displayName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              payload.expiresAt > payload.issuedAt else {
            throw ManualPairingError.invalidInvitation
        }
        guard now() <= payload.expiresAt else {
            throw ManualPairingError.expiredInvitation(payload.invitationID)
        }
        guard PairingDeviceIdentity.verify(
            envelope.signature, data: try Self.canonical(payload),
            publicKey: payload.inviterPublicSigningKey) else {
            throw ManualPairingError.invalidSignature
        }
        _ = try Self.parseEndpoint(payload.remoteURL)
        let invitationDigest = Self.digest(
            try ManualPairingTextCodec.canonicalData(envelope))
        let backend = BackendRef(
            id: payload.inviterDeviceID, displayName: payload.displayName,
            transport: .remote(url: payload.remoteURL))
        let trust = PeerTrustRecord(
            backendID: backend.id, peerDeviceID: payload.inviterDeviceID,
            peerPublicSigningKey: payload.inviterPublicSigningKey,
            preSharedKey: payload.preSharedKey)
        let responsePayload = ManualPairingResponsePayload(
            schemaVersion: ManualPairingResponsePayload.currentSchemaVersion,
            invitationID: payload.invitationID,
            invitationDigest: invitationDigest,
            targetDeviceID: payload.inviterDeviceID,
            joinerDeviceID: identity.id,
            joinerPublicSigningKey: identity.publicSigningKey)
        let response = ManualPairingResponseEnvelope(
            payload: responsePayload,
            signature: try identity.sign(Self.canonical(responsePayload)))

        try PairingFileTransaction.withExclusiveFiles([
            paths.trustedPeers, paths.backendRegistry, paths.exchangeLedger,
        ]) {
            var ledger = try ManualPairingLedger.load(from: paths.exchangeLedger)
            guard !ledger.consumedInvitations.contains(payload.invitationID) else {
                throw ManualPairingError.replayedInvitation(payload.invitationID)
            }
            var trusts = try Self.loadTrusts(from: paths.trustedPeers)
            try Self.mergeTrust(trust, into: &trusts)

            var refs = try RemoteBackendRecordStore.load(from: paths.backendRegistry)
            if let existing = refs.first(where: { $0.id == backend.id }) {
                guard existing == backend else {
                    throw ManualPairingError.trustConflict(backend.id)
                }
            } else {
                refs.append(backend)
            }
            ledger.consumedInvitations.append(payload.invitationID)
            try PairingFileTransaction.commit([
                .init(url: paths.trustedPeers,
                      data: try PeerTrustStore.encoded(trusts), mode: 0o600),
                .init(url: paths.backendRegistry,
                      data: try RemoteBackendRecordStore.encoded(refs), mode: 0o600),
                .init(url: paths.exchangeLedger, data: try ledger.encoded(), mode: 0o600),
            ])
        }
        return .invitationAccepted(
            response: try ManualPairingTextCodec.encodeResponse(response), backend: backend)
    }

    private func acceptResponse(
        _ envelope: ManualPairingResponseEnvelope
    ) throws -> ManualPairingImportResult {
        let payload = envelope.payload
        guard payload.schemaVersion == ManualPairingResponsePayload.currentSchemaVersion else {
            throw ManualPairingError.unsupportedVersion(payload.schemaVersion)
        }
        guard !payload.invitationID.isEmpty,
              payload.invitationDigest.count == 32,
              payload.joinerPublicSigningKey.count == 32,
              payload.joinerDeviceID == PairingDeviceIdentity.deviceID(
                for: payload.joinerPublicSigningKey),
              payload.joinerDeviceID != identity.id else {
            throw ManualPairingError.invalidResponse
        }
        guard payload.targetDeviceID == identity.id else {
            throw ManualPairingError.wrongTarget(
                expected: identity.id, actual: payload.targetDeviceID)
        }
        guard PairingDeviceIdentity.verify(
            envelope.signature, data: try Self.canonical(payload),
            publicKey: payload.joinerPublicSigningKey) else {
            throw ManualPairingError.invalidSignature
        }

        var acceptedPort: UInt16 = 0
        try PairingFileTransaction.withExclusiveFiles([
            paths.trustedPeers, paths.exchangeLedger, paths.listenerSettings,
        ]) {
            var ledger = try ManualPairingLedger.load(from: paths.exchangeLedger)
            guard !ledger.consumedResponses.contains(payload.invitationID) else {
                throw ManualPairingError.replayedResponse(payload.invitationID)
            }
            guard let pending = ledger.pending[payload.invitationID] else {
                throw ManualPairingError.responseNotPending(payload.invitationID)
            }
            guard pending.targetDeviceID == identity.id else {
                throw ManualPairingError.wrongTarget(
                    expected: identity.id, actual: pending.targetDeviceID)
            }
            guard pending.invitationDigest == payload.invitationDigest else {
                throw ManualPairingError.invalidResponse
            }
            guard now() <= pending.expiresAt else {
                throw ManualPairingError.expiredInvitation(payload.invitationID)
            }
            let trust = PeerTrustRecord(
                backendID: payload.joinerDeviceID, peerDeviceID: payload.joinerDeviceID,
                peerPublicSigningKey: payload.joinerPublicSigningKey,
                preSharedKey: pending.preSharedKey)
            var trusts = try Self.loadTrusts(from: paths.trustedPeers)
            // A process can be interrupted after trust reached disk but before listener settings
            // and the consumed ledger did.  An identical record is therefore an idempotent
            // continuation, while any different key/PSK remains a fail-closed conflict.
            try Self.mergeTrust(trust, into: &trusts)
            let settings = SessionDaemonSecureListenerSettings(port: pending.port)
            ledger.pending.removeValue(forKey: payload.invitationID)
            ledger.consumedResponses.append(payload.invitationID)
            try PairingFileTransaction.commit([
                .init(url: paths.trustedPeers,
                      data: try PeerTrustStore.encoded(trusts), mode: 0o600),
                // Config comes after trust: a crash cannot enable an unauthenticated listener.
                .init(url: paths.listenerSettings,
                      data: try SessionDaemonSecureListenerSettings.encoded(settings), mode: 0o600),
                // Consumption comes last so an interrupted commit can be retried instead of losing
                // the only response that contains the joiner's public identity.
                .init(url: paths.exchangeLedger, data: try ledger.encoded(), mode: 0o600),
            ])
            acceptedPort = pending.port
        }
        return .responseAccepted(port: acceptedPort, peerDeviceID: payload.joinerDeviceID)
    }

    private static func canonical<T: Encodable>(_ value: T) throws -> Data {
        try ManualPairingTextCodec.canonicalData(value)
    }

    private static func digest(_ data: Data) -> Data { Data(SHA256.hash(data: data)) }

    private static func loadTrusts(from url: URL) throws -> [PeerTrustRecord] {
        do { return try PeerTrustStore.load(from: url) }
        catch { throw ManualPairingError.storage("信任账本读不出来：\(error)") }
    }

    private static func mergeTrust(
        _ candidate: PeerTrustRecord, into records: inout [PeerTrustRecord]
    ) throws {
        let matches = records.filter {
            $0.backendID == candidate.backendID || $0.peerDeviceID == candidate.peerDeviceID
        }
        guard !matches.isEmpty else {
            records.append(candidate)
            return
        }
        guard matches.allSatisfy({ $0 == candidate }) else {
            throw ManualPairingError.trustConflict(candidate.peerDeviceID)
        }
    }

    private static func parseEndpoint(_ raw: String) throws -> (address: String, port: UInt16) {
        guard let components = URLComponents(string: raw),
              components.scheme?.lowercased() == "pendingcrew+tls",
              let host = components.host, !host.isEmpty,
              let rawPort = components.port,
              let port = UInt16(exactly: rawPort), port != 0,
              components.user == nil, components.password == nil,
              components.path.isEmpty || components.path == "/",
              components.query == nil, components.fragment == nil else {
            throw ManualPairingError.invalidEndpoint(raw)
        }
        return (raw, port)
    }
}

extension PairingDeviceIdentity {
    fileprivate func sign(_ data: Data) throws -> Data {
        do {
            return try Curve25519.Signing.PrivateKey(rawRepresentation: privateSigningKey)
                .signature(for: data)
        } catch {
            throw ManualPairingError.storage("本机设备身份无法签名")
        }
    }

    fileprivate static func verify(_ signature: Data, data: Data, publicKey: Data) -> Bool {
        guard let key = try? Curve25519.Signing.PublicKey(rawRepresentation: publicKey) else {
            return false
        }
        return key.isValidSignature(signature, for: data)
    }
}

/// Strict multi-file commit used by pairing.  Every participating file is locked in sorted order,
/// re-read while locked, staged before the first rename, and restored from its exact previous bytes
/// if any rename fails.  This prevents pairing from overwriting a backend/trust record added by a
/// concurrent settings action and avoids returning success with only half the files changed.
enum PairingFileTransaction {
    private struct OriginalFile {
        var data: Data
        var mode: mode_t
    }

    struct Update {
        var url: URL
        var data: Data
        var mode: mode_t
    }

    static func withExclusiveFiles<T>(_ urls: [URL], _ body: () throws -> T) throws -> T {
        let lockURLs = Array(Set(urls.map { $0.appendingPathExtension("lock").path }))
            .sorted().map(URL.init(fileURLWithPath:))
        return try acquire(lockURLs, index: 0, body)
    }

    static func commit(
        _ updates: [Update], beforeRename: ((Int, URL) throws -> Void)? = nil
    ) throws {
        guard Set(updates.map(\.url.path)).count == updates.count else {
            throw ManualPairingError.storage("同一事务重复写同一个文件")
        }
        let fm = FileManager.default
        var originals: [OriginalFile?] = []
        var stages: [URL] = []
        var renamedIndices: [Int] = []
        do {
            for update in updates {
                let directory = update.url.deletingLastPathComponent()
                try fm.createDirectory(at: directory, withIntermediateDirectories: true)
                chmod(directory.path, 0o700)
                if fm.fileExists(atPath: update.url.path) {
                    let attributes = try fm.attributesOfItem(atPath: update.url.path)
                    let mode = (attributes[.posixPermissions] as? NSNumber)?.uint16Value ?? 0o600
                    originals.append(.init(
                        data: try Data(contentsOf: update.url), mode: mode_t(mode)))
                } else {
                    originals.append(nil)
                }
                let stage = directory.appendingPathComponent(
                    ".\(update.url.lastPathComponent).pairing-\(UUID().uuidString).staged")
                try MultiProcessJSONStore.writeStaged(update.data, to: stage)
                guard chmod(stage.path, update.mode) == 0 else {
                    throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                }
                stages.append(stage)
            }
            for index in updates.indices {
                try beforeRename?(index, updates[index].url)
                guard rename(stages[index].path, updates[index].url.path) == 0 else {
                    throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                }
                renamedIndices.append(index)
                guard chmod(updates[index].url.path, updates[index].mode) == 0 else {
                    throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                }
            }
        } catch {
            var rollbackError: Error?
            // Staging failures have not modified any destination.  Rename failures only require
            // restoring the prefix that actually crossed the atomic rename boundary; touching a
            // later destination would corrupt an old file whose original bytes were never read.
            for index in renamedIndices.reversed() {
                do {
                    if let original = originals[index] {
                        try MultiProcessJSONStore.writeStaged(
                            original.data, to: updates[index].url)
                        chmod(updates[index].url.path, original.mode)
                    } else if fm.fileExists(atPath: updates[index].url.path) {
                        try fm.removeItem(at: updates[index].url)
                    }
                } catch { rollbackError = rollbackError ?? error }
            }
            for stage in stages { try? fm.removeItem(at: stage) }
            if let rollbackError {
                throw ManualPairingError.storage(
                    "事务失败且回滚失败：\(error.localizedDescription) / \(rollbackError.localizedDescription)")
            }
            throw error
        }
    }

    private static func acquire<T>(
        _ locks: [URL], index: Int, _ body: () throws -> T
    ) throws -> T {
        guard index < locks.count else { return try body() }
        let lock = locks[index]
        try FileManager.default.createDirectory(
            at: lock.deletingLastPathComponent(), withIntermediateDirectories: true)
        let fd = open(lock.path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else {
            throw ManualPairingError.storage("打不开事务锁 \(lock.path)：errno \(errno)")
        }
        defer { flock(fd, LOCK_UN); close(fd) }
        guard flock(fd, LOCK_EX) == 0 else {
            throw ManualPairingError.storage("拿不到事务锁 \(lock.path)：errno \(errno)")
        }
        return try acquire(locks, index: index + 1, body)
    }
}
