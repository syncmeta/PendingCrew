#if os(macOS)
import Foundation
import XCTest

/// P4：**同一段服务端代码跑在真 socket 上**。
///
/// `RemoteSessionBackendTests` 把同进程那条链路钉死了；这一组钉的是换成 socket
/// 之后仍然成立的那些事 —— 快照分片、实时字节、多 viewer 各看各的、viewer 走了
/// session 不受影响，以及 §5.4 的背压（宁可重同步，绝不丢中间一段）。
@MainActor
final class SessionProtocolOverSocketTests: XCTestCase {
    private let capabilities = ["screen-text", "terminal-bytes", "transcript-events"]

    /// endpoint 的传输契约是可靠双向**字节流**，不能偷认「一次回调就是一帧」。
    /// 第一块只有 2 字节；第二块既补完第一帧，又紧跟完整第二帧，同时覆盖半包与粘包。
    func test_server接受任意切分与粘包的可靠字节流() throws {
        let link = ByteStreamLink()
        let server = SessionProtocolServer(capabilities: capabilities, daemonBuild: "daemon")
        server.accept(link: link)

        let codec = SessionProtocolCodec()
        var stream = Data()
        stream.append(try codec.encode(.hello(.init(
            protocolVersion: SessionProtocolVersion.current,
            appBuild: "app",
            capabilities: capabilities))))
        stream.append(try codec.encode(.listSessions))

        link.deliver(Array(stream.prefix(2)))
        link.deliver(Array(stream.dropFirst(2)))

        let replies = link.sent.compactMap { try? codec.decodeDaemon($0) }
        XCTAssertEqual(replies.count, 2, "半帧不能丢；两帧粘在一次投递里也必须逐帧处理")
        guard case .hello? = replies.first else { return XCTFail("第一条应为 daemon hello") }
        guard case let .sessions(list)? = replies.last else { return XCTFail("第二条应为 sessions") }
        XCTAssertEqual(list.sessions, [])
    }

    func test_client接受任意切分与粘包的可靠字节流() throws {
        let link = ByteStreamLink()
        let client = SessionProtocolClient(link: link, capabilities: capabilities, appBuild: "app")
        var receivedList: SessionList?
        client.onSessionList = { receivedList = $0 }

        let codec = SessionProtocolCodec()
        var stream = Data()
        stream.append(try codec.encode(.hello(.init(
            protocolVersion: SessionProtocolVersion.current,
            daemonBuild: "daemon",
            capabilities: capabilities,
            sessionCount: 0,
            pid: 42))))
        stream.append(try codec.encode(.sessions(.init(sessions: []))))

        link.deliver(Array(stream.prefix(3)))
        link.deliver(Array(stream.dropFirst(3)))

        XCTAssertTrue(client.isConnected, "client 不得静默丢掉被切开的 hello")
        XCTAssertEqual(receivedList?.sessions, [], "粘在 hello 后的 sessions 也必须交付")
    }

    func test_真socket上attach拿到快照后续实时字节接得上() throws {
        let pair = try UnixSocketTransport.makePair()
        defer { pair.app.close(); pair.daemon.close() }

        let server = SessionProtocolServer(capabilities: capabilities, daemonBuild: "test-daemon")
        let client = SessionProtocolClient(link: pair.app, capabilities: capabilities,
                                           appBuild: "test-app")
        server.accept(link: pair.daemon)
        client.connect()

        let backend = ProtocolTestBackend(kind: .claudeCode)
        backend.terminalSnapshot = .init(cols: 80, rows: 25, bytes: Array("history".utf8))
        server.register(sessionId: "s1", backend: backend)

        // 握手是异步的：socket 上 hello/hello 各要一趟。协商没落地就 attach，
        // daemon 会因为缺 terminal-bytes 而不发快照 —— 那是真会发生的顺序问题，
        // 所以这里等一拍，而不是假装 attach 可以立刻发。
        try pump(until: { client.negotiatedCapabilities.contains("terminal-bytes") })

        let remote = client.attach(sessionId: "s1", kind: .claudeCode)
        try pump(until: { remote.completedSnapshotCount == 1 })
        XCTAssertEqual(remote.lastCompletedSnapshotBytes, Array("history".utf8))

        server.acceptTerminalBytes(sessionId: "s1", bytes: Array("live".utf8))
        try pump(until: { remote.lastTerminalFrameBytes == Array("live".utf8) })

        // 反向：键盘输入过 socket 落到真 backend 上。
        remote.sendRaw([0x0d])
        try pump(until: { backend.rawInputs == [[0x0d]] })
    }

    func test_两个viewer各自attach同一个session互不串() throws {
        let a = try UnixSocketTransport.makePair()
        let b = try UnixSocketTransport.makePair()
        defer { [a.app, a.daemon, b.app, b.daemon].forEach { $0.close() } }

        let server = SessionProtocolServer(capabilities: capabilities)
        server.accept(link: a.daemon)
        server.accept(link: b.daemon)
        let clientA = SessionProtocolClient(link: a.app, capabilities: capabilities)
        let clientB = SessionProtocolClient(link: b.app, capabilities: capabilities)
        clientA.connect()
        clientB.connect()

        let backend = ProtocolTestBackend(kind: .claudeCode)
        backend.terminalSnapshot = .init(cols: 80, rows: 25, bytes: [])
        server.register(sessionId: "shared", backend: backend)
        try pump(until: { clientA.isConnected && clientB.isConnected })

        let remoteA = clientA.attach(sessionId: "shared", kind: .claudeCode)
        let remoteB = clientB.attach(sessionId: "shared", kind: .claudeCode)
        try pump(until: { remoteA.completedSnapshotCount == 1 && remoteB.completedSnapshotCount == 1 })

        server.acceptTerminalBytes(sessionId: "shared", bytes: [7])
        try pump(until: { remoteA.lastTerminalFrameBytes == [7] && remoteB.lastTerminalFrameBytes == [7] })

        // A 走掉 → 只有 A 的 handle 作废；B 照常收字节，session 一点没动。
        a.app.close()
        try pump(until: { server.connectionCount == 1 })
        server.acceptTerminalBytes(sessionId: "shared", bytes: [8])
        try pump(until: { remoteB.lastTerminalFrameBytes == [8] })
        XCTAssertEqual(remoteA.lastTerminalFrameBytes, [7], "断开的 viewer 不该再收到字节")
        XCTAssertEqual(backend.stopCount, 0, "viewer 断开绝不能碰 session")
    }

    /// §5.5：**viewer 还不知道自己多大时不许动 daemon 的尺寸。**
    ///
    /// 真分家之后 viewer 是在窗口布局出来**之前**就连上并拉 roster 的。那时报一个
    /// 默认 80×25，等于每次重开 app 都把正在跑的 TUI 先按 80 列重排一次、给 agent
    /// 发一次 SIGWINCH，然后窗口布局出来再排回去 —— 而「重开 app 不打断在跑的
    /// session」正是这一期的全部意义。
    func test_attach报零尺寸时不动权威终端的宽高() throws {
        let pair = try UnixSocketTransport.makePair()
        defer { pair.app.close(); pair.daemon.close() }
        let server = SessionProtocolServer(capabilities: capabilities)
        let client = SessionProtocolClient(link: pair.app, capabilities: capabilities)
        server.accept(link: pair.daemon)
        client.connect()

        let backend = ProtocolTestBackend(kind: .claudeCode)
        backend.terminalSnapshot = .init(cols: 200, rows: 50, bytes: [])
        server.register(sessionId: "s", backend: backend)
        try pump(until: { client.isConnected })

        let remote = client.attach(sessionId: "s", kind: .claudeCode, announceViewport: false)
        try pump(until: { remote.completedSnapshotCount == 1 })
        XCTAssertEqual(backend.resizes, [], "零尺寸 attach 一次 resize 都不该发")

        // 窗口布局出来之后照常下传真尺寸。
        remote.resizeTerminal(cols: 200, rows: 50)
        try pump(until: { backend.resizes == [.init(cols: 200, rows: 50)] })
    }

    // MARK: - §5.4 背压

    /// **这条是这一组里最重要的一条**，而且它第一版是错的，值得把错法留在注释里：
    ///
    /// 第一版在**不转 runloop** 的情况下连灌 4 MiB。那样队列当然会溢出 —— 但溢出的
    /// 原因是「泵一次都没跑过」，跟背压水位一点关系都没有。**把水位判断改成
    /// `if false` 之后那一版照样绿**，也就是说它给出的绿与被测的那件事无关
    /// （CONTRIBUTING 第 5 条实例五：尺子量对了性质，量错了对象）。
    ///
    /// 现在每灌一批就让泵真跑一趟：泵跑得到、但**灌不出去**，积压才是水位造成的。
    /// 断言两件事：① 出现了 `resync`；② 重同步之后 viewer 那边逐字节等于权威缓冲区
    /// —— 也就是**没有出现「丢一段继续发后面的」**。后者才是硬规则，前者是它的副产物。
    func test_写不动的链路会重同步而不是丢掉中间一段() throws {
        let link = BackpressureLink()
        let server = SessionProtocolServer(capabilities: capabilities)
        server.accept(link: link)

        let backend = ProtocolTestBackend(kind: .claudeCode)
        var authoritative: [UInt8] = []
        backend.terminalSnapshot = .init(cols: 80, rows: 25, bytes: [])
        server.register(sessionId: "flood", backend: backend)

        // 握手 + attach 都由测试直接构造 app 侧的帧（不需要真的 client）。
        let codec = SessionProtocolCodec()
        link.deliverToServer(try codec.encode(.hello(
            .init(protocolVersion: 1, appBuild: "t", capabilities: capabilities))))
        link.deliverToServer(try codec.encode(.attach(.init(sessionId: "flood", cols: 80, rows: 25))))
        spin()

        // 链路彻底写不动：pendingWriteBytes 顶在高水位之上。
        link.pendingWriteBytes = SessionProtocolServer.socketWriteHighWaterBytes
        let batch = Array(repeating: UInt8(0x41), count: 64 * 1024)
        for _ in 0..<64 {                       // 4 MiB，稳超 2 MiB 的实时积压上限
            authoritative += batch
            backend.terminalSnapshot = .init(cols: 80, rows: 25, bytes: authoritative)
            server.acceptTerminalBytes(sessionId: "flood", bytes: batch)
            spin()                              // 泵每批都跑，但灌不出去
        }

        // 松开水位 → 憋着的 resync + 最新快照这时才发得出去。**断言必须在这之后**：
        // 灌爆期间队列里已经有了 resync，但它跟别的帧一样发不出去 —— 那正是对的。
        link.pendingWriteBytes = 0
        server.acceptTerminalBytes(sessionId: "flood", bytes: [])
        spin(turns: 400)

        XCTAssertTrue(link.sawResync, "灌爆之后必须发过 resync")
        XCTAssertEqual(link.assembledTerminalStream(), authoritative,
                       "重同步之后 viewer 手上的字节必须逐字节等于权威缓冲区")
    }

    // MARK: -

    /// 转一会儿 runloop，直到条件成立；超时就失败（而不是静默通过）。
    private func pump(until condition: () -> Bool,
                      timeout: TimeInterval = 5,
                      file: StaticString = #filePath, line: UInt = #line) throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() > deadline { return XCTFail("等待超时", file: file, line: line) }
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
    }

    /// 让主队列上排着的泵真跑一趟。
    private func spin(turns: Int = 4) {
        for _ in 0..<turns {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.001))
        }
    }
}

/// 模拟 TCP/TLS/UDS 都可能出现的任意字节交付边界；它刻意不替 endpoint 重组帧。
@MainActor
private final class ByteStreamLink: SessionMessageLink {
    var onReceive: ((Data) -> Void)?
    var onClose: (() -> Void)?
    var isOpen = true
    let isSynchronous = false
    let pendingWriteBytes = 0
    private(set) var sent: [Data] = []

    func send(_ bytes: Data) { sent.append(bytes) }
    func close() { isOpen = false }
    func deliver(_ bytes: [UInt8]) { onReceive?(Data(bytes)) }
}

/// 一条**写不动**的链路：`pendingWriteBytes` 由测试直接摆布，用来把服务端逼进
/// §5.4 那条路。它同时把收到的帧解开、按 handle 拼回字节流，好断言「没丢中间一段」。
@MainActor
private final class BackpressureLink: SessionMessageLink {
    var onReceive: ((Data) -> Void)?
    var onClose: (() -> Void)?
    var isOpen = true
    let isSynchronous = false
    var pendingWriteBytes = 0

    private(set) var sawResync = false
    private var snapshot: [UInt8] = []
    private var partialSnapshot: [UInt8] = []
    private var live: [UInt8] = []

    func send(_ framed: Data) {
        guard let frame = try? SessionFrameDecoder.decodeAll(framed).first else { return }
        switch frame {
        case let .snapshot(_, seq, isLast, bytes):
            if seq == 0 { partialSnapshot = [] }
            partialSnapshot += bytes
            if isLast {
                // 一份新快照到手 = 之前的一切作废，从这里重新开始算。
                snapshot = partialSnapshot
                partialSnapshot = []
                live = []
            }
        case let .terminal(_, bytes):
            live += bytes
        case let .control(json):
            if let type = (try? JSONSerialization.jsonObject(with: json)) as? [String: Any],
               type["type"] as? String == "resync" {
                sawResync = true
            }
        }
    }

    func close() { isOpen = false }

    func deliverToServer(_ framed: Data) { onReceive?(framed) }

    /// viewer 眼里当前的完整字节流 = 最后一份快照 + 其后的实时字节。
    func assembledTerminalStream() -> [UInt8] { snapshot + live }
}
#endif
