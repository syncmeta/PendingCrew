#if os(macOS)
import Foundation
import Combine

/// `CodexAppServerConnection` reports notifications in arrival order, but spawning
/// one independent MainActor Task for each event discarded that ordering. A single
/// AsyncStream consumer preserves the connection order across the actor hop.
final class CodexNotificationSequencer: @unchecked Sendable {
    struct Event: @unchecked Sendable {
        let method: String
        let params: [String: Any]
    }

    let stream: AsyncStream<Event>
    private let continuation: AsyncStream<Event>.Continuation

    init() {
        var captured: AsyncStream<Event>.Continuation!
        stream = AsyncStream { captured = $0 }
        continuation = captured
    }

    func yield(method: String, params: [String: Any]) {
        continuation.yield(.init(method: method, params: params))
    }
}

/// codex session over app-server. Conforms to SessionBackend so CrewSessionRun
/// treats it like the terminal backend. Output goes to `transcript` (rendered by
/// CodexTranscriptView). `send` runs a turn; `interrupt` cancels the active turn;
/// `stop` ends the process. Per-turn whiteboard + approvals are injected via the
/// providers supplied by the runner.
@MainActor
final class CodexAppServerBackend: ObservableObject, SessionBackend {
    let kind: LocalCodingAgentKind = .codex
    let transcript = CodexTranscript()
    @Published private(set) var status: SessionStatus = .running
    var statusPublisher: Published<SessionStatus>.Publisher { $status }
    /// app-server 有真 turn 生命周期：turn/started→activeTurnId 置位，turn/completed→清空。
    /// （唤醒注入门禁用，main 语义保留。）
    var isBusy: Bool { activeTurnId != nil }
    /// UI 头像「干活中/空闲」用,与 isBusy 同值但可观察。turn/started→true,completed→false。
    @Published private(set) var isWorking = false
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }
    /// runner 健康异常 —— codex 侧目前只有一个可靠信号:`account/*` server-request
    /// (token 刷新请求 = 登录态失效),见 `handleServerRequest` 的 `.account` 分支。
    @Published private(set) var health: CrewSessionHealth?
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { $health }

    private let connection: CodexAppServerConnection
    private let cwd: String
    private var model: String?
    private var effort: String?
    private let resumeThreadId: String?
    private let developerInstructions: String?
    private let mcpServers: [String: Any]?
    /// Pulls the unread-whiteboard string to inject as a leading text input each turn.
    private let whiteboardProvider: () -> String?
    /// Routes a codex approval request into the existing approval UI; returns the
    /// decision string ("accept"/"decline"/…) once the human answers.
    private let approvalProvider: (_ summary: String, _ decisions: [String]) async -> String
    /// 「codex 在要一个我们给不出的回答，已代它拒绝」→ 发群通知（Todo #6）。
    /// 由 runner 注入（backend 层不认识白板/mention 模型）。
    private let notifyUnanswerable: (_ summary: String) -> Void
    /// 一轮结束（`turn/completed`）时喂本轮最后一条 agent 正文出去 —— 「这一轮它没往
    /// 群里说过话就系统替它留痕」的 codex 半边（人类 Todo #25 层 1，claude 那半边是
    /// Stop hook）。判定与文案全在 `SessionTurnTrace`，backend 只负责喂正文。
    private let notifyTurnEnded: (_ lastAgentText: String) -> Void
    /// 握手拿到 threadId 时回调 —— runner 把它记进 `LocalAgentSessionStore`，
    /// 下次重启这个成员就能 `thread/resume` 回同一条线（Todo #28）。
    private let notifyThreadId: (_ threadId: String) -> Void
    /// `thread/resume` 失败、已降级成新起一条 thread 时回调（Todo #28 fail-loud）——
    /// runner 据此往群里如实说「原会话接不回来了，这是新开的」，不静默假装恢复。
    private let notifyResumeFallback: (_ failedThreadId: String, _ reason: String) -> Void
    /// P2 app-side transcript 只吃协议 event；构造时注入以覆盖 boot 的第一条通知。
    private let protocolNotificationSink: ((_ method: String, _ params: [String: Any]) -> Void)?

    private var threadId: String?
    /// 交接事务的提交门：只有 app-server 已握手并拿到真实 thread id，才算新机长
    /// 真正可接活。不能拿构造时默认的 `.running` 冒充启动成功。
    var isLaunchReady: Bool { threadId?.isEmpty == false }
    private var activeTurnId: String?
    private let notificationSequencer = CodexNotificationSequencer()
    private var notificationTask: Task<Void, Never>?
    private var approvalsReviewer: CodexProtocol.ApprovalsReviewer
    /// 已通知过的 server-request method —— 同一种一轮只喊一次，别让某个每回合都来的
    /// 请求把群刷爆（同 health 的「每 Kind 一次」纪律）。
    private var notifiedMethods: Set<String> = []

    init(executable: String, argv: [String], cwd: String, env: [String: String],
         model: String?, effort: String?, resumeThreadId: String?,
         approvalsReviewer: CodexProtocol.ApprovalsReviewer = .autoReview,
         developerInstructions: String?, mcpServers: [String: Any]?,
         whiteboardProvider: @escaping () -> String?,
         approvalProvider: @escaping (_ summary: String, _ decisions: [String]) async -> String,
         notifyUnanswerable: @escaping (_ summary: String) -> Void = { _ in },
         notifyTurnEnded: @escaping (_ lastAgentText: String) -> Void = { _ in },
         notifyThreadId: @escaping (_ threadId: String) -> Void = { _ in },
         notifyResumeFallback: @escaping (_ failedThreadId: String, _ reason: String) -> Void = { _, _ in },
         protocolNotificationSink: ((_ method: String, _ params: [String: Any]) -> Void)? = nil) {
        self.notifyTurnEnded = notifyTurnEnded
        self.notifyThreadId = notifyThreadId
        self.notifyResumeFallback = notifyResumeFallback
        self.protocolNotificationSink = protocolNotificationSink
        self.connection = CodexAppServerConnection(executable: executable, argv: argv, cwd: cwd, env: env)
        self.cwd = cwd
        self.model = model
        self.effort = effort
        self.resumeThreadId = resumeThreadId
        self.approvalsReviewer = approvalsReviewer
        self.developerInstructions = developerInstructions
        self.mcpServers = mcpServers
        self.whiteboardProvider = whiteboardProvider
        self.approvalProvider = approvalProvider
        self.notifyUnanswerable = notifyUnanswerable
    }

    /// Boot: handshake → thread/start → send the first turn (if any).
    func boot(initialPrompt: String?) {
        // 拉起自检（#541）：握手卡住不返回时,下面的 `Task` 永远走不到 catch,
        // status 就一直停在 `.running` + isWorking 恒假 → 点名报「空闲」,机长照常
        // 派活。看门狗盯「进程没了」「到点还没握上手」，两种都翻 launchFailed。
        startLaunchWatchdog()
        let eventStream = notificationSequencer.stream
        notificationTask = Task { @MainActor [weak self] in
            for await event in eventStream {
                guard let self, self.status == .running else { return }
                self.handleNotification(method: event.method, params: event.params)
            }
        }
        Task {
            do {
                try await connection.start(
                    onServerRequest: { [weak self] id, method, params in
                        Task { await self?.handleServerRequest(id: id, method: method, params: params) }
                    },
                    onNotification: { [notificationSequencer] method, params in
                        notificationSequencer.yield(method: method, params: params)
                    },
                    onTerminate: { [weak self] code in
                        Task { @MainActor in
                            guard let self, self.status == .running else { return }   // stop() already set .exited; don't clobber
                            self.status = .exited(code)
                            self.isWorking = false
                        }
                    })
                var result: [String: Any]?
                if let resumeThreadId, !resumeThreadId.isEmpty {
                    // Todo #28：续跑原 thread。接不回来（thread 没了 / server 报错）
                    // **不装死**：降级新起一条，并回调让 runner 在群里如实说明。
                    do {
                        result = try await connection.request(
                            method: "thread/resume",
                            params: CodexProtocol.threadResumeParams(
                                threadId: resumeThreadId,
                                cwd: cwd,
                                model: model,
                                effort: effort,
                                developerInstructions: developerInstructions,
                                mcpServers: mcpServers,
                                approvalsReviewer: approvalsReviewer)) as? [String: Any]
                    } catch {
                        self.notifyResumeFallback(resumeThreadId, error.localizedDescription)
                        result = try await self.startFreshThread()
                    }
                } else {
                    result = try await self.startFreshThread()
                }
                let tid = (result?["thread"] as? [String: Any])?["id"] as? String
                self.threadId = tid
                if let tid, !tid.isEmpty { self.notifyThreadId(tid) }
                if let p = initialPrompt, !p.isEmpty { self.send(p) }
            } catch {
                // 此前这里只留 `.exited(1)`,错误正文直接丢 —— 机长只看到「已退出」
                // 却拿不到任何原因。现在先翻 health（留痕可读 + 白板 @机长）再翻 status。
                self.reportLaunchFailure(.spawnFailed, underlying: error.localizedDescription)
            }
        }
    }

    /// 新起一条 thread（首次启动，或 `thread/resume` 接不回来时的降级路径）。
    private func startFreshThread() async throws -> [String: Any]? {
        try await connection.request(
            method: "thread/start",
            params: CodexProtocol.threadStartParams(
                cwd: cwd,
                model: model,
                effort: effort,
                developerInstructions: developerInstructions,
                mcpServers: mcpServers,
                approvalsReviewer: approvalsReviewer)) as? [String: Any]
    }

    /// Apply the native Codex reviewer to a live thread. Persistence is achieved by
    /// also sending the same reviewer on every start/resume; this RPC covers a thread
    /// that was already running when the app setting changed.
    func updateApprovalsReviewer(_ reviewer: CodexProtocol.ApprovalsReviewer) async throws {
        guard let threadId else {
            throw CodexRPCError.malformed("codex thread is not ready")
        }
        _ = try await connection.request(
            method: "thread/settings/update",
            params: CodexProtocol.threadSettingsUpdateParams(
                threadId: threadId, approvalsReviewer: reviewer))
        approvalsReviewer = reviewer
    }

    /// Codex app-server exposes model and effort as live thread settings. The
    /// generated v2 schema describes both as overrides for subsequent turns, so
    /// the session can switch in place without manufacturing a new thread.
    func applyProfileSwitch(_ command: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        guard let threadId else { return .neverIdle }
        do {
            let params: [String: Any]
            switch command.knob {
            case .model:
                params = CodexProtocol.threadSettingsUpdateParams(
                    threadId: threadId, model: command.value)
            case .effort:
                params = CodexProtocol.threadSettingsUpdateParams(
                    threadId: threadId, effort: command.value)
            }
            _ = try await connection.request(method: "thread/settings/update", params: params)
            switch command.knob {
            case .model: model = command.value
            case .effort: effort = command.value
            }
            return .applied("thread/settings/update acknowledged")
        } catch {
            return .rejected(error.localizedDescription)
        }
    }

    // MARK: - 拉起自检（#541）

    private var launchWatchdog: Task<Void, Never>?
    /// 起 app-server 的时刻（自检算 elapsed 用）。
    private var bootStartedAt = Date()
    /// 至今有没有任何一轮观测到子进程活着。**必须逐轮累积**：看门狗这个 Task 排在
    /// `boot()` 里那个跑 `connection.start()` 的 Task 之前，第一轮 `isProcessRunning`
    /// 必然在 `Process.run()` 之前问到，答案恒为 false。没有这个累积量，探针会把
    /// 「还没 fork 完」读成「起来后立刻退出」，每个 codex session 一拉起就被误报。
    private var everSawProcessAlive = false

    private func startLaunchWatchdog() {
        bootStartedAt = Date()
        everSawProcessAlive = false
        launchWatchdog = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                guard let self, self.status == .running else { return }
                let alive = await self.connection.isProcessRunning
                if alive { self.everSawProcessAlive = true }
                let verdict = SessionLaunchProbe.verdict(
                    spawned: true,                    // 进程起不来会直接抛错走 catch
                    processAlive: alive,
                    everAlive: self.everSawProcessAlive,
                    // codex 的「第一手活迹」= 握手拿到 threadId（能收活了）。
                    sawOutput: self.threadId != nil,
                    elapsed: Date().timeIntervalSince(self.bootStartedAt))
                if SessionLaunchProbe.isTerminal(verdict) {
                    self.reportLaunchFailure(verdict, underlying: nil)
                    return
                }
                if verdict == .alive { return }
                try? await Task.sleep(
                    nanoseconds: UInt64(SessionLaunchProbe.pollInterval * 1_000_000_000))
            }
        }
    }

    /// 终局裁决 → health（`launchFailed`）+ 该退的翻 status。health 先于 status
    /// （run 侧 finalize 会收掉 health 观察）。
    private func reportLaunchFailure(_ verdict: SessionLaunchVerdict, underlying: String?) {
        guard let detail = SessionLaunchProbe.failureDetail(
            verdict, kind: .codex, underlying: underlying) else { return }
        health = CrewSessionHealth(kind: .launchFailed, detail: detail)
        isWorking = false
        // stalled = 进程还在（可能正卡着握手）——不替人做主杀，状态已如实标异常。
        if verdict != .stalled { status = .exited(1) }
    }

    func send(_ text: String) {
        guard let threadId else { return }
        let wb = whiteboardProvider()
        Task {
            _ = try? await connection.request(
                method: "turn/start",
                params: CodexProtocol.turnStartParams(threadId: threadId, text: text, whiteboard: wb))
        }
    }

    func interrupt() {
        guard let threadId, let turnId = activeTurnId else { return }
        Task {
            _ = try? await connection.request(
                method: "turn/interrupt",
                params: CodexProtocol.turnInterruptParams(threadId: threadId, turnId: turnId))
        }
    }

    func stop() {
        guard status == .running else { return }
        status = .exited(nil)
        isWorking = false
        launchWatchdog?.cancel()   // 主动停的别被自检倒打一耙报成「拉起失败」
        notificationTask?.cancel()
        Task { await connection.terminate() }
    }

    private func handleNotification(method: String, params: [String: Any]) {
        transcript.apply(method: method, params: params)
        // Update/seal local lifecycle before relaying the event. The remote facade
        // may publish idle synchronously, and its idle callback must already see a
        // ready continuation lease.
        trackTurn(method: method, params: params)
        protocolNotificationSink?(method, params)
    }

    private func trackTurn(method: String, params: [String: Any]) {
        if let detected = CodexProtocol.sessionHealth(method: method, params: params) {
            health = detected
        }
        if method == "turn/started" {
            activeTurnId = (params["turn"] as? [String: Any])?["id"] as? String
            isWorking = true
        } else if method.hasPrefix("item/") {
            // First-hand transcript activity repairs a missing/delayed start edge.
            isWorking = true
        }
        if method == "turn/completed" {
            let completedId = (params["turn"] as? [String: Any])?["id"] as? String
            if let activeTurnId, let completedId, activeTurnId != completedId {
                return
            }
            // Seal the exact turn's continuation before publishing idle.
            notifyTurnEnded(lastAgentText())
            activeTurnId = nil
            isWorking = false
            // A later successful turn is first-hand proof that a sticky quota
            // health flag is stale (for example after switching subscription
            // windows). Clear it immediately instead of waiting for the old
            // reset wakeup to fire hours later.
            if (params["turn"] as? [String: Any])?["status"] as? String == "completed",
               health?.isQuotaRelated == true {
                health = nil
            }
        }
    }

    /// transcript 里最后一条 agent 正文（没有 → 空串 → 上层判定不发）。
    private func lastAgentText() -> String {
        for item in transcript.items.reversed() {
            if case let .agentMessage(text, _) = item.kind { return text }
        }
        return ""
    }

    /// EVERY server-request must be answered — codex blocks the turn waiting on our
    /// reply, so silently dropping one = 「运行中…」转圈不结束 forever (the codex hang).
    /// The old `guard hasSuffix("requestApproval") else { return }` dropped elicitations,
    /// user-input requests, dynamic tool calls, token refreshes — any of which hung the turn.
    private func handleServerRequest(id: Int, method: String, params: [String: Any]) async {
        switch CodexProtocol.serverRequestKind(method: method) {
        case .approval:
            let summary = (params["command"] as? String) ?? (params["reason"] as? String) ?? method
            guard CodexProtocol.approvalRequestDisposition(reviewer: approvalsReviewer)
                    == .presentCard else {
                // auto_review owns routine command/file/network decisions inside Codex.
                // A request that still reaches the client was already in flight while
                // switching modes; do not raise a card and, critically, do not emit the
                // "待审批" whiteboard notice that only makeApprovalProvider may create.
                try? await connection.respondError(
                    serverId: id,
                    code: -32000,
                    message: "approval request reached client while auto_review is enabled")
                return
            }
            let decisions = CodexProtocol.safeApprovalDecisions(params: params)
            let decision = await approvalProvider(summary, decisions)
            try? await connection.respond(
                serverId: id,
                result: CodexProtocol.approvalResponse(
                    method: method, params: params, decision: decision))
        case .elicitation:
            // codex 0.137.0 delivers the MCP tool-call approval prompt AS an elicitation
            // (`_meta.codex_approval_kind == "mcp_tool_call"`). Auto-approve those — the
            // crew comms tools are the session's own trusted channel (mirrors claude
            // excluding them from its PreToolUse gate); declining them rejected every
            // crew tool call. Genuine input-form elicitations still decline (no v1 UI; a
            // reply still lets the turn proceed instead of hanging). See elicitationResult.
            let result = CodexProtocol.elicitationResult(params: params)
            // 真·输入表单被我们代拒了 —— **别再闷声干这件事**（Todo #6）：不阻塞是对的
            // （阻塞就成了另一种死法），但群里得知道「它想要个回答、我们给不出」，
            // 否则人只看到这个 session 莫名其妙干不成活。
            if (result["action"] as? String) != "accept" {
                announceUnanswerable(method: method, params: params)
            }
            try? await connection.respond(serverId: id, result: result)
        case .account:
            // codex 要客户端刷新 ChatGPT token(account/chatgptAuthTokens/refresh)。
            // 无法代刷 —— 仍回错误让 turn 继续(不挂死),但同时翻 health:这是
            // 「codex 登录态失效」的强信号,浮出到成员状态点/白板,别再静默丢。
            health = CrewSessionHealth(
                kind: .authRequired,
                detail: "Codex 登录态失效（ChatGPT token 刷新请求客户端无法处理）—— 在终端跑 codex login 重新登录后重启该 session。")
            try? await connection.respondError(serverId: id, code: -32601,
                                               message: "client does not handle \(method)")
        case .unsupported:
            // Fail-safe for anything we don't model (item/tool/requestUserInput,
            // item/tool/call, attestation/*): reply with an error so codex
            // fails just that operation and continues the turn rather than blocking.
            // 同样发群（Todo #6）—— `item/tool/requestUserInput` 就是「工具在等人输入」，
            // 静默回错误等于把一次需要人的请求悄悄丢掉。
            announceUnanswerable(method: method, params: params)
            try? await connection.respondError(serverId: id, code: -32601,
                                               message: "client does not handle \(method)")
        }
    }

    /// Quota-reset wakeups use the shared `SessionBackend` hook. Codex health
    /// is protocol-driven, so there is no scanner to rearm; clearing the sticky
    /// quota flag is sufficient and lets a future notification raise it again.
    func clearQuotaHealth() {
        if health?.isQuotaRelated == true { health = nil }
    }

    /// 发一条「它要个回答、我们给不出、已代拒」的群通知；每种 method 只喊一次。
    private func announceUnanswerable(method: String, params: [String: Any]) {
        guard notifiedMethods.insert(method).inserted else { return }
        notifyUnanswerable(CodexProtocol.pendingRequestSummary(method: method, params: params))
    }
}
#endif
