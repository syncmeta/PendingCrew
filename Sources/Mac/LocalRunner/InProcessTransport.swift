#if os(macOS)
import Foundation

/// P2/P4 共用的传输边界。P2 的实现是同进程函数调用；P4 只需把这里换成 socket，
/// 上面的 codec / server / RemoteSessionBackend 不变。
protocol SessionTransport: AnyObject {
    var receiveFromApp: ((Data) -> Void)? { get set }
    var receiveFromDaemon: ((Data) -> Void)? { get set }
    var isConnected: Bool { get }
    func sendFromApp(_ frame: Data)
    func sendFromDaemon(_ frame: Data)
}

/// 不开 socket、不排队、不复制语义：两端收到的是 codec 产出的完整 framed bytes，
/// 当前调用栈里直接调用对端函数。这样 P2 已经穿过真实协议，同时保留可预测的测试时序。
final class InProcessTransport: SessionTransport {
    var receiveFromApp: ((Data) -> Void)?
    var receiveFromDaemon: ((Data) -> Void)?
    private(set) var isConnected = true

    func sendFromApp(_ frame: Data) {
        guard isConnected else { return }
        receiveFromApp?(frame)
    }

    func sendFromDaemon(_ frame: Data) {
        guard isConnected else { return }
        receiveFromDaemon?(frame)
    }

    /// P4 的 viewer 断线语义在此提前钉住：只断 viewer 链路，不碰任何 session。
    func disconnect() {
        isConnected = false
    }

    /// P2 直接复用同一对函数端点；P4 socket transport 会在这里换成新连接。
    func reconnect() { isConnected = true }
}

/// §4.5 的定时常量。P2 没有 socket，因此不启动计时器；P4 transport 直接复用这些值。
enum SessionReconnectPolicy {
    static let pingInterval: TimeInterval = 10
    static let pongTimeout: TimeInterval = 30
    static let daemonIdleTimeout: TimeInterval = 60

    static func delay(forAttempt attempt: Int) -> TimeInterval {
        guard attempt > 0 else { return 0.2 }
        guard attempt < 5 else { return 5 }
        return min(0.2 * pow(2, Double(attempt)), 5)
    }
}

/// app 侧 stateSeq 守卫。跳号只做一件事：请求 `listSessions` 全量覆盖；
/// 不缓存增量、不重放、不合并，后台单向权威。
final class SessionStateReconciler {
    private var lastSequence: [String: UInt64] = [:]
    private var waitingForFullState: Set<String> = []
    private let requestFullList: () -> Void
    private let apply: (_ sessionId: String, _ stateSeq: UInt64,
                        _ state: SessionProtocolState) -> Void

    init(requestFullList: @escaping () -> Void,
         apply: @escaping (_ sessionId: String, _ stateSeq: UInt64,
                           _ state: SessionProtocolState) -> Void) {
        self.requestFullList = requestFullList
        self.apply = apply
    }

    func receiveDelta(sessionId: String, stateSeq: UInt64, state: SessionProtocolState) {
        if waitingForFullState.contains(sessionId) { return }
        if let previous = lastSequence[sessionId], stateSeq != previous + 1 {
            waitingForFullState.insert(sessionId)
            requestFullList()
            return
        }
        lastSequence[sessionId] = stateSeq
        apply(sessionId, stateSeq, state)
    }

    func receiveFull(_ summary: SessionSummary) {
        waitingForFullState.remove(summary.sessionId)
        lastSequence[summary.sessionId] = summary.stateSeq
        apply(summary.sessionId, summary.stateSeq, summary.state)
    }

    func resetForReconnect() {
        lastSequence.removeAll()
        waitingForFullState.removeAll()
    }
}
#endif
