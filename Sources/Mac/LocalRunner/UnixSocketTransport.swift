#if os(macOS)
import Foundation

/// 一条**已经连上的**协议链路，单侧视角（spec
/// `docs/internal/2026-08-19-backend-split-design.md` §4.1）。
///
/// P2 的 `SessionTransport` 是「两端都在我手里」的对称抽象 —— 那只在同进程里成立。
/// 真分家之后每个进程只握得住自己这一端，所以协议服务端/客户端认的是这个单侧接口，
/// `InProcessTransport` 与 `UnixSocketTransport` 各自往上贴一层适配。
///
/// **`onReceive` 收到的永远是一条完整的 framed message**（codec 产出的那种，含 4 字节
/// 长度前缀）。socket 上收到的是任意切法的字节，重组由 `SessionFrameSplitter` 在链路
/// 内部做完 —— 上层一行都不用知道自己跑在哪种传输上。
@MainActor
protocol SessionMessageLink: AnyObject {
    var onReceive: ((Data) -> Void)? { get set }
    var onClose: (() -> Void)? { get set }
    var isOpen: Bool { get }
    /// 这条链路是否支持**同一调用栈内**的请求/应答（`InProcessTransport` 支持，
    /// socket 不支持）。同步 `screenText` 那条路只在 true 时成立，false 时调用方
    /// 必须降级 —— 见 `RemoteSessionBackend.screenText`。
    var isSynchronous: Bool { get }
    /// 发一条完整的 framed message。
    func send(_ framed: Data)
    /// 本端主动关闭。**不触发 `onClose`** —— 那是留给「对端走了 / 链路断了」的，
    /// 谁主动关的谁自己知道，重连策略不该被自己的 detach 触发。
    func close()
}

/// 把字节流切回「一条一条完整的 framed message」，**不解码内容**。
///
/// 与 `SessionFrameDecoder` 的分工：那个把帧解成 `SessionWireFrame`（要读 payload），
/// 这个只按长度前缀切边界。socket 链路上每秒几千帧，切边界不该顺带解一遍 JSON；
/// 而且切完原样交给既有 codec，P2 那条路径一个字都不用改。
///
/// 长度规则与 `SessionFrameDecoder` **必须一致**（`length >= 1`、
/// `<= maximumFrameLength`）—— 否则同一批字节在两处会有两种合法性判断。
struct SessionFrameSplitter {
    enum SplitError: Error, Equatable {
        case frameTooShort
        case frameTooLarge(Int)
    }

    private var buffer = Data()

    /// 还没凑齐一帧的残留字节数（诊断/测试用）。
    var pendingBytes: Int { buffer.count }

    mutating func append<S: DataProtocol>(_ bytes: S) throws -> [Data] {
        buffer.append(contentsOf: bytes)
        var out: [Data] = []
        while buffer.count >= 4 {
            let i = buffer.startIndex
            let length = (Int(buffer[i]) << 24) | (Int(buffer[i + 1]) << 16)
                | (Int(buffer[i + 2]) << 8) | Int(buffer[i + 3])
            guard length >= 1 else { throw SplitError.frameTooShort }
            guard length <= SessionFrameDecoder.maximumFrameLength else {
                throw SplitError.frameTooLarge(length)
            }
            let total = 4 + length
            guard buffer.count >= total else { break }
            out.append(Data(buffer[i..<(i + total)]))
            buffer.removeFirst(total)
        }
        return out
    }
}

/// `SessionFrameSplitter` 的引用型外壳，**只在 socket 的 IO 队列上被碰**。
///
/// 存在的理由：`UnixSocketTransport` 是 `@MainActor`，它的存储属性碰不得于 IO 队列；
/// 而重组必须严格按字节到达顺序做、不能先跳到主队列（跳过去就得先攒批，攒批就把
/// 「一帧到达 = 一次回调」的时序打乱了）。所以重组器单独拎出来，归 IO 队列独占。
private final class FrameReassembler: @unchecked Sendable {
    private var splitter = SessionFrameSplitter()
    func append(_ data: Data) throws -> [Data] { try splitter.append(data) }
}

/// `InProcessTransport` 的单侧适配。P2 的对称传输原样留着（它的语义和测试都还在用），
/// 这里只是把「app 这一端」「daemon 这一端」各包成一条 `SessionMessageLink`。
@MainActor
final class InProcessSessionLink: SessionMessageLink {
    enum Side { case app, daemon }

    var onReceive: ((Data) -> Void)?
    var onClose: (() -> Void)?
    var isOpen: Bool { transport.isConnected }
    /// 同进程直调 —— `screenText` 那条同步问答就是靠它成立的。
    let isSynchronous = true

    private let transport: InProcessTransport
    private let side: Side

    init(transport: InProcessTransport, side: Side) {
        self.transport = transport
        self.side = side
        switch side {
        case .app:
            transport.receiveFromDaemon = { [weak self] data in
                MainActor.assumeIsolated { self?.onReceive?(data) }
            }
        case .daemon:
            transport.receiveFromApp = { [weak self] data in
                MainActor.assumeIsolated { self?.onReceive?(data) }
            }
        }
    }

    func send(_ framed: Data) {
        switch side {
        case .app: transport.sendFromApp(framed)
        case .daemon: transport.sendFromDaemon(framed)
        }
    }

    func close() { transport.disconnect() }

    /// 对端消失（P2 里就是 `disconnectViewer()`）。与 `close()` 分开：这条会通知上层。
    func peerDisconnected() {
        transport.disconnect()
        onClose?()
    }
}

/// **P4 的传输层**：AF_UNIX SOCK_STREAM 上的一条链路。
///
/// 选它而不是 XPC 的理由见 spec §4.1，其中第一条在这里有直接体现：
/// `makePair()` 用 `socketpair(2)` 在同一进程里对接两端，整条协议（含分帧、重组、
/// 半包、粘包）因此进得了单元测试，不需要 launchd、不需要 GUI、不需要真的起 daemon。
///
/// **SIGPIPE**：对端先走时 `write(2)` 会给整个进程发 SIGPIPE，默认动作是**杀掉进程** ——
/// 一个 viewer 关窗口就能带走 daemon。`SO_NOSIGPIPE` 在这里是必需项，不是优化。
@MainActor
final class UnixSocketTransport: SessionMessageLink {
    enum SocketError: Error, Equatable, CustomStringConvertible {
        case pathTooLong(path: String, limit: Int)
        case socketFailed(errno: Int32)
        case connectFailed(errno: Int32)
        case bindFailed(errno: Int32)
        case listenFailed(errno: Int32)
        case socketpairFailed(errno: Int32)

        var description: String {
            switch self {
            case let .pathTooLong(path, limit):
                return "socket 路径超长（\(path.utf8.count) > \(limit)）：\(path)"
            case let .socketFailed(e): return "socket() 失败：\(String(cString: strerror(e)))"
            case let .connectFailed(e): return "connect() 失败：\(String(cString: strerror(e)))"
            case let .bindFailed(e): return "bind() 失败：\(String(cString: strerror(e)))"
            case let .listenFailed(e): return "listen() 失败：\(String(cString: strerror(e)))"
            case let .socketpairFailed(e): return "socketpair() 失败：\(String(cString: strerror(e)))"
            }
        }
    }

    /// `sockaddr_un.sun_path` 是 104 字节且要留结尾 NUL。超了不是「路径写不下」这种
    /// 小事 —— `bind` 会静默截断成另一个路径，于是 daemon 在 A 上听、viewer 去连 B。
    /// 所以这里 fail loud。
    static let maximumPathLength = 103

    var onReceive: ((Data) -> Void)?
    var onClose: (() -> Void)?
    private(set) var isOpen = true
    /// socket 上没有「同一调用栈里拿到回应」这回事。
    let isSynchronous = false

    /// 已经交给内核、但还没写出去的字节数。daemon 侧据此判断 viewer 是不是消费不过来。
    private(set) var pendingWriteBytes = 0

    private let io: DispatchIO
    private let queue: DispatchQueue
    private let reassembler = FrameReassembler()
    private var closed = false

    /// 接管一个**已连上**的 fd（accept 出来的，或 `socketpair` 的一端）。
    init(connectedFileDescriptor fd: Int32, label: String) {
        queue = DispatchQueue(label: "pendingcrew.socket." + label)
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        io = DispatchIO(type: .stream, fileDescriptor: fd, queue: queue) { _ in
            Darwin.close(fd)
        }
        // 攒批交付会把「一帧到达 = 一次回调」变成不可预测的合批；协议热路径本来就是
        // 小帧，合并交给内核即可。
        io.setLimit(lowWater: 1)
        startReading()
    }

    /// 连到一个正在监听的路径。
    static func connect(toPath path: String) throws -> UnixSocketTransport {
        guard path.utf8.count <= maximumPathLength else {
            throw SocketError.pathTooLong(path: path, limit: maximumPathLength)
        }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw SocketError.socketFailed(errno: errno) }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let length = Self.fill(&addr, with: path)
        let rc = withUnsafePointer(to: &addr) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(length))
            }
        }
        guard rc == 0 else {
            let saved = errno
            Darwin.close(fd)
            throw SocketError.connectFailed(errno: saved)
        }
        return UnixSocketTransport(connectedFileDescriptor: fd, label: "client")
    }

    /// 同进程对接的一对。**这就是 spec §4.1 说的「整条协议进单元测试」那条路。**
    static func makePair() throws -> (app: UnixSocketTransport, daemon: UnixSocketTransport) {
        var fds: [Int32] = [0, 0]
        let rc = fds.withUnsafeMutableBufferPointer { buffer in
            socketpair(AF_UNIX, SOCK_STREAM, 0, buffer.baseAddress)
        }
        guard rc == 0 else { throw SocketError.socketpairFailed(errno: errno) }
        return (UnixSocketTransport(connectedFileDescriptor: fds[0], label: "pair.app"),
                UnixSocketTransport(connectedFileDescriptor: fds[1], label: "pair.daemon"))
    }

    func send(_ framed: Data) {
        guard !closed, !framed.isEmpty else { return }
        pendingWriteBytes += framed.count
        let count = framed.count
        let dispatchData = framed.withUnsafeBytes { DispatchData(bytes: $0) }
        io.write(offset: 0, data: dispatchData, queue: queue) { [weak self] done, remaining, error in
            guard done else { return }
            let written = count - (remaining?.count ?? 0)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.pendingWriteBytes = max(0, self.pendingWriteBytes - written)
                    if error != 0 { self.peerVanished() }
                }
            }
        }
    }

    func close() {
        guard !closed else { return }
        closed = true
        isOpen = false
        io.close(flags: .stop)
    }

    // MARK: -

    private func startReading() {
        let reassembler = self.reassembler
        io.read(offset: 0, length: Int.max, queue: queue) { [weak self] done, data, error in
            // 重组在 IO 队列上推进（必须严格按字节到达顺序），切好的整帧再按 FIFO
            // 投到主队列 —— `DispatchQueue.main.async` 保序，`Task { @MainActor }`
            // 不保序，这里不能换。
            var frames: [Data] = []
            var failed = false
            if let data, !data.isEmpty {
                do { frames = try reassembler.append(Data(data)) }
                catch { failed = true }
            }
            let ended = failed || error != 0 || (done && (data?.isEmpty ?? true))
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, !self.closed else { return }
                    for frame in frames { self.onReceive?(frame) }
                    if ended { self.peerVanished() }
                }
            }
        }
    }

    private func peerVanished() {
        guard !closed else { return }
        closed = true
        isOpen = false
        io.close(flags: .stop)
        onClose?()
    }

    fileprivate static func fill(_ addr: inout sockaddr_un, with path: String) -> Int {
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let length = MemoryLayout<sockaddr_un>.size
        addr.sun_len = UInt8(length)
        return length
    }
}

/// 监听端：bind + listen + accept，每接一个连接产出一条 `UnixSocketTransport`。
///
/// **它不负责单实例互斥。** 那是 `daemon.lock` 上的 flock 的事（§6.2 闸门 2），
/// 而且顺序不能反：**先拿到锁、再 unlink 旧 socket 文件**。反过来的话，第二个
/// daemon 会在发现自己抢不到锁**之前**就把第一个 daemon 正在听的那个 socket 文件
/// 删掉 —— 老 daemon 还活着、还在 accept，但谁也连不上它了，而且没有任何报错。
@MainActor
final class UnixSocketListener {
    private let fd: Int32
    private let path: String
    private var source: DispatchSourceRead?
    private var closed = false

    var onAccept: ((UnixSocketTransport) -> Void)?

    init(path: String, backlog: Int32 = 16) throws {
        guard path.utf8.count <= UnixSocketTransport.maximumPathLength else {
            throw UnixSocketTransport.SocketError.pathTooLong(
                path: path, limit: UnixSocketTransport.maximumPathLength)
        }
        self.path = path
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw UnixSocketTransport.SocketError.socketFailed(errno: errno) }
        // 非阻塞：accept 循环靠 EAGAIN 收尾。阻塞 fd 上「事件来了就连着 accept」的写法
        // 会在第二次 accept 里把主线程挂住 —— 那时 daemon 整个不动了，且看不出原因。
        fcntl(fd, F_SETFL, fcntl(fd, F_GETFL, 0) | O_NONBLOCK)
        unlink(path)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let length = UnixSocketTransport.fill(&addr, with: path)
        let bound = withUnsafePointer(to: &addr) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(length))
            }
        }
        guard bound == 0 else {
            let saved = errno
            Darwin.close(fd)
            throw UnixSocketTransport.SocketError.bindFailed(errno: saved)
        }
        // 只有本用户连得上。
        chmod(path, 0o600)
        guard Darwin.listen(fd, backlog) == 0 else {
            let saved = errno
            Darwin.close(fd)
            unlink(path)
            throw UnixSocketTransport.SocketError.listenFailed(errno: saved)
        }
    }

    func start() {
        guard source == nil, !closed else { return }
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in
            MainActor.assumeIsolated {
                guard let self, !self.closed else { return }
                while true {
                    let client = Darwin.accept(self.fd, nil, nil)
                    guard client >= 0 else { return }
                    self.onAccept?(UnixSocketTransport(
                        connectedFileDescriptor: client, label: "server"))
                }
            }
        }
        // fd 的关闭必须交给 cancel handler：`cancel()` 是异步的，紧接着自己
        // `close(fd)` 会和 source 最后一次事件派发抢同一个 fd 号，而 fd 号是会被
        // 立刻复用的 —— 抢输了就是往一个不相干的新 fd 上 accept。
        source.setCancelHandler { [fd, path] in
            Darwin.close(fd)
            unlink(path)
        }
        self.source = source
        source.resume()
    }

    func close() {
        guard !closed else { return }
        closed = true
        if let source {
            source.cancel()          // fd 与 socket 文件在 cancel handler 里收
            self.source = nil
        } else {
            Darwin.close(fd)
            unlink(path)
        }
    }
}
#endif
