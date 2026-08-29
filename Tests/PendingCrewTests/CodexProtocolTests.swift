import XCTest


final class CodexProtocolTests: XCTestCase {
    func testInitializeParamsCarryClientInfoAndExperimentalApi() {
        let p = CodexProtocol.initializeParams(clientName: "PendingCrew", version: "1.0")
        let info = p["clientInfo"] as? [String: Any]
        XCTAssertEqual(info?["name"] as? String, "PendingCrew")
        XCTAssertEqual(info?["version"] as? String, "1.0")
        let caps = p["capabilities"] as? [String: Any]
        XCTAssertEqual(caps?["experimentalApi"] as? Bool, true)
        XCTAssertNotNil(caps?["optOutNotificationMethods"], "v1 should opt out of streaming deltas")
    }
    func testThreadStartRegistersMcpUnderConfigNotTopLevel() {
        // codex `thread/start` has NO top-level `mcpServers` field — MCP registers via
        // `config.mcp_servers` (real-schema verified). Guards against the silent-drop regression.
        let p = CodexProtocol.threadStartParams(cwd: "/repo", model: "gpt-5.5", effort: "high",
            developerInstructions: "world-model",
            mcpServers: ["crew": ["command": "/bin/helper", "args": ["--mcp-serve"]]])
        XCTAssertEqual(p["cwd"] as? String, "/repo")
        XCTAssertEqual(p["model"] as? String, "gpt-5.5")
        XCTAssertEqual(p["developerInstructions"] as? String, "world-model")
        XCTAssertEqual(p["approvalPolicy"] as? String, "on-request")
        XCTAssertEqual(p["sandbox"] as? String, "workspace-write")
        XCTAssertEqual(p["approvalsReviewer"] as? String, "auto_review")
        XCTAssertNil(p["mcpServers"], "must NOT use the bogus top-level field codex drops")
        let mcp = (p["config"] as? [String: Any])?["mcp_servers"] as? [String: Any]
        XCTAssertEqual((p["config"] as? [String: Any])?["model_reasoning_effort"] as? String, "high")
        let crew = mcp?["crew"] as? [String: Any]
        XCTAssertEqual(crew?["command"] as? String, "/bin/helper")
        XCTAssertEqual(crew?["args"] as? [String], ["--mcp-serve"])
    }
    func testThreadStartOmitsEmptyOptionals() {
        let p = CodexProtocol.threadStartParams(
            cwd: "/repo", model: nil, effort: nil,
            developerInstructions: "", mcpServers: nil)
        XCTAssertNil(p["model"]); XCTAssertNil(p["developerInstructions"]); XCTAssertNil(p["config"])
        XCTAssertEqual(p["approvalPolicy"] as? String, "on-request")   // policy always present
        XCTAssertEqual(p["approvalsReviewer"] as? String, "auto_review")
    }
    func testResumeAndLiveSettingsKeepAutoReview() {
        let resume = CodexProtocol.threadResumeParams(
            threadId: "thr_old", cwd: "/repo", model: "gpt-5.5", effort: "xhigh",
            developerInstructions: "world", mcpServers: nil)
        XCTAssertEqual(resume["threadId"] as? String, "thr_old")
        XCTAssertEqual(resume["approvalsReviewer"] as? String, "auto_review")
        XCTAssertEqual(resume["approvalPolicy"] as? String, "on-request")
        XCTAssertEqual(resume["sandbox"] as? String, "workspace-write")
        XCTAssertEqual(resume["excludeTurns"] as? Bool, true,
                       "resume must not return an unbounded historical turns payload")
        XCTAssertEqual(
            (resume["config"] as? [String: Any])?["model_reasoning_effort"] as? String,
            "xhigh")

        let update = CodexProtocol.threadSettingsUpdateParams(
            threadId: "thr_old", approvalsReviewer: .autoReview)
        XCTAssertEqual(update["threadId"] as? String, "thr_old")
        XCTAssertEqual(update["approvalsReviewer"] as? String, "auto_review")
    }
    func testStartResumeAndLiveSettingsCarryManualReviewer() {
        let start = CodexProtocol.threadStartParams(
            cwd: "/repo", model: nil, effort: nil,
            developerInstructions: nil, mcpServers: nil, approvalsReviewer: .user)
        let resume = CodexProtocol.threadResumeParams(
            threadId: "thr_old", cwd: "/repo", model: nil, effort: nil,
            developerInstructions: nil, mcpServers: nil, approvalsReviewer: .user)
        let update = CodexProtocol.threadSettingsUpdateParams(
            threadId: "thr_old", approvalsReviewer: .user)
        XCTAssertEqual(start["approvalsReviewer"] as? String, "user")
        XCTAssertEqual(resume["approvalsReviewer"] as? String, "user")
        XCTAssertEqual(update["approvalsReviewer"] as? String, "user")
    }
    func testLiveSettingsCanSwitchModelAndEffortWithoutChangingReviewer() {
        let update = CodexProtocol.threadSettingsUpdateParams(
            threadId: "thr_old", model: "gpt-5.6-sol", effort: "xhigh")
        XCTAssertEqual(update["threadId"] as? String, "thr_old")
        XCTAssertEqual(update["model"] as? String, "gpt-5.6-sol")
        XCTAssertEqual(update["effort"] as? String, "xhigh")
        XCTAssertNil(update["approvalsReviewer"])
    }
    func testTurnStartPutsWhiteboardInAdditionalContext() {
        // codex's native per-turn context channel is `turn/start.additionalContext`
        // (experimental, unlocked via initialize.experimentalApi). The whiteboard rides
        // there as a deduped `kind:"untrusted"` entry — NOT prepended into input.
        let p = CodexProtocol.turnStartParams(threadId: "thr_1", text: "go", whiteboard: "unread:1")
        XCTAssertEqual(p["threadId"] as? String, "thr_1")
        // user's text is the SOLE input item (whiteboard no longer rides in input)
        let input = p["input"] as? [[String: Any]]
        XCTAssertEqual(input?.count, 1)
        XCTAssertEqual(input?.first?["text"] as? String, "go")
        // whiteboard rides in additionalContext under a stable key, kind untrusted
        let ac = p["additionalContext"] as? [String: [String: Any]]
        let wb = ac?["crew_whiteboard"]
        XCTAssertEqual(wb?["value"] as? String, "unread:1")
        XCTAssertEqual(wb?["kind"] as? String, "untrusted")
    }
    func testTurnStartIsSingleTextInputWhenNoWhiteboard() {
        for wb in [nil, ""] as [String?] {
            let p = CodexProtocol.turnStartParams(threadId: "t", text: "go", whiteboard: wb)
            XCTAssertNil(p["additionalContext"])
            let input = p["input"] as? [[String: Any]]
            XCTAssertEqual(input?.count, 1)
            XCTAssertEqual(input?.first?["text"] as? String, "go")
        }
    }
    func testTurnInterruptParams() {
        let p = CodexProtocol.turnInterruptParams(threadId: "thr_1", turnId: "turn_9")
        XCTAssertEqual(p["threadId"] as? String, "thr_1")
        XCTAssertEqual(p["turnId"] as? String, "turn_9")
    }

    // MARK: - Server-request handling (codex → client) — every kind must be answerable.
    // Guards the codex hang: silently dropping any server-request blocks the turn forever.

    func testServerRequestKindClassifiesEveryRealMethod() {
        // Real `ServerRequest` enum, codex-cli 0.137.0 generate-json-schema.
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "item/commandExecution/requestApproval"), .approval)
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "item/fileChange/requestApproval"), .approval)
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "item/permissions/requestApproval"), .approval)
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "mcpServer/elicitation/request"), .elicitation)
        // account/* 单列(批 C 健康感知):仍回错误应答(不代刷 token),但要翻 health。
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "account/chatgptAuthTokens/refresh"), .account)
        // Everything we don't model → unsupported, but STILL answered (with an error), never dropped.
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "item/tool/requestUserInput"), .unsupported)
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "item/tool/call"), .unsupported)
        XCTAssertEqual(CodexProtocol.serverRequestKind(method: "attestation/generate"), .unsupported)
    }

    func testStructuredApprovalChoicesDoNotDropSafeStringChoices() {
        let params: [String: Any] = [
            "availableDecisions": [
                "accept",
                ["acceptWithExecpolicyAmendment": ["execpolicy_amendment": ["git", "switch"]]],
                "decline",
            ] as [Any],
        ]
        XCTAssertEqual(CodexProtocol.safeApprovalDecisions(params: params), ["accept", "decline"])
    }

    func testStructuredOnlyApprovalNeverTurnsPlainAllowIntoPersistentGrant() {
        let params: [String: Any] = [
            "availableDecisions": [
                ["acceptWithExecpolicyAmendment": ["execpolicy_amendment": ["git"]]],
                "decline",
            ] as [Any],
        ]
        XCTAssertEqual(CodexProtocol.safeApprovalDecisions(params: params), ["decline"])
    }

    func testUsageLimitHealthComesFromFailedTurnProtocolField() {
        let health = CodexProtocol.sessionHealth(method: "turn/completed", params: [
            "turn": [
                "status": "failed",
                "error": ["message": "Weekly limit reached", "codexErrorInfo": "usageLimitExceeded"],
            ],
        ])
        XCTAssertEqual(health?.kind, .usageLimit)
        XCTAssertTrue(health?.detail.contains("Weekly limit reached") == true)
    }

    func testRateLimitUpdatedReachedTypeRaisesQuotaHealth() {
        let health = CodexProtocol.sessionHealth(method: "account/rateLimits/updated", params: [
            "rateLimits": ["rateLimitReachedType": "rate_limit_reached"],
        ])
        XCTAssertEqual(health?.kind, .usageLimit)
    }

    func testUnrelatedCodexErrorDoesNotPretendToBeQuota() {
        XCTAssertNil(CodexProtocol.sessionHealth(method: "error", params: [
            "error": ["message": "busy", "codexErrorInfo": "serverOverloaded"],
            "willRetry": true,
        ]))
    }

    func testApprovalRoutingOnlyManualModePresentsACard() {
        XCTAssertEqual(
            CodexProtocol.approvalRequestDisposition(reviewer: .user), .presentCard)
        XCTAssertEqual(
            CodexProtocol.approvalRequestDisposition(reviewer: .autoReview),
            .rejectWithoutNotice,
            "auto_review must not call the provider that raises a card and 待审批 notice")
    }

    func testPermissionsApprovalUsesGrantedProfileEnvelope() {
        let requested: [String: Any] = [
            "network": ["enabled": true],
            "fileSystem": ["write": ["/repo/generated"]],
        ]
        let allow = CodexProtocol.approvalResponse(
            method: "item/permissions/requestApproval",
            params: ["permissions": requested], decision: "accept")
        XCTAssertEqual(allow["scope"] as? String, "turn")
        XCTAssertEqual(
            (allow["permissions"] as? [String: Any])?["network"] as? [String: Bool],
            ["enabled": true])

        let deny = CodexProtocol.approvalResponse(
            method: "item/permissions/requestApproval",
            params: ["permissions": requested], decision: "decline")
        XCTAssertTrue((deny["permissions"] as? [String: Any])?.isEmpty == true)
        XCTAssertNil(deny["decision"], "permissions response has no decision field in 0.145")

        let fileChange = CodexProtocol.approvalResponse(
            method: "item/fileChange/requestApproval", params: [:], decision: "accept")
        XCTAssertEqual(fileChange["decision"] as? String, "accept")
    }

    func testManualApprovalProviderCreatesOperableCardBeforeNoticeAndReturnsDecision() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-manual-approval-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let approvals = LocalApprovalStore(directory: directory)
        let board = LocalWhiteboardStore(directory: directory)
        let provider = CodexManualApprovalBridge.provider(
            crewId: "crew", sessionId: "codex-session", directory: directory,
            pollIntervalNanoseconds: 1_000_000, maxWaits: 1_000)

        let response = Task { await provider("git switch -c fix/x", ["accept", "decline"]) }

        var pending: ApprovalItem?
        for _ in 0..<100 where pending == nil {
            pending = approvals.pending(crewId: "crew").first
            if pending == nil { try await Task.sleep(nanoseconds: 1_000_000) }
        }
        let item = try XCTUnwrap(pending)
        XCTAssertEqual(item.kind, "permission")
        XCTAssertEqual(item.sessionId, "codex-session")
        XCTAssertEqual(item.summary, "git switch -c fix/x")
        // 卡片先落、群里那条通知后落 —— 两次写之间有真实的时间窗。**这里必须等，
        // 不能读一次就断言**：读一次在满载的全量跑里会撞进那个窗口（实测约一半
        // 概率红），而这一族的语义是「通知只有在卡片之后出现才算数」，不是「通知
        // 与卡片同一瞬间出现」。等到它出现即满足语义；等不到才是真红。
        var noticed = false
        for _ in 0..<100 where !noticed {
            noticed = board.list(crewId: "crew").contains { $0.text.contains("待审批：git switch") }
            if !noticed { try await Task.sleep(nanoseconds: 1_000_000) }
        }
        XCTAssertTrue(noticed, "a notice is valid only after the pending card exists")

        approvals.decide(crewId: "crew", id: item.id, decision: "allow")
        let decision = await response.value
        XCTAssertEqual(decision, "accept")
        XCTAssertTrue(approvals.pending(crewId: "crew").isEmpty)
    }

    func testManualApprovalTimeoutClosesStaleCard() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-manual-timeout-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let approvals = LocalApprovalStore(directory: directory)
        let provider = CodexManualApprovalBridge.provider(
            crewId: "crew", sessionId: "codex-session", directory: directory,
            pollIntervalNanoseconds: 1, maxWaits: 1)

        let decision = await provider("write outside workspace", ["accept", "decline"])
        XCTAssertEqual(decision, "decline")
        XCTAssertTrue(approvals.pending(crewId: "crew").isEmpty,
                      "a completed server request must not leave an inoperable stale card")
        XCTAssertEqual(approvals.list(crewId: "crew").first?.decision, "deny")
    }

    func testElicitationDeclineResultMatchesRealSchema() {
        // McpServerElicitationRequestResponse = { action: accept|decline|cancel, content? };
        // decline carries no content. v1 declines so the turn never blocks on us.
        let r = CodexProtocol.elicitationDeclineResult()
        XCTAssertEqual(r["action"] as? String, "decline")
        XCTAssertNil(r["content"], "decline carries no content")
    }

    func testElicitationResultAcceptsMcpToolCallApproval() {
        // codex 0.137.0 delivers the MCP tool-call approval AS an elicitation carrying
        // _meta.codex_approval_kind == "mcp_tool_call". Auto-accept it — crew tools are
        // trusted; declining it is what rejected the captain's post_to_crew check-in.
        let params: [String: Any] = [
            "serverName": "crew",
            "message": "Allow the crew MCP server to run tool \"post_to_crew\"?",
            "_meta": ["codex_approval_kind": "mcp_tool_call", "tool_description": "post to crew whiteboard"],
        ]
        let r = CodexProtocol.elicitationResult(params: params)
        XCTAssertEqual(r["action"] as? String, "accept")
    }

    func testElicitationResultDeclinesGenuineInputForm() {
        // A real MCP elicitation (no codex_approval_kind) has no v1 UI → decline so the
        // turn proceeds instead of hanging; we never blanket-accept and submit empty input.
        let params: [String: Any] = [
            "serverName": "crew",
            "message": "What is your name?",
            "requestedSchema": ["type": "object", "properties": ["name": ["type": "string"]]],
        ]
        let r = CodexProtocol.elicitationResult(params: params)
        XCTAssertEqual(r["action"] as? String, "decline")
    }
}
