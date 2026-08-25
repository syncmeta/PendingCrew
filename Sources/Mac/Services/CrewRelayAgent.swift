#if os(macOS)
import Foundation

/// edge 信箱同步代理（接合 v2 block 3 relay）。
///
/// 对所有 `remoteConversationId != nil` 的本地 crew 跑双向搬运。拉取侧
/// **hub 推送优先**（CC-P4）：每个绑定 crew 开一条 `CrewRealtimeClient` 连
/// `conv:<remoteId>` hub，`.changed` 帧 → 立即拉该 crew（`CrewHubPullCoalescer`
/// 保证同 crew 单飞、事件风暴合并成一次补拉）。5s 轮询保留作兜底 —— 覆盖
/// hub 断线/重连窗口的拉取补漏，且上行推送侧本就不经 hub：
/// - **拉**：`GET /v1/crews/:remoteId/messages?since=<relayCursor>` →
///   过滤掉 Mac 自己推上去的（`relay.origin == mac_relay`）和已同步过的
///   （按 edge message id 幂等）→ 写进本地白板（带 edge 侧发送者名字、
///   `senderSessionId = nil`）→ 游标推进到 `lastCursor`。
/// - **推**：本地白板推送水位之后的、本地产生的（非 relay 写入的）新消息 →
///   `POST messages`（senderLabel = 本地发送者名、localSessionId =
///   senderSessionId；edge 据此打 `origin:mac_relay`，拉取侧不会拉回）→
///   每发成功一条就推进水位（半途失败下个 tick 从断点续推）。
///
/// **绑定基线**：首 tick 发现水位为 nil 且白板非空 → 把水位钉在当前白板末尾、
/// 本 tick 不上行 —— 接入前的本地历史不回灌 edge（spec §3.3：只搬"之后"的）。
///
/// 纯合并/过滤/水位逻辑在 `CrewRelaySyncLogic`（可单测）；本类只做编排 + IO。
/// 网络错误静默吞掉（console log），下一 tick 自然重试 —— relay 本来就是
/// best-effort 搬运，断线靠 cursor/水位补齐。
///
/// captain 感知（M4）：relay 消息落进 LocalWhiteboardStore 后，captain 的
/// 世界观注入 / 唤醒（LocalSessionWorldModel 读同一份白板）自动覆盖，无需新通道。
@MainActor
final class CrewRelayAgent: ObservableObject {
    private weak var appModel: AppModel?
    /// task_request 自动起 session 用（#242）。MacThreePaneView 注入（与右栏
    /// 手动起 session 是同一个 runner —— 自动起的 run 直接出现在切换条上）。
    private weak var sessionRunner: CrewSessionRunner?
    private var timer: Timer?
    /// tick 重入保护 —— 上一轮还在网络 IO 时跳过本轮。
    private var ticking = false

    private let crewStore: LocalCrewStore
    private let whiteboard: LocalWhiteboardStore

    /// hub 订阅（CC-P4）：本地 crewId → 远端 conv hub 的 WS 客户端 / 事件泵 /
    /// 拉取合并器。与 relay 绑定集合的对账挂在已有的 5s tick 上（绑定变化低频，
    /// 心跳级对账足够；断线重连由 client 自身的 capped 退避负责）。
    private var hubClients: [String: CrewRealtimeClient] = [:]
    private var hubPumps: [String: Task<Void, Never>] = [:]
    private var hubCoalescers: [String: CrewHubPullCoalescer] = [:]

    init(crewStore: LocalCrewStore? = nil, whiteboard: LocalWhiteboardStore? = nil) {
        self.crewStore = crewStore ?? .shared
        self.whiteboard = whiteboard ?? .shared
    }

    /// 启动轮询（MacThreePaneView `.task` 调，幂等）。未登录时 tick 是 no-op，
    /// 登录后无需重启 —— 下个 tick 自动开始干活。
    ///
    /// ⚠️ 只有编排者进程有资格起它（spec §6.2 闸门 1）。viewer 里误起 = 当场崩，
    /// 不是悄悄跑成双头 —— 双头会让同一批账被两个进程交替覆盖、唤醒发两遍，
    /// 而那种症状事后基本查不出来。
    func start(appModel: AppModel, sessionRunner: CrewSessionRunner? = nil) {
        precondition(
            ProcessRole.current == .orchestrator,
            "\(type(of: self)).start 只能在编排者进程里调用，当前角色=\(ProcessRole.current.rawValue)")
        self.appModel = appModel
        self.sessionRunner = sessionRunner
        guard timer == nil else { return }
        let t = Timer(timeInterval: 5, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in await self?.tick() }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
        Task { await tick() }   // 启动立即跑一轮，不等首个 5s
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        reconcileHubConnections(bindings: [])   // 关掉全部 hub 订阅
    }

    // MARK: - Tick

    private func tick() async {
        guard !ticking else { return }
        ticking = true
        defer { ticking = false }
        // hub 对账放 auth guard 前 —— 退出登录时把已开的 hub 连接收干净。
        let bindings = (appModel?.isAuthenticated == true) ? crewStore.listRelayBindings() : []
        reconcileHubConnections(bindings: bindings)
        guard let appModel, appModel.isAuthenticated,
              let api = try? appModel.loggedAPIClient() else { return }
        for binding in bindings {
            await pull(binding, api: api)
            await push(binding, api: api)
        }
    }

    // MARK: - hub 订阅（CC-P4：远端消息推送 → 立即拉，5s 轮询降为兜底）

    /// 与 relay 绑定集合对账 hub 连接：新绑定开 socket + 事件泵，解绑/登出关掉。
    /// 幂等，挂在每个 tick 上。无 device-grant 凭据（imageAuth nil）时视同无绑定。
    private func reconcileHubConnections(bindings: [LocalCrewRelayBinding]) {
        let auth = appModel?.imageAuth
        let byId = Dictionary(bindings.map { ($0.crewId, $0.remoteConversationId) },
                              uniquingKeysWith: { a, _ in a })
        let desired: Set<String> = auth == nil ? [] : Set(byId.keys)
        let diff = CrewRelayHubLogic.reconcile(desired: desired, connected: Set(hubClients.keys))
        for crewId in diff.close {
            hubPumps[crewId]?.cancel(); hubPumps[crewId] = nil
            hubCoalescers[crewId] = nil
            if let client = hubClients.removeValue(forKey: crewId) {
                Task { await client.close() }
            }
        }
        guard let auth else { return }
        for crewId in diff.open {
            guard let remoteId = byId[crewId] else { continue }
            let client = CrewRealtimeClient(
                baseURL: auth.baseURL, crewId: remoteId, token: auth.token)
            hubClients[crewId] = client
            Task { await client.connect() }
            hubPumps[crewId] = Task { [weak self] in
                for await event in await client.events {
                    if Task.isCancelled { return }
                    if case .changed = event {
                        await self?.hubPull(crewId: crewId)
                    }
                }
            }
        }
    }

    /// hub 事件触发的即时拉取。合并器保证同 crew 单飞；飞行期间的事件合并成
    /// 结束后一次补拉（拉取按游标取增量，一次补拉覆盖全部积压）。
    /// 与 5s tick 的 pull 并发也安全：`known` 集合在网络返回后才读、追加是
    /// 同步段，重复条目会被幂等去重；`?since` 本就是闭区间语义。
    private func hubPull(crewId: String) async {
        var c = hubCoalescers[crewId] ?? CrewHubPullCoalescer()
        let start = c.shouldStart()
        hubCoalescers[crewId] = c
        guard start else { return }
        repeat {
            if let appModel, appModel.isAuthenticated,
               let api = try? appModel.loggedAPIClient(),
               let binding = crewStore.relayBinding(for: crewId) {
                await pull(binding, api: api)
            }
            var done = hubCoalescers[crewId] ?? CrewHubPullCoalescer()
            let again = done.didFinish()
            hubCoalescers[crewId] = done
            if !again { break }
        } while true
    }

    /// 拉：edge 新条目 → 本地白板。
    private func pull(_ b: LocalCrewRelayBinding, api: PendingCrewAPI) async {
        do {
            let page = try await api.listCrewWhiteboardPage(
                crewId: b.remoteConversationId, since: b.relayCursor)
            let known = whiteboard.relayRemoteIds(crewId: b.crewId)
            let importable = Set(CrewRelaySyncLogic.importableRemoteIds(
                pulled: page.whiteboard.map {
                    .init(remoteId: $0.id, isMacRelayOrigin: $0.relay?.origin == "mac_relay")
                },
                known: known))
            for entry in page.whiteboard where importable.contains(entry.id) {
                let text = entry.displayText
                guard !text.isEmpty else { continue }
                whiteboard.appendRelayMessage(
                    crewId: b.crewId,
                    remoteId: entry.id,
                    senderKind: entry.senderKind,
                    senderDisplayName: Self.remoteSenderName(entry),
                    text: text,
                    createdAt: entry.createdAt,
                    // Task 10 规则 2：mentions（映射回本地形状，滤掉本地不消费的
                    // bot/broadcast kind）/ inReplyTo / senderUserId 原样落地 ——
                    // 前者喂 #554 断链修复（CrewLocalMentionWakeLogic 规则 3）。
                    mentions: Self.localMentions(entry.mentions),
                    inReplyTo: entry.inReplyTo,
                    senderUserId: entry.senderUserId)
            }
            if let cursor = page.lastCursor {
                crewStore.setRelayCursor(crewId: b.crewId, cursor: cursor)
            }
            // Task 10 规则 4：crew_todo_add 落账。只对本轮 importable（首次落地）
            // 的条目触发 —— 已落地过的 remoteId 下轮 pull 不会再进 importable 集合，
            // 天然幂等，不需要像 task_request 那样另记一份 processed 账本（todo
            // 落账没有「明确不可行、标记跳过不重试」的分支，落地即完成）。
            for entry in page.whiteboard
                where importable.contains(entry.id) && entry.messageKind == "crew_todo_add" {
                let text = entry.displayText
                guard !text.isEmpty else { continue }
                await handleRemoteTodoAdd(text: text, binding: b)
            }
            // #242 遥控 v1：本批里的新 task_request 指令 → 本地起 session。
            // 白板写入在上面照常发生（用户能看到这条指令本身）。
            let requests = CrewRelaySyncLogic.newTaskRequests(
                pulled: page.whiteboard.map {
                    .init(remoteId: $0.id,
                          isMacRelayOrigin: $0.relay?.origin == "mac_relay",
                          isTaskRequest: $0.messageKind == "task_request",
                          action: $0.payload?.action,
                          taskBrief: $0.payload?.taskBrief,
                          runnerKind: $0.payload?.runnerKind)
                },
                processed: crewStore.processedTaskRequestIds(crewId: b.crewId))
            for req in requests {
                await handleTaskRequest(req, binding: b, api: api)
            }
        } catch {
            NSLog("[CrewRelayAgent] pull \(b.crewId) failed: \(error.localizedDescription)")
        }
    }

    // MARK: - crew_todo_add → 本地 Todo 落账（Task 10 规则 4）

    /// 远端（iOS）人类经 relay crew 发的 Todo → 落本地 `LocalTodoStore` + 群里
    /// 回执 + 唤醒机长。编排复用 `CrewLocalTodoLanding.land`（从
    /// `CrewChatView.sendTodo` 抽出，composer 直发 / relay 落地不重复两份）。
    /// 走 `appModel.backend`（Mac 上恒为 `LocalBackend`）而不是裸 `api` ——
    /// 回执要落本地白板（本地 session 能看到）且被 `push()` 自然带回 edge，
    /// 与人类在本机点 Todo 模式发送时的路径完全一致。fail-soft：失败只记日志，
    /// 不阻塞本 tick 其余处理（网络错误下一 tick 重试；importable 已消费，本条
    /// 不会重放 —— 失败即丢，与其它 relay best-effort 语义一致）。
    private func handleRemoteTodoAdd(text: String, binding b: LocalCrewRelayBinding) async {
        guard let runner = sessionRunner, let backend = appModel?.backend else { return }
        do {
            _ = try await CrewLocalTodoLanding.land(
                crewId: b.crewId, text: text, backend: backend, sessionRunner: runner)
        } catch {
            NSLog("[CrewRelayAgent] remote todo_add landing failed: \(error.localizedDescription)")
        }
    }

    // MARK: - task_request → 本地起 session（#242 遥控 v1）

    /// 处理一条远程 task_request：成功 / 明确不可行（无 runner / 无工作目录 /
    /// session 满）都标记 processed —— 不可行时上行一条 mac_relay 说明，用户
    /// 看到后可调整再发一条新指令；不做静默重试（避免每 tick 反复尝试 + 刷屏）。
    private func handleTaskRequest(
        _ req: CrewRelaySyncLogic.RelayTaskRequest,
        binding b: LocalCrewRelayBinding,
        api: PendingCrewAPI
    ) async {
        // 先落 processed —— start 是 async，防极端情况下（本 tick 内异常）重复处理。
        crewStore.markTaskRequestProcessed(crewId: b.crewId, remoteId: req.remoteId)

        func reportBack(_ text: String) async {
            do {
                // senderLabel 必带 → edge 打 origin:mac_relay，拉取侧不会拉回。
                _ = try await api.postCrewMessage(
                    crewId: b.remoteConversationId, text: text, senderLabel: "Mac")
            } catch {
                NSLog("[CrewRelayAgent] task_request report-back failed: \(error.localizedDescription)")
            }
        }

        guard let runner = sessionRunner else {
            NSLog("[CrewRelayAgent] task_request \(req.remoteId) skipped: no sessionRunner injected")
            return
        }
        guard let wd = b.workingDirectory, !wd.isEmpty else {
            await reportBack("任务已收到，但这个 crew 没有工作目录，设置后重发指令。")
            return
        }
        let dir = URL(fileURLWithPath: (wd as NSString).expandingTildeInPath)

        // 镜像 CrewSessionWindowView 手动起 worker session 的工序：
        // kind → isolation workdir → 世界观 → 本地 comms（MCP + hooks）。
        let kind: LocalCodingAgentKind =
            (req.runnerKind?.contains("codex") == true) ? .codex : .claudeCode
        var cfg = SessionConfig(kind: kind, initialPrompt: req.taskBrief)
        let workdir: URL
        do {
            workdir = try SessionWorkspace.resolve(
                crewDirectory: dir, isolation: cfg.isolation, hint: req.taskBrief)
        } catch {
            await reportBack("任务已收到，但准备 session 工作目录失败：\(error.localizedDescription)")
            return
        }
        let localSessionId = UUID().uuidString.lowercased()
        // 世界观 + comms 接线按 kind 分叉：claude 走文件标志（--append-system-prompt-file
        // / --settings / --mcp-config）；codex 没这些通道 —— 世界观经 developerInstructions、
        // MCP 经 mcpServers dict 走协议传入。best-effort：渲染失败 nil 不挡启动。
        var codexDevInstructions: String? = nil
        var codexMcp: [String: Any]? = nil
        let members: [CrewMember]
        if let backend = appModel?.backend {
            members = (try? await backend.listCrewMembers(crewId: b.crewId))?.members ?? []
        } else {
            members = []
        }
        let detail = crewStore.getCrew(b.crewId)
        // relay 自动起的 worker session 的白板发送者名：agent 名 + session id 前缀
        // （与手动起 / CrewSessionRun.displayName 同款），post_to_crew 不再裸 uuid。
        let workerLabel = kind.displayName + " · " + String(localSessionId.prefix(6))
        switch kind {
        case .claudeCode:
            // claude 行为不变：detail+backend 都在才渲染世界观文件；comms 恒接。
            if let detail, appModel?.backend != nil {
                cfg.appendSystemPromptFile = LocalSessionLaunch.renderWorldModelFile(
                    detail: detail, members: members, taskBrief: req.taskBrief,
                    workdir: workdir, sessionId: localSessionId, runnerKind: kind)
            }
            let comms = LocalSessionLaunch.prepareLocalCommsConfig(
                crewId: b.crewId, sessionId: localSessionId, label: workerLabel)
            cfg.settingsFile = comms.settings
            cfg.mcpConfigFile = comms.mcp
        case .codex:
            if let detail {
                codexDevInstructions = LocalSessionLaunch.renderWorldModelString(
                    detail: detail, members: members, taskBrief: req.taskBrief,
                    workdir: workdir, sessionId: localSessionId)
            }
            codexMcp = LocalSessionLaunch.codexMcpServers(
                crewId: b.crewId, sessionId: localSessionId, label: workerLabel)
        case .terminal:
            // relay 只编排会回话的 agent；纯终端只能由人在 Mac 新建页打开。
            return
        }
        do {
            try await runner.start(
                crewId: b.crewId,
                sessionId: localSessionId,
                config: cfg,
                workingDirectory: workdir,
                taskBrief: req.taskBrief,
                developerInstructions: codexDevInstructions,
                codexMcpServers: codexMcp)
            await reportBack("已启动 \(kind.displayName) session（\(localSessionId.prefix(8))），任务：\(req.taskBrief)")
        } catch {
            await reportBack("任务已收到，但起 session 失败：\(error.localizedDescription)")
        }
    }

    /// 推：本地新消息 → edge。
    private func push(_ b: LocalCrewRelayBinding, api: PendingCrewAPI) async {
        let local = whiteboard.list(crewId: b.crewId)
        // 绑定基线：水位 nil 且白板已有历史 → 钉在末尾，不回灌历史。
        if b.relayPushedThroughId == nil, let last = local.last {
            crewStore.setRelayPushedThrough(crewId: b.crewId, messageId: last.id)
            return
        }
        let pendingIds = Set(CrewRelaySyncLogic.pendingPushIds(
            local: local.map { .init(id: $0.id, isRelayWritten: $0.relayRemoteId != nil) },
            pushedThroughId: b.relayPushedThroughId))
        for m in local where pendingIds.contains(m.id) {
            do {
                // senderLabel 必带 —— edge 靠它打 origin:mac_relay（回环防护）。
                // Task 10 规则 1：mentions（本地形状 → edge CrewMention）/
                // replyToId 随文本一起透传上行，edge 读模型再把它们透出给 iOS。
                _ = try await api.postCrewMessage(
                    crewId: b.remoteConversationId,
                    text: m.text,
                    mentions: Self.wireMentions(m.mentions),
                    senderLabel: Self.localSenderLabel(m, captainName: b.captainName),
                    localSessionId: m.senderSessionId,
                    replyToId: m.inReplyTo)
                crewStore.setRelayPushedThrough(crewId: b.crewId, messageId: m.id)
            } catch {
                // 半途失败 → 水位停在最后成功那条，下一 tick 续推（顺序保持）。
                NSLog("[CrewRelayAgent] push \(b.crewId) failed: \(error.localizedDescription)")
                return
            }
        }
    }

    // MARK: - mentions 映射（Task 10 规则 1/2）

    /// edge `CrewMention` → 本地 `LocalWhiteboardMention`（拉取侧）。edge kind
    /// ∈ human/session/captain/broadcast/bot；本地只认 session/captain/human/broadcast，
    /// `bot` 滤掉（本地没有对应语义）。
    ///
    /// **#62：`broadcast` 不再滤。** 它过去被当成「本地不消费」丢掉，那时确实如此；
    /// 现在它是 `CrewWhiteboardVisibility` 的**显式放宽器**（`[broadcast, session(X)]`
    /// = 全组可见 + 只叫醒 X），滤掉就等于把作者明写的「全组都该看见」在落地这一步
    /// 静默降级回排他。唤醒面不受影响 —— `CrewLocalMentionWakeLogic` /
    /// `CrewLocalMentionInjectLogic` 照旧只认 session/captain。
    ///
    /// 空 / nil → nil（与 `LocalWhiteboardMessage.mentions` 的「无 @」语义对齐，
    /// 不用空数组占位）。
    static func localMentions(_ mentions: [CrewMention]?) -> [LocalWhiteboardMention]? {
        guard let mentions else { return nil }
        let mapped = mentions.compactMap { m -> LocalWhiteboardMention? in
            guard m.kind == "session" || m.kind == "captain"
                    || m.kind == "human" || m.kind == "broadcast" else { return nil }
            return LocalWhiteboardMention(kind: m.kind, targetId: m.targetId)
        }
        return mapped.isEmpty ? nil : mapped
    }

    /// 本地 `LocalWhiteboardMention` → edge `CrewMention`（推送侧）。两者形状
    /// 本就对齐（`LocalWhiteboardMention` 的 CodingKeys 就是照 edge 的
    /// `{kind, target_id}` 定的），直接逐项转型；nil → `[]`（`postCrewMessage`
    /// 的 `mentions` 形参无默认 nil，空数组语义即「无定向 @」）。
    static func wireMentions(_ mentions: [LocalWhiteboardMention]?) -> [CrewMention] {
        (mentions ?? []).map { CrewMention(kind: $0.kind, targetId: $0.targetId) }
    }

    // MARK: - 名字映射

    /// edge 条目在本地白板上的显示名（senderDisplayName）。edge 现在会下发解析出
    /// 的真实姓名（`sender_display_name`：human 走 live `users.display_name` /
    /// roster 快照，relay 上行消息优先 Mac 侧 `senderLabel`——见 edge
    /// `mapMessageToEntry`），user/human 优先用它；解析不出（老消息 / 未匹配到
    /// roster）才落回 kind + id 缩写兜底。
    static func remoteSenderName(_ entry: CrewWhiteboardEntry) -> String {
        switch entry.senderKind {
        case "user", "human":
            if let name = entry.senderDisplayName, !name.isEmpty { return name }
            return "PendingBot 用户" + (entry.senderUserId.map { " · \($0.prefix(8))" } ?? "")
        case "bot":
            return "bot" + (entry.senderBotId.map { " · \($0.prefix(8))" } ?? "")
        case "session":
            return CrewSenderNaming.sessionFallback(entry.senderSessionId)
        default:
            return entry.senderKind
        }
    }

    /// 本地消息上行时的 senderLabel（edge 落 attachments.senderLabel，iOS 端
    /// 展示来源）。session 有真名（senderName = session title）优先，缺才兜底
    /// 「会话 · xxxxxx」——否则 iOS 端全显缩写 id（#530 ⑥，title 单一真值）。
    static func localSenderLabel(_ m: LocalWhiteboardMessage, captainName: String?) -> String {
        switch m.senderKind {
        case "user": return "我 (Mac)"
        case "session":
            if let n = m.senderName, !n.isEmpty { return n }
            return CrewSenderNaming.sessionFallback(m.senderSessionId)
        // 旧 JSON 默认名存过英文 "Captain" —— 与本地展示同规则归一成「机长」。
        case "captain": return (captainName == "Captain" ? nil : captainName) ?? "机长"
        default: return m.senderKind
        }
    }
}
#endif
