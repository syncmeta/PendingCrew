import Foundation
import Combine

/// PendingCrew 数据后端的统一抽象。
///
/// **#63 第二期起只剩一个实现**：`LocalBackend`，macOS 上的常驻 home ——
/// crew CRUD / 白板 / 花名册全走它。原来还有一个 `EdgeBackend`（登录态的云端
/// API 面），随跨端遥控整层删除。iOS 上 `AppModel.backend` 恒 nil。
///
/// protocol 留着而不是把 `LocalBackend` 直接暴露给 UI：人类原话是「以后前后端
/// 解耦时重新做」—— 那一刀落下来时，第二个实现会重新出现在这个位置。
///
/// **设计原则**:返回值 shape 用 `CrewSummary` / `CrewDetail` / `UserSubject`
/// 那几个 model,UI 层 + `CrewStore` 因此不分叉。
@MainActor
protocol PendingCrewBackend: AnyObject {
    // MARK: - Subject
    func listMySubjects() async throws -> [UserSubject]

    // MARK: - Crew
    func listCrews() async throws -> [CrewSummary]
    func getCrew(_ crewId: String) async throws -> CrewDetail
    func createCrew(_ request: CreateCrewRequest) async throws -> CreateCrewResponse

    /// 把 crew 挂到一个父 crew 之下。
    /// - LocalBackend:写本地 DAG 父边(`LocalCrewStore.attachParent`,禁环)。
    ///   `childKeepsBps` 本地暂时忽略(责任分账是后续计费的事)。
    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws

    /// 解绑一条父边(删本地 DAG 父边)。
    func detachParent(crewId: String, parentCrewId: String) async throws

    // MARK: - Crew comms（白板/群聊，spec local-first §8.4 单后端）

    /// crew 白板时间线 —— 本地 `LocalWhiteboardStore` 映射成 `CrewWhiteboardEntry`。
    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry]

    /// crew 花名册 —— 从本地 crew 派生(captain + 本机人类 + 持久 session 成员)。
    func listCrewMembers(crewId: String) async throws -> CrewRoster

    /// 发一条群聊消息(默认广播)。`replyToId` 非 nil = 回复某条消息
    /// (LocalBackend 记本地白板 `in_reply_to` 引用)。`localAttachments` = 已由
    /// `CrewLocalAttachmentPersist` 落盘的附件条目,挂到白板消息上(Todo #3)。
    func postCrewMessage(
        crewId: String, text: String, mentions: [CrewMention],
        replyToId: String?,
        localAttachments: [LocalWhiteboardAttachment]) async throws

    /// crew 白板/花名册的**事件驱动变更流**（Phase 5：去 3s 轮询）。
    ///
    /// 每个 tick = 「这个 crew 的白板/成员可能有动静，去 refresh 一次」的信号
    /// （**不**携带 diff —— 订阅方收到即重拉 `listCrewWhiteboard` / `listCrewMembers`）。
    /// `LocalBackend`：基于 `LocalWhiteboardStore.changes`（本地 append 即 tick）。
    ///
    /// 调用方：`for await _ in backend.whiteboardChanges(crewId:) { await refresh() }`。
    /// **不**自带首刷 —— 订阅建立后调用方主动 refresh 一次兜住订阅前的状态。
    /// 流在 `Task` 取消（`.task(id:)` 生命周期结束）时由 `onTermination` 收尾
    /// （取消 Combine 订阅）。
    func whiteboardChanges(crewId: String) -> AsyncStream<Void>

    // MARK: - Models

    /// 拉 model 目录给 CreateSessionSheet 用。本地无动态目录 → 恒空 array
    /// (UI 据此 fallback 到手动 model id 输入框)。
    func listModels() async throws -> [ModelCatalogEntry]
}

extension PendingCrewBackend {
    /// 无本地附件的便捷重载 —— 既有调用点不用逐个补 `localAttachments: []`。
    func postCrewMessage(
        crewId: String, text: String, mentions: [CrewMention],
        replyToId: String?) async throws {
        try await postCrewMessage(
            crewId: crewId, text: text, mentions: mentions,
            replyToId: replyToId, localAttachments: [])
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
        // 本地态恒「本机」 —— 选了其它 machine（machineId 非 nil 且不是本机
        // device id）说明 UI 状态串了，拒绝。本地路径正常恒 nil。
        if let machineId = request.machineId, machineId != DeviceIdentity.current {
            throw PendingCrewBackendError.notSupportedInByok("只能用这台机器")
        }
        return store.createCrew(request)
    }

    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws {
        // 本地 DAG 父边 —— 「家」在本地。`childKeepsBps`
        // 本地暂忽略(责任分账是后续计费的事,本次只落 DAG 边)。禁环由
        // store 兜(parentCrewId 是 crewId 后代时抛 wouldCreateCycle)。
        try store.attachParent(crewId: crewId, parentCrewId: parentCrewId)
    }

    func detachParent(crewId: String, parentCrewId: String) async throws {
        store.detachParent(crewId: crewId, parentCrewId: parentCrewId)
    }

    func listModels() async throws -> [ModelCatalogEntry] {
        // 本地没有动态目录。CreateSessionSheet 看到空 array 时会自动 fallback
        // 到手动输入框。
        []
    }

    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry] {
        // 本地白板 message → 线上同形 entry（中栏渲染层只认这一个形状）。
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
                payload: CrewWhiteboardEntry.Payload(text: m.text),
                // 本地落盘附件 → 同形 CrewAttachment。`url` 用 file:// 绝对 URL，
                // 渲染端（CrewRemoteImage / FileAttachmentChip）据前缀分流本地读取。
                attachments: m.attachments.map { atts in
                    atts.map { a in
                        CrewAttachment(
                            id: a.id, kind: nil, mime: a.mime, size: a.size,
                            width: nil, height: nil,
                            url: URL(fileURLWithPath: a.path).absoluteString,
                            filename: a.filename)
                    }
                },
                // 发送者名收口在 CrewSenderNaming.localWireDisplayName:relay 远端名要显示,
                // 但本机人类自己发的消息(senderKind=="user")不折本地 senderName("人"),
                // 否则中栏 resolver 的 relay 守卫会把自己误判成 relay → 左对齐(#3)。
                senderDisplayName: CrewSenderNaming.localWireDisplayName(
                    senderKind: m.senderKind, localName: m.senderName),
                // 本地白板消息没有成员表行 id —— 恒 nil。
                senderMemberId: nil,
                // #377 — 本地白板消息的回复引用(Phase 6 已加 LocalWhiteboardMessage.inReplyTo)。
                inReplyTo: m.inReplyTo,
                // Task 10 — 本地白板消息的定向 @（Phase 7 落的 LocalWhiteboardMention）
                // 映射回同形 CrewMention，中栏 mention 高亮 / 唤醒判定读同一个形状。
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
        replyToId: String?,
        localAttachments: [LocalWhiteboardAttachment]) async throws {
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
