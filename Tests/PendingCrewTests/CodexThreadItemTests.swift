import XCTest


/// Fixtures here mirror the **real** `codex app-server` wire shapes captured via
/// `codex app-server generate-json-schema` (codex-cli 0.137.0, `v2/ThreadItem`),
/// reconciled 2026-06-16. Do not "simplify" them back to flat `text`/`name`
/// fields — that was the original mistake (tests asserted a fictional shape and
/// passed green while the decoder silently dropped data). Each case keeps the
/// exact key shape codex emits.
final class CodexThreadItemTests: XCTestCase {
    private func decode(_ json: String) throws -> CodexThreadItem {
        try JSONDecoder().decode(CodexThreadItem.self, from: Data(json.utf8))
    }

    func testDecodeAgentMessage() throws {
        // real: { type, id, text, phase: "commentary"|"final_answer"|null, memoryCitation }
        let item = try decode(#"{"id":"item_1","type":"agentMessage","text":"Done.","phase":"final_answer","memoryCitation":null}"#)
        guard case let .agentMessage(text, phase) = item.kind else { return XCTFail("expected agentMessage") }
        XCTAssertEqual(text, "Done.")
        XCTAssertEqual(phase, "final_answer")
        XCTAssertEqual(item.id, "item_1")
    }

    func testDecodeCommandExecution() throws {
        // real carries processId/source/commandActions/durationMs too — must be tolerated/ignored
        let item = try decode(#"{"id":"item_2","type":"commandExecution","command":"ls -la","cwd":"/repo","processId":null,"source":"agent","status":"completed","commandActions":[],"aggregatedOutput":"a\nb","exitCode":0,"durationMs":12}"#)
        guard case let .commandExecution(c) = item.kind else { return XCTFail("expected commandExecution") }
        XCTAssertEqual(c.command, "ls -la")
        XCTAssertEqual(c.cwd, "/repo")
        XCTAssertEqual(c.exitCode, 0)
        XCTAssertEqual(c.status, "completed")
        XCTAssertEqual(c.aggregatedOutput, "a\nb")
        XCTAssertEqual(c.actions, [])
    }

    func testDecodeOfficialCommandActionsForNaturalLanguageActivity() throws {
        let item = try decode(#"{"id":"item_actions","type":"commandExecution","command":"rg foo Sources && sed -n '1,20p' Sources/a.swift","cwd":"/repo","status":"completed","commandActions":[{"type":"search","command":"rg foo Sources","query":"foo","path":"Sources"},{"type":"read","command":"sed -n '1,20p' Sources/a.swift","name":"a.swift","path":"Sources/a.swift"}],"aggregatedOutput":null,"exitCode":0}"#)
        guard case let .commandExecution(command) = item.kind else {
            return XCTFail("expected commandExecution")
        }
        XCTAssertEqual(command.actions.map(\.kind), [.search, .read])
        XCTAssertEqual(command.actions[0].query, "foo")
        XCTAssertEqual(command.actions[1].name, "a.swift")
    }

    func testDecodeReasoningArrays() throws {
        // real: summary AND content are [String], not String
        let item = try decode(#"{"id":"r1","type":"reasoning","summary":["plan a","plan b"],"content":["thinking..."]}"#)
        guard case let .reasoning(summary, content) = item.kind else { return XCTFail("expected reasoning") }
        XCTAssertEqual(summary, "plan a\nplan b")
        XCTAssertEqual(content, "thinking...")
    }

    func testDecodeReasoningBareStringFallback() throws {
        // defensive: a bare-string variant still decodes (back-compat)
        let item = try decode(#"{"id":"r2","type":"reasoning","summary":"plan","content":"thinking..."}"#)
        guard case let .reasoning(summary, content) = item.kind else { return XCTFail("expected reasoning") }
        XCTAssertEqual(summary, "plan")
        XCTAssertEqual(content, "thinking...")
    }

    func testMcpToolCallUsesServerAndTool() throws {
        // real mcp: { server, tool, status, arguments } — no `name`
        let item = try decode(#"{"id":"t1","type":"mcpToolCall","server":"crew","tool":"post_to_crew","status":"completed","arguments":{}}"#)
        guard case let .toolCall(name, status) = item.kind else { return XCTFail("expected toolCall") }
        XCTAssertEqual(name, "crew.post_to_crew")
        XCTAssertEqual(status, "completed")
    }

    func testUnknownArmDegradesNotCrashes() throws {
        let item = try decode(#"{"id":"item_9","type":"someFutureType","blob":42}"#)
        guard case let .unknown(type) = item.kind else { return XCTFail("expected unknown") }
        XCTAssertEqual(type, "someFutureType")
        XCTAssertEqual(item.id, "item_9")
    }

    func testNewArmsFallThroughToUnknown() throws {
        // hookPrompt / imageView / imageGeneration / review modes / contextCompaction
        // are real 0.137.0 arms not modelled in the reduced view — must degrade, not crash.
        for t in ["hookPrompt", "imageView", "imageGeneration", "enteredReviewMode", "contextCompaction"] {
            let item = try decode("{\"id\":\"x\",\"type\":\"\(t)\"}")
            guard case let .unknown(type) = item.kind else { return XCTFail("expected unknown for \(t)") }
            XCTAssertEqual(type, t)
        }
    }

    func testMissingIdGetsSyntheticIdNotCrash() throws {
        let item = try decode(#"{"type":"agentMessage","text":"hi"}"#)
        XCTAssertFalse(item.id.isEmpty)   // synthesized, decode must not throw
    }

    func testDecodeUserMessageFromContentArray() throws {
        // real: { type:"userMessage", id, clientId, content:[UserInput] }; text lives in content
        let item = try decode(#"{"id":"u1","type":"userMessage","clientId":null,"content":[{"type":"text","text":"Hello!","text_elements":[]}]}"#)
        guard case let .userMessage(text) = item.kind else { return XCTFail("expected userMessage") }
        XCTAssertEqual(text, "Hello!")
    }

    func testDecodeUserMessageJoinsMultipleTextParts() throws {
        let item = try decode(#"{"id":"u2","type":"userMessage","content":[{"type":"text","text":"line 1","text_elements":[]},{"type":"image","url":"x"},{"type":"text","text":"line 2","text_elements":[]}]}"#)
        guard case let .userMessage(text) = item.kind else { return XCTFail("expected userMessage") }
        XCTAssertEqual(text, "line 1\nline 2")   // image part dropped from reduced view
    }

    func testDecodePlan() throws {
        let item = try decode(#"{"id":"p1","type":"plan","text":"Step 1: do X"}"#)
        guard case let .plan(text) = item.kind else { return XCTFail("expected plan") }
        XCTAssertEqual(text, "Step 1: do X")
    }

    func testDecodeFileChangeFromChangesArray() throws {
        // real: { type:"fileChange", id, changes:[{path,kind,diff}], status } — no `summary`
        let item = try decode(#"{"id":"f1","type":"fileChange","status":"completed","changes":[{"path":"README.md","kind":{"type":"update","move_path":null},"diff":"@@"},{"path":"src/a.swift","kind":{"type":"add"},"diff":"@@"}]}"#)
        guard case let .fileChange(fc) = item.kind else { return XCTFail("expected fileChange") }
        XCTAssertEqual(fc.status, "completed")
        XCTAssertEqual(fc.summary, "README.md, src/a.swift")   // derived from changes[].path
    }

    func testDecodeWebSearch() throws {
        let item = try decode(#"{"id":"w1","type":"webSearch","query":"swift codable","action":null}"#)
        guard case let .webSearch(query) = item.kind else { return XCTFail("expected webSearch") }
        XCTAssertEqual(query, "swift codable")
    }

    func testDynamicToolCallUsesTool() throws {
        // real dynamic: { namespace: string|null, tool, status, arguments } — no `name`
        let item = try decode(#"{"id":"d1","type":"dynamicToolCall","namespace":null,"tool":"run_script","status":"inProgress","arguments":{}}"#)
        guard case let .toolCall(name, status) = item.kind else { return XCTFail("expected toolCall") }
        XCTAssertEqual(name, "run_script")
        XCTAssertEqual(status, "inProgress")
    }

    func testDynamicToolCallWithNamespace() throws {
        let item = try decode(#"{"id":"d2","type":"dynamicToolCall","namespace":"local","tool":"run_script","status":"completed"}"#)
        guard case let .toolCall(name, _) = item.kind else { return XCTFail("expected toolCall") }
        XCTAssertEqual(name, "local/run_script")
    }

    func testCollabAgentToolCallUsesTool() throws {
        // real collab: { tool: CollabAgentTool (string enum), status, senderThreadId, receiverThreadIds, ... }
        let item = try decode(#"{"id":"ca1","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","senderThreadId":"thr_1","receiverThreadIds":[]}"#)
        guard case let .toolCall(name, status) = item.kind else { return XCTFail("expected toolCall") }
        XCTAssertEqual(name, "spawnAgent")
        XCTAssertEqual(status, "inProgress")
    }

    func testAgentMessageMissingPhaseIsNil() throws {
        let item = try decode(#"{"id":"a2","type":"agentMessage","text":"Working...","phase":null}"#)
        guard case let .agentMessage(text, phase) = item.kind else { return XCTFail("expected agentMessage") }
        XCTAssertEqual(text, "Working...")
        XCTAssertNil(phase)
    }

    func testCommandExecutionMissingExitCodeIsNil() throws {
        let item = try decode(#"{"id":"cmd1","type":"commandExecution","command":"echo hi","cwd":"/r","status":"inProgress","commandActions":[]}"#)
        guard case let .commandExecution(c) = item.kind else { return XCTFail("expected commandExecution") }
        XCTAssertEqual(c.command, "echo hi")
        XCTAssertNil(c.exitCode)
    }
}
