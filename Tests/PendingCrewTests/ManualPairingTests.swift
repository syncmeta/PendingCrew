#if os(macOS)
import CryptoKit
import Darwin
import Foundation
import XCTest

@MainActor
final class ManualPairingTests: XCTestCase {
    private let instant = Date(timeIntervalSince1970: 1_800_000_000)
    private var roots: [URL] = []

    override func tearDownWithError() throws {
        for root in roots { try? FileManager.default.removeItem(at: root) }
        roots.removeAll()
    }

    private func paths(_ name: String) -> ManualPairingPaths {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-pairing-\(name)-\(UUID().uuidString)", isDirectory: true)
        roots.append(root)
        return .init(
            trustedPeers: root.appendingPathComponent("trusted-peers.json"),
            backendRegistry: root.appendingPathComponent("registry.json"),
            exchangeLedger: root.appendingPathComponent("exchange-ledger.json"),
            listenerSettings: root.appendingPathComponent("listener.json"))
    }

    func test_invitationResponsePersistsBothTrustsBackendAndListenerUsingOnePSK() throws {
        let serverIdentity = PairingDeviceIdentity.generate()
        let viewerIdentity = PairingDeviceIdentity.generate()
        let serverPaths = paths("server")
        let viewerPaths = paths("viewer")
        let expectedPSK = Data(repeating: 0x6a, count: 32)
        var pskGenerationCount = 0
        let server = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "invite-once" },
            makePreSharedKey: {
                pskGenerationCount += 1
                return expectedPSK
            })
        let viewer = ManualPairingCoordinator(
            identity: viewerIdentity, paths: viewerPaths, now: { self.instant })

        let existingPeer = PairingDeviceIdentity.generate()
        let existingTrust = PeerTrustRecord(
            backendID: existingPeer.id, peerDeviceID: existingPeer.id,
            peerPublicSigningKey: existingPeer.publicSigningKey,
            preSharedKey: Data(repeating: 0x17, count: 32))
        try PeerTrustStore.save([existingTrust], to: viewerPaths.trustedPeers)
        let existingBackend = BackendRef(
            id: "existing-remote", displayName: "Existing",
            transport: .remote(url: "pendingcrew+tls://existing.local:7333"))
        try BackendRegistry.save([existingBackend], to: viewerPaths.backendRegistry)

        let invitation = try server.createInvitation(
            displayName: "Studio Mac", remoteURL: "pendingcrew+tls://studio.local:7443")
        let invitationEnvelope = try ManualPairingTextCodec.decodeInvitation(invitation)
        let invitationJSON = try JSONEncoder().encode(invitationEnvelope)
        XCTAssertFalse(
            String(decoding: invitationJSON, as: UTF8.self)
                .contains(serverIdentity.privateSigningKey.base64EncodedString()),
            "手动邀请不能泄露长期签名私钥")

        guard case let .invitationAccepted(response, backend) = try viewer.importText(invitation)
        else { return XCTFail("邀请没有产出可带回原机器的回应") }
        XCTAssertEqual(backend.id, serverIdentity.id)
        XCTAssertEqual(backend.displayName, "Studio Mac")
        XCTAssertEqual(backend.transport, .remote(url: "pendingcrew+tls://studio.local:7443"))
        XCTAssertEqual(pskGenerationCount, 1, "一对设备的 PSK 只能在邀请端生成一次")

        let viewerTrust = try XCTUnwrap(PeerTrustStore.load(from: viewerPaths.trustedPeers)
            .first(where: { $0.backendID == serverIdentity.id }))
        XCTAssertEqual(viewerTrust.backendID, serverIdentity.id)
        XCTAssertEqual(viewerTrust.peerDeviceID, serverIdentity.id)
        XCTAssertEqual(viewerTrust.preSharedKey, expectedPSK)
        let allViewerTrusts = try PeerTrustStore.load(from: viewerPaths.trustedPeers)
        XCTAssertTrue(allViewerTrusts.contains(existingTrust),
                      "配对事务必须保留锁内重读到的既有信任")
        XCTAssertTrue(BackendRegistry.load(from: viewerPaths.backendRegistry).refs
            .contains(existingBackend), "配对事务必须保留并发新增的 backend")
        XCTAssertEqual(
            BackendRegistry.load(from: viewerPaths.backendRegistry).refs
                .first(where: { $0.id == serverIdentity.id }),
            backend)

        let responseEnvelope = try ManualPairingTextCodec.decodeResponse(response)
        let responseJSON = String(decoding: try JSONEncoder().encode(responseEnvelope), as: UTF8.self)
        XCTAssertFalse(responseJSON.contains(expectedPSK.base64EncodedString()),
                       "回应应只证明持有邀请，不应再次携带 PSK")
        guard case let .responseAccepted(port, peerDeviceID) = try server.importText(response)
        else { return XCTFail("回应没有完成邀请端信任与监听配置") }
        XCTAssertEqual(port, 7443)
        XCTAssertEqual(peerDeviceID, viewerIdentity.id)

        let serverTrust = try XCTUnwrap(PeerTrustStore.load(from: serverPaths.trustedPeers).first)
        XCTAssertEqual(serverTrust.backendID, viewerIdentity.id)
        XCTAssertEqual(serverTrust.peerDeviceID, viewerIdentity.id)
        XCTAssertEqual(serverTrust.preSharedKey, expectedPSK)
        XCTAssertEqual(
            try SessionDaemonSecureListenerSettings.load(from: serverPaths.listenerSettings),
            .init(port: 7443))
    }

    func test_pairingFilesDriveRealBackendRegistryTLSAndProtocolHello() throws {
        let serverPaths = paths("loopback-server")
        let viewerPaths = paths("loopback-viewer")
        let serverIdentityFile = serverPaths.exchangeLedger
            .deletingLastPathComponent().appendingPathComponent("identity.json")
        let viewerIdentityFile = viewerPaths.exchangeLedger
            .deletingLastPathComponent().appendingPathComponent("identity.json")
        let serverIdentity = try DeviceIdentityStore.loadOrCreate(at: serverIdentityFile)
        let viewerIdentity = try DeviceIdentityStore.loadOrCreate(at: viewerIdentityFile)
        let port = try reserveLoopbackPort()
        let serverCoordinator = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "real-loopback" },
            makePreSharedKey: { Data(repeating: 0xa4, count: 32) })
        let viewerCoordinator = ManualPairingCoordinator(
            identity: viewerIdentity, paths: viewerPaths, now: { self.instant })

        let invitation = try serverCoordinator.createInvitation(
            displayName: "Paired Loopback",
            remoteURL: "pendingcrew+tls://127.0.0.1:\(port)")
        guard case let .invitationAccepted(response, importedBackend) =
            try viewerCoordinator.importText(invitation) else {
            return XCTFail("viewer 没有从真实邀请生成回应")
        }
        guard case .responseAccepted = try serverCoordinator.importText(response) else {
            return XCTFail("server 没有从真实回应完成配对")
        }

        let persistedConfiguration = try XCTUnwrap(
            SessionDaemonSecureListenerConfiguration.fromPersistentSettings(
                settingsFile: serverPaths.listenerSettings,
                loadIdentity: { try DeviceIdentityStore.loadOrCreate(at: serverIdentityFile) },
                loadTrustedPeers: {
                    try PeerTrustStore.load(from: serverPaths.trustedPeers)
                }))
        let listener = try SecureTCPListener(
            localIdentity: persistedConfiguration.localIdentity,
            trustedPeers: persistedConfiguration.trustedPeers,
            port: persistedConfiguration.port)
        let protocolServer = SessionProtocolServer(
            capabilities: SessionDaemonHost.defaultCapabilities,
            daemonBuild: "paired-persistent-daemon")
        var acceptedLink: SecureTCPTransport?
        listener.onAccept = { link in
            acceptedLink = link
            protocolServer.accept(link: link)
        }
        listener.start()
        defer { listener.close(); acceptedLink?.close() }
        try pump(until: { listener.port == port || listener.failure != nil })
        XCTAssertNil(listener.failure)

        let persistedBackends = BackendRegistry.load(from: viewerPaths.backendRegistry)
        XCTAssertNil(persistedBackends.problem)
        let backend = try XCTUnwrap(
            persistedBackends.refs.first(where: { $0.id == importedBackend.id }))
        let clientLink = try BackendRegistry.connectRemote(
            to: backend, identityFile: viewerIdentityFile,
            trustFile: viewerPaths.trustedPeers)
        defer { clientLink.close() }
        let protocolClient = SessionProtocolClient(
            link: clientLink, capabilities: SessionDaemonHost.defaultCapabilities,
            appBuild: "paired-persistent-viewer")
        var hello: SessionDaemonHello?
        var sessionList: SessionList?
        protocolClient.onDaemonHello = { hello = $0 }
        protocolClient.onSessionList = { sessionList = $0 }
        protocolClient.connect()
        protocolClient.requestSessionList()

        try pump(until: {
            (protocolClient.isConnected && hello != nil && sessionList != nil)
                || clientLink.failure != nil || listener.lastConnectionFailure != nil
        })
        XCTAssertNil(clientLink.failure, "viewer TLS：\(String(describing: clientLink.failure))")
        XCTAssertNil(listener.lastConnectionFailure,
                     "server TLS：\(String(describing: listener.lastConnectionFailure))")
        XCTAssertTrue(protocolClient.isConnected)
        XCTAssertEqual(hello?.daemonBuild, "paired-persistent-daemon")
        XCTAssertEqual(sessionList?.sessions, [])
    }

    func test_replayExpiredInvitationAndWrongBackendResponseFailClosed() throws {
        let serverIdentity = PairingDeviceIdentity.generate()
        let viewerIdentity = PairingDeviceIdentity.generate()
        let serverPaths = paths("server")
        let viewerPaths = paths("viewer")
        let server = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "one-shot" },
            makePreSharedKey: { Data(repeating: 0x55, count: 32) })
        let viewer = ManualPairingCoordinator(
            identity: viewerIdentity, paths: viewerPaths, now: { self.instant })
        let invitation = try server.createInvitation(
            displayName: "Office", remoteURL: "pendingcrew+tls://office.local:7443")
        guard case let .invitationAccepted(response, _) = try viewer.importText(invitation)
        else { return XCTFail("邀请未导入") }

        XCTAssertThrowsError(try viewer.importText(invitation)) {
            XCTAssertEqual($0 as? ManualPairingError, .replayedInvitation("one-shot"))
        }

        let wrongServer = ManualPairingCoordinator(
            identity: PairingDeviceIdentity.generate(), paths: serverPaths, now: { self.instant })
        XCTAssertThrowsError(try wrongServer.importText(response)) {
            guard case .wrongTarget? = $0 as? ManualPairingError else {
                return XCTFail("回应被错误后端接受或错误未分类：\($0)")
            }
        }

        _ = try server.importText(response)
        XCTAssertThrowsError(try server.importText(response)) {
            XCTAssertEqual($0 as? ManualPairingError, .replayedResponse("one-shot"))
        }

        let lateViewer = ManualPairingCoordinator(
            identity: PairingDeviceIdentity.generate(), paths: paths("late"),
            now: { self.instant.addingTimeInterval(ManualPairingCoordinator.invitationLifetime + 1) })
        XCTAssertThrowsError(try lateViewer.importText(invitation)) {
            XCTAssertEqual($0 as? ManualPairingError, .expiredInvitation("one-shot"))
        }
    }

    func test_tamperedResponseSignatureDoesNotWriteTrustOrListenerSettings() throws {
        let serverIdentity = PairingDeviceIdentity.generate()
        let serverPaths = paths("server")
        let server = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "tamper" },
            makePreSharedKey: { Data(repeating: 0x33, count: 32) })
        let viewer = ManualPairingCoordinator(
            identity: PairingDeviceIdentity.generate(), paths: paths("viewer"),
            now: { self.instant })
        let invitation = try server.createInvitation(
            displayName: "Server", remoteURL: "pendingcrew+tls://server.local:7443")
        guard case let .invitationAccepted(response, _) = try viewer.importText(invitation)
        else { return XCTFail("邀请未导入") }
        var envelope = try ManualPairingTextCodec.decodeResponse(response)
        envelope.signature[0] ^= 0xff

        XCTAssertThrowsError(
            try server.importText(ManualPairingTextCodec.encodeResponse(envelope))) {
            XCTAssertEqual($0 as? ManualPairingError, .invalidSignature)
        }
        XCTAssertEqual(try PeerTrustStore.load(from: serverPaths.trustedPeers), [])
        XCTAssertNil(try SessionDaemonSecureListenerSettings.load(
            from: serverPaths.listenerSettings))
    }

    func test_validJoinerSignatureCannotSubstituteAnotherInvitationDigest() throws {
        let serverIdentity = PairingDeviceIdentity.generate()
        let viewerIdentity = PairingDeviceIdentity.generate()
        let serverPaths = paths("digest-binding")
        let server = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "digest-bound" },
            makePreSharedKey: { Data(repeating: 0x49, count: 32) })
        let viewer = ManualPairingCoordinator(
            identity: viewerIdentity, paths: paths("digest-viewer"), now: { self.instant })
        let invitation = try server.createInvitation(
            displayName: "Bound Server", remoteURL: "pendingcrew+tls://bound.local:7443")
        guard case let .invitationAccepted(response, _) = try viewer.importText(invitation)
        else { return XCTFail("邀请未导入") }
        var substituted = try ManualPairingTextCodec.decodeResponse(response)
        substituted.payload.invitationDigest = Data(repeating: 0x99, count: 32)
        let joinerKey = try Curve25519.Signing.PrivateKey(
            rawRepresentation: viewerIdentity.privateSigningKey)
        substituted.signature = try joinerKey.signature(
            for: ManualPairingTextCodec.canonicalData(substituted.payload))

        XCTAssertThrowsError(try server.importText(
            ManualPairingTextCodec.encodeResponse(substituted))) {
            XCTAssertEqual($0 as? ManualPairingError, .invalidResponse)
        }
        XCTAssertEqual(try PeerTrustStore.load(from: serverPaths.trustedPeers), [])
        XCTAssertNil(try SessionDaemonSecureListenerSettings.load(
            from: serverPaths.listenerSettings))
        guard case .responseAccepted = try server.importText(response) else {
            return XCTFail("失败的替换尝试错误消费了 pending invite")
        }
    }

    func test_responseRetryCompletesAfterIdenticalTrustWasAlreadyPersisted() throws {
        let serverIdentity = PairingDeviceIdentity.generate()
        let viewerIdentity = PairingDeviceIdentity.generate()
        let serverPaths = paths("server-retry")
        let server = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "retry-after-trust" },
            makePreSharedKey: { Data(repeating: 0x71, count: 32) })
        let viewer = ManualPairingCoordinator(
            identity: viewerIdentity, paths: paths("viewer-retry"), now: { self.instant })
        let invitation = try server.createInvitation(
            displayName: "Retry Server", remoteURL: "pendingcrew+tls://retry.local:7443")
        let invitationEnvelope = try ManualPairingTextCodec.decodeInvitation(invitation)
        guard case let .invitationAccepted(response, _) = try viewer.importText(invitation)
        else { return XCTFail("邀请未导入") }

        // Model an abrupt process interruption after the first rename reached durable storage.
        let alreadyWrittenTrust = PeerTrustRecord(
            backendID: viewerIdentity.id, peerDeviceID: viewerIdentity.id,
            peerPublicSigningKey: viewerIdentity.publicSigningKey,
            preSharedKey: invitationEnvelope.payload.preSharedKey)
        try PeerTrustStore.save([alreadyWrittenTrust], to: serverPaths.trustedPeers)

        guard case let .responseAccepted(port, peerDeviceID) = try server.importText(response)
        else { return XCTFail("相同 trust 的重试没有续完监听配置与 ledger") }
        XCTAssertEqual(port, 7443)
        XCTAssertEqual(peerDeviceID, viewerIdentity.id)
        XCTAssertEqual(try PeerTrustStore.load(from: serverPaths.trustedPeers), [alreadyWrittenTrust],
                       "幂等续写不得产生重复 trust")
        XCTAssertEqual(try SessionDaemonSecureListenerSettings.load(
            from: serverPaths.listenerSettings), .init(port: 7443))
        XCTAssertThrowsError(try server.importText(response)) {
            XCTAssertEqual($0 as? ManualPairingError, .replayedResponse("retry-after-trust"))
        }
    }

    func test_transactionFailureAtEveryRenameBoundaryRestoresOldFilesAndCanRetry() throws {
        enum InjectedFailure: Error { case beforeRename(Int) }
        let transactionPaths = paths("rename-boundaries")
        let root = transactionPaths.trustedPeers.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let urls = (0..<3).map { root.appendingPathComponent("file-\($0).json") }
        let old = urls.indices.map { Data("old-\($0)".utf8) }
        let new = urls.indices.map { Data("new-\($0)".utf8) }
        let updates = urls.indices.map {
            PairingFileTransaction.Update(url: urls[$0], data: new[$0], mode: 0o600)
        }

        for failureIndex in urls.indices {
            for index in urls.indices {
                try old[index].write(to: urls[index])
                XCTAssertEqual(chmod(urls[index].path, mode_t(0o640 + index)), 0)
            }
            XCTAssertThrowsError(try PairingFileTransaction.commit(
                updates, beforeRename: { index, _ in
                    if index == failureIndex { throw InjectedFailure.beforeRename(index) }
                }))
            for index in urls.indices {
                XCTAssertEqual(try Data(contentsOf: urls[index]), old[index],
                               "边界 \(failureIndex) 失败后旧文件 \(index) 被改动")
                let attributes = try FileManager.default.attributesOfItem(
                    atPath: urls[index].path)
                let mode = attributes[.posixPermissions] as? NSNumber
                XCTAssertEqual(mode?.intValue, 0o640 + index,
                               "边界 \(failureIndex) 失败后旧文件权限 \(index) 被改动")
            }

            try PairingFileTransaction.commit(updates)
            for index in urls.indices {
                XCTAssertEqual(try Data(contentsOf: urls[index]), new[index],
                               "边界 \(failureIndex) 失败后不能安全重试")
            }
        }
    }

    func test_stagingFailureDoesNotDeleteUninspectedLaterExistingFile() throws {
        let transactionPaths = paths("staging-failure")
        let root = transactionPaths.trustedPeers.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let first = root.appendingPathComponent("first.json")
        let blocker = root.appendingPathComponent("not-a-directory")
        let broken = blocker.appendingPathComponent("broken.json")
        let later = root.appendingPathComponent("later.json")
        let firstOld = Data("first-old".utf8)
        let laterOld = Data("later-old".utf8)
        try firstOld.write(to: first)
        try Data("blocker".utf8).write(to: blocker)
        try laterOld.write(to: later)

        XCTAssertThrowsError(try PairingFileTransaction.commit([
            .init(url: first, data: Data("first-new".utf8), mode: 0o600),
            .init(url: broken, data: Data("broken-new".utf8), mode: 0o600),
            .init(url: later, data: Data("later-new".utf8), mode: 0o600),
        ]))
        XCTAssertEqual(try Data(contentsOf: first), firstOld)
        XCTAssertEqual(try Data(contentsOf: later), laterOld,
                       "staging 失败不得删除尚未读取 original 的后续旧文件")
    }

    func test_persistentListenerSettingsRoundTripAndFailClosedOnCorruptionOrNoTrust() throws {
        let settingsFile = paths("settings").listenerSettings
        try SessionDaemonSecureListenerSettings.save(.init(port: 7443), to: settingsFile)
        XCTAssertEqual(
            try SessionDaemonSecureListenerSettings.load(from: settingsFile), .init(port: 7443))

        let identity = PairingDeviceIdentity.generate()
        let peer = PairingDeviceIdentity.generate()
        let trust = PeerTrustRecord(
            backendID: peer.id, peerDeviceID: peer.id,
            peerPublicSigningKey: peer.publicSigningKey,
            preSharedKey: Data(repeating: 0x22, count: 32))
        let configuration = try XCTUnwrap(
            SessionDaemonSecureListenerConfiguration.fromPersistentSettings(
                settingsFile: settingsFile,
                loadIdentity: { identity }, loadTrustedPeers: { [trust] }))
        XCTAssertEqual(configuration.port, 7443)
        XCTAssertEqual(configuration.trustedPeers, [trust])

        XCTAssertThrowsError(
            try SessionDaemonSecureListenerConfiguration.fromPersistentSettings(
                settingsFile: settingsFile,
                loadIdentity: { identity }, loadTrustedPeers: { [] }))

        try Data("{ broken".utf8).write(to: settingsFile)
        XCTAssertThrowsError(try SessionDaemonSecureListenerSettings.load(from: settingsFile),
                             "损坏配置不能被当成未启用后继续 local-only")
    }


    private func pump(
        until condition: () -> Bool, timeout: TimeInterval = 5,
        file: StaticString = #filePath, line: UInt = #line
    ) throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() > deadline { return XCTFail("等待超时", file: file, line: line) }
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
    }

    private func reserveLoopbackPort() throws -> UInt16 {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw POSIXError(.EIO) }
        defer { close(fd) }
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = 0
        address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0 else { throw POSIXError(.EADDRINUSE) }
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let named = withUnsafeMutablePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(fd, $0, &length)
            }
        }
        guard named == 0 else { throw POSIXError(.EIO) }
        return UInt16(bigEndian: address.sin_port)
    }
}
#endif
