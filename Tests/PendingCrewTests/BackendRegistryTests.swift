#if os(macOS)
import Foundation
import XCTest

/// 「我认识哪些后端」（人类 Todo #121）。
///
/// 这一层最要紧的三条都不是「能存能读」：
/// ① 本机那条**不是特例**、但也**删不掉**；
/// ② 远程那一档**绝不许静默降级成本机**；
/// ③ 存盘读不动时**不许显示成空列表**。
final class BackendRegistryTests: XCTestCase {

    private func tempURL() -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-backends-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        addTeardownBlock {
            chmod(dir.path, 0o755)
            try? FileManager.default.removeItem(at: dir)
        }
        return dir.appendingPathComponent("backends.json")
    }

    private func remote(_ id: String, url: String? = nil) -> BackendRef {
        BackendRef(id: id, displayName: id,
                   transport: .remote(url: url ?? "wss://\(id).example"))
    }

    // MARK: - 本机那条：是列表的一员，但删不掉

    func testTheLocalBackendIsAlwaysPresentAndFirst() {
        let refs = BackendRegistry.normalize([remote("a"), remote("b")])
        XCTAssertEqual(refs.first?.id, BackendRegistry.localId, "本机那条不在第一位")
        XCTAssertTrue(refs.first?.isBuiltIn == true)
        XCTAssertEqual(refs.count, 3)
    }

    func testTheLocalBackendCannotBeDeleted() {
        let refs = BackendRegistry.normalize([remote("a")])
        guard case let .refused(why) = BackendRegistry.removing(
            BackendRegistry.localId, from: refs) else {
            return XCTFail("内置的本机后端被删掉了 —— 删完这个界面没有任何后端可连")
        }
        XCTAssertTrue(why.contains("删不掉"), why)
    }

    func testOtherBackendsCanBeDeleted() {
        let refs = BackendRegistry.normalize([remote("a"), remote("b")])
        guard case let .removed(left) = BackendRegistry.removing("a", from: refs) else {
            return XCTFail("正常的后端删不掉")
        }
        XCTAssertEqual(left.map(\.id), [BackendRegistry.localId, "b"])
    }

    func testRemovingSomethingThatIsNotThereSaysSoRatherThanPretending() {
        guard case let .refused(why) = BackendRegistry.removing(
            "nope", from: BackendRegistry.normalize([])) else {
            return XCTFail("删一个不存在的却报成功")
        }
        XCTAssertTrue(why.contains("没有这个后端"), why)
    }

    /// 存盘里伪造一条 `isBuiltIn: true` 不作数 —— 内置只有一条，由我们说了算，
    /// 否则任何人往那个 json 里塞一行就能造出第二个「删不掉」的条目。
    func testAForgedBuiltInFlagIsIgnored() {
        var forged = remote("evil"); forged.isBuiltIn = true
        let refs = BackendRegistry.normalize([forged])
        XCTAssertEqual(refs.filter(\.isBuiltIn).map(\.id), [BackendRegistry.localId])
    }

    func testDuplicateIdsKeepOnlyTheFirst() {
        let refs = BackendRegistry.normalize([remote("a"), remote("a"), remote("b")])
        XCTAssertEqual(refs.map(\.id), [BackendRegistry.localId, "a", "b"])
    }

    // MARK: - 远程：只有安全地址 + 已配对身份才可连，而且绝不降级

    /// **这条是这个文件的重点。** 静默降级的症状是：人填了远程地址、界面显示
    /// 「已连接」，而他看到的其实是自己这台机器上的 session。那种错查不出来。
    func testUnpairedRemoteIsRefusedAndNeverFallsBackToLocal() {
        guard case let .unsupported(why) = BackendRegistry.connectivity(
            of: remote("tokyo", url: "pendingcrew+tls://tokyo.example:7443"),
            trustedPeers: []) else {
            return XCTFail("没配对的远程后端被当成可连")
        }
        XCTAssertTrue(why.contains("没有退回本机"), "没说清不会降级：\(why)")
        XCTAssertTrue(why.contains("配对"), "没说清缺的是信任记录：\(why)")
    }

    func testPairedRemoteWithSecureAddressBecomesConnectable() throws {
        let peer = PairingDeviceIdentity.generate()
        let trust = PeerTrustRecord(
            backendID: "tokyo", peerDeviceID: peer.id,
            peerPublicSigningKey: peer.publicSigningKey,
            preSharedKey: Data(repeating: 0x71, count: 32))
        XCTAssertEqual(
            BackendRegistry.connectivity(
                of: remote("tokyo", url: "pendingcrew+tls://127.0.0.1:7443"),
                trustedPeers: [trust]),
            .supported)
    }

    func testPairedRemoteStillRejectsAnInsecureAddress() throws {
        let peer = PairingDeviceIdentity.generate()
        let trust = PeerTrustRecord(
            backendID: "tokyo", peerDeviceID: peer.id,
            peerPublicSigningKey: peer.publicSigningKey,
            preSharedKey: Data(repeating: 0x71, count: 32))
        guard case let .unsupported(why) = BackendRegistry.connectivity(
            of: remote("tokyo", url: "tcp://127.0.0.1:7443"),
            trustedPeers: [trust]) else {
            return XCTFail("明文 TCP 地址被当成可连")
        }
        XCTAssertTrue(why.contains("安全"), why)
        XCTAssertTrue(why.contains("没有退回本机"), why)
    }

    func testLocalIsConnectable() {
        XCTAssertEqual(BackendRegistry.connectivity(of: BackendRegistry.builtInLocal()),
                       .supported)
    }

    // MARK: - 存 / 读

    func testSaveThenLoadRoundTrips() throws {
        let url = tempURL()
        try BackendRegistry.save(BackendRegistry.normalize([remote("a"), remote("b")]), to: url)
        guard case let .loaded(refs) = BackendRegistry.load(from: url) else {
            return XCTFail("存完读不回来")
        }
        XCTAssertEqual(refs.map(\.id), [BackendRegistry.localId, "a", "b"])
        XCTAssertTrue(refs[1].isRemote)
    }

    /// 内置那条**不写进存盘文件** —— 它每次现算；写进去会在 `PENDINGCREW_DATA_DIR`
    /// 挪走之后变成一条指向旧 socket 路径的假记录。
    func testTheBuiltInEntryIsNotPersisted() throws {
        let url = tempURL()
        try BackendRegistry.save(BackendRegistry.normalize([remote("a")]), to: url)
        let raw = try String(contentsOf: url, encoding: .utf8)
        XCTAssertFalse(raw.contains("\"\(BackendRegistry.localId)\""),
                       "内置那条被写进盘了：\(raw)")
    }

    func testNoFileYetIsFreshNotAnError() {
        guard case let .fresh(refs) = BackendRegistry.load(from: tempURL()) else {
            return XCTFail("全新机器被报成了错误")
        }
        XCTAssertEqual(refs.map(\.id), [BackendRegistry.localId])
    }

    /// **读不动不许显示成空列表** —— 人刚加过三个，界面却干干净净，
    /// 他会以为自己的配置被清了。
    func testUnreadableFileStillShowsLocalAndSaysWhy() throws {
        try XCTSkipIf(getuid() == 0, "root 绕过权限位")
        let url = tempURL()
        try BackendRegistry.save(BackendRegistry.normalize([remote("a")]), to: url)
        XCTAssertEqual(chmod(url.deletingLastPathComponent().path, 0), 0)
        defer { chmod(url.deletingLastPathComponent().path, 0o755) }

        let load = BackendRegistry.load(from: url)
        XCTAssertEqual(load.refs.map(\.id), [BackendRegistry.localId],
                       "读不动时连本机那条都没给 —— 界面会变成「一个后端都没有」")
        XCTAssertNotNil(load.problem, "读不动却一声不吭")
        XCTAssertTrue(load.problem?.contains("没有被删") == true, load.problem ?? "")
    }

    func testCorruptFileIsDegradedNotFresh() throws {
        let url = tempURL()
        try Data("{ 这不是 JSON".utf8).write(to: url)
        let load = BackendRegistry.load(from: url)
        XCTAssertNotNil(load.problem, "解不开却被当成「全新机器」")
        XCTAssertEqual(load.refs.map(\.id), [BackendRegistry.localId])
    }

    /// 不认识的 `kind` 要解码失败（而不是被猜成本机），这样上面那条降级才会生效。
    func testAnUnknownTransportKindFailsToDecodeRatherThanGuessing() throws {
        let url = tempURL()
        try Data(#"[{"id":"x","displayName":"x","kind":"carrierPigeon","address":"?"}]"#.utf8)
            .write(to: url)
        XCTAssertNotNil(BackendRegistry.load(from: url).problem,
                        "不认识的传输方式被猜成了某种能连的东西")
    }

    func testPersistedRemoteSelectionResolvesAndMissingRemoteFailsClosed() throws {
        let registry = tempURL()
        let selection = registry.deletingLastPathComponent().appendingPathComponent("selection.json")
        let tokyo = remote("tokyo", url: "pendingcrew+tls://127.0.0.1:7443")
        try BackendRegistry.save([tokyo], to: registry)
        try BackendRegistry.select(tokyo, selectionFile: selection)

        XCTAssertEqual(
            BackendRegistry.selectedBackend(registryFile: registry, selectionFile: selection),
            .selected(tokyo))

        try FileManager.default.removeItem(at: registry)
        guard case let .unavailable(reason) = BackendRegistry.selectedBackend(
            registryFile: registry, selectionFile: selection) else {
            return XCTFail("显式选择消失后被静默退回本机")
        }
        XCTAssertTrue(reason.contains("没有退回本机"), reason)
    }

    func testNoSelectionYetDefaultsToBuiltInLocal() {
        let registry = tempURL()
        let selection = registry.deletingLastPathComponent().appendingPathComponent("selection.json")
        guard case let .selected(ref) = BackendRegistry.selectedBackend(
            registryFile: registry, selectionFile: selection) else {
            return XCTFail("全新安装应该从内置本机开始")
        }
        XCTAssertEqual(ref.id, BackendRegistry.localId)
    }
}

final class DevicePairingIdentityTests: XCTestCase {
    private func tempDirectory() -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-device-identity-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }

    func testLocalIdentityIsPersistentAndPrivateOnDisk() throws {
        let url = tempDirectory().appendingPathComponent("identity.json")
        let first = try DeviceIdentityStore.loadOrCreate(at: url)
        let second = try DeviceIdentityStore.loadOrCreate(at: url)

        XCTAssertEqual(first, second, "第二次启动换了身份，既有配对会全部失效")
        XCTAssertEqual(first.privateSigningKey.count, 32)
        XCTAssertEqual(first.publicSigningKey.count, 32)
        XCTAssertFalse(first.id.isEmpty)
        let mode = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? NSNumber)
        XCTAssertEqual(mode.intValue & 0o777, 0o600, "长期私钥文件必须只让本用户读写")

        let broken = Data("{ broken".utf8)
        try broken.write(to: url)
        XCTAssertThrowsError(try DeviceIdentityStore.loadOrCreate(at: url),
                             "身份损坏时不能静默生成新身份继续连接")
        XCTAssertEqual(try Data(contentsOf: url), broken, "损坏身份必须保留给诊断，不能覆盖")
    }

    func testPeerTrustRoundTripsAndCorruptionFailsClosed() throws {
        let url = tempDirectory().appendingPathComponent("trusted-peers.json")
        let peer = PairingDeviceIdentity.generate()
        let record = PeerTrustRecord(
            backendID: "office-mac", peerDeviceID: peer.id,
            peerPublicSigningKey: peer.publicSigningKey,
            preSharedKey: Data(repeating: 0x42, count: 32))

        try PeerTrustStore.save([record], to: url)
        XCTAssertEqual(try PeerTrustStore.load(from: url), [record])
        let mode = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? NSNumber)
        XCTAssertEqual(mode.intValue & 0o777, 0o600, "PSK 信任账本必须只让本用户读写")

        try Data("{ broken".utf8).write(to: url)
        XCTAssertThrowsError(try PeerTrustStore.load(from: url),
                             "损坏的信任账本不能被当成空账本后继续连接")
    }
}

/// 设置「后端」页每一行的实况与「重启后台」按钮。
///
/// 重点和 `BackendRegistryTests` 同一条：**本机探针的读数绝不许填到别的条目上**。
@MainActor
final class BackendLiveStatusTests: XCTestCase {

    private let paths = PendingCrewDaemonPaths.standard(
        dataRoot: URL(fileURLWithPath: "/tmp/pc-live-status-tests", isDirectory: true))

    /// 一碰就红的探针 —— 用来证明「根本没去探」。
    private var mustNotProbe: BackendRegistry.LocalProbe {
        .init(daemonIsRunning: { XCTFail("不该探本机后台"); return true },
              query: { XCTFail("不该跟本机后台握手"); throw CocoaError(.featureUnsupported) })
    }

    private func probe(running: Bool = true,
                       _ result: @escaping () throws -> SessionDaemonStatusSnapshot)
        -> BackendRegistry.LocalProbe {
        .init(daemonIsRunning: { running }, query: result)
    }

    private func snapshot(build: String = "0.1.40(1)",
                          statuses: [SessionWireStatus]) -> SessionDaemonStatusSnapshot {
        SessionDaemonStatusSnapshot(
            hello: .init(protocolVersion: 1, daemonBuild: build, capabilities: [],
                         sessionCount: statuses.count, pid: 777, viewerCount: 1, startedAt: 0),
            sessions: statuses.enumerated().map { index, status in
                SessionSummary(sessionId: "s\(index)", stateSeq: 1,
                               state: .init(status: status, isWorking: false,
                                            displayIsTyping: false, health: nil,
                                            pendingDecision: nil, kind: "claude_code",
                                            launchParameterProblem: nil, scrollState: nil))
            })
    }

    // MARK: - 实况：管不了的条目绝不探本机

    func testRemoteIsNeverProbedLocally() {
        let ref = BackendRef(id: "tokyo", displayName: "东京",
                             transport: .remote(url: "wss://tokyo.example"))
        guard case let .unsupported(why) = BackendRegistry.liveStatus(
            of: ref, paths: paths, probe: mustNotProbe) else {
            return XCTFail("远程条目给出了实况 —— 那只可能是本机的读数")
        }
        XCTAssertTrue(why.contains("没有退回本机"), why)
    }

    /// 一条 socket 条目、但不是本数据根那一个：探针只认得本数据根，不许拿它的读数冒充。
    func testAForeignSocketIsNeverFilledWithTheLocalReading() {
        let ref = BackendRef(id: "other", displayName: "别处",
                             transport: .localSocket(path: "/tmp/somewhere-else.sock"))
        guard case let .unsupported(why) = BackendRegistry.liveStatus(
            of: ref, paths: paths, probe: mustNotProbe) else {
            return XCTFail("别的 socket 条目被填上了本机后台的实况")
        }
        XCTAssertTrue(why.contains("不会拿后者的实况冒充它"), why)
    }

    // MARK: - 实况：本机那条走同一个函数

    func testNotRunningDoesNotHandshake() {
        let status = BackendRegistry.liveStatus(
            of: BackendRegistry.builtInLocal(paths: paths), paths: paths,
            probe: .init(daemonIsRunning: { false },
                         query: { XCTFail("没在跑还去握手"); throw CocoaError(.featureUnsupported) }))
        XCTAssertEqual(status, .notRunning)
    }

    func testAHandshakeFailureIsUndecidableNotNotRunning() {
        let status = BackendRegistry.liveStatus(
            of: BackendRegistry.builtInLocal(paths: paths), paths: paths,
            probe: probe { throw SessionDaemonStatusProbe.ProbeError.timeout })
        guard case let .undecidable(why) = status else {
            return XCTFail("问不出被说成了别的：\(status) —— 「没在跑」会让人以为什么都不会被打断")
        }
        XCTAssertTrue(why.contains("超时"), why)
    }

    /// **已退出的不算「在跑」**。后台的 records 退出后照旧留着，`hello.sessionCount` 把它们也数进去。
    func testExitedSessionsAreNotCountedAsRunning() {
        let snap = snapshot(statuses: [.running, .running, .exited(0)])
        XCTAssertEqual(snap.hello.sessionCount, 3, "前提：握手里的数混着已退出的")
        XCTAssertEqual(BackendRegistry.runningSessionCount(in: snap), 2)
        XCTAssertEqual(
            BackendRegistry.liveStatus(of: BackendRegistry.builtInLocal(paths: paths),
                                       paths: paths, probe: probe { snap }),
            .running(build: "0.1.40(1)", pid: 777, runningSessions: 2, retainedSessions: 1))
    }

    // MARK: - 重启入口

    private func action(_ status: BackendLiveStatus, app: String = "0.1.40(1)",
                        viewer: Bool = true) -> (title: String, text: String)? {
        guard case let .available(title, text) = BackendRegistry.restartAction(
            for: status, appBuild: app, interfaceRelaunchesBackend: viewer) else { return nil }
        return (title, text)
    }

    /// viewer 里「停」完界面会马上再拉一个 —— 叫「停用」就是在骗人。
    func testInAViewerItIsARestartNotAStop() {
        let a = action(.running(build: "0.1.40(1)", pid: 1, runningSessions: 2, retainedSessions: 0))
        XCTAssertEqual(a?.title, "重启后台")
        XCTAssertTrue(a?.text.contains("会打断正在跑的 2 个 session") == true, a?.text ?? "")
        XCTAssertTrue(a?.text.contains("马上拉起") == true, a?.text ?? "")
        XCTAssertTrue(a?.text.contains("不会自动接回") == true,
                      "同版重启是人自己按的，规格是不问 —— 得提前说清：\(a?.text ?? "")")
    }

    func testADifferentBuildIsAReplacementAndPromisesToAsk() {
        let a = action(.running(build: "0.1.34(9)", pid: 1, runningSessions: 3, retainedSessions: 0))
        XCTAssertEqual(a?.title, "换成 0.1.40(1)")
        XCTAssertTrue(a?.text.contains("换完会问你") == true, a?.text ?? "")
    }

    func testOutsideAViewerItIsAStopAndSaysItStaysStopped() {
        let a = action(.running(build: "0.1.40(1)", pid: 1, runningSessions: 1, retainedSessions: 0),
                       viewer: false)
        XCTAssertEqual(a?.title, "停止后台")
        XCTAssertTrue(a?.text.contains("不会自动再起") == true, a?.text ?? "")
    }

    /// 没东西在跑时**不许吓人**，也别说「打断 0 个」。
    func testNothingRunningDoesNotScare() {
        let a = action(.running(build: "0.1.40(1)", pid: 1, runningSessions: 0, retainedSessions: 4))
        XCTAssertTrue(a?.text.contains("不会打断任何东西") == true, a?.text ?? "")
        XCTAssertFalse(a?.text.contains("0 个") == true, a?.text ?? "")
    }

    /// 卡死、握手超时的后台最需要重启 —— 仍给按，但照实说说不清会打断几个。
    func testUndecidableStillOffersRestartButAdmitsItCannotCount() {
        let a = action(.undecidable("握手超时"))
        XCTAssertNotNil(a, "问不出的后台不给重启 —— 那正是最需要重启的那种")
        XCTAssertTrue(a?.text.contains("说不清会打断几个") == true, a?.text ?? "")
    }

    func testUnsupportedAndNotRunningAreUnavailableWithTheReason() {
        XCTAssertEqual(BackendRegistry.restartAction(for: .unsupported("远程还没做"),
                                                     appBuild: "x", interfaceRelaunchesBackend: true),
                       .unavailable("远程还没做"))
        guard case .unavailable = BackendRegistry.restartAction(
            for: .notRunning, appBuild: "x", interfaceRelaunchesBackend: true) else {
            return XCTFail("没在跑的后台给了重启按钮")
        }
    }
}
#endif
