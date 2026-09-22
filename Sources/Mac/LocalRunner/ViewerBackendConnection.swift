#if os(macOS)
import Foundation

/// Protocol bootstrap and reconnect loop for a remote viewer leg.
///
/// It knows only a connect factory and `SessionProtocolClient`; there is deliberately no daemon
/// spawn, Unix-socket lookup, or local takeover branch here.  That makes a remote failure a visible
/// remote failure instead of an opportunity to show local sessions under a remote label.
@MainActor
final class ViewerBackendConnection {
    static let handshakeTimeout: TimeInterval = 15

    enum State: Equatable {
        case stopped
        case connecting
        case connected
        case waitingToRetry(attempt: Int, reason: String)
    }

    typealias RetryScheduler = (_ attempt: Int, _ action: @escaping () -> Void) -> Void
    typealias HandshakeTimeoutScheduler = (
        _ timeout: TimeInterval, _ action: @escaping () -> Void
    ) -> Void

    var onClientCreated: ((SessionProtocolClient) -> Void)?
    var onHello: ((SessionDaemonHello) -> Void)?
    var onSessionList: ((SessionList) -> Void)?
    var onStateChange: ((State) -> Void)?

    private(set) var state: State = .stopped {
        didSet { onStateChange?(state) }
    }
    private(set) var reconnectAttempt = 0
    private(set) var client: SessionProtocolClient?

    private let capabilities: [String]
    private let appBuild: String
    private let connector: any SessionMessageLinkConnecting
    private let scheduleRetry: RetryScheduler
    private let scheduleHandshakeTimeout: HandshakeTimeoutScheduler
    private var link: (any SessionMessageLink)?
    private var stopped = true
    private var generation = 0

    init(capabilities: [String], appBuild: String,
         connect: @escaping () throws -> any SessionMessageLink,
         scheduleRetry: RetryScheduler? = nil,
         scheduleHandshakeTimeout: HandshakeTimeoutScheduler? = nil) {
        self.capabilities = capabilities
        self.appBuild = appBuild
        connector = ClosureSessionMessageLinkConnector(connect)
        self.scheduleRetry = scheduleRetry ?? { attempt, action in
            let delay = SessionReconnectPolicy.delay(forAttempt: max(0, attempt - 1))
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                MainActor.assumeIsolated { action() }
            }
        }
        self.scheduleHandshakeTimeout = scheduleHandshakeTimeout ?? { timeout, action in
            DispatchQueue.main.asyncAfter(deadline: .now() + timeout) {
                MainActor.assumeIsolated { action() }
            }
        }
    }

    func start() {
        guard stopped else { return }
        stopped = false
        reconnectAttempt = 0
        connectNow()
    }

    func stop() {
        guard !stopped || state != .stopped else { return }
        stopped = true
        generation += 1
        tearDownCurrentLink()
        state = .stopped
    }

    /// Heartbeat and other higher-level liveness checks use the same reconnect path as `onClose`.
    func connectionLost(reason: String) {
        guard !stopped else { return }
        tearDownCurrentLink()
        waitToRetry(reason: reason)
    }

    private func connectNow() {
        guard !stopped else { return }
        generation += 1
        let currentGeneration = generation
        tearDownCurrentLink()
        state = .connecting
        do {
            let link = try connector.connect()
            let client = SessionProtocolClient(
                link: link, capabilities: capabilities, appBuild: appBuild)
            self.link = link
            self.client = client
            client.onLinkClosed = { [weak self, weak link] in
                MainActor.assumeIsolated {
                    guard let self, let link, self.link === link else { return }
                    let reason = link.terminalErrorDescription ?? "远程后端断开了连接"
                    self.connectionLost(reason: reason)
                }
            }
            client.onDaemonHello = { [weak self, weak client, weak link] hello in
                MainActor.assumeIsolated {
                    guard let self, let client, let link, !self.stopped,
                          self.generation == currentGeneration,
                          self.client === client, self.link === link else { return }
                    self.reconnectAttempt = 0
                    self.state = .connected
                    self.onHello?(hello)
                }
            }
            client.onSessionList = { [weak self, weak client] list in
                MainActor.assumeIsolated {
                    guard let self, let client, !self.stopped,
                          self.generation == currentGeneration,
                          self.client === client else { return }
                    self.onSessionList?(list)
                }
            }
            onClientCreated?(client)
            scheduleHandshakeTimeout(Self.handshakeTimeout) { [weak self, weak client, weak link] in
                guard let self, let client, let link, !self.stopped,
                      self.generation == currentGeneration,
                      self.client === client, self.link === link,
                      case .connecting = self.state else { return }
                self.connectionLost(reason: "远程后端握手超时（\(Int(Self.handshakeTimeout)) 秒）")
            }
            client.connect()
            client.requestSessionList()
        } catch {
            waitToRetry(reason: "远程后端连接失败：\(error)")
        }
    }

    private func waitToRetry(reason: String) {
        guard !stopped else { return }
        reconnectAttempt += 1
        let attempt = reconnectAttempt
        let scheduledGeneration = generation
        state = .waitingToRetry(attempt: attempt, reason: reason)
        scheduleRetry(attempt) { [weak self] in
            guard let self, !self.stopped, self.generation == scheduledGeneration,
                  case .waitingToRetry = self.state else { return }
            self.connectNow()
        }
    }

    private func tearDownCurrentLink() {
        client?.close()
        link?.close()
        client = nil
        link = nil
    }
}
#endif
