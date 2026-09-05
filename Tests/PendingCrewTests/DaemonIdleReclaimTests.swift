#if os(macOS)
import Foundation
import XCTest

/// 半开链路回收（§4.5 `daemonIdleTimeout`）。
///
/// **这条以前是死的**：常量 2026-08 就声明了，全库唯一另一处引用是一句断言它等于 60
/// 的单测 —— 也就是说「后台会回收半开连接」这件事从来没有发生过。心跳只做了 viewer
/// 半边（`pongTimeout` 收不到 pong 就重连），daemon 这半没接。
///
/// **为什么它非有不可**：`dropConnection` 只由 `link.onClose` 触发，而对端睡死 / 崩溃 /
/// 网线拔掉时 **FIN 根本不来**。那条连接会永远留在 `connections` 里：`connectionCount`
/// 虚高、daemon 照旧往一条死 socket 灌字节、它 attach 的 handle 也一直挂在 `records` 上。
@MainActor
final class DaemonIdleReclaimTests: XCTestCase {

    /// 一条**只会消失、不会说再见**的链路：它永远不调用 `onClose`，正是半开的定义。
    private final class SilentLink: SessionMessageLink {
        var onReceive: ((Data) -> Void)?
        var onClose: (() -> Void)?
        var isOpen = true
        let isSynchronous = false
        let pendingWriteBytes = 0
        private(set) var sent: [Data] = []
        private(set) var closeCount = 0
        func send(_ bytes: Data) { sent.append(bytes) }
        func close() { isOpen = false; closeCount += 1 }
    }

    private let t0 = Date(timeIntervalSince1970: 1_000_000)

    func test_对端不发FIN就消失_超过idleTimeout后连接必须被回收() {
        let server = SessionProtocolServer(capabilities: [], reclaimsIdleConnections: true)
        let link = SilentLink()
        server.accept(link: link, now: t0)
        XCTAssertEqual(server.connectionCount, 1)

        // 对端就此安静：不再发任何字节，也不 close（FIN 不来）。
        let reclaimed = server.reclaimIdleConnections(
            now: t0.addingTimeInterval(SessionReconnectPolicy.daemonIdleTimeout + 1))

        XCTAssertEqual(reclaimed, 1)
        XCTAssertEqual(server.connectionCount, 0, "半开连接没被回收，它会永远留在 connections 里")
        XCTAssertEqual(link.closeCount, 1, "回收时没把链路关掉，fd 会漏")
    }

    func test_没到时限不许回收() {
        let server = SessionProtocolServer(capabilities: [], reclaimsIdleConnections: true)
        server.accept(link: SilentLink(), now: t0)
        let reclaimed = server.reclaimIdleConnections(
            now: t0.addingTimeInterval(SessionReconnectPolicy.daemonIdleTimeout - 1))
        XCTAssertEqual(reclaimed, 0)
        XCTAssertEqual(server.connectionCount, 1, "还没到阈值就被回收 —— 会把慢但活着的 viewer 踢掉")
    }

    /// 正常连接每 `pingInterval`（10s）就有一次 ping 进来，安静不到 60s。
    /// 这条钉的是「**收到字节**要把计时重置」——不重置的话活连接也会被误杀。
    func test_收到对端字节要重置计时() throws {
        let server = SessionProtocolServer(capabilities: [], reclaimsIdleConnections: true)
        let link = SilentLink()
        server.accept(link: link, now: t0)

        // 50 秒时对端说了话（ping 就是这么来的）。
        let ping = try SessionProtocolCodec().encode(SessionAppMessage.ping(.init(nonce: 7)))
        link.onReceive?(ping)

        // 再过 59 秒 —— 距 accept 已 109s，但距**最近一次收到**不到 60s。
        let reclaimed = server.reclaimIdleConnections(now: Date().addingTimeInterval(59))
        XCTAssertEqual(reclaimed, 0)
        XCTAssertEqual(server.connectionCount, 1, "把还在说话的连接回收了")
    }

    /// **守卫**：没打开开关的 server 一个都不许回收。
    ///
    /// `InProcessSessionProtocolBridge` 那台就是这种 —— 同进程桥两端同生共死、
    /// app 侧不发 ping，接上回收就是 60 秒后误杀。**原来这条只写在注释里**，
    /// 而注释拦不住顺手改代码的人（父机长用我自己的标准指出的）。
    func test_没打开开关的server一个都不回收() {
        let server = SessionProtocolServer(capabilities: [])   // 默认 false
        let link = SilentLink()
        server.accept(link: link, now: t0)

        let reclaimed = server.reclaimIdleConnections(now: t0.addingTimeInterval(10_000))

        XCTAssertEqual(reclaimed, 0)
        XCTAssertEqual(server.connectionCount, 1, "没打开开关却回收了 —— 同进程桥会被这样打死")
        XCTAssertEqual(link.closeCount, 0)
    }

    /// 多条连接时只回收该回收的那条，别误伤旁边的。
    func test_只回收安静的那条() {
        let server = SessionProtocolServer(capabilities: [], reclaimsIdleConnections: true)
        let stale = SilentLink()
        let fresh = SilentLink()
        server.accept(link: stale, now: t0)
        server.accept(link: fresh, now: t0.addingTimeInterval(100))
        XCTAssertEqual(server.connectionCount, 2)

        let reclaimed = server.reclaimIdleConnections(now: t0.addingTimeInterval(120))
        XCTAssertEqual(reclaimed, 1)
        XCTAssertEqual(server.connectionCount, 1)
        XCTAssertEqual(stale.closeCount, 1)
        XCTAssertEqual(fresh.closeCount, 0, "误伤了还活着的那条")
    }
}
#endif
