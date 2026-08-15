import XCTest
// McpServer.swift + LocalCrewControlStore.swift 编进 test bundle（见 project.yml）。

/// 机长 session 工具（inspect_session / nudge_session / stop_session）helper 侧单测：
/// 注册（仅机长可见）、captain 门、命令落盘字段、应答文件 long-poll。
/// app 侧执行（runner.applySessionOp 的终端快照/按键注入）依赖活 run，归 #443 活验。
final class McpServerSelfHealToolsTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("mcp-heal-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }
    private func server(_ dir: URL, isCaptain: Bool) -> McpServer {
        let s = McpServer(store: LocalWhiteboardStore(directory: dir),
                          approvals: LocalApprovalStore(directory: dir),
                          control: LocalCrewControlStore(directory: dir),
                          crewId: "c", sessionId: "cap-1", isCaptain: isCaptain)
        // 单测不等真超时（默认 ~10s）。
        s.commandResponsePollInterval = 0.01
        s.commandResponseMaxWaits = 3
        return s
    }
    private func call(_ s: McpServer, tool: String, args: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"\(tool)","arguments":\(args)}}
        """) ?? ""
    }

    func testToolsListedOnlyForCaptain() {
        let cap = server(tempDir(), isCaptain: true)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertTrue(cap.contains("inspect_session"))
        XCTAssertTrue(cap.contains("nudge_session"))
        XCTAssertTrue(cap.contains("stop_session"))
        XCTAssertTrue(cap.contains("操作不可撤销"))
        XCTAssertTrue(cap.contains("Esc 只打断当前一轮"))
        XCTAssertTrue(cap.contains("进程仍然存活"))
        let worker = server(tempDir(), isCaptain: false)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertFalse(worker.contains("inspect_session"))
        XCTAssertFalse(worker.contains("nudge_session"))
        XCTAssertFalse(worker.contains("stop_session"))
    }

    func testWorkerCannotCall() {
        let dir = tempDir()
        let s = server(dir, isCaptain: false)
        XCTAssertTrue(call(s, tool: "inspect_session", args: #"{"session_id":"w1"}"#).contains("仅机长可用"))
        XCTAssertTrue(call(s, tool: "nudge_session", args: #"{"session_id":"w1","input":"Enter"}"#).contains("仅机长可用"))
        XCTAssertTrue(call(s, tool: "stop_session", args: #"{"session_id":"w1","reason":"取消"}"#).contains("仅机长可用"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty, "被拒的调用不该落命令")
    }

    func testInspectEnqueuesCommandAndTimesOutWithoutApp() {
        let dir = tempDir()
        let s = server(dir, isCaptain: true)
        let out = call(s, tool: "inspect_session", args: #"{"session_id":"worker-abc"}"#)
        // app 不在跑 → 超时提示（不静默）。
        XCTAssertTrue(out.contains("超时无应答"), out)
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "inspect_session")
        XCTAssertEqual(cmds[0].crewId, "c")
        XCTAssertEqual(cmds[0].sessionId, "worker-abc")
    }

    func testNudgeEnqueuesCommandWithInput() {
        let dir = tempDir()
        let s = server(dir, isCaptain: true)
        _ = call(s, tool: "nudge_session", args: #"{"session_id":"worker-abc","input":"Enter"}"#)
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "nudge_session")
        XCTAssertEqual(cmds[0].sessionId, "worker-abc")
        XCTAssertEqual(cmds[0].note, "Enter")
    }

    func testStopEnqueuesTargetReasonAndRequester() {
        let dir = tempDir()
        let s = server(dir, isCaptain: true)
        let out = call(s, tool: "stop_session", args: #"{"session_id":"worker-abc","reason":"任务取消"}"#)
        XCTAssertTrue(out.contains("超时无应答"), out)
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "stop_session")
        XCTAssertEqual(cmds[0].crewId, "c")
        XCTAssertEqual(cmds[0].sessionId, "worker-abc")
        XCTAssertEqual(cmds[0].note, "任务取消")
        XCTAssertEqual(cmds[0].requesterSessionId, "cap-1")
    }

    func testEmptyArgsRejected() {
        let s = server(tempDir(), isCaptain: true)
        XCTAssertTrue(call(s, tool: "inspect_session", args: #"{"session_id":""}"#).contains("ERROR"))
        XCTAssertTrue(call(s, tool: "nudge_session", args: #"{"session_id":"w1","input":"  "}"#).contains("ERROR"))
        XCTAssertTrue(call(s, tool: "stop_session", args: #"{"session_id":"w1","reason":"  "}"#).contains("ERROR"))
    }

    func testAwaitPicksUpResponseFile() {
        // 应答半边：app 写应答 → helper long-poll 取到（读到即删，一次性消费）。
        let dir = tempDir()
        let control = LocalCrewControlStore(directory: dir)
        let s = server(dir, isCaptain: true)
        s.commandResponseMaxWaits = 50
        let cmdId = control.enqueueInspectSession(crewId: "c", targetSessionId: "w1")
        control.writeCommandResponse(crewId: "c", commandId: cmdId, text: "「w1」状态：空闲")
        XCTAssertEqual(s.awaitCommandResponse(commandId: cmdId), "「w1」状态：空闲")
        XCTAssertNil(control.takeCommandResponse(crewId: "c", commandId: cmdId), "应答读后即删")
    }
}
