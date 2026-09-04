#if os(macOS)
import XCTest

/// **连不上之后那一串动作的顺序**（设计 §9.2）——判据本身在
/// `OrchestrationFallbackTests`，这一组盯的是**执行**：什么时候才许去取锁、
/// 接管时那条 viewer 腿有没有真的停、没接管时锁有没有真的放回去。
///
/// 为什么要单独一组：这三件事任何一件做错，症状都不是「报错」而是「安静地坏」——
/// 取锁太早会让我们**自己刚拉起来的那个 daemon 因为锁被占而当场退出**（然后永远
/// 连不上，且看不出为什么）；接管后不停 viewer 腿会在下一次重连成功时变成
/// 「本地编排 + 连上的 daemon」两个 host；没接管却把锁攥着不放，则是让真正的
/// daemon 永远起不来。
@MainActor
final class OrchestrationFallbackCoordinatorTests: XCTestCase {

    private var dataRoot: URL!
    /// 记录协调器都干了些什么。
    private var acquireCalls = 0
    private var tookOver: String?
    private var stoppedLeg = false
    private var scheduledReconnect = false
    /// 被接管拿走的那把锁（测试自己持有，免得当场释放）。
    private var takenHandle: SessionOrchestratorLock.Handle?

    override func setUpWithError() throws {
        dataRoot = URL(fileURLWithPath: "/tmp/pcrew-fbcoord-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        takenHandle = nil
        try? FileManager.default.removeItem(at: dataRoot)
    }

    private func makeCoordinator() -> OrchestrationFallbackCoordinator {
        OrchestrationFallbackCoordinator(
            dataRoot: dataRoot,
            hooks: .init(
                acquireLock: { [weak self] root in
                    self?.acquireCalls += 1
                    return SessionOrchestratorLock.acquire(dataRoot: root, kind: "app")
                },
                takeOver: { [weak self] handle, reason in
                    self?.takenHandle = handle
                    self?.tookOver = reason
                },
                stopViewerLeg: { [weak self] in self?.stoppedLeg = true },
                scheduleReconnect: { [weak self] in self?.scheduledReconnect = true }))
    }

    /// **锁只在拉 daemon 明确失败之后才许取。** 后台刚被拉起来、还在等它监听的
    /// 那一段里去抢锁，会让它自己当场退出 —— 我们把自己的后路堵死，而症状只是
    /// 「一直连不上」。
    func test_后台刚起来还没连上时不许去抢锁() {
        let coordinator = makeCoordinator()
        let decision = coordinator.handle(spawn: .launched, linkFailure: nil)

        XCTAssertEqual(acquireCalls, 0, "这一步根本不该去碰锁 —— 抢了它，我们刚拉起来的 daemon 就起不来了")
        guard case .keepConnecting = decision else { return XCTFail("实际：\(decision)") }
        XCTAssertTrue(scheduledReconnect, "没接管就要接着重连，不能停在那儿")
        XCTAssertNil(tookOver)
        XCTAssertFalse(stoppedLeg)
    }

    /// 对端回过话（协议不兼容 / 握手 / attach 失败）—— 同样不许碰锁：
    /// 能回话就说明那边有东西在。
    func test_对端回过话时不许去抢锁() {
        let coordinator = makeCoordinator()
        let decision = coordinator.handle(
            spawn: .launched, linkFailure: .protocolIncompatible("版本对不上"))

        XCTAssertEqual(acquireCalls, 0)
        guard case .refuse = decision else { return XCTFail("实际：\(decision)") }
        XCTAssertNil(tookOver)
    }

    /// 唯一允许的那一支：拉 daemon 明确失败 → 取锁 → 到手 → 接管，
    /// **并且把 viewer 那条腿整个停掉**（不停的话下一次重连成功就是两个 host）。
    func test_接管时必须把viewer那条腿停掉且不再重连() {
        let coordinator = makeCoordinator()
        let decision = coordinator.handle(spawn: .failed("拉不起来"), linkFailure: nil)

        XCTAssertEqual(acquireCalls, 1)
        guard case .takeOverLocally = decision else { return XCTFail("实际：\(decision)") }
        XCTAssertNotNil(tookOver, "接管要把锁交出去 —— 没有锁的接管就是无凭据的双头")
        XCTAssertTrue(stoppedLeg, "接管之后还留着重连 = 迟早出现「本地编排 + 连上的 daemon」")
        XCTAssertFalse(scheduledReconnect, "接管之后不该再安排重连")
    }

    /// 拉 daemon 失败、但锁被别人占着 —— **不接管，而且刚才那次取锁不许留下占用**。
    /// 攥着不放会让真正的 daemon 永远起不来，那是我们自己造的死结。
    func test_没接管时不许把锁攥着不放() throws {
        // 先让别人占着（另一个 app 窗口这种，不听 socket）。
        var incumbent: SessionOrchestratorLock.Handle?
        switch SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: "app") {
        case let .acquired(handle): incumbent = handle
        case let other: return XCTFail("前提不成立：\(other)")
        }

        let coordinator = makeCoordinator()
        let decision = coordinator.handle(spawn: .failed("拉不起来"), linkFailure: nil)
        guard case .refuse = decision else { return XCTFail("实际：\(decision)") }
        XCTAssertNil(tookOver)
        XCTAssertTrue(scheduledReconnect)

        // 占用者放手 —— 这之后锁必须**真的**空出来。协调器刚才那次取锁若把句柄
        // 留在了什么地方，这里就拿不到，而生产上的症状是「真正的 daemon 永远起不来」。
        incumbent = nil
        XCTAssertNil(incumbent)
        switch SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: "probe") {
        case .acquired:
            break
        case let other:
            XCTFail("占用者已经放手，锁却还被占着 —— 协调器把它攥着没放：\(other)")
        }
    }

}
#endif
