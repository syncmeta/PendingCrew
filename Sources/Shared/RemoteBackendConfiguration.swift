import Foundation

enum RemoteBackendConfigurationError: LocalizedError, Equatable {
    case registryUnreadable(String)
    case backendMissing(String)
    case notRemote(String)
    case trustUnreadable(String)
    case trustMissing(String)
    case duplicateTrust(String)

    var errorDescription: String? {
        switch self {
        case let .registryUnreadable(reason): return "远端后端配置读不出来：\(reason)"
        case let .backendMissing(id): return "远端后端 \(id) 不在持久配置中"
        case let .notRemote(id): return "后端 \(id) 不是远端 TLS 后端"
        case let .trustUnreadable(reason): return "配对信任读不出来：\(reason)"
        case let .trustMissing(id): return "远端后端 \(id) 没有配对信任"
        case let .duplicateTrust(id): return "远端后端 \(id) 有重复信任记录"
        }
    }
}

enum RemoteBackendRecordStore {
    static func load(from url: URL) throws -> [BackendRef] {
        do {
            return try JSONDecoder().decode([BackendRef].self, from: Data(contentsOf: url))
        } catch let error as NSError
            where error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError {
            return []
        } catch {
            throw ManualPairingError.storage("后端列表读不出来：\(error.localizedDescription)")
        }
    }

    static func encoded(_ refs: [BackendRef]) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try encoder.encode(refs.filter { !$0.isBuiltIn })
    }
}

struct RemoteBackendConfiguration: Equatable {
    var backend: BackendRef
    var localIdentity: PairingDeviceIdentity
    var peer: PeerTrustRecord

    static func load(
        backendID: String,
        registryFile: URL = DevicePairingPaths.backendRegistry,
        trustFile: URL = DevicePairingPaths.trustedPeers,
        localIdentity: PairingDeviceIdentity
    ) throws -> RemoteBackendConfiguration {
        let refs: [BackendRef]
        do { refs = try RemoteBackendRecordStore.load(from: registryFile) }
        catch { throw RemoteBackendConfigurationError.registryUnreadable(error.localizedDescription) }
        guard let backend = refs.first(where: { $0.id == backendID }) else {
            throw RemoteBackendConfigurationError.backendMissing(backendID)
        }
        guard backend.isRemote else { throw RemoteBackendConfigurationError.notRemote(backendID) }
        let peers: [PeerTrustRecord]
        do { peers = try PeerTrustStore.load(from: trustFile) }
        catch { throw RemoteBackendConfigurationError.trustUnreadable(error.localizedDescription) }
        let matches = peers.filter { $0.backendID == backendID }
        guard matches.count <= 1 else {
            throw RemoteBackendConfigurationError.duplicateTrust(backendID)
        }
        guard let peer = matches.first else {
            throw RemoteBackendConfigurationError.trustMissing(backendID)
        }
        return .init(backend: backend, localIdentity: localIdentity, peer: peer)
    }

    static func production() throws -> RemoteBackendConfiguration? {
        let refs: [BackendRef]
        do { refs = try RemoteBackendRecordStore.load(from: DevicePairingPaths.backendRegistry) }
        catch { throw RemoteBackendConfigurationError.registryUnreadable(error.localizedDescription) }
        let remote = refs.filter(\.isRemote)
        guard let backend = remote.first else { return nil }
        guard remote.count == 1 else {
            throw RemoteBackendConfigurationError.registryUnreadable(
                "登记了多个远端后端，本批 iOS 尚未提供选择器")
        }
        return try load(backendID: backend.id, localIdentity: DeviceIdentityStore.loadOrCreate())
    }

    func parameters() throws -> SecureConnectionParameters {
        guard case let .remote(raw) = backend.transport,
              let components = URLComponents(string: raw),
              components.scheme?.lowercased() == "pendingcrew+tls",
              let host = components.host, !host.isEmpty,
              let rawPort = components.port,
              let port = UInt16(exactly: rawPort), port != 0,
              components.user == nil, components.password == nil,
              components.path.isEmpty || components.path == "/",
              components.query == nil, components.fragment == nil else {
            throw RemoteBackendConfigurationError.notRemote(backend.id)
        }
        guard peer.backendID == backend.id, peer.isValid else {
            throw RemoteBackendConfigurationError.trustMissing(backend.id)
        }
        return .init(host: host, port: port, localIdentity: localIdentity, peer: peer)
    }
}
