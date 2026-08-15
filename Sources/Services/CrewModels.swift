// Crew wire-model types extracted from PendingCrewAPI.swift so they can be
// compiled into Foundation-only contexts (test bundles, etc.) without dragging
// in the full `PendingCrewAPI` class and its `URLSession` / Auth dependencies.
//
// PendingCrewAPI.swift imports this file implicitly (same module).
import Foundation

// MARK: - Crew session models (T4.5)

/// Result of `claimSession`. The claimed crew_session row is free-form on the
/// wire and not modeled yet (the runner already holds the sessionId it
/// claimed); only the lease id is decoded. `leaseId` nil = nothing claimable.
struct SessionClaim: Decodable {
    let leaseId: String?
}

/// One server crew session row (from `listCrewSessions`). Mirrors the
/// `crew_sessions` columns the list endpoint returns.
struct CrewSessionSummary: Decodable, Identifiable, Equatable {
    let id: String
    let runnerKind: String
    let status: String
    let taskBrief: String
    let progressSummary: String?
    let createdAt: String
    let startedAt: String?
    let finishedAt: String?

    enum CodingKeys: String, CodingKey {
        case id
        case runnerKind = "runner_kind"
        case status
        case taskBrief = "task_brief"
        case progressSummary = "progress_summary"
        case createdAt = "created_at"
        case startedAt = "started_at"
        case finishedAt = "finished_at"
    }
}

/// One transcript event from a session's durable log (`getSessionEvents`).
struct CrewSessionEvent: Decodable, Identifiable, Equatable {
    let id: String
    let eventType: String
    let visibility: String
    let summary: String?
    let createdAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case eventType = "event_type"
        case visibility
        case summary
        case createdAt = "created_at"
    }
}

/// One pending `ask_human` interaction (T4.5). The agent is blocked waiting on
/// `question`; the operator answers with free text.
struct CrewInteraction: Decodable, Identifiable, Equatable {
    let id: String
    let question: String
    let requestedAt: String?
}

// MARK: - Crew whiteboard / group chat models (spec §9)

/// A mention target for a crew message. `@session <id>` routes to that
/// session's mailbox; `@captain` to the captain's session; empty list or
/// `broadcast` fans out to all sessions.
struct CrewMention: Codable, Equatable {
    let kind: String          // 'session' | 'captain' | 'broadcast' | 'human'
    let targetId: String?

    enum CodingKeys: String, CodingKey {
        case kind
        case targetId = "target_id"
    }

    static func session(_ id: String) -> CrewMention { .init(kind: "session", targetId: id) }
    static let captain = CrewMention(kind: "captain", targetId: nil)
    static let broadcast = CrewMention(kind: "broadcast", targetId: nil)
}

/// One image / file attachment on a whiteboard message. Mirror of PendingBot
/// iOS `Attachment` — the server hydrates these from `messages.attachments`
/// `{ ids: [...] }` into full objects, so the wire already carries mime / size /
/// url / filename (PendingCrew has no direct Supabase access to resolve ids).
/// `url` is the auth-gated `/v1/uploads/<id>` path, fetched with the
/// device-grant bearer token.
struct CrewAttachment: Decodable, Identifiable, Equatable, Hashable {
    let id: String
    let kind: String?
    let mime: String
    let size: Int?
    let width: Int?
    let height: Int?
    let url: String
    let filename: String?

    /// True when this attachment renders as an inline image (vs a file chip).
    var isImage: Bool { mime.lowercased().hasPrefix("image/") }
}

/// One whiteboard entry (a crew_announcements row) rendered in the middle-pane
/// group chat. `summary` is the display line; `payload.kind == "interaction"`
/// marks an ask_human card (T4.5) so the timeline can render it specially.
struct CrewWhiteboardEntry: Decodable, Identifiable, Equatable {
    let id: String
    let senderKind: String          // 'session' | 'user' | 'bot' | ...
    let senderSessionId: String?
    let senderUserId: String?       // 新增：user 作者
    let senderBotId: String?        // 新增：bot 作者
    let messageKind: String         // 'instruction' | 'announcement' | 'permission_request' | ...
    let summary: String?
    let createdAt: String
    let payload: Payload?
    /// Image / file attachments on this message. Optional → entries without
    /// attachments (and older server builds) decode fine. Server hydrates
    /// these from `messages.attachments` `{ ids: [...] }` into full objects;
    /// `url` is the auth-gated `/v1/uploads/<id>` path `CrewRemoteImage`
    /// fetches with the device-grant bearer token.
    let attachments: [CrewAttachment]?
    /// Mac relay 上行的来源标注（接合 v2 block 3）。非 nil 且 `origin ==
    /// "mac_relay"` = 这条是 Mac 自己推上去的 —— 拉取侧据此过滤防回环。
    /// edge 老消息 / 非 relay 消息为 nil。
    let relay: Relay?
    /// 发送者显示名。**edge 现在会下发这个字段**（Phase 3：白板读模型经 roster
    /// 解析人类 user_id / session log_payload.session_id → member.display_name）；
    /// 解析不出 / relay 老消息为 nil。`LocalBackend` 把 relay 搬进本地白板的消息
    /// 映射成 entry 时也回填 edge 侧名字。`CrewSenderResolver` 优先用它。
    let senderDisplayName: String?
    /// 发送者对应的 crew 成员 id（`temporary_group_members.id`）。edge Phase 3 随
    /// `sender_display_name` 一起下发（解析不出为 nil）。客户端可据此关联 roster
    /// 行（头像/状态徽标），无强依赖。
    let senderMemberId: String?
    /// 被回复消息的白板 id（#377）。edge `in_reply_to` —— 人类消息从 attachments
    /// jsonb、session 帖从 log_payload.in_reply_to 统一透出。非 nil = 这条是对
    /// `inReplyTo` 那条的回复;客户端在已加载 entries 里按 id 本地查被引用消息的
    /// 发送者 + 内容摘要,渲染成引用条(IM 式「回复 小绿:…」)。解析不出 / 老消息为 nil。
    let inReplyTo: String?
    /// 结构化 @ mentions（Task 3：发送时有 @ 才落；无 → nil）。`kind` ∈
    /// 'human'/'session'/'captain'/'broadcast'/'bot' —— relay 落地侧（Task 10）
    /// 把 session/captain/human 三种映射进本地 `LocalWhiteboardMention`，其余
    /// （bot/broadcast，本地唤醒决策不消费）滤掉。
    let mentions: [CrewMention]?

    struct Relay: Decodable, Equatable {
        let origin: String
        let senderLabel: String?
        let localSessionId: String?
    }

    struct Payload: Decodable, Equatable {
        let text: String?
        let kind: String?                // 'interaction' for ask_human cards
        let question: String?
        let status: String?
        let permissionRequestId: String?
        // #242 遥控 v1 — messageKind == "task_request" 的结构化指令体
        // ({ action:'run_session', runner_kind?, task_brief })。全 optional →
        // 老消息 / 非指令消息照常解码。
        let action: String?
        let taskBrief: String?
        let runnerKind: String?

        enum CodingKeys: String, CodingKey {
            case text, kind, question, status, action
            case permissionRequestId = "permission_request_id"
            case taskBrief = "task_brief"
            case runnerKind = "runner_kind"
        }
    }

    enum CodingKeys: String, CodingKey {
        case id
        case senderKind = "sender_kind"
        case senderSessionId = "sender_session_id"
        case senderUserId = "sender_user_id"
        case senderBotId = "sender_bot_id"
        case messageKind = "message_kind"
        case summary
        case createdAt = "created_at"
        case payload
        case attachments
        case relay
        case senderDisplayName = "sender_display_name"
        case senderMemberId = "sender_member_id"
        case inReplyTo = "in_reply_to"
        case mentions
    }

    /// Best display text: explicit payload text → summary → empty.
    var displayText: String { payload?.text ?? summary ?? "" }
    /// True if this row is an ask_human interaction card.
    var isInteraction: Bool { payload?.kind == "interaction" }
}

/// `listCrewWhiteboardPage` 的响应：白板条目 + 续传游标（接合 v2 block 3）。
/// `lastCursor` = 本批最大 created_at；空批为 nil（游标原地不动）。
struct CrewWhiteboardPage: Decodable {
    let whiteboard: [CrewWhiteboardEntry]
    let lastCursor: String?
}

/// One invitable bot (`listMyBots`, `GET /v1/me/bots`)：自己的 bot + 加过
/// 联系人的非 private bot，供「邀 bot 进 relay crew」picker 用。响应是
/// snake_case row，显式 CodingKeys。
struct InvitableBot: Decodable, Identifiable, Equatable, Hashable {
    let id: String
    let slug: String?
    let displayName: String
    let modelId: String?
    let visibility: String
    let isMine: Bool

    enum CodingKeys: String, CodingKey {
        case id
        case slug
        case visibility
        case displayName = "display_name"
        case modelId = "model_id"
        case isMine = "is_mine"
    }
}

/// One invitable friend (`listContacts`, `GET /v1/contacts`)：当前用户的好友
/// 列表，供「邀人进 relay crew」picker 用。edge 返回本来就是 camelCase，
/// 不需要显式 CodingKeys（与 crews 系端点一致，PendingCrewAPI.perform() 用
/// 裸 JSONDecoder）。`addedAt` 是 epoch 秒，暂无展示需求，先不解。
struct CrewContact: Decodable, Identifiable, Equatable, Hashable {
    let userId: String
    let alias: String?
    let displayName: String?
    let avatarPath: String?
    let avatarSeed: String?

    var id: String { userId }
    /// 备注优先，其次昵称，都没有兜底「好友」——与 PendingBot 侧「备注优先」
    /// 的展示口径保持一致。
    var rowName: String { alias ?? displayName ?? "好友" }
}

/// The crew roster (`listCrewMembers`). `captainBotId` lets the UI tag which
/// bot member is the captain.
struct CrewRoster: Decodable {
    let captainBotId: String?
    let members: [CrewMember]

    enum CodingKeys: String, CodingKey {
        case captainBotId = "captainBotId"
        case members
    }
}

/// One crew member (a row of `temporary_group_members`). `memberKind` ∈
/// human / bot / captain / code_session.
struct CrewMember: Decodable, Identifiable, Equatable {
    let id: String
    let memberKind: String
    let userId: String?
    let botId: String?
    let codeSessionId: String?
    let displayName: String?
    let role: String?
    let status: String
    let representsCrewId: String?
    /// For code_session members: the live crew_session status (running/queued/…).
    /// nil for non-session members. Terminal sessions are already filtered out
    /// server-side, so a present value here is non-terminal.
    let sessionStatus: String?
    /// 成员创建时刻（ISO8601 字符串）。成员列表按它倒序（新的在上，#15）；
    /// 拿不到就 nil（排在有时刻的之后）。
    let createdAt: String?

    init(
        id: String, memberKind: String, userId: String?, botId: String?,
        codeSessionId: String?, displayName: String?, role: String?, status: String,
        representsCrewId: String?, sessionStatus: String?, createdAt: String? = nil
    ) {
        self.id = id
        self.memberKind = memberKind
        self.userId = userId
        self.botId = botId
        self.codeSessionId = codeSessionId
        self.displayName = displayName
        self.role = role
        self.status = status
        self.representsCrewId = representsCrewId
        self.sessionStatus = sessionStatus
        self.createdAt = createdAt
    }

    enum CodingKeys: String, CodingKey {
        case id
        case memberKind = "member_kind"
        case userId = "user_id"
        case botId = "bot_id"
        case codeSessionId = "code_session_id"
        case displayName = "display_name"
        case role
        case status
        case representsCrewId = "represents_crew_id"
        case sessionStatus
        case createdAt = "created_at"
    }
}

/// The per-turn context bundle (`getSessionInbox`): crew whiteboard + this
/// session's unread mailbox. Only the two fields the runner injects are
/// decoded; `session` / `crew` in the response are ignored.
struct CrewSessionInbox: Decodable {
    let whiteboard: [CrewWhiteboardEntry]
    let mailbox: [CrewMailboxItem]
}

/// One unread mailbox item targeted at this session (a `@session` / broadcast
/// message someone dropped on the crew, fanned out to this session's mailbox).
struct CrewMailboxItem: Decodable, Identifiable, Equatable {
    let id: String
    let senderKind: String
    let senderSessionId: String?
    let messageKind: String
    let summary: String?
    let status: String
    let createdAt: String
    let payload: CrewWhiteboardEntry.Payload?

    var displayText: String { payload?.text ?? summary ?? "" }

    enum CodingKeys: String, CodingKey {
        case id
        case senderKind = "sender_kind"
        case senderSessionId = "sender_session_id"
        case messageKind = "message_kind"
        case summary
        case status
        case createdAt = "created_at"
        case payload
    }
}

// MARK: - Create crew request/response

/// 对应 edge `POST /v1/crews` 的 body。
///
/// 服务端契约（已部署）：`{ responsibleSubjectId, title?, machineId?,
/// workingDirectory?, captain }`。
/// - `title` 缺省/null = 自动（留给 captain 之后总结命名）。
/// - `machineId` 缺省/null = 本机（edge 派生 runtime_location = local_host）。
/// 老的 `runtimeLocation` / `tag` / `peerDeviceId` / `flyMachineId` 已去掉。
struct CreateCrewRequest: Encodable {
    let responsibleSubjectId: String
    let title: String?            // nil/空 = 自动(留给 captain)
    let machineId: String?        // nil = 本机
    let workingDirectory: String? // 表单解析出的目录路径(CrewGround/手动/上次)
    /// 机长以哪个本机 coding agent 跑（`LocalCodingAgentKind.rawValue` ——
    /// "claude_code" / "codex"）。nil = 旧请求 / 未指定 → 下游回落 `.codex`。
    /// edge 暂不消费这个键（zod 会 strip 掉未知字段），但本地 store 会持久化它，
    /// 让机长 session 真的按这个 kind 起。
    let captainAgentKind: String?
    /// 只给本地 store 用的初始标题来源；不进入 edge 请求 body。
    /// CreateCrewSheet 的自动档即使传了可见地名，也要明确标成 placeholder，
    /// 否则仅凭非空 title 无法和人类手填区分。
    let initialTitleSource: LocalCrewTitleSource?
    let captain: Captain

    private enum CodingKeys: String, CodingKey {
        case responsibleSubjectId, title, machineId, workingDirectory, captainAgentKind, captain
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(responsibleSubjectId, forKey: .responsibleSubjectId)
        try c.encodeIfPresent(title, forKey: .title)
        try c.encodeIfPresent(machineId, forKey: .machineId)
        try c.encodeIfPresent(workingDirectory, forKey: .workingDirectory)
        try c.encodeIfPresent(captainAgentKind, forKey: .captainAgentKind)
        try c.encode(captain, forKey: .captain)
    }

    enum Captain: Encodable, Equatable {
        case systemGenerated(templateName: String?)
        case reuseBot(botId: String)

        enum CodingKeys: String, CodingKey {
            case source
            case templateName
            case botId
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            switch self {
            case .systemGenerated(let name):
                try container.encode("system_generated", forKey: .source)
                if let name { try container.encode(name, forKey: .templateName) }
            case .reuseBot(let botId):
                try container.encode("reuse_bot", forKey: .source)
                try container.encode(botId, forKey: .botId)
            }
        }
    }

    static func make(
        responsibleSubjectId: String,
        title: String?,
        machineId: String?,
        workingDirectory: String?,
        captainAgentKind: String?,
        initialTitleSource: LocalCrewTitleSource? = nil,
        captain: Captain
    ) -> CreateCrewRequest {
        CreateCrewRequest(
            responsibleSubjectId: responsibleSubjectId,
            title: title,
            machineId: machineId,
            workingDirectory: workingDirectory,
            captainAgentKind: captainAgentKind,
            initialTitleSource: initialTitleSource,
            captain: captain
        )
    }
}

struct CreateCrewResponse: Decodable {
    let crewId: String
    let captainBotId: String?
}

/// `POST /v1/device-grant/mint` 的响应（家族凭据静默换 grant）。形状对齐
/// device-login consume：grant token + 元数据。字段全 optional 解耦 edge 端
/// 演进，调用方只硬依赖 `deviceGrantToken`。
struct MintGrantResponse: Decodable {
    let deviceGrantToken: String?
    let grantId: String?
    let subjectId: String?
    let grantKind: String?
    let scopes: [String]?
}

// MARK: - Error type

enum PendingCrewAPIError: LocalizedError {
    case invalidResponse
    case http(status: Int, code: String?, message: String?)

    var errorDescription: String? {
        switch self {
        case .invalidResponse:
            return "服务器返回了非 HTTP 响应"
        case let .http(status, code, message):
            let prefix = "HTTP \(status)"
            if let message, !message.isEmpty { return "\(prefix): \(message)" }
            if let code { return "\(prefix) (\(code))" }
            return prefix
        }
    }
}
