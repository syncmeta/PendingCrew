// Crew wire-model types extracted from PendingCrewAPI.swift so they can be
// compiled into Foundation-only contexts (test bundles, etc.) without dragging
// in the full `PendingCrewAPI` class and its `URLSession` / Auth dependencies.
//
// PendingCrewAPI.swift imports this file implicitly (same module).
import Foundation

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

/// One whiteboard entry rendered in the middle-pane group chat. `summary` is the
/// display line. 形状对齐 edge 的 `crew_announcements` 读模型 —— #63 第二期删掉
/// edge 那一层之后，唯一的构造点是 `LocalBackend.listCrewWhiteboard`（本地白板
/// 消息 → 同形 entry），保留这个形状是为了中栏渲染层不必跟着改一遍。
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
    /// 发送者显示名。`LocalBackend` 用本地白板消息的 `senderName` 合成
    /// （见 `CrewSenderNaming.localWireDisplayName`）；`CrewSenderResolver` 优先
    /// 用它 —— 本地 session 不进 roster，没有它就只能落兜底「会话」。
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
    /// 'human'/'session'/'captain'/'broadcast'/'bot'。
    let mentions: [CrewMention]?

    struct Payload: Decodable, Equatable {
        let text: String?
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
        case senderDisplayName = "sender_display_name"
        case senderMemberId = "sender_member_id"
        case inReplyTo = "in_reply_to"
        case mentions
    }

    /// Best display text: explicit payload text → summary → empty.
    var displayText: String { payload?.text ?? summary ?? "" }
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
