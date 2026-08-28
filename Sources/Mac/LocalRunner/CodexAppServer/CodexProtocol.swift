#if os(macOS)
import Foundation

/// Param builders for the core-loop methods. Plain dictionaries (paired with
/// CodexRPCMessage encoding) keep this Foundation-only + unit-testable.
enum CodexProtocol {
    enum ApprovalsReviewer: String, Codable, CaseIterable, Sendable {
        case autoReview = "auto_review"
        case user

        var displayName: String {
            switch self {
            case .autoReview: return "Approve for me"
            case .user: return "手动批准"
            }
        }
    }

    static func initializeParams(clientName: String, version: String) -> [String: Any] {
        // Shapes reconciled against `codex app-server generate-json-schema` (codex-cli 0.137.0):
        // ClientInfo = {name, title?, version}; InitializeCapabilities = {experimentalApi,
        // requestAttestation, optOutNotificationMethods?}. `requestAttestation` is non-optional in
        // the schema (we don't want upstream attestation → false). `item/agentMessage/delta` is a
        // real ServerNotification method, so opting out of it actually suppresses the delta stream
        // (v1 renders on `item/completed` only).
        ["clientInfo": ["name": clientName, "title": clientName, "version": version],
         "capabilities": [
             "experimentalApi": true,
             "requestAttestation": false,
             "optOutNotificationMethods": ["item/agentMessage/delta"],
         ]]
    }
    /// MCP servers register through codex's per-thread `config` override —
    /// `config.mcp_servers.<name> = { command, args, env? }` (the structured form of
    /// `-c mcp_servers.<name>...` / the `[mcp_servers.*]` config.toml table). There is
    /// **no** top-level `mcpServers` field on `thread/start`; codex silently drops
    /// unknown keys, so the old shape registered nothing and the session got zero crew
    /// tools. Verified against `codex app-server generate-json-schema` + `codex mcp add`
    /// (codex-cli 0.137.0).
    static func threadStartParams(
        cwd: String,
        model: String?,
        effort: String?,
        developerInstructions: String?,
        mcpServers: [String: Any]?,
        approvalsReviewer: ApprovalsReviewer = .autoReview
    ) -> [String: Any] {
        var p: [String: Any] = [
            "cwd": cwd,
            "approvalPolicy": "on-request",
            "sandbox": "workspace-write",
            "approvalsReviewer": approvalsReviewer.rawValue,
        ]
        if let model, !model.isEmpty { p["model"] = model }
        if let di = developerInstructions, !di.isEmpty { p["developerInstructions"] = di }
        var config: [String: Any] = [:]
        if let effort, !effort.isEmpty { config["model_reasoning_effort"] = effort }
        if let mcp = mcpServers, !mcp.isEmpty { config["mcp_servers"] = mcp }
        if !config.isEmpty { p["config"] = config }
        return p
    }

    static func threadResumeParams(
        threadId: String,
        cwd: String,
        model: String?,
        effort: String?,
        developerInstructions: String?,
        mcpServers: [String: Any]?,
        approvalsReviewer: ApprovalsReviewer = .autoReview
    ) -> [String: Any] {
        var p = threadStartParams(
            cwd: cwd,
            model: model,
            effort: effort,
            developerInstructions: developerInstructions,
            mcpServers: mcpServers,
            approvalsReviewer: approvalsReviewer)
        p["threadId"] = threadId
        // Codex 0.149 added `excludeTurns` for clients that only need to rejoin the
        // live thread. Without it, `thread/resume` serializes the complete persisted
        // turn history into one JSON line. A long-lived captain produced a 6 MB
        // rollout whose resume response did not finish before our 25 s launch probe;
        // the app-server and crew MCP were already alive, but PendingCrew could not
        // observe the thread id until the entire line arrived and falsely reported a
        // stalled launch. The transcript is protocol-event driven and does not use
        // historical turns, so omitting them is both sufficient and bounded.
        p["excludeTurns"] = true
        return p
    }

    static func threadSettingsUpdateParams(
        threadId: String,
        approvalsReviewer: ApprovalsReviewer
    ) -> [String: Any] {
        ["threadId": threadId, "approvalsReviewer": approvalsReviewer.rawValue]
    }
    /// The unread crew whiteboard rides in via **`turn/start.additionalContext`** —
    /// codex's native per-turn context channel. The field IS in the v2 schema, gated
    /// behind `#[experimental("turn/start.additionalContext")]`, which we unlock via
    /// `initialize.capabilities.experimentalApi:true` (see initializeParams). codex
    /// core dedupes by key (`AdditionalContextStore::merge` only emits a fragment when
    /// a key's value changed → a turn with no new whiteboard injects NOTHING, zero
    /// prefix churn) and renders `kind:"untrusted"` as a `role:"user"` fragment wrapped
    /// in `<external_<key>>…</external_<key>>` — injection hygiene: a teammate's message
    /// is DATA, never instructions to this agent. The user's own text stays the sole
    /// `input[]` item.
    ///
    /// History: a prior impl prepended the whiteboard as a leading `text` input item
    /// after wrongly judging the schema as lacking this field. The mistake was a
    /// name-collision — `makeWhiteboardProvider` reuses claude's `HookEmitter`, whose
    /// envelope key is *also* `additionalContext` (a plain string), and that got
    /// conflated with codex's `additionalContext` (a `{key:{value,kind}}` map). Same
    /// name, different protocol. (⚠️ needs a live codex run to confirm the field is
    /// honored — schema-shape only so far; see docs/tech-debt.md.)
    static func turnStartParams(threadId: String, text: String, whiteboard: String?) -> [String: Any] {
        var p: [String: Any] = [
            "threadId": threadId,
            "input": [["type": "text", "text": text, "text_elements": []]],
        ]
        if let wb = whiteboard, !wb.isEmpty {
            p["additionalContext"] = ["crew_whiteboard": ["value": wb, "kind": "untrusted"]]
        }
        return p
    }
    static func turnInterruptParams(threadId: String, turnId: String) -> [String: Any] {
        ["threadId": threadId, "turnId": turnId]
    }

    // MARK: - Server-initiated requests (codex → client)

    /// Classifies an inbound server-request so the backend ALWAYS answers it. Silently
    /// dropping any server-request blocks the turn forever (codex waits on our reply) —
    /// that is the codex「运行中…」转圈不结束 root cause. The real `ServerRequest` enum
    /// (codex-cli 0.137.0 `generate-json-schema`) is: three `*requestApproval` +
    /// `mcpServer/elicitation/request` + `item/tool/requestUserInput` + `item/tool/call`
    /// + `account/chatgptAuthTokens/refresh` + `attestation/generate`.
    enum ServerRequestKind: Equatable { case approval, elicitation, account, unsupported }

    /// Only a manual-review thread may surface an approval request to PendingCrew.
    /// Keeping this decision pure makes the no-phantom-notice invariant testable:
    /// auto_review requests are answered fail-closed without raising a card or notice.
    enum ApprovalRequestDisposition: Equatable { case presentCard, rejectWithoutNotice }

    static func approvalRequestDisposition(
        reviewer: ApprovalsReviewer
    ) -> ApprovalRequestDisposition {
        reviewer == .user ? .presentCard : .rejectWithoutNotice
    }

    /// Response envelopes differ for permissions requests. Codex 0.145 expects a
    /// GrantedPermissionProfile rather than `{decision: ...}`: allow echoes the
    /// requested profile, while deny grants an empty profile, both scoped to this turn.
    static func approvalResponse(
        method: String, params: [String: Any], decision: String
    ) -> [String: Any] {
        guard method == "item/permissions/requestApproval" else {
            return ["decision": decision]
        }
        let requested = params["permissions"] as? [String: Any] ?? [:]
        return [
            "permissions": decision == "accept" ? requested : [:],
            "scope": "turn",
        ]
    }

    static func serverRequestKind(method: String) -> ServerRequestKind {
        if method.hasSuffix("requestApproval") { return .approval }   // command / fileChange / permissions
        if method == "mcpServer/elicitation/request" { return .elicitation }
        // account/chatgptAuthTokens/refresh —— codex 要客户端刷新 ChatGPT token。
        // 我们无法代刷(回错误让 turn 继续),但这是「codex 登录态失效」的强信号,
        // 单列出来给健康感知用(此前混在 unsupported 里被静默丢,分诊第 7 点)。
        if method.hasPrefix("account/") { return .account }
        return .unsupported
    }

    /// `availableDecisions` is a union in Codex 0.145: ordinary choices are
    /// strings, while persistent exec/network-policy choices are structured
    /// objects. Casting the whole array to `[String]` drops *every* choice as
    /// soon as one structured option is present. PendingCrew's current card is
    /// deliberately only allow/deny, so expose only the non-persistent choices
    /// it can represent without silently broadening an approval.
    static func safeApprovalDecisions(params: [String: Any]) -> [String] {
        guard let raw = params["availableDecisions"] as? [Any] else {
            return ["accept", "decline"]
        }
        let supported = raw.compactMap { $0 as? String }.filter {
            $0 == "accept" || $0 == "decline" || $0 == "cancel"
        }
        return supported.isEmpty ? ["decline"] : supported
    }

    /// Translate Codex app-server health notifications into the shared runner
    /// health model. These are protocol fields, not message-text heuristics:
    /// - `error` / failed `turn/completed`: `codexErrorInfo=usageLimitExceeded`
    /// - `account/rateLimits/updated`: non-null reached type / spend-control hit
    static func sessionHealth(method: String, params: [String: Any]) -> CrewSessionHealth? {
        if method == "account/rateLimits/updated",
           let limits = params["rateLimits"] as? [String: Any] {
            if let reached = limits["rateLimitReachedType"] as? String, !reached.isEmpty {
                return CrewSessionHealth(
                    kind: .usageLimit,
                    detail: "Codex 额度已到上限（\(reached)），已安排在额度重置后继续。")
            }
            if limits["spendControlReached"] as? Bool == true {
                return CrewSessionHealth(
                    kind: .usageLimit,
                    detail: "Codex 用量控制上限已触发，已安排在额度重置后继续。")
            }
            return nil
        }

        let error: [String: Any]?
        if method == "error" {
            error = params["error"] as? [String: Any]
        } else if method == "turn/completed" {
            error = (params["turn"] as? [String: Any])?["error"] as? [String: Any]
        } else {
            error = nil
        }
        guard let error, let code = error["codexErrorInfo"] as? String else { return nil }
        let message = (error["message"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let nonemptyMessage = message.flatMap { $0.isEmpty ? nil : $0 }
        switch code {
        case "usageLimitExceeded":
            return CrewSessionHealth(
                kind: .usageLimit,
                detail: (nonemptyMessage ?? "Codex 额度已到上限")
                    + "；已安排在额度重置后继续。")
        case "unauthorized":
            return CrewSessionHealth(
                kind: .authRequired,
                detail: nonemptyMessage ?? "Codex 登录态失效，请重新登录后重启该 session。")
        default:
            return nil
        }
    }

    /// Reply for `mcpServer/elicitation/request`. CRITICAL: in codex-cli 0.137.0 the
    /// **MCP tool-call approval prompt is delivered AS an elicitation** carrying
    /// `_meta.codex_approval_kind == "mcp_tool_call"` (verified against a live
    /// `codex app-server` turn — NOT a `*/requestApproval`). So blanket-declining every
    /// elicitation declined every crew tool call: codex marked the `mcpToolCall` failed
    /// with `error:"user rejected MCP tool call"`, which is why the codex captain's
    /// `post_to_crew` check-in was rejected and never posted.
    ///
    /// The crew comms tools (`post_to_crew`/`read_whiteboard`/`ask`/…) are the session's
    /// own trusted nervous system and must auto-approve — exactly as the claude backend
    /// excludes them from its PreToolUse gate (`LocalSessionLaunch` `permGates =
    /// "computer-use"`; crew tools never gated). A crew session only ever wires the one
    /// trusted `crew` MCP server, so accepting `mcp_tool_call` elicitations is correct
    /// here (dangerous ops — shell/file — still gate via `*/requestApproval`). Genuine
    /// input-form elicitations (no `codex_approval_kind`) keep declining: v1 has no UI to
    /// fill them, and declining lets the turn proceed instead of hanging on a reply we'd
    /// never send. Shape per `McpServerElicitationRequestResponse` =
    /// `{ action: accept|decline|cancel, content? }`; accept/decline carry no content.
    static func elicitationResult(params: [String: Any]) -> [String: Any] {
        let meta = params["_meta"] as? [String: Any]
        if (meta?["codex_approval_kind"] as? String) == "mcp_tool_call" {
            return ["action": "accept"]
        }
        return elicitationDeclineResult()
    }

    /// Bare decline reply for a genuine (non-approval) elicitation.
    static func elicitationDeclineResult() -> [String: Any] { ["action": "decline"] }

    /// 人话描述一条「codex 在要一个我们给不出的回答」的 server-request（Todo #6）。
    /// 这些请求过去是**静默**处理的（真 elicitation 直接 decline、unsupported 直接回
    /// 错误）—— 结果是这个 session 被要求填表单、被我们替它拒了，而群里没有任何人知道。
    /// 不阻塞是对的（阻塞就成了另一种死法），但必须亮出来。
    static func pendingRequestSummary(method: String, params: [String: Any]) -> String {
        for key in ["message", "prompt", "reason", "question"] {
            if let s = params[key] as? String, !s.isEmpty { return s }
        }
        return "codex 发来一个需要人回答的请求（\(method)）"
    }
}
#endif
