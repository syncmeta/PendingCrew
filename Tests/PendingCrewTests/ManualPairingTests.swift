#if os(macOS)
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

        let viewerTrust = try XCTUnwrap(PeerTrustStore.load(from: viewerPaths.trustedPeers).first)
        XCTAssertEqual(viewerTrust.backendID, serverIdentity.id)
        XCTAssertEqual(viewerTrust.peerDeviceID, serverIdentity.id)
        XCTAssertEqual(viewerTrust.preSharedKey, expectedPSK)
        XCTAssertEqual(
            BackendRegistry.load(from: viewerPaths.backendRegistry).refs.first(where: \.isRemote),
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
}
#endif
