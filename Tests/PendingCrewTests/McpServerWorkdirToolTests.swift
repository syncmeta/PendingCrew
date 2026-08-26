import XCTest
// McpServer.swift + LocalCrewControlStore.swift 编进 test bundle（见 project.yml）。

/// 机长 `change_workdir` 的 helper 侧单测：只机长可见可调、参数卫生、命令落盘字段
/// （预览 vs 执行、目标 crew、连不连子群）、以及超时那句必须留活口。
/// app 侧的规划/执行归 `WorkdirMigrationPlan/Executor` 那两套单测。
final class McpServerWorkdirToolTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcp-workdir-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL, isCaptain: Bool) -> McpServer {
        let s = McpServer(store: LocalWhiteboardStore(directory: dir),
                          approvals: LocalApprovalStore(directory: dir),
                          control: LocalCrewControlStore(directory: dir),
                          crewId: "c", sessionId: "cap-1", isCaptain: isCaptain)
        s.commandResponsePollInterval = 0.005
        s.commandResponseMaxWaits = 2
        return s
    }

    private func call(_ s: McpServer, args: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"change_workdir","arguments":\(args)}}
        """) ?? ""
    }

    func testListedOnlyForCaptain() {
        let cap = server(tempDir(), isCaptain: true)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertTrue(cap.contains("change_workdir"))
        // 工具描述必须把这几件事说在明处，否则机长会用错。
        XCTAssertTrue(cap.contains("预览"))
        // 会话日志不搬了 → 这个工具**一次做完、没有尾巴**。描述里必须把这句说明白，
        // 并给出依据（claude `--resume` 按会话号找全盘、不按目录），否则机长看到
        // 「改目录居然不用管在跑的 session」会本能怀疑，还得再问一遍。
        XCTAssertTrue(cap.contains("一次做完"), "描述要讲明没有第二趟")
        XCTAssertTrue(cap.contains("--resume"), "要给出「不用搬会话」的依据")
        // 反面：清扫模式删掉之后，这些词会变成**指向虚空的指令** —— 机长照着再调一次，
        // 什么也不会发生，而它以为自己补上了什么。过期的工具描述比死代码更能骗人。
        for stale in ["留待清扫", "幂等", "再调一次"] {
            XCTAssertFalse(cap.contains(stale), "工具描述里不该再出现「\(stale)」")
        }
        let worker = server(tempDir(), isCaptain: false)
            .handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertFalse(worker.contains("change_workdir"))
    }

    func testWorkerCannotCallAndNothingIsEnqueued() {
        let dir = tempDir()
        let out = call(server(dir, isCaptain: false), args: #"{"new_path":"/tmp"}"#)
        XCTAssertTrue(out.contains("仅机长可用"), out)
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty,
                      "被拒的调用不该落命令")
    }

    func testEmptyPathRejectedWithoutEnqueue() {
        let dir = tempDir()
        let out = call(server(dir, isCaptain: true), args: #"{"new_path":"   "}"#)
        XCTAssertTrue(out.contains("new_path 不能为空"), out)
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }

    /// 不带 confirm = 预览。命令落盘时 `confirm` 必须是 false，否则机长一调就动手了。
    func testDefaultsToPreviewAndIncludesChildren() {
        let dir = tempDir()
        let out = call(server(dir, isCaptain: true), args: #"{"new_path":"/tmp/newdir"}"#)
        XCTAssertTrue(out.contains("超时无应答"), out)
        // app 不在跑时要留活口：命令可能仍在执行，去群里看回执。
        XCTAssertTrue(out.contains("群里"), out)
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "change_workdir")
        XCTAssertEqual(cmds[0].path, "/tmp/newdir")
        XCTAssertEqual(cmds[0].confirm, false, "没带 confirm 就绝不能落成执行")
        XCTAssertEqual(cmds[0].includeChildren, true, "默认连子 crew 一起迁")
        XCTAssertEqual(cmds[0].sessionId, "cap-1", "发起者要带上——它自己不算拦路")
        XCTAssertNil(cmds[0].title, "没指定 crew = 本 crew")
    }

    func testConfirmAndTargetAndChildrenFlagsRoundTrip() {
        let dir = tempDir()
        _ = call(server(dir, isCaptain: true),
                 args: #"{"new_path":"/tmp/x","crew":"驾驶舱改造","include_children":false,"confirm":true}"#)
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].confirm, true)
        XCTAssertEqual(cmds[0].includeChildren, false)
        XCTAssertEqual(cmds[0].title, "驾驶舱改造")
    }
}
