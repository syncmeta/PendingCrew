#if os(macOS)
import Foundation
import XCTest

@MainActor
final class ViewerBackendConnectionTests: XCTestCase {
    private enum TestError: Error { case refused }

    func test_connectFactoryRunsProtocolHelloAndListThenDisconnectReconnects() throws {
        let server = SessionProtocolServer(capabilities: ["terminal-bytes"], daemonBuild: "remote")
        var appLinks: [InProcessSessionLink] = []
        var scheduled: [(attempt: Int, action: () -> Void)] = []
        var clients: [SessionProtocolClient] = []
        var hellos: [SessionDaemonHello] = []
        var lists: [SessionList] = []

        let connection = ViewerBackendConnection(
            capabilities: ["terminal-bytes"], appBuild: "viewer",
            connect: {
                let transport = InProcessTransport()
                let app = InProcessSessionLink(transport: transport, side: .app)
                let daemon = InProcessSessionLink(transport: transport, side: .daemon)
                appLinks.append(app)
                server.accept(link: daemon)
                return app
            },
            scheduleRetry: { attempt, action in scheduled.append((attempt, action)) })
        connection.onClientCreated = { clients.append($0) }
        connection.onHello = { hellos.append($0) }
        connection.onSessionList = { lists.append($0) }

        connection.start()
        XCTAssertEqual(connection.state, .connected)
        XCTAssertEqual(clients.count, 1)
        XCTAssertEqual(hellos.map(\.daemonBuild), ["remote"])
        XCTAssertEqual(lists.map(\.sessions), [[]])

        appLinks[0].peerDisconnected()
        XCTAssertEqual(connection.state, .waitingToRetry(
            attempt: 1, reason: "远程后端断开了连接"))
        XCTAssertEqual(scheduled.map(\.attempt), [1])

        scheduled.removeFirst().action()
        XCTAssertEqual(connection.state, .connected)
        XCTAssertEqual(clients.count, 2, "重连必须新建协议客户端，不能沿用已断的 decoder 状态")
        XCTAssertEqual(hellos.count, 2)
        XCTAssertEqual(lists.count, 2)
        XCTAssertEqual(connection.reconnectAttempt, 0)

        clients[0].onSessionList?(.init(sessions: []))
        XCTAssertEqual(lists.count, 2,
                       "旧 client 排队中的迟到 list 不能越过重连边界覆盖新 roster")
    }

    func test_factoryErrorIsVisibleAndRetryableWithoutAnyImplicitFallback() {
        var attempts = 0
        var scheduled: [(attempt: Int, action: () -> Void)] = []
        let connection = ViewerBackendConnection(
            capabilities: [], appBuild: "viewer",
            connect: {
                attempts += 1
                throw TestError.refused
            },
            scheduleRetry: { attempt, action in scheduled.append((attempt, action)) })

        connection.start()
        guard case let .waitingToRetry(attempt, reason) = connection.state else {
            return XCTFail("建连错误没有进入可见且可重试的错误态：\(connection.state)")
        }
        XCTAssertEqual(attempt, 1)
        XCTAssertTrue(reason.contains("refused"), reason)
        XCTAssertEqual(attempts, 1, "factory 失败后不能偷偷调用第二个（本机）connector")

        scheduled.removeFirst().action()
        XCTAssertEqual(attempts, 2)
        guard case .waitingToRetry(attempt: 2, _) = connection.state else {
            return XCTFail("第二次错误没有递增重连次数：\(connection.state)")
        }

        connection.stop()
        XCTAssertEqual(connection.state, .stopped)
        XCTAssertTrue(scheduled.isEmpty || attempts == 2)
    }

    func test_preHandshakeTimeoutRetriesAndStaleTimeoutCannotBreakNewConnection() {
        let server = SessionProtocolServer(capabilities: [], daemonBuild: "remote")
        var connectCount = 0
        var timeouts: [() -> Void] = []
        var retries: [() -> Void] = []
        let connection = ViewerBackendConnection(
            capabilities: [], appBuild: "viewer",
            connect: {
                connectCount += 1
                let transport = InProcessTransport()
                let app = InProcessSessionLink(transport: transport, side: .app)
                if connectCount > 1 {
                    server.accept(link: InProcessSessionLink(transport: transport, side: .daemon))
                }
                return app
            },
            scheduleRetry: { _, action in retries.append(action) },
            scheduleHandshakeTimeout: { _, action in timeouts.append(action) })

        connection.start()
        XCTAssertEqual(connection.state, .connecting,
                       "能建字节流但没有 hello 时不能误报 connected")
        XCTAssertEqual(timeouts.count, 1)

        timeouts[0]()
        guard case let .waitingToRetry(attempt, reason) = connection.state else {
            return XCTFail("握手超时没有进入可见重试态：\(connection.state)")
        }
        XCTAssertEqual(attempt, 1)
        XCTAssertTrue(reason.contains("握手超时"), reason)

        retries.removeFirst()()
        XCTAssertEqual(connection.state, .connected)
        XCTAssertEqual(connectCount, 2)
        XCTAssertEqual(timeouts.count, 2)

        timeouts[0]()
        timeouts[1]()
        XCTAssertEqual(connection.state, .connected,
                       "旧 generation 或已经 hello 的 timeout 不能打断当前连接")
    }

    func test_stopReleasesClientAndLinkWithoutCallbackRetainCycle() {
        let server = SessionProtocolServer(capabilities: [], daemonBuild: "remote")
        weak var weakClient: SessionProtocolClient?
        weak var weakLink: InProcessSessionLink?
        let connection = ViewerBackendConnection(
            capabilities: [], appBuild: "viewer",
            connect: {
                let transport = InProcessTransport()
                let app = InProcessSessionLink(transport: transport, side: .app)
                weakLink = app
                server.accept(link: InProcessSessionLink(transport: transport, side: .daemon))
                return app
            },
            scheduleHandshakeTimeout: { _, _ in })
        connection.onClientCreated = { weakClient = $0 }

        connection.start()
        XCTAssertNotNil(weakClient)
        XCTAssertNotNil(weakLink)

        connection.stop()
        XCTAssertNil(weakClient, "client 的回调闭包不能反向强持有 client")
        XCTAssertNil(weakLink, "client teardown 后不能因 hello/timeout 闭包继续持有 link")
    }
}
#endif
