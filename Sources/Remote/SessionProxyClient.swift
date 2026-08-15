import Foundation

/// The runner-side WebSocket transport for T4.5 cross-device remote control.
///
/// Wraps a `URLSessionWebSocketTask` connected to the session's
/// `SessionProxyDO` (`GET /v1/sessions/<id>/proxy/connect?role=runner`). It is
/// the live counterpart to the HTTP `CrewSessionServerLink`: where that mirrors
/// the durable transcript via `appendSessionEvent`, this pushes low-latency
/// `session.state` to viewers and receives `session.command` /
/// `permission.decision` back.
///
/// Responsibilities kept here on purpose:
///   * connect + subscribe(role: .runner) + await the `subscribed` ack,
///   * publish `session.state` (re-published on every reconnect so a viewer
///     that joined while we were down still sees current state),
///   * surface runner-actionable inbound frames (command / decision) over
///     `inbound`, an `AsyncStream` the run consumes on the MainActor — this
///     avoids threading a `@Sendable` closure that captures the non-Sendable
///     run,
///   * keep-alive ping + automatic reconnect with capped exponential backoff
///     until `close()` is called.
///
/// Auth: the device-grant bearer goes on the upgrade request's `Authorization`
/// header — the HTTP route authenticates it and stamps trusted facts onto the
/// DO upgrade (see `apps/edge/src/routes/session-proxy.ts`). The in-band
/// `subscribe.token` is a defence-in-depth echo only.
///
/// Self-contained: only Foundation + the `SessionProxyProtocol` codec, so it
/// compiles into the `PendingCrewTests` LocalRunner bundle and stays unit
/// testable (the request-building + frame-routing seams below) without a live
/// socket.
actor SessionProxyClient {
    /// Runner-actionable inbound frames (`.command` / `.permissionDecision`).
    /// The `subscribed` ack, errors, and viewer-facing frames are handled
    /// internally and never reach here. Consumed on the MainActor by the run.
    let inbound: AsyncStream<SessionProxyInbound>

    /// Viewer-side live state fan-out (`session.state` from the runner). Dormant
    /// for a runner peer (a runner never receives state). A watching client
    /// (role:.viewer) consumes this to render progress live instead of polling.
    let states: AsyncStream<SessionStateSnapshot>

    private let connectRequest: URLRequest
    private let role: ProxyRole
    private let token: String?
    private let session: URLSession
    private let inboundContinuation: AsyncStream<SessionProxyInbound>.Continuation
    private let statesContinuation: AsyncStream<SessionStateSnapshot>.Continuation

    private var task: URLSessionWebSocketTask?
    private var receiveLoop: Task<Void, Never>?
    private var pingLoop: Task<Void, Never>?
    /// Latest published state — re-sent after every (re)connect so a freshly
    /// connected viewer doesn't have to wait for the next transcript line.
    private var latestState: SessionState?
    private var reconnectAttempts = 0
    private var closed = false
    private var subscribed = false

    /// Ping cadence + backoff bounds. Conservative — the DO is non-hibernating
    /// so an idle socket only needs an occasional liveness probe.
    private static let pingInterval: Duration = .seconds(20)
    private static let maxBackoff: Double = 30

    init(
        baseURL: URL,
        sessionId: String,
        role: ProxyRole = .runner,
        token: String?,
        session: URLSession = .shared
    ) {
        self.role = role
        self.token = token
        self.session = session
        self.connectRequest = Self.makeConnectRequest(
            baseURL: baseURL, sessionId: sessionId, role: role, token: token)
        var cont: AsyncStream<SessionProxyInbound>.Continuation!
        self.inbound = AsyncStream { cont = $0 }
        self.inboundContinuation = cont
        var stateCont: AsyncStream<SessionStateSnapshot>.Continuation!
        self.states = AsyncStream { stateCont = $0 }
        self.statesContinuation = stateCont
    }

    // MARK: - Public lifecycle

    /// Open the socket and start the receive + ping loops. Idempotent-ish: a
    /// second call while already connected is a no-op. Safe to call before
    /// any state is published.
    func connect() {
        guard !closed, task == nil else { return }
        openSocket()
    }

    /// Publish the runner's current state to all viewers. Stored so it can be
    /// re-published on reconnect. Best-effort — a send failure is swallowed and
    /// the reconnect path will re-publish `latestState`.
    func publish(_ state: SessionState) {
        latestState = state
        guard subscribed, let task else { return }
        send(.sessionState(state), over: task)
    }

    /// Raise a manual-mode permission request to viewers. Best-effort.
    /// `id` = local correlation id (echoed back on `permission.request.ack`).
    /// Returns false when the frame could not be sent (not subscribed yet) so
    /// the caller can retry after (re)connect.
    @discardableResult
    func requestPermission(id: String? = nil, action: String, payload: [String: JSONValue]?, riskLevel: String?) -> Bool {
        guard subscribed, let task else { return false }
        send(.permissionRequest(id: id, action: action, payload: payload, riskLevel: riskLevel), over: task)
        return true
    }

    /// Viewer → DO: send a command to the session's runner (e.g. `cancel`).
    /// Best-effort over the live socket; the DO persists + queues it durably, so
    /// a momentary disconnect still reaches the runner on its next inbox pull.
    func sendCommand(kind: String, payload: [String: JSONValue]? = nil) {
        guard subscribed, let task else { return }
        send(viewer: .command(kind: kind, payload: payload), over: task)
    }

    /// Viewer → DO: approve/reject a pending permission request. Best-effort.
    func sendPermissionDecision(requestId: String, decision: String) {
        guard subscribed, let task else { return }
        send(viewer: .permissionDecision(requestId: requestId, decision: decision), over: task)
    }

    /// Tear down for good. Cancels loops + the socket and finishes `inbound`.
    /// After this the client will not reconnect.
    func close() {
        guard !closed else { return }
        closed = true
        subscribed = false
        pingLoop?.cancel(); pingLoop = nil
        receiveLoop?.cancel(); receiveLoop = nil
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        inboundContinuation.finish()
        statesContinuation.finish()
    }

    // MARK: - Socket bring-up

    private func openSocket() {
        subscribed = false
        let task = session.webSocketTask(with: connectRequest)
        self.task = task
        task.resume()
        // Send the subscribe frame immediately; the DO acks with `subscribed`,
        // which flips us live and triggers the first state re-publish.
        send(.subscribe(role: role, token: token), over: task)
        startReceiveLoop(on: task)
        startPingLoop(on: task)
    }

    private func startReceiveLoop(on task: URLSessionWebSocketTask) {
        receiveLoop?.cancel()
        receiveLoop = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    let message = try await task.receive()
                    guard let self else { return }
                    if case let .string(text) = message {
                        await self.dispatch(SessionProxyInbound.parse(text))
                    }
                    // Binary frames aren't part of this protocol — ignore.
                } catch {
                    // Receive failed → the socket is dead. Hand off to the
                    // reconnect path (unless we were closed deliberately).
                    await self?.handleDisconnect(task)
                    return
                }
            }
        }
    }

    private func startPingLoop(on task: URLSessionWebSocketTask) {
        pingLoop?.cancel()
        pingLoop = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.pingInterval)
                if Task.isCancelled { return }
                let ok = await withCheckedContinuation { (cont: CheckedContinuation<Bool, Never>) in
                    task.sendPing { error in cont.resume(returning: error == nil) }
                }
                if !ok {
                    await self?.handleDisconnect(task)
                    return
                }
            }
        }
    }

    // MARK: - Inbound routing

    /// Route one parsed inbound frame. `subscribed` flips us live + re-publishes
    /// state; runner-actionable frames go out on `inbound`; viewer-facing
    /// `session.state` (which the runner-oriented `SessionProxyInbound` parses as
    /// `.unknown`) is decoded and yielded on `states`; anything left is logged
    /// and dropped. Exposed at `internal` so a test can drive it without a
    /// socket; production calls it from the receive loop.
    func dispatch(_ frame: SessionProxyInbound) {
        switch frame {
        case .subscribed:
            subscribed = true
            reconnectAttempts = 0
            if let latestState, let task {
                send(.sessionState(latestState), over: task)
            }
        case .command, .permissionDecision, .permissionRequestAck:
            inboundContinuation.yield(frame)
        case let .error(code, message):
            print("[session-proxy] server error frame: \(code) \(message)")
        case let .unknown(raw):
            // A viewer's live state fan-out lands here (session.state is not a
            // runner-facing frame, so SessionProxyInbound degrades it). Decode
            // and surface it; truly-unknown frames just log.
            if let snapshot = SessionStateSnapshot.parse(raw) {
                statesContinuation.yield(snapshot)
            } else {
                print("[session-proxy] ignoring unknown inbound frame: \(raw)")
            }
        }
    }

    // MARK: - Reconnect

    /// Called when the live socket dies (receive error or failed ping). Drops
    /// the dead socket and, unless closed, schedules a backed-off reconnect.
    private func handleDisconnect(_ deadTask: URLSessionWebSocketTask) {
        // A stale loop from a previous socket can fire after we've already
        // moved on — ignore unless this is still the current task.
        guard !closed, task === deadTask else { return }
        subscribed = false
        task?.cancel(with: .abnormalClosure, reason: nil)
        task = nil
        pingLoop?.cancel(); pingLoop = nil
        receiveLoop?.cancel(); receiveLoop = nil

        let attempt = reconnectAttempts
        reconnectAttempts += 1
        let delay = min(Self.maxBackoff, pow(2, Double(attempt)))
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self else { return }
            await self.reconnectIfNeeded()
        }
    }

    private func reconnectIfNeeded() {
        guard !closed, task == nil else { return }
        openSocket()
    }

    // MARK: - Send helper

    private func send(viewer frame: SessionProxyViewerOutbound, over task: URLSessionWebSocketTask) {
        guard let data = try? frame.jsonData(),
              let text = String(data: data, encoding: .utf8) else { return }
        task.send(.string(text)) { error in
            if let error { print("[session-proxy] viewer send failed: \(error)") }
        }
    }

    private func send(_ frame: SessionProxyOutbound, over task: URLSessionWebSocketTask) {
        guard let data = try? frame.jsonData(),
              let text = String(data: data, encoding: .utf8) else { return }
        task.send(.string(text)) { error in
            if let error { print("[session-proxy] send failed: \(error)") }
        }
    }

    // MARK: - Request building (pure; unit tested)

    /// Build the WebSocket upgrade request: `http(s)` → `ws(s)`, the
    /// `/v1/sessions/<id>/proxy/connect` path, `?role=` query, and the
    /// device-grant bearer on `Authorization`. Pure + static so a test can
    /// assert the wire shape without opening a socket.
    static func makeConnectRequest(
        baseURL: URL,
        sessionId: String,
        role: ProxyRole,
        token: String?
    ) -> URLRequest {
        var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
            ?? URLComponents()
        switch components.scheme?.lowercased() {
        case "https": components.scheme = "wss"
        case "http": components.scheme = "ws"
        default: components.scheme = "wss"
        }
        // Preserve any base path (rare for the prod api host, but correct).
        var path = components.path
        if path.hasSuffix("/") { path.removeLast() }
        components.path = path + "/v1/sessions/\(sessionId)/proxy/connect"
        components.queryItems = [URLQueryItem(name: "role", value: role.rawValue)]

        var request = URLRequest(url: components.url!)
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }
}
