#if os(macOS)
import XCTest

/// P4 传输层。**这一组的存在理由就是 spec §4.1 的第一条**：两端在同一个进程里用
/// `socketpair()` 对接，于是分帧/重组/半包/粘包全都进得了单元测试 —— 不需要 launchd、
/// 不需要 GUI、不需要真的起一个 daemon。
@MainActor
final class UnixSocketTransportTests: XCTestCase {

    // MARK: - 切边界（这把尺子量的是「一次回调 = 一条完整帧」）

    func test_逐字节喂也切得出原样的三条帧() throws {
        let frames = [
            try SessionFrameEncoder.encode(.control(Data("{\"type\":\"ping\"}".utf8))),
            try SessionFrameEncoder.encode(.terminal(handle: 7, bytes: [1, 2, 3])),
            try SessionFrameEncoder.encode(.snapshot(handle: 7, seq: 0, isLast: true, bytes: [9])),
        ]
        let stream = frames.reduce(Data(), +)

        var splitter = SessionFrameSplitter()
        var out: [Data] = []
        for byte in stream {
            out += try splitter.append(Data([byte]))
        }

        XCTAssertEqual(out, frames)
        XCTAssertEqual(splitter.pendingBytes, 0)
    }

    /// 粘包：三条帧一次性喂进去，仍然是三条，不是一坨。
    func test_一次喂进整条流也切成三条() throws {
        let frames = [
            try SessionFrameEncoder.encode(.control(Data("{\"type\":\"listSessions\"}".utf8))),
            try SessionFrameEncoder.encode(.terminal(handle: 1, bytes: Array(repeating: 0x41, count: 5000))),
            try SessionFrameEncoder.encode(.control(Data("{\"type\":\"ping\"}".utf8))),
        ]
        var splitter = SessionFrameSplitter()
        XCTAssertEqual(try splitter.append(frames.reduce(Data(), +)), frames)
    }

    /// 半包：只喂到一半时**一条都不给**，剩下的字节留着。
    func test_半条帧不交付且留在缓冲里() throws {
        let frame = try SessionFrameEncoder.encode(.terminal(handle: 3, bytes: [1, 2, 3, 4, 5]))
        var splitter = SessionFrameSplitter()
        XCTAssertEqual(try splitter.append(frame.dropLast(2)), [])
        XCTAssertEqual(splitter.pendingBytes, frame.count - 2)
        XCTAssertEqual(try splitter.append(frame.suffix(2)), [frame])
        XCTAssertEqual(splitter.pendingBytes, 0)
    }

    /// 长度上限与 `SessionFrameDecoder` 同源：两处判断必须一致，否则同一批字节
    /// 在切边界这一层合法、在解码那一层非法（或反过来），而错法是静默的。
    func test_超长长度前缀当场报错而不是申请无界内存() {
        var over = Data()
        let length = UInt32(SessionFrameDecoder.maximumFrameLength + 1)
        over.append(UInt8((length >> 24) & 0xff)); over.append(UInt8((length >> 16) & 0xff))
        over.append(UInt8((length >> 8) & 0xff)); over.append(UInt8(length & 0xff))
        var splitter = SessionFrameSplitter()
        XCTAssertThrowsError(try splitter.append(over)) { error in
            XCTAssertEqual(error as? SessionFrameSplitter.SplitError,
                           .frameTooLarge(SessionFrameDecoder.maximumFrameLength + 1))
        }
    }

    func test_零长度前缀当场报错() {
        var splitter = SessionFrameSplitter()
        XCTAssertThrowsError(try splitter.append(Data([0, 0, 0, 0]))) { error in
            XCTAssertEqual(error as? SessionFrameSplitter.SplitError, .frameTooShort)
        }
    }

    // MARK: - 真 socket 往返

    func test_socketpair上整条协议往返且大帧不被截断() throws {
        let pair = try UnixSocketTransport.makePair()
        defer { pair.app.close(); pair.daemon.close() }

        let codec = SessionProtocolCodec()
        // 一条控制帧 + 一条远超单次 read 的终端帧（512 KiB）—— 后者必然跨多次
        // read 到达，正是重组器要顶的那一刀。
        let big = Array(repeating: UInt8(0x5a), count: 512 * 1024)
        let sent: [Data] = [
            try codec.encode(.hello(.init(protocolVersion: 1, appBuild: "test",
                                          capabilities: ["terminal-bytes"]))),
            try SessionFrameEncoder.encode(.terminal(handle: 42, bytes: big)),
            try codec.encode(.ping(.init(nonce: 7))),
        ]

        var received: [Data] = []
        let done = expectation(description: "三条帧全部到达")
        pair.daemon.onReceive = { frame in
            received.append(frame)
            if received.count == sent.count { done.fulfill() }
        }
        for frame in sent { pair.app.send(frame) }

        wait(for: [done], timeout: 10)
        XCTAssertEqual(received, sent)

        // 到达的是**协议认得出**的东西，不只是字节相等。
        XCTAssertEqual(try codec.decodeApp(received[2]), .ping(.init(nonce: 7)))
        guard case let .terminal(handle, bytes)? = try SessionFrameDecoder.decodeAll(received[1]).first
        else { return XCTFail("第二条不是终端帧") }
        XCTAssertEqual(handle, 42)
        XCTAssertEqual(bytes.count, big.count)
        XCTAssertEqual(bytes, big)
    }

    func test_双向都能收发() throws {
        let pair = try UnixSocketTransport.makePair()
        defer { pair.app.close(); pair.daemon.close() }
        let codec = SessionProtocolCodec()

        let toDaemon = expectation(description: "app → daemon")
        let toApp = expectation(description: "daemon → app")
        pair.daemon.onReceive = { data in
            if (try? codec.decodeApp(data)) == .listSessions { toDaemon.fulfill() }
        }
        pair.app.onReceive = { data in
            if case .pong? = try? codec.decodeDaemon(data) { toApp.fulfill() }
        }
        pair.app.send(try codec.encode(.listSessions))
        pair.daemon.send(try codec.encode(.pong(.init(nonce: nil))))
        wait(for: [toDaemon, toApp], timeout: 10)
    }

    /// §4.5：viewer 断开 → daemon 侧那端收到 `onClose`（然后才谈得上「handle 作废、
    /// session 照跑」）。**本端自己 `close()` 不算断线**，所以那条不该触发回调。
    func test_对端走掉会通知本端而自己关掉不会() throws {
        let pair = try UnixSocketTransport.makePair()
        let peerGone = expectation(description: "对端消失")
        pair.daemon.onClose = { peerGone.fulfill() }

        var selfCloseFired = false
        pair.app.onClose = { selfCloseFired = true }
        pair.app.close()

        wait(for: [peerGone], timeout: 10)
        XCTAssertFalse(selfCloseFired, "本端主动 close() 不该触发 onClose")
        XCTAssertFalse(pair.daemon.isOpen)
        pair.daemon.close()
    }

    // MARK: - 监听端

    func test_监听端能接多个viewer且各自独立() throws {
        let path = Self.temporarySocketPath()
        let listener = try UnixSocketListener(path: path)
        defer { listener.close() }
        var accepted: [UnixSocketTransport] = []
        let two = expectation(description: "接到两个连接")
        two.expectedFulfillmentCount = 2
        listener.onAccept = { accepted.append($0); two.fulfill() }
        listener.start()

        let a = try UnixSocketTransport.connect(toPath: path)
        let b = try UnixSocketTransport.connect(toPath: path)
        defer { a.close(); b.close() }
        wait(for: [two], timeout: 10)

        let codec = SessionProtocolCodec()
        let onlyA = expectation(description: "只有 A 那条链路收到给 A 的帧")
        accepted[1].onReceive = { _ in XCTFail("B 的链路不该收到 A 的帧") }
        accepted[0].onReceive = { data in
            if (try? codec.decodeApp(data)) == .listSessions { onlyA.fulfill() }
        }
        a.send(try codec.encode(.listSessions))
        wait(for: [onlyA], timeout: 10)
        accepted.forEach { $0.close() }
    }

    /// `sun_path` 只有 104 字节，超了 `bind` 会**静默截断**成另一个路径 ——
    /// daemon 在 A 上听、viewer 去连 B，而且两边都不报错。所以必须当场拒绝。
    func test_路径超长当场拒绝而不是静默截断() {
        let long = "/tmp/" + String(repeating: "x", count: 200) + ".sock"
        XCTAssertThrowsError(try UnixSocketListener(path: long)) { error in
            guard case .pathTooLong? = error as? UnixSocketTransport.SocketError else {
                return XCTFail("应报 pathTooLong，实际 \(error)")
            }
        }
        XCTAssertThrowsError(try UnixSocketTransport.connect(toPath: long))
    }

    func test_没人在听时连接失败而不是挂住() {
        XCTAssertThrowsError(try UnixSocketTransport.connect(
            toPath: Self.temporarySocketPath())) { error in
            guard case .connectFailed? = error as? UnixSocketTransport.SocketError else {
                return XCTFail("应报 connectFailed，实际 \(error)")
            }
        }
    }

    // MARK: -

    /// 短路径：`sun_path` 只有 104 字节，DerivedData 下的临时目录动辄上百字符。
    private static func temporarySocketPath() -> String {
        "/tmp/pcrew-t-\(UUID().uuidString.prefix(8)).sock"
    }
}
#endif
