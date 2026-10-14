#if os(macOS)
import Foundation
import Darwin

struct CrewdLocalIPCError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// Kernel observations, never server-supplied PID/path/UUID claims.
struct CrewdKernelProcessIdentity: Equatable, Sendable {
    let pid: Int32
    let uid: uid_t
    let parentPID: Int32
    let startSeconds: UInt64
    let startMicroseconds: UInt64
    let bootSession: String

    static func capture(_ pid: Int32) throws -> Self {
        var info = proc_bsdinfo()
        let size = MemoryLayout<proc_bsdinfo>.size
        guard pid > 0, proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(size)) == size,
              info.pbi_pid == UInt32(pid), info.pbi_start_tvsec > 0 else {
            throw CrewdLocalIPCError(message: "无法核对本机后台的内核进程身份。")
        }
        var count = 0
        guard sysctlbyname("kern.bootsessionuuid", nil, &count, nil, 0) == 0,
              count > 1, count <= 128 else {
            throw CrewdLocalIPCError(message: "无法核对当前内核启动身份。")
        }
        var bytes = [CChar](repeating: 0, count: count)
        guard sysctlbyname("kern.bootsessionuuid", &bytes, &count, nil, 0) == 0,
              bytes.last == 0 else {
            throw CrewdLocalIPCError(message: "内核启动身份读取不完整。")
        }
        let boot = String(cString: bytes)
        guard UUID(uuidString: boot) != nil else {
            throw CrewdLocalIPCError(message: "内核启动身份无效。")
        }
        return Self(pid: pid, uid: info.pbi_uid, parentPID: Int32(info.pbi_ppid),
                    startSeconds: info.pbi_start_tvsec, startMicroseconds: info.pbi_start_tvusec,
                    bootSession: boot)
    }

    static func ownChild(_ pid: Int32) throws -> Self {
        let identity = try capture(pid)
        guard identity.uid == geteuid(), getuid() == geteuid(), identity.parentPID == getpid() else {
            throw CrewdLocalIPCError(message: "后台不是当前 App 的同身份直接子进程，不能授予退出权限。")
        }
        return identity
    }
}

private final class CrewdIPCConnectCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    func cancel() { lock.lock(); cancelled = true; lock.unlock() }
    var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return cancelled }
}

/// AF_UNIX NDJSON byte transport. It owns only this connection, never a daemon.
/// DispatchIO retains the actual fd until its cleanup handler; no private
/// Network.framework fd access or independent probe socket is used for identity.
@MainActor
final class CrewdLocalIPC {
    let initialPeer: CrewdKernelProcessIdentity
    var onData: ((Data) -> Void)?
    var onEnd: ((String) -> Void)?
    private let descriptor: Int32
    private let io: DispatchIO
    private let queue = DispatchQueue(label: "pendingcrew.crewd.local-ipc")
    private var closed = false
    private var queuedBytes = 0
    static let maximumQueuedBytes = 64 * 1024 * 1024

    private init(descriptor: Int32, peer: CrewdKernelProcessIdentity) {
        self.descriptor = descriptor; self.initialPeer = peer
        io = DispatchIO(type: .stream, fileDescriptor: descriptor, queue: queue) { _ in
            Darwin.close(descriptor)
        }
        io.setLimit(lowWater: 1)
    }

    static func connect(path: String) async throws -> CrewdLocalIPC {
        guard !path.utf8.contains(0), !path.isEmpty, path.utf8.count <= 103 else {
            throw CrewdLocalIPCError(message: "本机后台连接路径无效或过长。")
        }
        let cancellation = CrewdIPCConnectCancellation()
        let fd: Int32 = try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                DispatchQueue.global(qos: .userInitiated).async {
                    do { continuation.resume(returning: try open(path: path, cancellation: cancellation)) }
                    catch { continuation.resume(throwing: error) }
                }
            }
        } onCancel: { cancellation.cancel() }
        do {
            guard !Task.isCancelled else { throw CancellationError() }
            return CrewdLocalIPC(descriptor: fd, peer: try peerIdentity(fd))
        } catch { Darwin.close(fd); throw error }
    }

    nonisolated private static func open(path: String, cancellation: CrewdIPCConnectCancellation) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw CrewdLocalIPCError(message: "无法创建本机后台连接。") }
        var keep = false
        defer { if !keep { Darwin.close(fd) } }
        var one: Int32 = 1
        guard fcntl(fd, F_SETFD, FD_CLOEXEC) == 0,
              fcntl(fd, F_SETFL, O_NONBLOCK) == 0,
              setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size)) == 0 else {
            throw CrewdLocalIPCError(message: "本机连接安全属性设置失败。")
        }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutableBytes(of: &address.sun_path) { bytes in
            bytes.copyBytes(from: Array(path.utf8)); bytes[path.utf8.count] = 0
        }
        let result = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        if result != 0 {
            guard errno == EINPROGRESS else { throw CrewdLocalIPCError(message: "本机后台暂不可连接。") }
            let deadline = DispatchTime.now().uptimeNanoseconds + 5_000_000_000
            while true {
                if cancellation.isCancelled { throw CancellationError() }
                guard DispatchTime.now().uptimeNanoseconds < deadline else {
                    throw CrewdLocalIPCError(message: "本机后台连接超时。")
                }
                var state = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
                let polled = poll(&state, 1, 100)
                if polled < 0 {
                    if errno == EINTR { continue }
                    throw CrewdLocalIPCError(message: "本机后台连接等待失败。")
                }
                if polled == 0 { continue }
                var socketError: Int32 = 0
                var size = socklen_t(MemoryLayout<Int32>.size)
                guard getsockopt(fd, SOL_SOCKET, SO_ERROR, &socketError, &size) == 0,
                      socketError == 0, state.revents & Int16(POLLOUT) != 0 else {
                    throw CrewdLocalIPCError(message: "本机后台连接失败。")
                }
                break
            }
        }
        guard !cancellation.isCancelled else { throw CancellationError() }
        keep = true; return fd
    }

    nonisolated private static func peerIdentity(_ fd: Int32) throws -> CrewdKernelProcessIdentity {
        var pid: Int32 = 0
        var size = socklen_t(MemoryLayout<Int32>.size)
        var uid: uid_t = 0; var gid: gid_t = 0
        guard getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, &pid, &size) == 0,
              size == MemoryLayout<Int32>.size, pid > 0,
              getpeereid(fd, &uid, &gid) == 0, uid == geteuid(), getuid() == geteuid() else {
            throw CrewdLocalIPCError(message: "本机连接的内核对端身份未确认。")
        }
        let identity = try CrewdKernelProcessIdentity.capture(pid)
        guard identity.uid == uid else { throw CrewdLocalIPCError(message: "本机对端身份读取不一致。") }
        return identity
    }

    /// Recheck the same open fd and process birth, not a separately connected probe.
    func verifiedPeer() throws -> CrewdKernelProcessIdentity {
        guard !closed else { throw CrewdLocalIPCError(message: "原本机连接已经关闭，进程归属未确认。") }
        let current = try Self.peerIdentity(descriptor)
        guard current == initialPeer else { throw CrewdLocalIPCError(message: "本机连接的内核进程身份已经改变。") }
        return current
    }

    func startReading() {
        io.read(offset: 0, length: Int.max, queue: queue) { [weak self] done, bytes, error in
            let data = bytes.map { Data($0) }
            let ended = error != 0 || (done && (bytes?.isEmpty ?? true))
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, !self.closed else { return }
                    // Preserve FIFO: final received bytes precede disconnect.
                    if let data, !data.isEmpty { self.onData?(data) }
                    if ended { self.end(error == 0 ? "本机后台连接已结束。" : "本机后台连接中断。") }
                }
            }
        }
    }

    func send(_ data: Data) throws {
        guard !closed else { throw CrewdLocalIPCError(message: "本机连接已关闭，尚未提交。") }
        guard data.count <= Self.maximumQueuedBytes - queuedBytes else {
            throw CrewdLocalIPCError(message: "本机发送队列已满，尚未提交。")
        }
        queuedBytes += data.count
        let count = data.count
        let bytes = data.withUnsafeBytes { DispatchData(bytes: $0) }
        io.write(offset: 0, data: bytes, queue: queue) { [weak self] done, remaining, error in
            guard done else { return }
            let written = count - (remaining?.count ?? 0)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, !self.closed else { return }
                    self.queuedBytes = max(0, self.queuedBytes - written)
                    if error != 0 { self.end("本机写入中断，提交结果未知。") }
                }
            }
        }
    }

    func close() {
        guard !closed else { return }
        closed = true; io.close(flags: .stop)
    }
    private func end(_ message: String) { close(); onEnd?(message) }
}
#endif
