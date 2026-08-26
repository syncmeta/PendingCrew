import Foundation
import Combine

/// PendingCrew 数据后端的统一抽象。
///
/// **接合 v2(spec 2026-06-10:本地为家,edge 是邮差)—— 双轨平行路由作废**:
/// - `LocalBackend` 是**常驻 home**:macOS 上 crew CRUD / 白板 / 花名册
///   永远走它,本地 crew 永远在、永远显示。
/// - `EdgeBackend` 退化成"叠加能力的 API 面":遥控、未来的信箱 + 邀请,
///   以及 iOS(暂无本地后端)的过渡路径。**不再平行路由 crew 数据**;
///   能力判定用 `AppModel.isAuthenticated`,不要再用 `mode` 选后端。
///
/// **设计原则**:protocol 返回值 shape **跟 edge 一致**(`CrewSummary` /
/// `CrewDetail` / `UserSubject` 几个 model 复用),这样 UI 层 + `CrewStore`
/// 不分叉。
///
/// **未来扩展**:edge 信箱(store-and-forward)+ 远端成员接入在 block 3
/// 落地时,以"本地真源 + edge 中转"接进 LocalBackend 的白板路径,而不是
/// 给 EdgeBackend 恢复平行地位。
@MainActor
protocol PendingCrewBackend: AnyObject {
    // MARK: - Subject
    func listMySubjects() async throws -> [UserSubject]

    // MARK: - Machines
    /// 本账号可用的机器列表（本机 / peer 电脑 / Fly machine）。
    /// - EdgeBackend：`GET /v1/machines`。
    /// - LocalBackend：返回单元素 `[本机]`（未登录也有一台「本机」）。
    func listMachines() async throws -> [Machine]
    /// 把当前设备 upsert 成本账号的一台 computer 机器，返回 machineId。
    /// - EdgeBackend：`POST /v1/machines/register-self`。
    /// - LocalBackend：no-op，返回合成的本机 id。幂等。
    func registerSelfMachine() async throws -> String

    // MARK: - Crew
    func listCrews() async throws -> [CrewSummary]
    func getCrew(_ crewId: String) async throws -> CrewDetail
    func createCrew(_ request: CreateCrewRequest) async throws -> CreateCrewResponse

    /// 把 crew 挂到一个父 crew 之下。
    /// - LocalBackend:写本地 DAG 父边(`LocalCrewStore.attachParent`,禁环)。
    ///   `childKeepsBps` 本地暂时忽略(责任分账是后续计费的事)。
    /// - EdgeBackend:`POST /v1/crews/:crewId/attach-parent`(带 bps)。
    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws

    /// 解绑一条父边。LocalBackend → 删本地 DAG 父边;EdgeBackend → 暂无端点,
    /// 抛 `notSupportedInByok`(DAG 解绑目前是本地特性)。
    func detachParent(crewId: String, parentCrewId: String) async throws

    // MARK: - Crew comms（白板/群聊，spec local-first §8.4 单后端）

    /// crew 白板时间线。edge → `GET /v1/crews/:id/messages`;BYOK → 本地
    /// `LocalWhiteboardStore`(映射成同形 `CrewWhiteboardEntry`)。
    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry]

    /// crew 花名册。edge → `GET /v1/crews/:id/members`;BYOK → 从本地 crew 派生
    /// (captain + 本机人类)。
    func listCrewMembers(crewId: String) async throws -> CrewRoster

    /// 发一条群聊消息(默认广播)。`attachmentIds` 只 edge 支持;BYOK 非空即抛
    /// `notSupportedInByok`。`replyToId` 非 nil = 回复某条消息(edge 端点据此自动
    /// @ 原发送者并记 `reply_to`;LocalBackend 记本地白板 `in_reply_to` 引用)。
    /// `localAttachments` 只本地支持(Todo #3:已由 `CrewChatAttachmentStore` 落盘
    /// 的附件条目,LocalBackend 挂到白板消息上;EdgeBackend 非空即抛)。
    func postCrewMessage(
        crewId: String, text: String, mentions: [CrewMention],
        attachmentIds: [String], replyToId: String?,
        localAttachments: [LocalWhiteboardAttachment]) async throws

    /// crew 白板/花名册的**事件驱动变更流**（Phase 5：去 3s 轮询）。
    ///
    /// 每个 tick = 「这个 crew 的白板/成员可能有动静，去 refresh 一次」的信号
    /// （**不**携带 diff —— 订阅方收到即重拉 `listCrewWhiteboard` / `listCrewMembers`）。
    /// - `LocalBackend`：基于 `LocalWhiteboardStore.changes`（本地 append 即 tick）。
    /// - `EdgeBackend`：基于 `CrewRealtimeClient` 的 hub 事件（收到白板相关表的
    ///   `.changed` 即 tick；连接/重连/鉴权在实现内管理）。
    ///
    /// 调用方：`for await _ in backend.whiteboardChanges(crewId:) { await refresh() }`。
    /// **不**自带首刷 —— 订阅建立后调用方主动 refresh 一次兜住订阅前的状态。
    /// 流在 `Task` 取消（`.task(id:)` 生命周期结束）时由 `onTermination` 收尾
    /// （Local：取消 Combine 订阅；Edge：关 hub 连接）。
    func whiteboardChanges(crewId: String) -> AsyncStream<Void>

    // MARK: - Models

    /// 拉 model 目录给 CreateSessionSheet 用。
    /// - 登录态:走 edge `GET /v1/models`,返回 native + OpenRouter 全集。
    /// - BYOK:edge 不可达,返回空 array(UI 改走手动 model id 输入框)。
    func listModels() async throws -> [ModelCatalogEntry]
}

extension PendingCrewBackend {
    /// 无本地附件的便捷重载 —— 既有调用点不用逐个补 `localAttachments: []`。
    func postCrewMessage(
        crewId: String, text: String, mentions: [CrewMention],
        attachmentIds: [String], replyToId: String?) async throws {
        try await postCrewMessage(
            crewId: crewId, text: text, mentions: mentions,
            attachmentIds: attachmentIds, replyToId: replyToId, localAttachments: [])
    }
}

// MARK: - Edge backend

/// 登录态后端 —— wrap PendingCrewAPI。
///
/// 每次方法调用前重新 resolve `(baseURL, token)`,避免 url / 凭据变化
/// 后还用老 instance。
final class EdgeBackend: PendingCrewBackend {
    private unowned let appModel: AppModel

    init(appModel: AppModel) {
        self.appModel = appModel
    }

    func listMySubjects() async throws -> [UserSubject] {
        try await client().listMySubjects()
    }

    func listMachines() async throws -> [Machine] {
        try await client().listMachines()
    }

    func registerSelfMachine() async throws -> String {
        try await client().registerSelfMachine()
    }

    func listCrews() async throws -> [CrewSummary] {
        try await client().listCrews()
    }

    func getCrew(_ crewId: String) async throws -> CrewDetail {
        try await client().getCrew(crewId)
    }

    func createCrew(_ request: CreateCrewRequest) async throws -> CreateCrewResponse {
        try await client().createCrew(request)
    }

    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws {
        try await client().attachParent(
            crewId: crewId,
            parentCrewId: parentCrewId,
            childKeepsBps: childKeepsBps
        )
    }

    func detachParent(crewId: String, parentCrewId: String) async throws {
        // edge 暂无解绑端点 —— DAG 解绑目前是本地特性。
        throw PendingCrewBackendError.notSupportedInByok(
            "云端 crew 暂不支持解绑父 crew"
        )
    }

    func listModels() async throws -> [ModelCatalogEntry] {
        try await client().listModels()
    }

    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry] {
        try await client().listCrewWhiteboard(crewId: crewId)
    }

    func listCrewMembers(crewId: String) async throws -> CrewRoster {
        try await client().listCrewMembers(crewId: crewId)
    }

    func postCrewMessage(
        crewId: String, text: String, mentions: [CrewMention],
        attachmentIds: [String], replyToId: String?,
        localAttachments: [LocalWhiteboardAttachment]) async throws {
        // 本地落盘附件是 LocalBackend 专属通道 —— edge 有自己的 upload → id 流。
        guard localAttachments.isEmpty else {
            throw PendingCrewBackendError.notSupportedInByok("云端群聊不支持本地附件通道")
        }
        try await client().postCrewMessage(
            crewId: crewId, text: text, mentions: mentions,
            attachmentIds: attachmentIds, replyToId: replyToId)
    }

    /// 登录态白板变更流 = crew 级 hub WS（`CrewRealtimeClient`）的白板相关 `.changed`。
    ///
    /// hub 帧是 content-agnostic 的「该 crew 有动静」信号 —— 这里把它过滤到群聊
    /// 数据相关的表（`crew_announcements` = 白板时间线，`crew_sessions` /
    /// `messages` 也会改 roster / 内容），每条相关帧 → 一个 `Void` tick。连接、
    /// 20s ping、指数退避重连都在 client 里；流随订阅 `Task` 取消而 `close()`。
    ///
    /// **跨平台**：`CrewRealtimeClient` 现在 macOS + iOS 都编（只依赖 Foundation），
    /// 所以 iPad 登录态群聊也接 hub 推送 —— 不再返回立即 finish 的空流降级。
    /// 调用方建立订阅时仍主动 refresh 一次兜住订阅前的状态。
    func whiteboardChanges(crewId: String) -> AsyncStream<Void> {
        guard let auth = appModel.imageAuth else {
            // 理论上登录态才走 EdgeBackend；防御性：无 device-grant → 空流。
            return AsyncStream { $0.finish() }
        }
        let client = CrewRealtimeClient(
            baseURL: auth.baseURL, crewId: crewId, token: auth.token)
        return AsyncStream { continuation in
            let pump = Task {
                await client.connect()
                for await event in await client.events {
                    if Task.isCancelled { break }
                    if case let .changed(table, _, _) = event,
                       Self.whiteboardRelevantTables.contains(table) {
                        continuation.yield(())
                    }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in
                pump.cancel()
                Task { await client.close() }
            }
        }
    }

    /// hub `.changed` 帧里与群聊白板/roster 相关的表 —— 命中即 tick。其余（voice_*
    /// 已在 codec 滤掉）忽略。`crew_announcements` 是白板时间线主表；`crew_sessions`
    /// 改成员/状态、`messages` 是会话消息，都会影响中栏/右栏渲染。
    private static let whiteboardRelevantTables: Set<String> = [
        "crew_announcements", "crew_sessions", "messages",
    ]

    private func client() throws -> PendingCrewAPI {
        guard let credential = appModel.credential else {
            throw PendingCrewBackendError.notAuthenticated
        }
        guard let url = URL(string: appModel.apiBaseURL) else {
            throw PendingCrewBackendError.invalidConfig("API base URL 无效")
        }
        return PendingCrewAPI(baseURL: url, bearerToken: credential.token)
    }
}

// MARK: - Local backend (BYOK)

/// BYOK 本地后端 —— wrap LocalCrewStore。
///
/// `listMySubjects` 返回单元素假 subject `local-byok` —— 复用 UI 层 picker
/// 形状,不再分叉(Spec §5.3 双轨边界明确"BYOK 本地态依然有 subject 概念,
/// 只是只有一个,代表 '本机/这台机器'")。
final class LocalBackend: PendingCrewBackend {
    /// 写死的"本机 BYOK"subject id —— LocalCrewStore 里 crew 都标这个
    /// responsibleSubjectId。后续 Phase 引真本机 captain 时这个 id 也是
    /// 责任分账的唯一 holder。
    static let localSubjectId = "local-byok"

    private let store: LocalCrewStore
    private let whiteboard: LocalWhiteboardStore

    /// store / whiteboard 必传 —— 不给默认值,避免 @MainActor `.shared` 在 init
    /// 的 nonisolated 默认参数 evaluation 里冲突(Swift 6 mode 报错)。调用方在
    /// `@MainActor` 环境里显式传 `.shared`。
    init(store: LocalCrewStore, whiteboard: LocalWhiteboardStore) {
        self.store = store
        self.whiteboard = whiteboard
    }

    func listMySubjects() async throws -> [UserSubject] {
        [UserSubject(
            id: Self.localSubjectId,
            kind: "byok",
            displayName: "人",
            role: nil,
            userId: nil
        )]
    }

    func listMachines() async throws -> [Machine] {
        // 本地态恒只有一台「本机」 —— 合成一个 computer Machine（id = 安装级
        // device id，与 registerSelfMachine 返回值一致）。CreateCrewSheet 据
        // `count > 1` 判定是否显示 machine 选择，本地态恒不显示。
        [Machine(
            id: DeviceIdentity.current,
            kind: Machine.Kind.computer.rawValue,
            deviceId: DeviceIdentity.current,
            displayName: DeviceIdentity.displayName,
            flyMachineId: nil,
            status: "online",
            lastSeenAt: nil
        )]
    }

    func registerSelfMachine() async throws -> String {
        // 本地无服务端可注册 —— no-op，返回本机合成 id（= device id）。
        DeviceIdentity.current
    }

    func listCrews() async throws -> [CrewSummary] {
        store.listCrews()
    }

    func getCrew(_ crewId: String) async throws -> CrewDetail {
        guard let detail = store.getCrew(crewId) else {
            throw PendingCrewBackendError.notFound("本地 crew \(crewId) 不存在")
        }
        return detail
    }

    func createCrew(_ request: CreateCrewRequest) async throws -> CreateCrewResponse {
        // BYOK 本地态恒「本机」 —— 选了其它 machine（machineId 非 nil 且不是
        // 本机 device id）说明 UI 状态串了，拒绝。本地路径正常恒 nil。
        if let machineId = request.machineId, machineId != DeviceIdentity.current {
            throw PendingCrewBackendError.notSupportedInByok(
                "未登录时只能用这台机器;要用其它设备请先登录 PendingBot 账号"
            )
        }
        return store.createCrew(request)
    }

    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws {
        // 本地 DAG 父边 —— 「家」在本地,组织模型不走 edge。`childKeepsBps`
        // 本地暂忽略(责任分账是后续计费的事,本次只落 DAG 边)。禁环由
        // store 兜(parentCrewId 是 crewId 后代时抛 wouldCreateCycle)。
        try store.attachParent(crewId: crewId, parentCrewId: parentCrewId)
    }

    func detachParent(crewId: String, parentCrewId: String) async throws {
        store.detachParent(crewId: crewId, parentCrewId: parentCrewId)
    }

    func listModels() async throws -> [ModelCatalogEntry] {
        // BYOK 没法走 edge 拿动态目录。CreateSessionSheet 看到空 array 时
        // 会自动 fallback 到手动输入框。
        []
    }

    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry] {
        // 本地白板 message → edge 同形 entry。中栏两态渲染不分叉。
        whiteboard.list(crewId: crewId).map { m in
            CrewWhiteboardEntry(
                id: m.id,
                senderKind: m.senderKind,
                // session 作者 id 透传(原写死 nil)—— 中栏据此解析 session 名 + 点气泡
                // 跳右栏对应终端 + 回复定位到该 session。user 消息本就 nil(不受影响)。
                senderSessionId: m.senderSessionId,
                senderUserId: m.senderUserId,
                senderBotId: nil,
                messageKind: "instruction",
                summary: m.text,
                createdAt: m.createdAt,
                payload: CrewWhiteboardEntry.Payload(
                    text: m.text, kind: nil, question: nil, status: nil, permissionRequestId: nil,
                    action: nil, taskBrief: nil, runnerKind: nil),
                // 本地落盘附件 → edge 同形 CrewAttachment。`url` 用 file:// 绝对
                // URL —— 渲染端（CrewRemoteImage / FileAttachmentChip）据前缀分流
                // 本地读取，与登录态 `/v1/uploads/<id>` 相对路径天然区分。
                attachments: m.attachments.map { atts in
                    atts.map { a in
                        CrewAttachment(
                            id: a.id, kind: nil, mime: a.mime, size: a.size,
                            width: nil, height: nil,
                            url: URL(fileURLWithPath: a.path).absoluteString,
                            filename: a.filename)
                    }
                },
                relay: nil,
                // 发送者名收口在 CrewSenderNaming.localWireDisplayName:relay 远端名要显示,
                // 但本机人类自己发的消息(senderKind=="user")不折本地 senderName("人"),
                // 否则中栏 resolver 的 relay 守卫会把自己误判成 relay → 左对齐(#3)。
                senderDisplayName: CrewSenderNaming.localWireDisplayName(
                    senderKind: m.senderKind, relayName: m.senderDisplayName, localName: m.senderName),
                // 本地白板消息没有 edge 成员 id —— 恒 nil。
                senderMemberId: nil,
                // #377 — 本地白板消息的回复引用(Phase 6 已加 LocalWhiteboardMessage.inReplyTo)。
                inReplyTo: m.inReplyTo,
                // Task 10 — 本地白板消息的定向 @（Phase 7 落的 LocalWhiteboardMention）
                // 映射回 edge 同形 CrewMention，中栏 mention 高亮/唤醒判定不分本地/relay。
                mentions: m.mentions?.map { CrewMention(kind: $0.kind, targetId: $0.targetId) }
            )
        }
    }

    func listCrewMembers(crewId: String) async throws -> CrewRoster {
        // 从本地 crew 派生:captain(若有)+ 本机人类 + 持久 session 成员。
        guard let detail = store.getCrew(crewId) else {
            return CrewRoster(captainBotId: nil, members: [])
        }
        var members: [CrewMember] = []
        if let cap = detail.captain {
            members.append(CrewMember(
                id: cap.botId, memberKind: "captain", userId: nil, botId: cap.botId,
                codeSessionId: nil, displayName: cap.displayName, role: "captain",
                status: "active", representsCrewId: crewId, sessionStatus: nil))
        }
        members.append(CrewMember(
            id: LocalWhiteboardStore.localUserId, memberKind: "human",
            userId: LocalWhiteboardStore.localUserId, botId: nil, codeSessionId: nil,
            displayName: "人", role: nil, status: "active",
            representsCrewId: nil, sessionStatus: nil))
        // 持久 session 成员（chunk 4 补口）：session 拉起时经
        // `LocalCrewStore.recordSessionMember` 登记,退出/重启后仍是成员。
        // live 运行状态由视图层 merge 在跑的 run 补上,这里不掺。
        for s in store.sessionMembers(crewId: crewId) {
            members.append(CrewMember(
                id: s.sessionId, memberKind: "code_session", userId: nil, botId: nil,
                codeSessionId: s.sessionId, displayName: s.displayName, role: nil,
                status: "active", representsCrewId: nil, sessionStatus: nil,
                createdAt: s.createdAt))
        }
        return CrewRoster(captainBotId: detail.captain?.botId, members: members)
    }

    func postCrewMessage(
        crewId: String, text: String, mentions: [CrewMention],
        attachmentIds: [String], replyToId: String?,
        localAttachments: [LocalWhiteboardAttachment]) async throws {
        guard attachmentIds.isEmpty else {
            throw PendingCrewBackendError.notSupportedInByok("未登录时暂不支持群聊附件")
        }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        // 附件-only（无正文只发图）也放行（Todo #3）。
        guard !trimmed.isEmpty || !localAttachments.isEmpty else { return }
        // 本机人类显示名「人」—— agent 看的白板渲染成「- 人: …」而非裸「人类」。
        // `replyToId` 记成本地白板的 `in_reply_to` 引用。
        //
        // **mentions 落盘**（Todo #62 ③）：在此之前这个形参收了就扔 —— @ 只喂给了
        // 旁边那条直投唤醒链（`CrewChatView.send()` 单独编排），消息本身一个字不带。
        // 于是人类 @ 谁都是全组可见（`CrewWhiteboardVisibility` 看的是消息上的
        // mentions），composer 的「全体」（`broadcast`）在这条路上从来没落过盘。
        // 现在真存下来：人类的 @ 第一次有了和 `post_to_crew` 一样的语义 —— 手打
        // `@小王` 排他（#543），「回复」的自动 @ 是 `[broadcast, 被回复者]`
        // （全组可见 + 只叫醒他，组装在 `CrewComposerMentionParser.mentionsToSend`）。
        // 唤醒面一行没改：谁被叫醒仍由 `CrewLocalMentionInjectLogic` 决定。
        whiteboard.appendUserMessage(
            crewId: crewId, text: trimmed, senderName: "人", inReplyTo: replyToId,
            attachments: localAttachments,
            mentions: mentions.map(LocalWhiteboardMention.init))
    }

    /// 本地白板变更流（去 3s 轮询）。两个上游合流成 `Void` tick：
    /// 1. `whiteboard.changes` 按 crewId 过滤 —— **本进程** append（人类发送）即推。
    /// 2. `whiteboard.directoryChanged` —— **跨进程**目录监听（helper 子进程经
    ///    `post_to_crew` 写 agent 进展也覆盖）。目录事件不带 crewId，所以这条要
    ///    自己做相关性判定：比 `<crewId>.json` 的 mtime+size，真变了才 yield。
    ///
    /// 相关性这道闸是 #443（点进群聊转彩虹圈）的主因修复 —— 目录里 600+ 个
    /// session 状态文件（`.cursor`/`.turn`/approvals/todos/quota…）在有 session
    /// 活着时持续在写，此前每一次写都会让中栏重拉整板、`LazyVStack` 全量重测。
    /// 判定只做在**这条流**上：`directoryChanged` 本身语义不变，todo / approvals /
    /// 改名通道 / listen / mention 唤醒各自关心别的文件，不能一刀切只放行白板 json。
    ///
    /// 首次订阅时 `startWatching()` 起目录监听（幂等）。订阅 `Task` 取消时
    /// `onTermination` 退订两条 Combine（目录监听是 app 级共享，留着不停）。
    func whiteboardChanges(crewId: String) -> AsyncStream<Void> {
        whiteboard.startWatching()
        let store = whiteboard
        let inProcess = whiteboard.changes
        let crossProcess = whiteboard.directoryChanged
        return AsyncStream { continuation in
            // 种子取建流那一刻的指纹 —— 调用方建流前已经全量拉过一次，从当下起步。
            let gate = FileChangeGateBox(seed: store.fingerprint(crewId: crewId))
            let c1 = inProcess
                .filter { $0 == crewId }
                .sink { _ in
                    // 本进程自己的 append：无条件推，同时把指纹记下 —— 同一次写盘
                    // 随后还会触发一个目录事件，不吞掉就成了双份刷新。
                    gate.sync(store.fingerprint(crewId: crewId))
                    continuation.yield(())
                }
            let c2 = crossProcess
                .sink { _ in
                    guard gate.shouldYield(store.fingerprint(crewId: crewId)) else { return }
                    continuation.yield(())
                }
            continuation.onTermination = { _ in c1.cancel(); c2.cancel() }
        }
    }
}

// MARK: - Errors

enum PendingCrewBackendError: LocalizedError {
    case notAuthenticated
    case invalidConfig(String)
    case notFound(String)
    /// BYOK 路径试图调用 logged-only 端点。
    case notSupportedInByok(String)

    var errorDescription: String? {
        switch self {
        case .notAuthenticated:
            return "未登录"
        case .invalidConfig(let msg), .notFound(let msg), .notSupportedInByok(let msg):
            return msg
        }
    }
}
