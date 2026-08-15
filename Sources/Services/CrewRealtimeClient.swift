import Foundation

/// 一个 crew 的实时 hub WebSocket 客户端（Phase 4b）。
///
/// **跨平台**：只依赖 Foundation（`URLSessionWebSocketTask`），无 macOS-only API，
/// 所以 macOS（本地 runner 唤醒 `CrewMailboxWaker`）与 iOS/iPad（登录态 EdgeBackend
/// 群聊实时刷新）共用同一条 hub 客户端，不再 `#if os(macOS)` gate。
///
/// 连接 `GET /v1/realtime-hub/conv/:crewId`（crewId == crew_conversation_id，
/// 是个 UUID），带 device-grant bearer。hub 是 **content-agnostic 的扇出层**：
/// 连上后只会收到 `{type:'ready', ts}` 一帧握手，之后是 `{type:'change',
/// table, op, record}` 行变更帧；客户端 **不**回任何帧（hub 的 `webSocketMessage`
/// 忽略一切入站消息，见 `apps/edge/src/durable-objects/hub.ts`）。
///
/// 与 `SessionProxyClient` 的关系：复用同一套 WS 基础设施（`URLSessionWebSocketTask`、
/// 20s ping keepalive、capped 指数退避重连、`Authorization: Bearer` 放 device-grant），
/// 但协议简单得多 —— **没有 subscribe 握手、没有出站帧、没有 viewer/runner 角色**。
///
/// **关键语义（Phase 4b 勘察确认）**：hub 帧 **不含** recipient_session_ids / @列表，
/// 所以它只是「这个 crew 有动静」的信号。订阅方（runner）收到 `.changed` →
/// 拉一次 `GET /v1/sessions/:sessionId/inbox` 看自己的 mailbox（@我的权威源）→
/// 空闲且有@我的待处理项 → 唤醒注入。把「3s 轮询」换成「事件驱动拉取」。
///
/// 自包含：只依赖 Foundation + 本文件里的纯 codec，所以能编进 `PendingCrewTests`
/// LocalRunner bundle，codec（frame JSON → 事件）可在不开 socket 的前提下单测。
/// 活体 socket 行为留手动 E2E。
actor CrewRealtimeClient {
    /// 对外事件流。runner（Phase 4b）和 UI（Phase 5）都订阅这条。
    /// `.changed` 是唯一会推的事件类型 —— `ready` 握手、voice_* 等非 crew 帧
    /// 在内部消化/忽略，不上抛。
    let events: AsyncStream<CrewRealtimeEvent>

    private let connectRequest: URLRequest
    private let token: String?
    private let session: URLSession
    private let continuation: AsyncStream<CrewRealtimeEvent>.Continuation

    private var task: URLSessionWebSocketTask?
    private var receiveLoop: Task<Void, Never>?
    private var pingLoop: Task<Void, Never>?
    private var reconnectAttempts = 0
    private var closed = false

    /// Ping cadence + backoff bounds（与 SessionProxyClient 一致）。
    private static let pingInterval: Duration = .seconds(20)
    private static let maxBackoff: Double = 30

    init(
        baseURL: URL,
        crewId: String,
        token: String?,
        session: URLSession = .shared
    ) {
        self.token = token
        self.session = session
        self.connectRequest = Self.makeConnectRequest(
            baseURL: baseURL, crewId: crewId, token: token)
        var cont: AsyncStream<CrewRealtimeEvent>.Continuation!
        self.events = AsyncStream { cont = $0 }
        self.continuation = cont
    }

    // MARK: - Public lifecycle

    /// 开 socket + 起接收/ping 循环。幂等：已连上时第二次调是 no-op。
    func connect() {
        guard !closed, task == nil else { return }
        openSocket()
    }

    /// 永久关停。取消循环 + socket 并 finish 事件流；之后不再重连。
    func close() {
        guard !closed else { return }
        closed = true
        pingLoop?.cancel(); pingLoop = nil
        receiveLoop?.cancel(); receiveLoop = nil
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        continuation.finish()
    }

    // MARK: - Socket bring-up

    private func openSocket() {
        let task = session.webSocketTask(with: connectRequest)
        self.task = task
        task.resume()
        // hub 无 subscribe 握手 —— 连上即开始扇出，直接进收/ping 循环。
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
                        await self.dispatch(CrewRealtimeEvent.parse(text))
                    }
                    // 二进制帧不属于本协议 —— 忽略。
                } catch {
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

    /// 路由一帧已解析事件。`.changed` 上抛订阅方；`.ready` / `.ignored` 内部消化。
    /// 一次成功收帧也意味着连接健康 → 复位退避计数。`internal` 暴露让测试不开
    /// socket 也能驱动它（生产从接收循环调）。
    func dispatch(_ event: CrewRealtimeEvent) {
        reconnectAttempts = 0
        switch event {
        case .changed:
            continuation.yield(event)
        case .ready, .ignored:
            break
        }
    }

    // MARK: - Reconnect

    private func handleDisconnect(_ deadTask: URLSessionWebSocketTask) {
        guard !closed, task === deadTask else { return }
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

    // MARK: - Request building (pure; unit tested)

    /// 构造 WS 升级请求：`http(s)` → `ws(s)`，路径
    /// `/v1/realtime-hub/conv/<crewId>`，device-grant bearer 放 `Authorization`。
    /// 纯 + static，让测试不开 socket 也能断言线缆形状。
    static func makeConnectRequest(
        baseURL: URL,
        crewId: String,
        token: String?
    ) -> URLRequest {
        var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
            ?? URLComponents()
        switch components.scheme?.lowercased() {
        case "https": components.scheme = "wss"
        case "http": components.scheme = "ws"
        default: components.scheme = "wss"
        }
        var path = components.path
        if path.hasSuffix("/") { path.removeLast() }
        components.path = path + "/v1/realtime-hub/conv/\(crewId)"

        var request = URLRequest(url: components.url!)
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }
}

// MARK: - CrewRealtimeEvent (codec; pure, unit tested)

/// hub 下行帧解码后的 Swift 事件。
///
/// 对应线缆 union（`apps/edge/src/lib/realtime-publish.ts`）：
/// - `{type:'change', table, op, record}` → `.changed`
/// - `{type:'ready', ts}`（连接握手）→ `.ready`
/// - 其它（voice_call / voice_cost 等非 crew 帧，或 malformed）→ `.ignored`
enum CrewRealtimeEvent: Equatable, Sendable {
    /// 某张实时表有行变更。`record` 自带的 `id`（DB 行恒有）best-effort 抽出，
    /// 给消费方做去重/日志；缺失时为 nil。**注意**：帧不含 @列表，所以
    /// `.changed` 只是「该 crew 有动静」的拉取信号，不是「@了谁」的权威源。
    case changed(table: String, op: String, recordId: String?)
    /// 连接就绪握手（`{type:'ready', ts}`）。
    case ready
    /// 非 crew-runner 关心的帧（voice_*、未知 type、malformed JSON）—— 丢弃。
    case ignored

    /// 解析一帧。永不抛 —— 坏帧降级成 `.ignored`，接收循环不会被垃圾帧打挂。
    static func parse(_ raw: String) -> CrewRealtimeEvent {
        guard let data = raw.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = obj["type"] as? String else {
            return .ignored
        }
        switch type {
        case "change":
            guard let table = obj["table"] as? String,
                  let op = obj["op"] as? String else {
                return .ignored
            }
            let recordId = (obj["record"] as? [String: Any])?["id"] as? String
            return .changed(table: table, op: op, recordId: recordId)
        case "ready":
            return .ready
        default:
            // voice_call / voice_cost 等非 crew 帧，runner 不关心。
            return .ignored
        }
    }
}
