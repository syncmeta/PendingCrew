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

        let crews = try await remote.listCrews()
        let chief = try await remote.chiefLayer()
        let detail = try await remote.getCrew(created.crewId)
        let initialWhiteboard = try await remote.listCrewWhiteboard(crewId: created.crewId)
        let roster = try await remote.listCrewMembers(crewId: created.crewId)
        XCTAssertEqual(crews.map(\.id), [created.crewId])
        XCTAssertEqual(chief?.id, LocalCrew.chiefCrewId)
        XCTAssertEqual(detail.crew.title, "远端数据面")
        XCTAssertEqual(initialWhiteboard, [])
        XCTAssertEqual(roster.members.filter { $0.memberKind == "human" }.count, 1)

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
        let warning = try await remote.postCrewMessage(
            crewId: created.crewId, text: "来自 iPhone", mentions: [.broadcast],
            replyToId: nil, localAttachments: [], extraReferences: [])
        XCTAssertNil(warning)
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

    /// #121 第五批的承重回归：crew / session / terminal / 结构化审批必须走同一次
    /// TLS accept、同一个 SessionProtocol link。审批特意落到真实临时账本再从远端改写，
    /// 不能用 `SessionProtocolState.pendingDecision` 或客户端假数组冒充。
    func test_oneTLSLinkCarriesCrewSessionTerminalAndStructuredApprovals() async throws {
        let serverPaths = paths("fifth-server")
        let iosPaths = paths("fifth-ios")
        let serverIdentityFile = identityFile(for: serverPaths)
        let iosIdentityFile = identityFile(for: iosPaths)
        let serverIdentity = try DeviceIdentityStore.loadOrCreate(at: serverIdentityFile)
        let iosIdentity = try DeviceIdentityStore.loadOrCreate(at: iosIdentityFile)
        let port = try reserveLoopbackPort()

        let serverPairing = ManualPairingCoordinator(
            identity: serverIdentity, paths: serverPaths, now: { self.instant },
            makeInvitationID: { "ios-session-loopback" },
            makePreSharedKey: { Data(repeating: 0xd8, count: 32) })
        let iosPairing = ManualPairingCoordinator(
            identity: iosIdentity, paths: iosPaths, now: { self.instant })
        let invitation = try serverPairing.createInvitation(
            displayName: "Home Mac",
            remoteURL: "pendingcrew+tls://127.0.0.1:\(port)")
        guard case let .invitationAccepted(response, importedBackend) =
                try iosPairing.importText(invitation) else {
            return XCTFail("iOS 没有产出配对回应")
        }
        _ = try serverPairing.importText(response)

        let crewStore = LocalCrewStore(baseDirectory: temporaryDirectory("fifth-crews"))
        let whiteboard = LocalWhiteboardStore(directory: temporaryDirectory("fifth-whiteboard"))
        let created = crewStore.createCrew(.make(
            responsibleSubjectId: LocalBackend.localSubjectId,
            title: "远端会话", machineId: nil, workingDirectory: "/tmp/remote-session",
            captainAgentKind: "claudeCode", captain: .systemGenerated(templateName: nil)))
        let localBackend = LocalBackend(store: crewStore, whiteboard: whiteboard)
        let approvals = LocalApprovalStore(directory: temporaryDirectory("fifth-approvals"))
        let permissionID = try XCTUnwrap(approvals.raise(
            crewId: created.crewId, kind: "permission", sessionId: "remote-session",
            summary: "允许运行测试命令"))
        let deniedPermissionID = try XCTUnwrap(approvals.raise(
            crewId: created.crewId, kind: "permission", sessionId: "remote-session",
            summary: "拒绝危险命令"))
        let questionID = try XCTUnwrap(approvals.raise(
            crewId: created.crewId, kind: "decision", sessionId: "remote-session",
            summary: "请选择发布窗口"))

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
            capabilities: SessionDaemonHost.defaultCapabilities + [
                CrewRPC.capability, ApprovalRPC.capability,
            ], daemonBuild: "fifth-daemon")
        protocolServer.crewBackend = localBackend
        protocolServer.approvalStore = approvals
        let terminal = ProtocolTestBackend(kind: .claudeCode)
        terminal.terminalSnapshot = .init(
            cols: 80, rows: 25, bytes: Array("initial terminal\n".utf8))
        protocolServer.register(sessionId: "remote-session", backend: terminal)
        let transcript = CodexTranscript()
        transcript.apply(method: "item/completed", params: ["item": [
            "id": "initial-answer", "type": "agentMessage", "phase": "final_answer",
            "text": "initial codex transcript",
        ]])
        let codex = ProtocolTestBackend(kind: .codex)
        codex.codexHistory = transcript.items
        protocolServer.register(sessionId: "codex-session", backend: codex)
        protocolServer.runSummaryProvider = { sessionID in
            guard sessionID == "remote-session" || sessionID == "codex-session" else { return nil }
            return .init(
                crewId: created.crewId, role: "captain",
                title: sessionID == "codex-session" ? "远端 Codex" : "远端机长",
                taskBrief: "第五批", workingDirectory: "/tmp/remote-session",
                model: nil, effort: nil, pendingProfile: nil,
                approvalsReviewer: nil, permissionModeOverride: nil,
                startedAt: self.instant.timeIntervalSince1970, runStatus: "running",
                exitCode: nil, exitReason: nil, awaitingReply: "approval")
        }
        var acceptCount = 0
        var acceptedLink: SecureTCPTransport?
        listener.onAccept = { link in
            acceptCount += 1
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

        let remoteCrewIDs = try await remote.listCrews().map(\.id)
        XCTAssertEqual(remoteCrewIDs, [created.crewId])
        let sessions = try await remote.listRemoteSessions()
        XCTAssertEqual(Set(sessions.map(\.sessionId)), Set(["remote-session", "codex-session"]))
        let channel = try await remote.openSession(sessionID: "remote-session")
        try await eventually { channel.terminalText.contains("initial terminal") }
        let reopened = try await remote.openSession(sessionID: "remote-session")
        XCTAssertTrue(channel === reopened)
        try await eventually { protocolServer.attachmentCount(sessionId: "remote-session") == 1 }
        protocolServer.publishTerminalBytes(
            sessionId: "remote-session", bytes: Array("live output\n".utf8))
        try await eventually { channel.terminalText.contains("live output") }
        XCTAssertEqual(channel.terminalText.components(separatedBy: "live output").count - 1, 1,
                       "重复进入同一详情不得重复 attach/重复实时帧")
        let codexChannel = try await remote.openSession(sessionID: "codex-session")
        try await eventually { codexChannel.transcriptText.contains("initial codex transcript") }
        protocolServer.acceptCodexNotification(
            sessionId: "codex-session", method: "item/completed", params: ["item": [
                "id": "live-answer", "type": "agentMessage", "phase": "final_answer",
                "text": "live codex notification",
            ]])
        try await eventually { codexChannel.transcriptText.contains("live codex notification") }
        XCTAssertTrue(codexChannel.transcriptText.contains("[回复]"),
                      "iOS 必须复用权威 CodexThreadItem/CodexTranscriptText 解析")

        try await eventually {
            channel.pendingApprovals.count == 3
                && protocolServer.approvalSubscriptionCount(crewId: created.crewId) == 1
        }
        await Task.yield()
        let pushedPermissionID = try XCTUnwrap(approvals.raise(
            crewId: created.crewId, kind: "permission", sessionId: "remote-session",
            summary: "daemon 新增审批"))
        try await eventually {
            channel.pendingApprovals.contains { $0.id == pushedPermissionID }
        }
        try await remote.decideApproval(
            crewID: created.crewId, approvalID: permissionID, decision: "allow")
        try await remote.decideApproval(
            crewID: created.crewId, approvalID: deniedPermissionID, decision: "deny")
        try await remote.decideApproval(
            crewID: created.crewId, approvalID: pushedPermissionID, decision: "deny")
        do {
            try await remote.answerApproval(
                crewID: created.crewId, approvalID: questionID, reply: "   ")
            XCTFail("daemon 必须拒绝空 ask answer")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("不能为空"), "实际错误：\(error)")
        }
        XCTAssertEqual(approvals.item(crewId: created.crewId, id: questionID)?.status, "pending")
        try await remote.answerApproval(
            crewID: created.crewId, approvalID: questionID, reply: "今晚")
        try await eventually { channel.pendingApprovals.isEmpty }
        XCTAssertEqual(approvals.item(crewId: created.crewId, id: permissionID)?.decision, "allow")
        XCTAssertEqual(
            approvals.item(crewId: created.crewId, id: deniedPermissionID)?.decision, "deny")
        XCTAssertEqual(
            approvals.item(crewId: created.crewId, id: pushedPermissionID)?.decision, "deny")
        XCTAssertEqual(approvals.item(crewId: created.crewId, id: questionID)?.reply, "今晚")
        remote.closeSession(sessionID: "remote-session")
        remote.closeSession(sessionID: "codex-session")
        try await eventually { protocolServer.attachmentCount(sessionId: "remote-session") == 0 }
        try await eventually { protocolServer.attachmentCount(sessionId: "codex-session") == 0 }
        let detachedText = channel.terminalText
        protocolServer.publishTerminalBytes(
            sessionId: "remote-session", bytes: Array("after detach\n".utf8))
        await Task.yield()
        XCTAssertEqual(channel.terminalText, detachedText)
        XCTAssertEqual(acceptCount, 1, "crew/session/terminal/approval 不得另开第二条连接")
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

    func test_disconnectFailsCrewApprovalAndSessionListRequestsTogether() async throws {
        let identity = PairingDeviceIdentity.generate()
        let peerIdentity = PairingDeviceIdentity.generate()
        let backend = BackendRef(
            id: peerIdentity.id, displayName: "Mac",
            transport: .remote(url: "pendingcrew+tls://example.invalid:7443"))
        let configuration = RemoteBackendConfiguration(
            backend: backend, localIdentity: identity,
            peer: .init(backendID: backend.id, peerDeviceID: peerIdentity.id,
                        peerPublicSigningKey: peerIdentity.publicSigningKey,
                        preSharedKey: Data(repeating: 0x44, count: 32)))
        let link = CrewRPCStubLink()
        link.respondsToApprovalList = false
        let remote = RemotePendingCrewBackend(
            configuration: configuration,
            connector: ClosureSessionMessageLinkConnector { link }, requestTimeout: 30)
        try await remote.connect()

        let crew = Task { try await remote.listCrews() }
        let sessions = Task { try await remote.listRemoteSessions() }
        let approvals = Task { try await remote.listApprovals(crewID: "c", sessionID: "s") }
        try await eventually { remote.pendingRequestCount == 3 }
        link.failRemote()

        do { _ = try await crew.value; XCTFail("crew request 未因断线恢复") } catch { }
        do { _ = try await sessions.value; XCTFail("session list 未因断线恢复") } catch { }
        do { _ = try await approvals.value; XCTFail("approval request 未因断线恢复") } catch { }
        XCTAssertEqual(remote.pendingRequestCount, 0)
    }

    func test_terminatedClientIgnoresLateTerminalFramesBeforeReconnect() async throws {
        let link = CrewRPCStubLink()
        let client = CrewRPCClient(link: link, timeout: 2, generation: 41)
        try await client.connect()
        let channel = RemoteSessionChannel(sessionID: "late")
        client.attach(channel)
        let handle = try XCTUnwrap(link.lastAttachedHandle)
        link.deliverTerminal(handle: handle, text: "before-close")
        try await eventually { channel.terminalText.contains("before-close") }

        link.failRemote()
        try await eventually {
            if case .disconnected = channel.connectionState { return true }
            return false
        }
        link.deliverTerminal(handle: handle, text: "late-after-close")
        await Task.yield()
        XCTAssertFalse(channel.terminalText.contains("late-after-close"),
                       "failAll 后即使旧 client 尚被持有，也必须彻底丢弃迟到 frame")
    }

    func test_nonNumberedPendingDecisionIsReadOnlyWithoutSelectedIndex() async throws {
        let link = CrewRPCStubLink()
        let client = CrewRPCClient(link: link, timeout: 2, generation: 7)
        try await client.connect()
        let channel = RemoteSessionChannel(sessionID: "menu")
        client.attach(channel)
        let baseState = SessionProtocolState(
            status: .running, isWorking: false, displayIsTyping: false,
            health: nil,
            pendingDecision: .init(prompt: "选择", options: ["甲", "乙"], numbered: false),
            kind: "claude_code", launchParameterProblem: nil, scrollState: nil)
        channel.apply(
            summary: .init(sessionId: "menu", stateSeq: 1, state: baseState), generation: 7)
        channel.chooseTerminalDecision(optionIndex: 1)
        XCTAssertTrue(link.sentInputBytes.isEmpty,
                      "wire 没有 selectedIndex 时不能猜当前高亮并发送方向键")

        var numberedState = baseState
        numberedState.pendingDecision = .init(
            prompt: "选择", options: ["甲", "乙"], numbered: true)
        channel.apply(
            summary: .init(sessionId: "menu", stateSeq: 2, state: numberedState), generation: 7)
        channel.chooseTerminalDecision(optionIndex: 1)
        XCTAssertEqual(link.sentInputBytes, [Array("2\r".utf8)])
    }

    func test_concurrentConnectIsSingleFlightAndLateOldHelloCannotWin() async throws {
        let identity = PairingDeviceIdentity.generate()
        let peerIdentity = PairingDeviceIdentity.generate()
        let backend = BackendRef(
            id: peerIdentity.id, displayName: "Mac",
            transport: .remote(url: "pendingcrew+tls://example.invalid:7443"))
        let configuration = RemoteBackendConfiguration(
            backend: backend, localIdentity: identity,
            peer: .init(backendID: backend.id, peerDeviceID: peerIdentity.id,
                        peerPublicSigningKey: peerIdentity.publicSigningKey,
                        preSharedKey: Data(repeating: 0x22, count: 32)))
        var links: [CrewRPCStubLink] = []
        let remote = RemotePendingCrewBackend(
            configuration: configuration,
            connector: ClosureSessionMessageLinkConnector {
                let link = CrewRPCStubLink(automaticallyCompletesHello: false)
                links.append(link)
                return link
            }, requestTimeout: 2)

        let first = Task { try await remote.connect() }
        let concurrent = Task { try await remote.connect() }
        try await eventually { links.count == 1 }
        links[0].deliverHello()
        try await first.value
        try await concurrent.value
        XCTAssertEqual(links.count, 1, "并发 connect 必须共用同一条 in-flight 握手")
        XCTAssertTrue(remote.isConnected)

        remote.disconnect()
        let old = Task { try await remote.connect() }
        try await eventually { links.count == 2 }
        remote.disconnect()
        do {
            try await old.value
            XCTFail("主动换代必须终止旧握手")
        } catch { }

        let replacement = Task { try await remote.connect() }
        try await eventually { links.count == 3 }
        links[1].deliverHello()
        await Task.yield()
        XCTAssertFalse(remote.isConnected, "旧连接迟到的 hello 不能污染当前代")
        links[2].deliverHello()
        try await replacement.value
        XCTAssertTrue(remote.isConnected)
        remote.disconnect()
    }

    func test_reconnectRestoresAttachAndSubscriptionsWhileOldGenerationIsIgnored() async throws {
        let identity = PairingDeviceIdentity.generate()
        let peerIdentity = PairingDeviceIdentity.generate()
        let backend = BackendRef(
            id: peerIdentity.id, displayName: "Mac",
            transport: .remote(url: "pendingcrew+tls://example.invalid:7443"))
        let configuration = RemoteBackendConfiguration(
            backend: backend, localIdentity: identity,
            peer: .init(backendID: backend.id, peerDeviceID: peerIdentity.id,
                        peerPublicSigningKey: peerIdentity.publicSigningKey,
                        preSharedKey: Data(repeating: 0x33, count: 32)))
        let summary = SessionSummary(
            sessionId: "s1", stateSeq: 1,
            state: .init(
                status: .running, isWorking: true, displayIsTyping: true,
                health: nil, pendingDecision: nil, kind: "claude_code",
                launchParameterProblem: nil, scrollState: nil),
            run: .init(
                crewId: "crew-1", role: "worker", title: "远端 worker",
                taskBrief: "test", workingDirectory: "/tmp", model: nil, effort: nil,
                pendingProfile: nil, approvalsReviewer: nil, permissionModeOverride: nil,
                startedAt: instant.timeIntervalSince1970, runStatus: "running",
                exitCode: nil, exitReason: nil, awaitingReply: nil))
        var links: [CrewRPCStubLink] = []
        let remote = RemotePendingCrewBackend(
            configuration: configuration,
            connector: ClosureSessionMessageLinkConnector {
                let link = CrewRPCStubLink()
                link.sessionList = .init(sessions: [summary])
                links.append(link)
                return link
            }, requestTimeout: 2)

        let whiteboardTask = Task { @MainActor in
            for await _ in remote.whiteboardChanges(crewId: "crew-1") { return }
        }
        defer { whiteboardTask.cancel(); remote.disconnect() }
        try await remote.connect()
        let channel = try await remote.openSession(sessionID: "s1")
        try await eventually { links[0].lastAttachedHandle != nil }
        let oldHandle = try XCTUnwrap(links[0].lastAttachedHandle)
        links[0].deliverTerminal(handle: oldHandle, text: "old-live")
        try await eventually { channel.terminalText.contains("old-live") }
        channel.setApprovals([.init(
            id: "old-approval", kind: "permission", sessionId: "s1", summary: "旧审批",
            status: "pending", reply: nil, decision: nil, createdAt: "now")], generation: 1)
        XCTAssertEqual(channel.pendingApprovals.map(\.id), ["old-approval"])

        links[0].failRemote()
        try await eventually {
            if case .disconnected = channel.connectionState { return true }
            return false
        }
        try await remote.connect()
        XCTAssertTrue(channel.pendingApprovals.isEmpty,
                      "新连接 bind 时必须先清掉仍可操作的旧审批")
        try await eventually {
            links.count == 2 && links[1].lastAttachedHandle != nil
                && links[1].subscribedCrewIDs.contains("crew-1")
                && links[1].approvalCrewIDs.contains("crew-1")
        }

        links[0].deliverTerminal(handle: oldHandle, text: "late-old")
        await Task.yield()
        XCTAssertFalse(channel.terminalText.contains("late-old"),
                       "旧连接迟到终端帧不得污染新一代")
        let newHandle = try XCTUnwrap(links[1].lastAttachedHandle)
        links[1].deliverTerminal(handle: newHandle, text: "new-live")
        try await eventually { channel.terminalText.contains("new-live") }
        XCTAssertEqual(links.count, 2, "重连只新建一条替代连接")
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
    private let automaticallyCompletesHello: Bool
    private var receivedHello: SessionAppHello?
    var sessionList: SessionList?
    private(set) var lastAttachedHandle: UInt32?
    private(set) var subscribedCrewIDs: Set<String> = []
    private(set) var approvalCrewIDs: Set<String> = []
    private(set) var sentInputBytes: [[UInt8]] = []
    private(set) var detachedHandles: [UInt32] = []
    private var nextHandle: UInt32 = 1
    var respondsToApprovalList = true

    init(automaticallyCompletesHello: Bool = true) {
        self.automaticallyCompletesHello = automaticallyCompletesHello
    }

    func send(_ framed: Data) {
        guard let message = try? codec.decodeApp(framed) else { return }
        if case let .hello(hello) = message {
            receivedHello = hello
            if automaticallyCompletesHello { deliverHello() }
        }
        if case .listSessions = message, let sessionList {
            deliver(.sessions(sessionList))
        }
        if case let .attach(value) = message {
            let handle = nextHandle
            nextHandle &+= 1
            lastAttachedHandle = handle
            deliver(.attached(.init(
                sessionId: value.sessionId, handle: handle, snapshotFrames: 0)))
        }
        if case let .input(value) = message { sentInputBytes.append(value.bytes) }
        if case let .detach(value) = message { detachedHandles.append(value.handle) }
        if case let .control(value) = message,
           case let .string(raw)? = value.arguments["payload"],
           let payload = Data(base64Encoded: raw) {
            if value.op == CrewRPC.Op.subscribeWhiteboard,
               let scope = try? JSONDecoder().decode(CrewRPC.CrewID.self, from: payload) {
                subscribedCrewIDs.insert(scope.crewId)
            } else if value.op == ApprovalRPC.Op.subscribe,
                      let scope = try? JSONDecoder().decode(ApprovalRPC.Scope.self, from: payload),
                      let requestID = value.requestId {
                approvalCrewIDs.insert(scope.crewId)
                let response = (try? JSONEncoder().encode(ApprovalRPC.Empty())) ?? Data("{}".utf8)
                deliver(.event(.init(
                    kind: ApprovalRPC.resultEvent, requestId: requestID,
                    fields: ["payload": .string(response.base64EncodedString())])))
            } else if value.op == ApprovalRPC.Op.list, respondsToApprovalList,
                      let requestID = value.requestId {
                let response = (try? JSONEncoder().encode([ApprovalItem]())) ?? Data("[]".utf8)
                deliver(.event(.init(
                    kind: ApprovalRPC.resultEvent, requestId: requestID,
                    fields: ["payload": .string(response.base64EncodedString())])))
            }
        }
        // Crew requests intentionally stay pending until the test disconnects or times out.
    }

    func close() { isOpen = false }

    func failRemote() {
        isOpen = false
        onClose?()
    }

    func deliverHello() {
        guard let hello = receivedHello else { return }
        let response = SessionDaemonMessage.hello(.init(
            protocolVersion: hello.protocolVersion, daemonBuild: "stub",
            capabilities: [CrewRPC.capability, ApprovalRPC.capability, "terminal-bytes"],
            sessionCount: 0, pid: 1))
        deliver(response)
    }

    func deliverTerminal(handle: UInt32, text: String) {
        deliver(.data(.init(handle: handle, bytes: Array(text.utf8))))
    }

    private func deliver(_ message: SessionDaemonMessage) {
        if let data = try? codec.encode(message) { onReceive?(data) }
    }
}
#endif
