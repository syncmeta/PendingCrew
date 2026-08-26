#if os(macOS)
import Foundation
import XCTest

/// `--daemon` 进程的三件事：单实例锁、socket 监听、崩溃善后。
///
/// **每一条都跑在临时目录上。** 这台机器上随时有一个真 PendingCrew 在跑，
/// 单测碰真 `Application Support` 就是活生生的双头（§6.2 要防的正是这个）。
@MainActor
final class SessionDaemonHostTests: XCTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        // socket 路径要短（`sun_path` 104 字节），所以不用 DerivedData 下的临时目录。
        directory = URL(fileURLWithPath: "/tmp/pcrew-d-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private func paths() -> PendingCrewDaemonPaths {
        .init(socket: directory.appendingPathComponent("d.sock").path,
              lock: directory.appendingPathComponent(SessionOrchestratorLock.fileName),
              registry: directory.appendingPathComponent("d.registry.json"),
              log: directory.appendingPathComponent("d.log"),
              socketFallbackReason: nil)
    }

    // MARK: - 单实例锁（§6.2 闸门 2）

    func test_第二个daemon拿不到锁并且知道谁在跑() throws {
        let paths = paths()
        let first = SessionDaemonHost(paths: paths)
        first.onCrewNotice = { _, _ in }
        try first.start()
        defer { first.stop() }

        let second = SessionDaemonHost(paths: paths)
        second.onCrewNotice = { _, _ in }
        XCTAssertThrowsError(try second.start()) { error in
            guard case let .alreadyOrchestrated(detail)? = error as? SessionDaemonHost.StartError else {
                return XCTFail("第二个 daemon 必须报 alreadyOrchestrated，实际 \(error)")
            }
            // 「谁占着」必须回答三样，缺一样人就得再查一轮。
            XCTAssertTrue(detail.contains("pid \(ProcessInfo.processInfo.processIdentifier)"), detail)
            XCTAssertTrue(detail.contains("启动于："), detail)
            XCTAssertTrue(detail.contains(directory.path), detail)
        }
        XCTAssertEqual(SessionDaemonControl.runningDaemonPid(paths: paths),
                       Int32(ProcessInfo.processInfo.processIdentifier))
    }

    func test_第一个停了之后第二个拿得到锁() throws {
        let paths = paths()
        let first = SessionDaemonHost(paths: paths)
        first.onCrewNotice = { _, _ in }
        try first.start()
        first.stop()

        XCTAssertNil(SessionDaemonControl.runningDaemonPid(paths: paths))
        let second = SessionDaemonHost(paths: paths)
        second.onCrewNotice = { _, _ in }
        XCTAssertNoThrow(try second.start())
        second.stop()
    }

    // MARK: - 监听

    func test_daemon起来之后viewer连得上并握得上手() throws {
        let paths = paths()
        let host = SessionDaemonHost(paths: paths)
        host.onCrewNotice = { _, _ in }
        try host.start()
        defer { host.stop() }

        let link = try UnixSocketTransport.connect(toPath: paths.socket)
        defer { link.close() }
        let client = SessionProtocolClient(
            link: link, capabilities: SessionDaemonHost.defaultCapabilities, appBuild: "test")
        client.connect()

        let deadline = Date().addingTimeInterval(5)
        while !client.isConnected, Date() < deadline {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(client.isConnected, "握手没完成")
        XCTAssertEqual(host.server.connectionCount, 1)
    }

    // MARK: - §8.2 崩溃善后（这一组是整条线上唯一「判错就杀掉无辜进程」的地方）

    /// 记录对得上 → 真的回收。
    func test_启动时把对得上账的孤儿回收掉() throws {
        let child = Process()
        child.executableURL = URL(fileURLWithPath: "/bin/sleep")
        child.arguments = ["60"]
        try child.run()
        defer { if child.isRunning { child.terminate() } }

        let identity = try XCTUnwrap(
            SessionOrphanReaper.identity(forRunning: child.processIdentifier))
        try writeRegistry(entries: [
            .init(sessionId: "s1", crewId: "c1", identity: identity),
        ])

        var notices: [(String, String)] = []
        let host = SessionDaemonHost(paths: paths())
        host.onCrewNotice = { notices.append(($0, $1)) }
        try host.start()
        defer { host.stop() }

        XCTAssertTrue(waitForExit(child), "对得上账的孤儿应当被回收")
        XCTAssertEqual(notices.map(\.0), ["c1"])
        XCTAssertTrue(notices[0].1.contains("被中断"), notices[0].1)
        XCTAssertFalse(FileManager.default.fileExists(atPath: paths().registry.path),
                       "核对完的 registry 该清掉，免得下一轮重放")
    }

    /// **这条是整个 P4 里最不能错的一条。**
    ///
    /// registry 记的 pid 现在属于**另一个**进程（启动时刻对不上）。守卫必须
    /// 一根手指都不碰它，并且把「我没有动它」说出来。
    func test_pid被复用时那个进程活得好好的而且白板上说了没动它() throws {
        let innocent = Process()
        innocent.executableURL = URL(fileURLWithPath: "/bin/sleep")
        innocent.arguments = ["60"]
        try innocent.run()
        defer { if innocent.isRunning { innocent.terminate() } }

        var recorded = try XCTUnwrap(
            SessionOrphanReaper.identity(forRunning: innocent.processIdentifier))
        // 同一个 pid，但「当初那个进程」启动得更早 —— 这就是 pid 复用的形状。
        recorded.startTimeSeconds -= 3600
        try writeRegistry(entries: [
            .init(sessionId: "s-old", crewId: "c1", identity: recorded),
        ])

        var notices: [(String, String)] = []
        let host = SessionDaemonHost(paths: paths())
        host.onCrewNotice = { notices.append(($0, $1)) }
        try host.start()
        defer { host.stop() }

        // 给回收动作充分的时间去发生（如果它错误地要发生的话）。
        for _ in 0..<20 {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(innocent.isRunning, "pid 被复用时绝不能动那个进程")
        XCTAssertEqual(notices.count, 1)
        XCTAssertTrue(notices[0].1.contains("我没有动它"), notices[0].1)

        let log = (try? String(contentsOf: paths().log, encoding: .utf8)) ?? ""
        XCTAssertTrue(log.contains("pid 被复用"), "日志里也要留痕：\n" + log)
    }

    func test_没有registry时不发任何通告() throws {
        var notices = 0
        let host = SessionDaemonHost(paths: paths())
        host.onCrewNotice = { _, _ in notices += 1 }
        try host.start()
        defer { host.stop() }
        XCTAssertEqual(notices, 0)
    }

    // MARK: -

    private func writeRegistry(entries: [SessionProcessRegistry.Entry]) throws {
        var registry = SessionProcessRegistry(daemonPid: 1, daemonStartedAt: Date())
        registry.entries = entries
        try JSONEncoder().encode(registry).write(to: paths().registry)
    }

    private func waitForExit(_ process: Process, timeout: TimeInterval = 5) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while process.isRunning, Date() < deadline {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
        }
        return !process.isRunning
    }
}
#endif
