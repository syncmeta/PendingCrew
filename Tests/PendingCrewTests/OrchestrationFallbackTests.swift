#if os(macOS)
import XCTest

/// **§9.2 那张表，一行一条测试。**
///
/// 这一组守的不变量只有一句：**「连不上后台」永远不足以成为自己接管的理由。**
/// 只有当本进程**确实拿到了独占编排锁**（= 确定没有别人在编排）**且**拉 daemon
/// **确实失败**时，才允许回退本地编排。其余每一种观测 —— 包括「锁被占着但读不出
/// 是谁」这种看起来最像「那就我来吧」的情形 —— 一律禁止静默接管。
///
/// 为什么这条值得一整组测试：两种翻车方向相反，各自都很贵。只连不退 = 用户双击
/// 图标、界面在、什么都不动、不报错；连不上就接管 = 那边其实有 daemon 时当场双头，
/// 两个进程写同一批账、同一批唤醒发两遍，事后极难定位。
final class OrchestrationFallbackTests: XCTestCase {

    private var dataRoot: URL!

    override func setUpWithError() throws {
        dataRoot = URL(fileURLWithPath: "/tmp/pcrew-fallback-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dataRoot)
    }

    private func holder(kind: String) -> SessionOrchestratorLock.Holder {
        .init(kind: kind, pid: 4242, startTimeSeconds: 1, startTimeMicroseconds: 2,
              dataRoot: dataRoot.path, acquiredAt: Date())
    }

    private func decide(_ lock: SessionOrchestratorLock.Outcome?,
                        spawn: OrchestrationFallback.Spawn = .failed("拉不起来"),
                        link: OrchestrationFallback.LinkFailure? = nil)
        -> OrchestrationFallback.Decision {
        OrchestrationFallback.decide(lock: lock, spawn: spawn, linkFailure: link,
                                     dataRoot: dataRoot)
    }

    // MARK: - 唯一允许的那一支

    /// 锁**到手**（= 确定没有别人在编排）且拉 daemon **确实失败** —— 只有这一支允许。
    func test_锁到手且拉不起后台时允许临时本地接管() throws {
        let outcome = SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: "app")
        guard case .acquired = outcome else {
            return XCTFail("测试自己都没拿到锁，前提不成立：\(outcome)")
        }
        guard case let .takeOverLocally(reason) = decide(outcome) else {
            return XCTFail("这是表里唯一允许接管的一支，实际：\(decide(outcome))")
        }
        XCTAssertTrue(reason.contains("接管"), reason)
    }

    /// 锁到手，但 daemon **起来了**（只是还没连上）—— 不许接管，继续重连。
    /// 「进程起来了」和「连得上」是两件事，把前者当后者正是这条线上的老毛病。
    func test_锁到手但后台已经起来时不许接管() throws {
        let outcome = SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: "app")
        guard case .acquired = outcome else {
            return XCTFail("前提不成立：\(outcome)")
        }
        guard case .keepConnecting = decide(outcome, spawn: .launched) else {
            return XCTFail("后台进程已经起来了，不构成接管的理由，实际：\(decide(outcome, spawn: .launched))")
        }
    }

    // MARK: - 一律禁止的那几支

    /// 锁被一个 **daemon** 占着：那边真的在编排，这次没连上是链路的事。
    func test_锁被daemon占着时继续重连而不是接管() {
        guard case .keepConnecting = decide(.heldBy(holder(kind: "daemon"))) else {
            return XCTFail("锁被 daemon 占着还接管 = 当场双头，实际：\(decide(.heldBy(holder(kind: "daemon"))))")
        }
    }

    /// **这一行是整张表的重点。** 锁被占着、但读不出是谁 —— 归属不明。
    ///
    /// 「读不出是谁」最像「那大概没人在管，我来吧」，而它恰恰是最危险的一种：
    /// 读不出不等于没有。把这一行改成允许，这条测试当场红。
    func test_归属不明时禁止静默接管() {
        let decision = decide(.heldBy(nil))
        guard case let .refuse(detail) = decision else {
            return XCTFail("归属不明 = 不许接管（读不出是谁≠没有人），实际：\(decision)")
        }
        XCTAssertTrue(detail.contains("读不出") || detail.contains("归属"),
                      "错误要说清卡在哪：\(detail)")
    }

    /// 锁被一个**不听 socket** 的东西占着（另一个 inproc 窗口、崩到一半的进程）。
    func test_锁被非daemon占着时是冲突不是接管() {
        guard case .refuse = decide(.heldBy(holder(kind: "app"))) else {
            return XCTFail("实际：\(decide(.heldBy(holder(kind: "app"))))")
        }
    }

    /// 锁文件打不开 —— 拿不到锁就没资格当唯一所有者（锁自己的注释里就是这么写的）。
    func test_锁文件打不开时禁止接管() {
        let decision = decide(.unavailable("目录不可写"))
        guard case let .refuse(detail) = decision else {
            return XCTFail("实际：\(decision)")
        }
        XCTAssertTrue(detail.contains("目录不可写"), "原因要原样带出来：\(detail)")
    }

    /// 连上了但协议不兼容：**能回话的对端说明那边有东西在**，接管就是双头。
    /// （§4.4 的能力集取交集是另一回事，不许因此自己接管。）
    func test_协议不兼容时禁止接管() {
        let decision = decide(nil, spawn: .launched,
                              link: .protocolIncompatible("daemon 协议 2 / app 协议 1"))
        guard case let .refuse(detail) = decision else { return XCTFail("实际：\(decision)") }
        XCTAssertTrue(detail.contains("协议"), detail)
    }

    /// 连上了但握手没完成 —— 同上，可操作错误 + 重试，不接管。
    func test_握手失败时禁止接管() {
        let decision = decide(nil, spawn: .launched, link: .handshakeFailed("5 秒没回应"))
        guard case .refuse = decision else { return XCTFail("实际：\(decision)") }
    }

    /// 连上了但 attach 失败 —— 同上。
    func test_attach失败时禁止接管() {
        let decision = decide(nil, spawn: .launched, link: .attachFailed("拿不到快照"))
        guard case .refuse = decision else { return XCTFail("实际：\(decision)") }
    }

    /// 链路层的失败**压过**锁的观测：哪怕锁这一刻恰好到手，只要对端刚才回过话，
    /// 就说明那边有东西在，不许接管。
    func test_链路失败压过锁到手() throws {
        let outcome = SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: "app")
        guard case .acquired = outcome else { return XCTFail("前提不成立：\(outcome)") }
        let decision = decide(outcome, spawn: .failed("拉不起来"),
                              link: .handshakeFailed("对端不回话"))
        guard case .refuse = decision else {
            return XCTFail("对端回过话就不许接管，实际：\(decision)")
        }
    }

    // MARK: - 负向对照

    /// **这一组的验法本身。** 上面每一条 `refuse` 都是「把它改成允许就当场红」的形状：
    /// 这里把那句话变成可执行的 —— 一个「连不上就接管」的天真实现，喂进表里那几行
    /// 禁止的观测，**必须**与真实现给出不同的结论。哪天有人把契约削弱成天真版，
    /// 这条会连同上面那几条一起红。
    func test_天真版连不上就接管与契约在每一行禁止项上都不同() {
        func naive(_ spawn: OrchestrationFallback.Spawn) -> OrchestrationFallback.Decision {
            if case let .failed(reason) = spawn { return .takeOverLocally(reason) }
            return .keepConnecting("重连中")
        }
        let forbidden: [(String, SessionOrchestratorLock.Outcome?, OrchestrationFallback.LinkFailure?)] = [
            ("锁被 daemon 占着", .heldBy(holder(kind: "daemon")), nil),
            ("归属不明", .heldBy(nil), nil),
            ("锁被非 daemon 占着", .heldBy(holder(kind: "app")), nil),
            ("锁文件打不开", .unavailable("目录不可写"), nil),
            ("协议不兼容", nil, .protocolIncompatible("版本对不上")),
            ("握手失败", nil, .handshakeFailed("不回话")),
            ("attach 失败", nil, .attachFailed("拿不到快照")),
        ]
        for (what, lock, link) in forbidden {
            let real = decide(lock, spawn: .failed("拉不起来"), link: link)
            let dumb = naive(.failed("拉不起来"))
            XCTAssertNotEqual(real, dumb, "「\(what)」这一行被削弱成了天真版：\(real)")
            if case .takeOverLocally = real {
                XCTFail("「\(what)」这一行不许接管")
            }
        }
    }
}
#endif
