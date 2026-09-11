import Foundation

/// crew-comms helper 的进程入口逻辑（re-exec self；spec local-first chunk 4）。
/// 由 `PendingCrewEntry.main()` 在 argv 带 helper flag 时调用，跑完即返回。
///
/// 子命令（flag 形式，因为是同一 app 二进制的 argv，不是子进程参数）：
///   `--mcp-serve --crew <id> --dir <path> --session <id> [--captain] [--agent claude|codex]` → MCP stdio server
///   `--mcp-hook  --crew <id> --dir <path> --session <id>`           → 吐未读白板的 PostToolUse hook JSON
///   `--mcp-permission-hook --crew <id> --dir <path> --gate <a,b>`   → 权限审批 PreToolUse hook
///   `--mcp-turn-hook --crew <id> --dir <path> --session <id>`       → 回合结束 Stop hook（#25 留痕）
enum McpHelperMain {
    /// argv 含 helper flag → 跑对应模式、返回 true（caller 应 return，不起 GUI）。
    /// 不含 → 返回 false（正常起 GUI）。
    @discardableResult
    static func runIfHelper(_ args: [String]) -> Bool {
        let serve = args.contains("--mcp-serve")
        let hook = args.contains("--mcp-hook")
        let permHook = args.contains("--mcp-permission-hook")
        let turnHook = args.contains("--mcp-turn-hook")
        guard serve || hook || permHook || turnHook else { return false }

        let crewId = value("--crew", args) ?? ""
        let sessionId = value("--session", args) ?? ""
        let dir = value("--dir", args).map { URL(fileURLWithPath: $0) }
        let store = LocalWhiteboardStore(directory: dir)

        if serve {
            // newline-delimited JSON-RPC over stdin/stdout（MCP stdio）。
            // `--captain` → 解锁机长专用 answer_decision 工具（chunk2 T4）。
            // `--label` → post_to_crew 写白板时的发送者显示名（白板不再裸 uuid）。
            // `--agent claude|codex` → 本 session 跑在哪家 runner 上，供
            // set_session_profile 挑对照哪张可用模型表（Todo #37）。没传就两家都对照。
            let captain = args.contains("--captain")
            let label = value("--label", args)
            let agent = value("--agent", args).flatMap { ["claude", "codex"].contains($0) ? $0 : nil }
            let server = McpServer(store: store, approvals: LocalApprovalStore(directory: dir),
                                   control: LocalCrewControlStore(directory: dir),
                                   crewId: crewId, sessionId: sessionId, isCaptain: captain,
                                   sessionLabel: label,
                                   quotaDirectory: dir,
                                   todos: LocalTodoStore(directory: dir),
                                   plans: CockpitPlanStore(directory: dir),
                                   agentKey: agent)
            McpHelperServeLoop(handle: { server.handleLine($0) }).run()
        } else if permHook {
            // PreToolUse hook：gate 命中的工具 → raise 待审批 + 阻塞 long-poll allow/deny，
            // 吐 permissionDecision 拦截/放行。不命中 → 无输出（走 claude 正常流程）。
            // stdin 是单个 hook JSON 对象（一次读到 EOF）。
            let gates = (value("--gate", args) ?? "").split(separator: ",").map(String.init).filter { !$0.isEmpty }
            // #75 ②：权限请求提到**人类 Todo**、放行票走 `PermissionGrantStore`，
            // 两个都跟着 `--dir` 走 —— 不给就会静默落到真实数据目录。
            let permission = McpPermissionHook(approvals: LocalApprovalStore(directory: dir),
                                               crewId: crewId, sessionId: sessionId,
                                               gates: gates, board: store,
                                               todos: LocalTodoStore(directory: dir, ledger: .human),
                                               grants: PermissionGrantStore(directory: dir))
            let data = FileHandle.standardInput.readDataToEndOfFile()
            let input = String(data: data, encoding: .utf8) ?? ""
            if let out = permission.handle(input) {
                print(out)
                fflush(stdout)
            }
        } else if turnHook {
            // Stop hook：这一轮它一次都没往群里说过话 → 系统替它把收尾话头发进群（#25）。
            // 永不阻塞 agent：只做副作用，无输出。
            let turn = McpTurnHook(board: store, crewId: crewId, sessionId: sessionId,
                                   sessionLabel: value("--label", args),
                                   isCaptain: args.contains("--captain"),
                                   markerDirectory: store.resolvedDirectory)
            let data = FileHandle.standardInput.readDataToEndOfFile()
            turn.handle(String(data: data, encoding: .utf8) ?? "")
        } else {
            // PostToolUse hook：吐本 session 未读白板（带"可信"提示）。无未读 → 不输出。
            // `--captain` → 注入多带全机 crew 组织树概览（#24 机长视野）。
            let emitter = HookEmitter(store: store, crewId: crewId, sessionId: sessionId,
                                      cursorDir: store.resolvedDirectory,
                                      isCaptain: args.contains("--captain"))
            if let out = emitter.emitAndAdvance() {
                print(out)
                fflush(stdout)
            }
        }
        return true
    }

    private static func value(_ name: String, _ args: [String]) -> String? {
        guard let i = args.firstIndex(of: name), i + 1 < args.count else { return nil }
        return args[i + 1]
    }
}
