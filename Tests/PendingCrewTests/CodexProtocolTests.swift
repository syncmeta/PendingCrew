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

    func testOutdatedCLIIsVisibleEvenWithObjectErrorInfo() {
        let message = "The 'gpt-6-astra' model requires a newer version of Codex. Please upgrade..."
        let health = CodexProtocol.sessionHealth(method: "turn/completed", params: [
            "turn": ["status": "failed", "error": [
                "message": message,
                "codexErrorInfo": ["httpConnectionFailed": ["httpStatusCode": 400]],
            ]],
        ])
        XCTAssertNotNil(health, "A failed turn must not become silent idle")
        XCTAssertTrue(health?.detail.contains(message) == true)
        XCTAssertTrue(health?.detail.contains("版本") == true)
    }

    func testUnknownTerminalFailureIsVisibleButRetryingErrorIsNotTerminal() {
        let failed = CodexProtocol.sessionHealth(method: "turn/completed", params: [
            "turn": ["status": "failed", "error": ["message": "new server error", "codexErrorInfo": ["futureVariant": 1]]],
        ])
        XCTAssertEqual(failed?.kind, .turnFailed)
        XCTAssertTrue(failed?.detail.contains("new server error") == true)
        XCTAssertEqual(CodexProtocol.sessionHealth(method: "turn/completed", params: ["turn": ["status": "failed"]])?.kind, .turnFailed)
        XCTAssertNil(CodexProtocol.sessionHealth(method: "turn/completed", params: ["turn": ["status": "completed"]]))
        XCTAssertNil(CodexProtocol.sessionHealth(method: "error", params: ["error": ["message": "transient"], "willRetry": true]))
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

final class CodexPipeReadabilityTests: XCTestCase {
    func testEOFDisarmsReadabilityHandlerInsteadOfSpinningForever() {
        let pipe = Pipe()
        let reader = pipe.fileHandleForReading
        reader.readabilityHandler = { _ in }
        pipe.fileHandleForWriting.closeFile()

        var received: Data?
        CodexPipeReadability.drain(reader) { received = $0 }

        XCTAssertNil(received)
        XCTAssertNil(reader.readabilityHandler,
                     "EOF must disarm the handler; otherwise Foundation repeatedly calls availableData")
    }

    func testPayloadStaysArmedAndIsDelivered() {
        let pipe = Pipe()
        let reader = pipe.fileHandleForReading
        reader.readabilityHandler = { _ in }
        pipe.fileHandleForWriting.write(Data("hello".utf8))

        var received: Data?
        CodexPipeReadability.drain(reader) { received = $0 }

        XCTAssertEqual(received, Data("hello".utf8))
        XCTAssertNotNil(reader.readabilityHandler)
        pipe.fileHandleForWriting.closeFile()
    }
}

/// 写侧的 EPIPE（2026-09-05 那次「5 个 crew 同时掉 session」的病根）。
///
/// 读侧的孪生在上面：`CodexPipeReadabilityTests`。同一个文件、同一类病 —— 上一笔
/// `0399845` 只修了读侧的 EOF 空转，写侧的 EPIPE 留在了原地。
///
/// **这一条为什么必须是「抛」而不是「崩」**：`writeLine` 的签名一直写着 `throws`，
/// 6 个调用点也都老老实实写了 `try` —— 但函数体调的是 ObjC 的 `writeData:`，
/// EPIPE 时抛的是 `NSFileHandleOperationException`，Swift 的 `catch` 接不住。
/// 于是那 6 个 `try` 全是摆设，唯一真会发生的错误恰恰是它们捕不到的那个。
final class CodexPipeWriteTests: XCTestCase {
    /// 对端没了 → 必须拿到一个**能 catch 的 Swift 错误**。
    ///
    /// 这条测试同时是一次**测量**，测的不是我们自己的代码：如果这个进程里 SIGPIPE
    /// 没被忽略，`write(2)` 会直接用信号打死进程 —— 那样换 API 也没用，而且死得比
    /// NSException 更安静。**它跑出绿（而不是把 test runner 带走）本身就是那个读数。**
    func testBrokenPipeSurfacesAsCatchableSwiftErrorRatherThanKillingTheProcess() {
        let pipe = Pipe()
        pipe.fileHandleForReading.closeFile()   // 对端先走，正是 codex 子进程死掉时的形状

        var caught: NSError?
        do {
            try CodexPipeWrite.line("{}", to: pipe.fileHandleForWriting)
            XCTFail("对端已关，这次写必须失败；悄悄成功说明这条测试根本没量到 EPIPE")
        } catch {
            caught = error as NSError
        }

        // 钉死量到的确实是 EPIPE(32)，不是别的失败 —— 否则这条测试会在别的原因下假绿。
        let posix = (caught?.userInfo[NSUnderlyingErrorKey] as? NSError) ?? caught
        XCTAssertEqual(posix?.domain, NSPOSIXErrorDomain, "实际拿到：\(String(describing: caught))")
        XCTAssertEqual(posix?.code, Int(EPIPE), "实际拿到：\(String(describing: caught))")
    }

    /// 上面那条依赖「SIGPIPE 已被忽略」，而这个保证必须是 `CodexPipeWrite` 自己给的，
    /// 不能是进程里碰巧有别人关过。把 arming 拿掉，上面那条会从 failure 变成 crash ——
    /// 这一条则会直接红，且红得看得懂。
    func testWritingArmsTheProcessAgainstSIGPIPEItself() throws {
        let pipe = Pipe()
        try CodexPipeWrite.line("warm", to: pipe.fileHandleForWriting)
        pipe.fileHandleForWriting.closeFile()
        _ = pipe.fileHandleForReading.readDataToEndOfFile()

        var current = sigaction()
        XCTAssertEqual(sigaction(SIGPIPE, nil, &current), 0)
        // C 函数指针不是 Equatable，按位比。
        let handler = unsafeBitCast(current.__sigaction_u.__sa_handler, to: UInt.self)
        XCTAssertEqual(handler, unsafeBitCast(SIG_IGN, to: UInt.self),
                       "SIGPIPE 仍是默认动作 —— 对端一走，写侧会被信号无声打死，连异常日志都不留")
    }

    /// 正常那半：补上换行、原样送达。别只测失败路径把编码写坏了。
    func testWritesTheLineWithATrailingNewline() throws {
        let pipe = Pipe()
        try CodexPipeWrite.line("{\"id\":1}", to: pipe.fileHandleForWriting)
        pipe.fileHandleForWriting.closeFile()

        let received = pipe.fileHandleForReading.readDataToEndOfFile()
        XCTAssertEqual(String(data: received, encoding: .utf8), "{\"id\":1}\n")
    }
}
// 红证的复现方式（变异）：把 `CodexPipeWrite.line` 的函数体换回
//     handle.write(Data((line + "\n").utf8))
// 上面第一条测试就不是 failure，而是把整个 test runner 崩掉 —— 这正是它要挡的事。
