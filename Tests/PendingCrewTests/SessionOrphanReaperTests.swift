#if os(macOS)
import Darwin
import XCTest

/// §8.2 的双重核对。**这一组是这条线上唯一一处「判错就是杀掉一个无辜进程」的地方**，
/// 所以判定那一半是纯函数，且每一种分支都在这里钉着。
final class SessionOrphanReaperTests: XCTestCase {

    private func identity(pid: Int32 = 4242, pgid: Int32 = 4242,
                          sec: Int64 = 1_700_000_000, usec: Int32 = 123_456,
                          command: String = "claude") -> SessionProcessIdentity {
        .init(pid: pid, pgid: pgid, startTimeSeconds: sec,
              startTimeMicroseconds: usec, command: command)
    }

    func test_pid已不在只记账() {
        XCTAssertEqual(SessionOrphanReaper.decide(recorded: identity(), current: nil),
                       .alreadyGone)
    }

    func test_启动时刻一致才认作我们的孤儿() {
        XCTAssertEqual(SessionOrphanReaper.decide(recorded: identity(), current: identity()),
                       .reap)
    }

    /// 病根就在这一条：pid 在、名字也一样，**只有启动时刻对不上**。
    func test_pid复用时绝不动手() {
        let reused = identity(sec: 1_700_009_999)
        guard case let .pidReused(current) = SessionOrphanReaper.decide(
            recorded: identity(), current: reused) else {
            return XCTFail("启动时刻不一致必须判 pidReused")
        }
        XCTAssertEqual(current, reused)
    }

    /// **微秒也算数。** 秒对上、微秒对不上仍然是另一个进程 —— 这条如果被「差不多
    /// 就行」松掉，误杀窗口会从「同一微秒」放大到「同一秒」，而进程在同一秒内
    /// 起两个是家常便饭。
    func test_只差微秒也判复用() {
        let almost = identity(usec: 123_457)
        guard case .pidReused = SessionOrphanReaper.decide(
            recorded: identity(), current: almost) else {
            return XCTFail("微秒不一致必须判 pidReused")
        }
    }

    func test_只有reap会真的发信号() {
        XCTAssertFalse(SessionOrphanReaper.apply(.alreadyGone, recorded: identity()))
        XCTAssertFalse(SessionOrphanReaper.apply(
            .pidReused(current: identity(sec: 1)), recorded: identity()))
    }

    /// 说明必须说清「我没动手」—— 静默留一个孤儿和静默杀一个无辜进程一样查不出来。
    func test_pid复用的说明里必须写着没有动它() {
        let text = SessionOrphanReaper.describe(
            sessionId: "s1", recorded: identity(),
            decision: .pidReused(current: identity(sec: 1_700_009_999, command: "zsh")))
        XCTAssertTrue(text.contains("pid 被复用"), text)
        XCTAssertTrue(text.contains("我没有动它"), text)
        XCTAssertTrue(text.contains("zsh"), text)
    }

    // MARK: - 真 sysctl

    func test_probe读得到本进程且与getpgid一致() throws {
        let me = getpid()
        let identity = try XCTUnwrap(SessionOrphanReaper.probe(pid: me))
        XCTAssertEqual(identity.pid, me)
        XCTAssertEqual(identity.pgid, getpgid(me))
        XCTAssertGreaterThan(identity.startTimeSeconds, 1_500_000_000)
        XCTAssertFalse(identity.command.isEmpty)
    }

    func test_probe对不存在的pid返回nil() {
        // 找一个当前肯定没被占用的 pid：从上限往下扫。
        var candidate: Int32 = 99_990
        while candidate > 90_000, SessionOrphanReaper.probe(pid: candidate) != nil {
            candidate -= 1
        }
        XCTAssertNil(SessionOrphanReaper.probe(pid: candidate))
        XCTAssertEqual(SessionOrphanReaper.decide(
            recorded: identity(pid: candidate),
            current: SessionOrphanReaper.probe(pid: candidate)), .alreadyGone)
    }

    /// **端到端：对着一个真进程验那道守卫。**
    ///
    /// 纯函数测的是判定表；这条测的是「记下来的那份身份真的能把同一个进程认回来」，
    /// 以及**同一个活进程、只把记录里的启动时刻挪一微秒，守卫就不肯动手** ——
    /// 那正是 pid 复用发生时的形状（pid 在、进程活着、只是不是当初那个）。
    func test_对着真进程验守卫_同一个才认挪一微秒就不认() throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sleep")
        process.arguments = ["30"]
        try process.run()
        defer { if process.isRunning { process.terminate() } }

        let live = try XCTUnwrap(
            SessionOrphanReaper.identity(forRunning: process.processIdentifier))
        XCTAssertEqual(SessionOrphanReaper.decide(recorded: live, current: live), .reap)

        var stale = live
        stale.startTimeMicroseconds &+= 1
        guard case .pidReused = SessionOrphanReaper.decide(recorded: stale, current: live) else {
            return XCTFail("记录与实际差一微秒时必须判 pidReused —— 这就是 pid 复用的形状")
        }

        process.terminate()
        process.waitUntilExit()
    }

    // MARK: - registry

    func test_registry按sessionId去重并可遗忘() {
        var registry = SessionProcessRegistry()
        registry.record(sessionId: "s", crewId: "c", identity: identity(pid: 1))
        registry.record(sessionId: "s", crewId: "c", identity: identity(pid: 2))
        XCTAssertEqual(registry.entries.map(\.identity.pid), [2])
        registry.forget(sessionId: "s")
        XCTAssertTrue(registry.entries.isEmpty)
    }

    func test_registry可往返编码() throws {
        var registry = SessionProcessRegistry(daemonPid: 7, daemonStartedAt: Date(timeIntervalSince1970: 10))
        registry.record(sessionId: "s", crewId: "c", identity: identity())
        let data = try JSONEncoder().encode(registry)
        XCTAssertEqual(try JSONDecoder().decode(SessionProcessRegistry.self, from: data), registry)
    }
}
#endif
