#if os(macOS)
import Foundation
import XCTest

/// #121 第四批的行为尺子。
///
/// 这组不把 trust / PSK / backend 手工拼成 TLS 参数：第一条从 Mac invitation 开始，
/// 让“iOS 侧”导入、生成 response、让 Mac 消费，再只从双方持久文件启动真实 loopback
/// TLS 和 crew RPC。这样信任方向、backend id、PSK identity 任一接反都会红。
@MainActor
final class RemotePendingCrewBackendTests: XCTestCase {
    private let instant = Date(timeIntervalSince1970: 1_800_000_000)

    func test_pairingArtifactsDriveRealTLSCrewDataPostAndChangePush() async throws {
        let serverPaths = paths("server")
        let iosPaths = paths("ios")
        let serverIdentityFile = identityFile(for: serverPaths)
        let iosIdentityFile = identityFile(for: iosPaths)
        let serverIdentity = try DeviceIdentityStore.loadOrCreate(at: serverIdentityFile)
        let iosIdentity = try DeviceIdentityStore.loadOrCreate(at: iosIdentityFile)
        let port = try reserveLoopbackPort()

        let serverPairing = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "ios-real-loopback" },
            makePreSharedKey: { Data(repeating: 0xc7, count: 32) })
        let iosPairing = ManualPairingCoordinator(
            identity: iosIdentity, paths: iosPaths, now: { self.instant })
        let invitation = try serverPairing.createInvitation(
            displayName: "Home Mac",
            remoteURL: "pendingcrew+tls://127.0.0.1:\(port)")
        guard case let .invitationAccepted(response, importedBackend) =
                try iosPairing.importText(invitation) else {
            return XCTFail("iOS 没有从邀请产出回应和持久配置")
        }
        guard case .responseAccepted = try serverPairing.importText(response) else {
            return XCTFail("Mac 没有消费 iOS 回应")
        }

        let crewDirectory = temporaryDirectory("crew-ledger")
        let whiteboardDirectory = temporaryDirectory("whiteboard-ledger")
        let crewStore = LocalCrewStore(baseDirectory: crewDirectory)
        let whiteboard = LocalWhiteboardStore(directory: whiteboardDirectory)
        let created = crewStore.createCrew(.make(
            responsibleSubjectId: LocalBackend.localSubjectId,
            title: "远端数据面", machineId: nil, workingDirectory: "/tmp/remote",
            captainAgentKind: "codex", captain: .systemGenerated(templateName: nil)))
        let localBackend = LocalBackend(store: crewStore, whiteboard: whiteboard)

        let persistedListener = try XCTUnwrap(
            SessionDaemonSecureListenerConfiguration.fromPersistentSettings(
                settingsFile: serverPaths.listenerSettings,
                loadIdentity: { try DeviceIdentityStore.loadOrCreate(at: serverIdentityFile) },
                loadTrustedPeers: { try PeerTrustStore.load(from: serverPaths.trustedPeers) }))
        let listener = try SecureTCPListener(
            localIdentity: persistedListener.localIdentity,
            trustedPeers: persistedListener.trustedPeers,
            port: persistedListener.port)
        let protocolServer = SessionProtocolServer(
            capabilities: SessionDaemonHost.defaultCapabilities + [CrewRPC.capability],
            daemonBuild: "crew-rpc-daemon")
        protocolServer.crewBackend = localBackend
        var acceptedLink: SecureTCPTransport?
        listener.onAccept = { link in
            acceptedLink = link
            protocolServer.accept(link: link)
        }
        listener.start()
        defer { listener.close(); acceptedLink?.close() }
        try await eventually { listener.port == port || listener.failure != nil }
        XCTAssertNil(listener.failure)

        let configuration = try RemoteBackendConfiguration.load(
            backendID: importedBackend.id,
            registryFile: iosPaths.backendRegistry,
            trustFile: iosPaths.trustedPeers,
            localIdentity: try DeviceIdentityStore.loadOrCreate(at: iosIdentityFile))
        let remote = RemotePendingCrewBackend(configuration: configuration, requestTimeout: 2)
        defer { remote.disconnect() }
        try await remote.connect()

        XCTAssertEqual(try await remote.listCrews().map(\.id), [created.crewId])
        XCTAssertEqual(try await remote.chiefLayer()?.id, LocalCrew.chiefCrewId)
        XCTAssertEqual(try await remote.getCrew(created.crewId).crew.title, "远端数据面")
        XCTAssertEqual(try await remote.listCrewWhiteboard(crewId: created.crewId), [])
        XCTAssertEqual(
            try await remote.listCrewMembers(crewId: created.crewId).members
                .filter { $0.memberKind == "human" }.count,
            1)

        let changed = expectation(description: "同一 SessionProtocol 连接推白板变化")
        let watcher = Task { @MainActor in
            for await _ in remote.whiteboardChanges(crewId: created.crewId) {
                changed.fulfill()
                return
            }
        }
        defer { watcher.cancel() }
        try await eventually {
            protocolServer.crewSubscriptionCount(crewId: created.crewId) == 1
        }
        XCTAssertNil(try await remote.postCrewMessage(
            crewId: created.crewId, text: "来自 iPhone", mentions: [.broadcast],
            replyToId: nil, localAttachments: [], extraReferences: []))
        await fulfillment(of: [changed], timeout: 2)

        let rows = try await remote.listCrewWhiteboard(crewId: created.crewId)
        XCTAssertEqual(rows.last?.displayText, "来自 iPhone")
        XCTAssertEqual(whiteboard.list(crewId: created.crewId).last?.text, "来自 iPhone",
                       "远端 post 必须由 daemon 的 LocalBackend 真落白板")

        do {
            _ = try await remote.postCrewMessage(
                crewId: created.crewId, text: "附件", mentions: [], replyToId: nil,
                localAttachments: [.init(
                    id: "a", mime: "text/plain", size: 1, path: "/tmp/a.txt")],
                extraReferences: [])
            XCTFail("iOS 本批不支持附件时必须明确拒绝")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("附件"), "实际错误：\(error)")
        }
    }

    func test_disconnectAndTimeoutFailEveryPendingCrewRequest() async throws {
        let identity = PairingDeviceIdentity.generate()
        let peerIdentity = PairingDeviceIdentity.generate()
        let backend = BackendRef(
            id: peerIdentity.id, displayName: "Mac",
            transport: .remote(url: "pendingcrew+tls://example.invalid:7443"))
        let configuration = RemoteBackendConfiguration(
            backend: backend,
            localIdentity: identity,
            peer: .init(
                backendID: backend.id, peerDeviceID: peerIdentity.id,
                peerPublicSigningKey: peerIdentity.publicSigningKey,
                preSharedKey: Data(repeating: 0x11, count: 32)))

        let disconnectingLink = CrewRPCStubLink()
        let disconnected = RemotePendingCrewBackend(
            configuration: configuration,
            connector: ClosureSessionMessageLinkConnector { disconnectingLink },
            requestTimeout: 30)
        try await disconnected.connect()
        let disconnectedRequest = Task { try await disconnected.listCrews() }
        await Task.yield()
        XCTAssertEqual(disconnected.pendingRequestCount, 1)
        disconnectingLink.failRemote()
        do {
            _ = try await disconnectedRequest.value
            XCTFail("断线不得让 RPC 永久挂起")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("断"), "实际错误：\(error)")
        }
        XCTAssertEqual(disconnected.pendingRequestCount, 0)

        let stalledLink = CrewRPCStubLink()
        let timedOut = RemotePendingCrewBackend(
            configuration: configuration,
            connector: ClosureSessionMessageLinkConnector { stalledLink },
            requestTimeout: 0.02)
        try await timedOut.connect()
        do {
            _ = try await timedOut.listCrews()
            XCTFail("无回应不得永久挂起")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("超时"), "实际错误：\(error)")
        }
        XCTAssertEqual(timedOut.pendingRequestCount, 0)
    }

    private func paths(_ name: String) -> ManualPairingPaths {
        let root = temporaryDirectory(name)
        return .init(
            trustedPeers: root.appendingPathComponent("trusted.json"),
            backendRegistry: root.appendingPathComponent("backends.json"),
            exchangeLedger: root.appendingPathComponent("exchange.json"),
            listenerSettings: root.appendingPathComponent("listener.json"))
    }

    private func identityFile(for paths: ManualPairingPaths) -> URL {
        paths.exchangeLedger.deletingLastPathComponent().appendingPathComponent("identity.json")
    }

    private func temporaryDirectory(_ name: String) -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("pendingcrew-rpc-\(name)-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func eventually(
        timeout: TimeInterval = 5, _ condition: @escaping @MainActor () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() > deadline { throw TestFailure.timeout }
            try await Task.sleep(nanoseconds: 10_000_000)
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

    private enum TestFailure: Error { case timeout }
}

@MainActor
private final class CrewRPCStubLink: SessionMessageLink {
    var onReceive: ((Data) -> Void)?
    var onClose: (() -> Void)?
    private(set) var isOpen = true
    let isSynchronous = false
    let pendingWriteBytes = 0
    private let codec = SessionProtocolCodec()

    func send(_ framed: Data) {
        guard let message = try? codec.decodeApp(framed) else { return }
        if case let .hello(hello) = message {
            let response = SessionDaemonMessage.hello(.init(
                protocolVersion: hello.protocolVersion,
                daemonBuild: "stub",
                capabilities: [CrewRPC.capability], sessionCount: 0, pid: 1))
            if let data = try? codec.encode(response) { onReceive?(data) }
        }
        // Crew requests intentionally stay pending until the test disconnects or times out.
    }

    func close() { isOpen = false }

    func failRemote() {
        isOpen = false
        onClose?()
    }
}
#endif
