import Foundation

/// 精简 PendingBot Edge client。
///
/// 当前覆盖端点：
/// - device-login（创建 challenge + poll）
/// - crew read/write（list / detail / create / attach-parent）
/// - me（subjects 列表）
///
/// 后续 share-change / runner / config 端点等到真正需要时再加，避免重蹈老
/// PendingCrew "提前抽象 → 不匹配实际需求" 的覆辙。
struct PendingCrewAPI {
    let baseURL: URL
    let session: URLSession
    /// 当前登录 grant token（`pdg_*`）。device-login 端点不需要；其它
    /// `/v1/*` 端点都通过 `Authorization: Bearer <token>` 鉴权。
    let bearerToken: String?

    init(baseURL: URL, bearerToken: String? = nil, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
        self.bearerToken = bearerToken
    }

    // MARK: - Device-login

    /// `POST /v1/device-login/challenges`。
    ///
    /// `subjectId` 可选 — 不传 = 让 PendingBot 端选 subject（默认本人
    /// user_account）。传了相当于 "我希望以这个 subject 登录"，PendingBot
    /// 用户 approve 时必须用同一个 subject 否则 409 subject_mismatch
    /// （edge 端反欺骗）。
    func createDeviceLoginChallenge(
        subjectId: String? = nil,
        deviceName: String = currentDeviceName(),
        devicePublicKey: String = "scaffold-placeholder-key-0123456789abcdef"
    ) async throws -> DeviceLoginChallenge {
        struct Body: Encodable {
            let appKind: String
            let deviceName: String
            let devicePublicKey: String
            let scopes: [String]
            let subjectId: String?
        }
        let body = Body(
            appKind: "pendingcrew_macos",
            deviceName: deviceName,
            devicePublicKey: devicePublicKey,
            scopes: ["subject:read", "crew:read", "crew:write", "runner:read", "runner:write"],
            subjectId: subjectId
        )
        return try await post(path: "v1/device-login/challenges", body: body, authenticated: false)
    }

    /// `GET /v1/device-login/challenges/:id?secret=...`。
    ///
    /// PendingCrew 周期 poll；approve 那一刻这个调用会返回
    /// `deviceGrantToken`（之后再 poll 同一个 challenge 会 410 expired）。
    func pollDeviceLoginChallenge(
        challengeId: String,
        secret: String
    ) async throws -> DeviceLoginPollResponse {
        let url = baseURL
            .appendingPathComponent("v1/device-login/challenges")
            .appendingPathComponent(challengeId)
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "secret", value: secret)]
        var request = URLRequest(url: components.url!)
        request.httpMethod = "GET"
        // device-login poll 端点本身就是 unauthenticated（secret 在 query
        // 里），不要塞 bearer。
        return try await perform(request)
    }

    // MARK: - Direct-login family credential

    /// `POST /v1/me/family-credential` — 用 Supabase session 的 access token（Bearer）
    /// 签发一张家族 SSO 凭据（`pfa_*`）。
    ///
    /// **直接登录路径**（PendingBot Mac 自身登录，无需扫码）：
    ///   1. Supabase 完成登录，拿到 `accessToken`（user JWT）。
    ///   2. 调本方法 → 得到 `pfa_` 凭据 + 绑定的 `subjectId`。
    ///   3. 再调 `mintGrant(familyCredential: pfa_, ...)` 换 scoped device grant（`pdg_*`）。
    ///
    /// edge 走 `requireSession()` 门，所以 Authorization 用 Supabase user JWT，
    /// 不是 `pdg_` / `pfa_`。
    func issueFamilyCredential(
        accessToken: String,
        deviceName: String = currentDeviceName()
    ) async throws -> CrewFamilyCredentialResponse {
        struct Body: Encodable { let deviceName: String }
        return try await post(
            path: "v1/me/family-credential",
            body: Body(deviceName: deviceName),
            bearer: accessToken
        )
    }

    // MARK: - Family SSO mint

    /// `POST /v1/device-grant/mint` — 用家族凭据（`pfa_*`，来自共享 keychain
    /// 组 `FamilyCredentialStore`）静默换本 app 自己的 scoped device grant，
    /// 免扫码。鉴权用 `familyCredential` 作 bearer（不是实例的
    /// `bearerToken` —— 此时本 app 还没有 grant）。
    func mintGrant(
        familyCredential: String,
        subjectId: String,
        grantKind: String,
        scopes: [String],
        deviceName: String = currentDeviceName(),
        devicePublicKey: String = "scaffold-placeholder-key-0123456789abcdef"
    ) async throws -> MintGrantResponse {
        struct Body: Encodable {
            let subjectId: String
            let grantKind: String
            let scopes: [String]
            let appKind: String
            let deviceName: String
            let devicePublicKey: String
        }
        return try await post(
            path: "v1/device-grant/mint",
            body: Body(
                subjectId: subjectId,
                grantKind: grantKind,
                scopes: scopes,
                appKind: "pendingcrew_macos",
                deviceName: deviceName,
                devicePublicKey: devicePublicKey
            ),
            bearer: familyCredential
        )
    }

    // MARK: - Subjects

    /// 返回 caller 能代表/登录的 subject(目前固定 1 个,即 grant 绑定的那个)。
    ///
    /// 历史:**PendingCrew 用 device grant token 鉴权**(`pdg_*`),不是 supabase
    /// user JWT。`/v1/me/subjects`(复数)走 `requireSession()`,只接受 user JWT,
    /// 调起来 401 → CreateCrewSheet picker 空 → "没有可代表的 subject"。
    ///
    /// 正确路径:`/v1/me/subject`(单数)走 `requireSubjectAuth(['subject:read'])`,
    /// device grant 兼容,返回 **grant 当前代表的 subject + wallet**。
    /// PendingCrew grant 颁发时 iOS 那边已经选过 subject(spec v2 §4.4),所以单数足够。
    /// 复数 endpoint 留给 iOS PendingBot 的"选 subject 批准 Mac 扫码"picker 用。
    ///
    /// 包成 `[UserSubject]` 单元素 array 是为了不改 UI 层 picker 形状 ——
    /// 后续 phase 加"已登录态切换 subject"时这里改成调真正的多 subject endpoint。
    func listMySubjects() async throws -> [UserSubject] {
        struct Response: Decodable {
            let subject: Row
            // 登录这台机的 PendingCrew 用户(auth.users.id) —— device grant
            // 路径是 grant 的 granted_by_user_id,user-JWT 路径是 caller 自己。
            // 群聊用它区分"自己的消息"。subject row 之外的顶层字段。
            let userId: String?
            struct Row: Decodable {
                let id: String
                let subjectType: String
                let displayName: String
                // `/v1/me/subject` 直接回 supabase row(snake_case),
                // 不像 crews 路由那样手动 map camelCase,所以这里要显式
                // CodingKeys。perform() 用裸 JSONDecoder(crews 端返回的
                // 已是 camelCase),不能加全局 convertFromSnakeCase 否则
                // 反过来破坏 crews 解码。
                enum CodingKeys: String, CodingKey {
                    case id
                    case subjectType = "subject_type"
                    case displayName = "display_name"
                }
            }
            enum CodingKeys: String, CodingKey {
                case subject
                case userId = "user_id"
            }
        }
        let resp: Response = try await get(path: "v1/me/subject")
        return [UserSubject(
            id: resp.subject.id,
            kind: resp.subject.subjectType,
            displayName: resp.subject.displayName,
            role: nil,
            userId: resp.userId
        )]
    }

    /// `GET /v1/me/bots` — 当前用户可邀进 crew 的 bot 列表（自己的 bot 直通 +
    /// 加过联系人的非 private bot；edge 侧对齐 crew_add_member_for_subject
    /// 谓词，见 apps/edge/src/routes/me.ts）。Needs `subject:read`，device
    /// grant 兼容。响应是 snake_case row，显式 CodingKeys 解码（同
    /// listMySubjects 的理由：perform() 用裸 JSONDecoder）。
    func listMyBots() async throws -> [InvitableBot] {
        struct Resp: Decodable { let bots: [InvitableBot] }
        let r: Resp = try await get(path: "v1/me/bots")
        return r.bots
    }

    /// `GET /v1/contacts` — 当前用户的好友列表，供「邀人进 crew」picker 用。
    /// 邀请人类走 `addCrewMember(kind: "human")`，服务端只允许邀已加好友的人
    /// （非好友 → 42501）。响应本来就是 camelCase，直接解 `CrewContact`。
    func listContacts() async throws -> [CrewContact] {
        struct Resp: Decodable { let contacts: [CrewContact] }
        let r: Resp = try await get(path: "v1/contacts")
        return r.contacts
    }

    // MARK: - Machines

    /// `GET /v1/machines` — 本账号可用机器列表（本机 / peer / Fly machine）。
    func listMachines() async throws -> [Machine] {
        struct Resp: Decodable { let machines: [Machine] }
        let r: Resp = try await get(path: "v1/machines")
        return r.machines
    }

    /// `POST /v1/machines/register-self` — 把当前设备 upsert 成本账号的一台
    /// computer 机器，返回 machineId。幂等（服务端 upsert by device_id）。
    func registerSelfMachine() async throws -> String {
        struct Body: Encodable { let deviceId: String; let displayName: String }
        struct Resp: Decodable { let machineId: String }
        let r: Resp = try await post(
            path: "v1/machines/register-self",
            body: Body(deviceId: DeviceIdentity.current, displayName: DeviceIdentity.displayName)
        )
        return r.machineId
    }

    // MARK: - Crews

    /// `GET /v1/crews`。
    func listCrews() async throws -> [CrewSummary] {
        struct Response: Decodable { let crews: [CrewSummary] }
        let resp: Response = try await get(path: "v1/crews")
        return resp.crews
    }

    /// `GET /v1/crews/:crewId`。
    func getCrew(_ crewId: String) async throws -> CrewDetail {
        try await get(path: "v1/crews/\(crewId)")
    }

    /// `POST /v1/crews`。返回新建 crew + 自动创建的 captain bot id。
    func createCrew(_ request: CreateCrewRequest) async throws -> CreateCrewResponse {
        try await post(path: "v1/crews", body: request)
    }

    // MARK: - Models

    /// `GET /v1/models` — 拉模型目录给 CreateSessionSheet 的 picker。
    ///
    /// edge 端返回的是裸 array(不是 `{ models: [...] }` envelope),所以这里
    /// 直接解 `[ModelCatalogEntry]`。
    func listModels() async throws -> [ModelCatalogEntry] {
        try await get(path: "v1/models")
    }

    /// `POST /v1/crews/:crewId/attach-parent`。
    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws {
        struct Body: Encodable {
            let parentCrewId: String
            let childKeepsBps: Int
        }
        struct Response: Decodable { let ok: Bool }
        let _: Response = try await post(
            path: "v1/crews/\(crewId)/attach-parent",
            body: Body(parentCrewId: parentCrewId, childKeepsBps: childKeepsBps)
        )
    }

    // MARK: - Crew sessions (T4.5 runner lifecycle)
    //
    // The HTTP client surface for cross-device remote control. These mirror
    // the shipped server routes (apps/edge/src/routes/{crew,runner-hosts}.ts);
    // the runner-loop slice consumes them next. All but `createSession`
    // require a **device-grant** bearer with `runner:write` scope. `payload`
    // isn't plumbed yet — `summary`/`progressSummary` cover the human-readable
    // bits and the server fills `payload` with its `{}` default; structured
    // payloads wait for a cross-platform JSON value (JSONValue is Mac-only,
    // this file compiles for iOS too).

    /// `POST /v1/crew/:crewId/sessions` — open a crew session (status
    /// 'queued'). Returns the server session id. `runnerKind` ∈
    /// local_claude_code / local_codex / local_opencode / local_kilo /
    /// cloud_sandbox. Needs `crew:write`.
    ///
    /// **Path is singular `/v1/crew`** — `routes/crew.ts` (sessions / links /
    /// mailbox) mounts at `/v1/crew`, while `routes/crews.ts` (list / get /
    /// create crew) mounts at the plural `/v1/crews`. Easy to mix up.
    func createSession(crewId: String, runnerKind: String, taskBrief: String) async throws -> String {
        struct Body: Encodable { let runnerKind: String; let taskBrief: String }
        struct Response: Decodable { let sessionId: String }
        let resp: Response = try await post(
            path: "v1/crew/\(crewId)/sessions",
            body: Body(runnerKind: runnerKind, taskBrief: taskBrief)
        )
        return resp.sessionId
    }

    /// `POST /v1/runner-hosts` — register this machine as a runner host.
    /// Returns the runner host id used for claim/events/finish. The
    /// `responsibleSubjectId` must equal the device grant's subject.
    func registerRunnerHost(
        responsibleSubjectId: String,
        displayName: String?,
        allowedRunnerKinds: [String]
    ) async throws -> String {
        struct Body: Encodable {
            let responsibleSubjectId: String
            let displayName: String?
            let allowedRunnerKinds: [String]
        }
        struct Response: Decodable { let runnerHostId: String }
        let resp: Response = try await post(
            path: "v1/runner-hosts",
            body: Body(
                responsibleSubjectId: responsibleSubjectId,
                displayName: displayName,
                allowedRunnerKinds: allowedRunnerKinds
            )
        )
        return resp.runnerHostId
    }

    /// `POST /v1/runner-hosts/:hostId/sessions/:sessionId/claim` — claim a
    /// specific session for this host (assigns a runner lease, status →
    /// running). `leaseId` is nil when there was nothing claimable.
    func claimSession(
        runnerHostId: String,
        sessionId: String,
        runnerKinds: [String]? = nil
    ) async throws -> SessionClaim {
        struct Body: Encodable { let runnerKinds: [String]? }
        return try await post(
            path: "v1/runner-hosts/\(runnerHostId)/sessions/\(sessionId)/claim",
            body: Body(runnerKinds: runnerKinds)
        )
    }

    /// `POST /v1/runner-hosts/:hostId/sessions/:sessionId/events` — append a
    /// runner event. `eventType` ∈ started / context_injected / status /
    /// tool_call / tool_result / permission_requested / permission_resolved /
    /// artifact_created / posted_to_crew / blocked. `visibility` ∈ controllers
    /// / crew_members / private_system. Returns the new event id.
    func appendSessionEvent(
        runnerHostId: String,
        sessionId: String,
        eventType: String,
        visibility: String = "crew_members",
        summary: String?,
        progressSummary: String? = nil
    ) async throws -> String {
        struct Body: Encodable {
            let eventType: String
            let visibility: String
            let summary: String?
            let progressSummary: String?
        }
        struct Response: Decodable { let eventId: String }
        let resp: Response = try await post(
            path: "v1/runner-hosts/\(runnerHostId)/sessions/\(sessionId)/events",
            body: Body(
                eventType: eventType,
                visibility: visibility,
                summary: summary,
                progressSummary: progressSummary
            )
        )
        return resp.eventId
    }

    /// `GET /v1/crew/:crewId/sessions` — list a crew's server sessions (most
    /// recent first). The viewer side reads this to watch sessions running on
    /// another host. Needs `crew:read`.
    func listCrewSessions(crewId: String) async throws -> [CrewSessionSummary] {
        struct Response: Decodable { let items: [CrewSessionSummary] }
        let resp: Response = try await get(path: "v1/crew/\(crewId)/sessions")
        return resp.items
    }

    /// `GET /v1/crew/sessions/:sessionId/events` — the durable transcript log
    /// for one session (oldest first). What a viewer renders to follow a
    /// session without being its runner. Needs `crew:read`.
    func getSessionEvents(sessionId: String) async throws -> [CrewSessionEvent] {
        struct Response: Decodable { let items: [CrewSessionEvent] }
        let resp: Response = try await get(path: "v1/crew/sessions/\(sessionId)/events")
        return resp.items
    }

    /// `POST /v1/runner-hosts/:hostId/sessions/:sessionId/finish` — close out
    /// the session. `status` ∈ completed / failed / cancelled.
    @discardableResult
    func finishSession(
        runnerHostId: String,
        sessionId: String,
        status: String,
        summary: String? = nil,
        progressSummary: String? = nil
    ) async throws -> Bool {
        struct Body: Encodable {
            let status: String
            let summary: String?
            let progressSummary: String?
        }
        struct Response: Decodable { let ok: Bool }
        let resp: Response = try await post(
            path: "v1/runner-hosts/\(runnerHostId)/sessions/\(sessionId)/finish",
            body: Body(status: status, summary: summary, progressSummary: progressSummary)
        )
        return resp.ok
    }

    /// `POST /v1/permission-requests/:id/decide` — decide a T4.3 permission
    /// request. Runner-side use: mirror a locally-answered approval to the
    /// server row so remote viewers' pending cards clear (#204). The endpoint
    /// resolves the deciding user from the device grant's granting user.
    func decidePermissionRequest(id: String, decision: String) async throws {
        struct Body: Encodable { let decision: String }
        struct Response: Decodable {}
        let _: Response = try await post(
            path: "v1/permission-requests/\(id)/decide",
            body: Body(decision: decision)
        )
    }

    // MARK: - Crew interactions (T4.5 ask_human / 人在回路)

    /// `GET /v1/sessions/:sessionId/interactions` — list this session's pending
    /// `ask_human` interactions awaiting a human reply. The operator's viewer
    /// polls this to surface "the agent is asking you something".
    func listSessionInteractions(sessionId: String) async throws -> [CrewInteraction] {
        struct Response: Decodable { let items: [CrewInteraction] }
        let resp: Response = try await get(path: "v1/sessions/\(sessionId)/interactions")
        return resp.items
    }

    /// `POST /v1/interactions/:reqId/answer` — answer an interaction with free
    /// text. The session's blocked `ask_human` tool picks it up and continues.
    @discardableResult
    func answerInteraction(reqId: String, reply: String) async throws -> Bool {
        struct Body: Encodable { let reply: String }
        struct Response: Decodable { let ok: Bool }
        let resp: Response = try await post(path: "v1/interactions/\(reqId)/answer", body: Body(reply: reply))
        return resp.ok
    }

    // MARK: - Crew group chat / whiteboard (spec §9 中栏)

    /// `GET /v1/crews/:crewId/messages` — the crew's whiteboard (group chat
    /// timeline) for the middle pane. crewId == crew_conversation_id. Oldest
    /// first. Needs `crew:read`.
    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry] {
        struct Response: Decodable { let whiteboard: [CrewWhiteboardEntry] }
        let resp: Response = try await get(path: "v1/crews/\(crewId)/messages")
        return resp.whiteboard
    }

    /// `GET /v1/crews/:crewId/messages?since=<cursor>` — 带游标的分页拉取
    /// （接合 v2 block 3：CrewRelayAgent 轮询用）。`since` = 上次响应的
    /// `lastCursor`（ISO8601，**闭区间** —— 边界条目会重复出现，调用方按
    /// entry id 去重）。Needs `crew:read`。
    func listCrewWhiteboardPage(crewId: String, since: String?) async throws -> CrewWhiteboardPage {
        // `appendingPathComponent` 会把 "?" 转义,query 走 URLComponents
        // (同 pollDeviceLoginChallenge 的做法)。
        let url = baseURL.appendingPathComponent("v1/crews/\(crewId)/messages")
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        if let since, !since.isEmpty {
            components.queryItems = [URLQueryItem(name: "since", value: since)]
        }
        var request = URLRequest(url: components.url!)
        request.httpMethod = "GET"
        attachAuth(to: &request)
        return try await perform(request)
    }

    /// `POST /v1/crews/:crewId/messages` — drop a message on the crew
    /// whiteboard. `mentions` routes it: `@session <id>` → that session's
    /// mailbox; `@captain`; empty/`@broadcast` → all sessions. `attachmentIds`
    /// are ids returned by `uploadAttachment`, persisted on the message as
    /// `{ ids: [...] }` and hydrated → renderable objects on read. Needs
    /// `crew:write`.
    @discardableResult
    /// `senderLabel` / `localSessionId`（接合 v2 block 3）：Mac relay 代发本地
    /// captain / session 话语时的来源标注 —— 任一非 nil 时 edge 会给消息打
    /// `origin:'mac_relay'`，Mac 拉取侧据此防回环。**relay 上行必须至少带
    /// senderLabel**，否则消息不会被标记、会被自己拉回来。
    func postCrewMessage(
        crewId: String,
        text: String,
        mentions: [CrewMention] = [],
        attachmentIds: [String] = [],
        senderLabel: String? = nil,
        localSessionId: String? = nil,
        replyToId: String? = nil
    ) async throws -> String? {
        struct Body: Encodable {
            let content: String
            let mentions: [CrewMention]
            let attachmentIds: [String]?
            let senderLabel: String?
            let localSessionId: String?
            let replyTo: String?

            enum CodingKeys: String, CodingKey {
                case content, mentions, attachmentIds, senderLabel, localSessionId
                case replyTo = "reply_to"
            }
        }
        // The handler returns `{ messageId }`; `announcementId` stays nil (this
        // endpoint never emitted one). Decode both leniently so neither shape
        // throws — callers only care that the POST succeeded.
        struct Response: Decodable { let announcementId: String?; let messageId: String? }
        let resp: Response = try await post(
            path: "v1/crews/\(crewId)/messages",
            body: Body(
                content: text,
                mentions: mentions,
                attachmentIds: attachmentIds.isEmpty ? nil : attachmentIds,
                senderLabel: senderLabel,
                localSessionId: localSessionId,
                // `reply_to`(#377):edge `CrewMessageBody` 已接受并落 attachments
                // jsonb 的 `in_reply_to`,读模型 `mapMessageToEntry` 统一透出供气泡渲染
                // 被回复引用。回复的「自动 @ 原发送者」另走 `mentions`(两端都生效)。
                replyTo: replyToId
            )
        )
        return resp.messageId ?? resp.announcementId
    }

    // MARK: - Attachment upload (compose half)

    /// `POST /v1/upload` — multipart upload of one file. The endpoint accepts a
    /// device-grant bearer with `crew:write` (PendingCrew's grant has it) and
    /// returns the attachment `id`, which is then passed to `postCrewMessage`
    /// via `attachmentIds`. Owner + quota attribute to the user who granted
    /// PendingCrew (server-side `effectiveOwnerUserId`).
    ///
    /// Wire: multipart/form-data with a single `file` part. We deliberately
    /// omit `conversationId` — the server binds the id to a crew message at
    /// send time and re-checks membership on every `/v1/uploads/:id` read, and
    /// the device-grant path rejects a `conversationId` it can't RLS-probe.
    func uploadAttachment(data: Data, filename: String, mime: String) async throws -> String {
        let boundary = "PendingCrewBoundary-\(UUID().uuidString)"
        var body = Data()
        func append(_ string: String) { body.append(string.data(using: .utf8)!) }
        append("--\(boundary)\r\n")
        // `filename` lands on the File's name → attachments.filename server-side.
        append("Content-Disposition: form-data; name=\"file\"; filename=\"\(filename)\"\r\n")
        append("Content-Type: \(mime)\r\n\r\n")
        body.append(data)
        append("\r\n--\(boundary)--\r\n")

        var request = URLRequest(url: baseURL.appendingPathComponent("v1/upload"))
        request.httpMethod = "POST"
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        attachAuth(to: &request)

        // Response: `{ id, r2_key, mime, size, filename, url, created_at, deduped? }`
        // (routes/upload.ts) — we only need the id.
        struct Response: Decodable { let id: String }
        let resp: Response = try await perform(request)
        return resp.id
    }

    /// `GET /v1/crews/:crewId/members` — the crew roster (spec §9: everyone is a
    /// member — human / captain / code_session / temp bot). Needs `crew:read`.
    func listCrewMembers(crewId: String) async throws -> CrewRoster {
        try await get(path: "v1/crews/\(crewId)/members")
    }

    /// `POST /v1/crews/:crewId/members` — 把 PendingBot 侧的 bot / 人类账号
    /// 拉进 relay crew conversation（接合 v2 block 3）。`kind` ∈ bot / human，
    /// bot 须 caller 可用、human 须好友/同 subject 成员（校验在 edge RPC）。
    /// 幂等：已是成员也回 201。Needs `crew:write` + grant subject 匹配。
    func addCrewMember(crewId: String, kind: String, botId: String? = nil, userId: String? = nil) async throws {
        struct Body: Encodable {
            let kind: String
            let botId: String?
            let userId: String?
        }
        struct Response: Decodable { let crewId: String }
        let _: Response = try await post(
            path: "v1/crews/\(crewId)/members",
            body: Body(kind: kind, botId: botId, userId: userId)
        )
    }

    // MARK: - Session inbox (runner per-turn context: whiteboard + mailbox)

    /// `GET /v1/sessions/:sessionId/inbox` — the per-turn context bundle the
    /// runner injects into the agent's prompt (spec §9.2/§9.5): the crew
    /// whiteboard + this session's unread mailbox. Needs `crew:read`.
    func getSessionInbox(sessionId: String) async throws -> CrewSessionInbox {
        try await get(path: "v1/sessions/\(sessionId)/inbox")
    }

    /// `POST /v1/sessions/:sessionId/inbox/mark-delivered` — mark mailbox items
    /// as delivered once they've been folded into a prompt turn, so they don't
    /// re-inject next turn. Needs `crew:write`.
    @discardableResult
    func markInboxDelivered(sessionId: String, itemIds: [String]) async throws -> Bool {
        guard !itemIds.isEmpty else { return true }
        struct Body: Encodable { let itemIds: [String]; enum CodingKeys: String, CodingKey { case itemIds = "item_ids" } }
        struct Response: Decodable { let ok: Bool? }
        let resp: Response = try await post(
            path: "v1/sessions/\(sessionId)/inbox/mark-delivered",
            body: Body(itemIds: itemIds)
        )
        return resp.ok ?? true
    }

    // MARK: - Internals

    private func get<Response: Decodable>(path: String) async throws -> Response {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = "GET"
        attachAuth(to: &request)
        return try await perform(request)
    }

    /// `bearer` 非 nil 时用它覆盖实例的 `bearerToken`（mintGrant 用家族凭据
    /// 鉴权，此时还没有本 app 的 grant）；nil 保持原行为。
    private func post<Body: Encodable, Response: Decodable>(
        path: String,
        body: Body,
        authenticated: Bool = true,
        bearer: String? = nil
    ) async throws -> Response {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(body)
        if let bearer {
            request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        } else if authenticated {
            attachAuth(to: &request)
        }
        return try await perform(request)
    }

    private func attachAuth(to request: inout URLRequest) {
        guard let token = bearerToken else { return }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    }

    /// 发请求，对**连接建立阶段**的瞬时失败重试。
    ///
    /// 病根：Cloudflare 广告 `alt-svc h3`，macOS URLSession 会机会性升级到
    /// HTTP/3(QUIC over UDP)。某些网络路径（TUN 模式代理只转 TCP 不转 UDP、弱网、
    /// UDP/443 被挡）下 QUIC 握手失败 → `URLError.secureConnectionFailed`
    /// （"A TLS error caused the secure connection to fail"），而 HTTP/2(TCP)
    /// 完全正常。重试时 URLSession 会把该 host 降级回 HTTP/2 → 成功。
    ///
    /// **只重试连接建立类错误**（secureConnectionFailed / cannotConnectToHost /
    /// DNS）—— 这些必然发生在请求送达服务器**之前**，所以对非幂等 POST
    /// （createSession 等）也安全，不会重复创建。
    private func dataWithConnectRetry(
        for request: URLRequest, attempts: Int = 3
    ) async throws -> (Data, URLResponse) {
        var lastError: Error = PendingCrewAPIError.invalidResponse
        for i in 0..<attempts {
            do { return try await session.data(for: request) }
            catch let e as URLError where Self.isConnectStageTransient(e) {
                lastError = e
                if i < attempts - 1 { try? await Task.sleep(nanoseconds: 350_000_000) }
            }
        }
        throw lastError
    }

    private static func isConnectStageTransient(_ e: URLError) -> Bool {
        switch e.code {
        case .secureConnectionFailed, .cannotConnectToHost,
             .cannotFindHost, .dnsLookupFailed:
            return true
        default:
            return false
        }
    }

    private func perform<Response: Decodable>(_ request: URLRequest) async throws -> Response {
        let (data, response) = try await dataWithConnectRetry(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw PendingCrewAPIError.invalidResponse
        }
        if !(200..<300).contains(http.statusCode) {
            // edge 用 { error: { code, message?, detail? } } envelope
            // (apps/edge/src/lib/http-error.ts)，直接解出来给上层。
            let envelope = (try? JSONDecoder().decode(ErrorEnvelope.self, from: data))?.error
            throw PendingCrewAPIError.http(
                status: http.statusCode,
                code: envelope?.code,
                message: envelope?.message
            )
        }
        return try JSONDecoder().decode(Response.self, from: data)
    }
}

// MARK: - Family credential response

/// `POST /v1/me/family-credential` 的响应。
///
/// edge 返回 camelCase JSON（`familyCredential.token`、`familyCredential.subjectId`、
/// `displayName`），与字段名完全对应，不需要 CodingKeys。
struct CrewFamilyCredentialResponse: Decodable {
    struct Cred: Decodable {
        /// `pfa_` 前缀的家族 SSO 凭据明文 token（只在此响应里出现一次，请立即存档）。
        let token: String
        /// 凭据绑定的默认 subject（通常是个人 `user_account` 主体）。
        /// `mintGrant` 时应以此值作 `subjectId`，除非调用方想换目标 subject。
        let subjectId: String
    }
    let familyCredential: Cred
    /// 绑定 subject 的显示名（`personal.display_name`），可选。
    let displayName: String?
    /// 用户头像 seed（`users.custom_fields.avatar_seed`，缺则 edge 回落 user id），
    /// 可选：旧 edge 不回此字段时 decode 不失败。
    let avatarSeed: String?
}

// MARK: - Error envelope (private to PendingCrewAPI)

private struct ErrorEnvelope: Decodable {
    let error: ErrorBody

    struct ErrorBody: Decodable {
        let code: String?
        let message: String?
    }
}

// MARK: - Device name helper (private to PendingCrewAPI)

#if canImport(IOKit)
import IOKit
#endif

private func currentDeviceName() -> String {
    #if os(macOS)
    return Host.current().localizedName ?? "Mac"
    #elseif os(iOS)
    return UIDevice.current.name
    #else
    return "PendingCrew Device"
    #endif
}

#if os(iOS)
import UIKit
#endif
