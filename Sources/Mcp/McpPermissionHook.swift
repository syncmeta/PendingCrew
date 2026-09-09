import Foundation

/// 权限审批 PreToolUse hook（spec 2026-06-08-pendingcrew-ask-approval-design §3「权限类」）。
///
/// claude 的 PreToolUse hook 把 `{tool_name, tool_input, tool_use_id, session_id, …}`
/// 喂到 stdin（spike 6 实测）。本 hook 判断该工具是否需人工授权（`gates` 子串命中
/// tool_name）：
/// - **命中** → raise 一条 `permission` 待审批 + **阻塞 long-poll** 直到人类在待审批
///   列表 allow/deny，再吐 PreToolUse 的 `permissionDecision` 输出拦截（deny）/ 放行
///   （allow）该工具。
/// - **不命中** → 返回 nil（caller 不输出 → claude 正常流程；auto mode 下自动放行）。
///
/// 与 `ask`（决策类经 captain）不同，**权限类绕过 captain 直达人类**：由 PreToolUse
/// 这一关直接拦截，不进群聊、不等 captain 转交。
///
/// **自包含 Foundation**（编进 `pendingcrew-mcp` re-exec helper + PendingCrewTests bundle）。
final class McpPermissionHook {
    let approvals: LocalApprovalStore
    let crewId: String
    /// **本地** session id（= startSession 的 localSessionId，经 `--session` argv 传入）。
    /// 待审批必须归档在这个 id 下，右栏内联卡片才按 `run.sessionId` 过滤得到 ——
    /// **不能**用 PreToolUse hook stdin 里 claude 自己的 `session_id`（另一个 UUID 空间，
    /// 卡片会过滤不到、压根不显示）。
    let sessionId: String
    /// 命中任一（子串匹配 tool_name）即需人工授权。空 → 不 gate 任何工具。
    let gates: [String]
    /// 可选：raise 待审批时往本地群聊白板贴一条通知（spec §6 通知半边，v1 降级 reporter）。
    let board: LocalWhiteboardStore?
    /// 人类 Todo（`.human` 那本）—— 权限请求现在提到这儿（#75 ②）。
    let todos: LocalTodoStore?
    /// 一次性放行票。人同意那条 Todo 之后由 app 侧写，这里读并当场作废。
    ///
    /// **必填，故意不给默认值。** 给一个 `PermissionGrantStore()` 的默认值就意味着
    /// 漏传时静默落到**真实数据目录** —— 那正是 2026-09-08/09 连着栽两次的形状
    /// （见 `McpServerTestDirectoryContractTests`）。这里让编译器替我们看住。
    let grants: PermissionGrantStore

    init(approvals: LocalApprovalStore, crewId: String, sessionId: String = "",
         gates: [String], board: LocalWhiteboardStore? = nil,
         todos: LocalTodoStore? = nil, grants: PermissionGrantStore) {
        self.approvals = approvals
        self.crewId = crewId
        self.sessionId = sessionId
        self.gates = gates
        self.board = board
        self.todos = todos
        self.grants = grants
    }

    /// 处理一条 PreToolUse hook stdin JSON。
    /// - 需授权 → raise + 阻塞拿决定 → 返回 hook 输出 JSON 字符串（allow/deny）。
    /// - 不需授权 → nil（caller 不输出，走 claude 正常流程）。
    /// `maxWaits` 仅给单测（到点保守判 deny）；`pollInterval` 单测可调小。
    func handle(_ stdinJson: String, pollInterval: TimeInterval = 0.5, maxWaits: Int? = nil) -> String? {
        guard let data = stdinJson.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let toolName = obj["tool_name"] as? String
        else { return nil }

        guard gates.contains(where: { !$0.isEmpty && toolName.contains($0) }) else { return nil }

        // 驾驶舱计划 #75 ②：**不再阻塞等人**。
        //
        // 旧路：raise 一条 `kind: "permission"` 待审批 → long-poll 最多一小时 → 到点判 deny。
        // 那是人类反复问的「怎么又停了」的另一半。
        //
        // 新路：当场拒 + 提一条人类 Todo 说清「我要跑 X、为了 Y」，agent 去干别的，
        // 人同意之后留下一张一次性票，它重跑时见票放行。
        let summary = permissionSummary(toolName: toolName, toolInput: obj["tool_input"])
        let pending = todos?.list(crewId: crewId).contains {
            !$0.isDeleted && $0.status != "completed" && $0.permissionTool == toolName
        } ?? false
        switch PermissionRequestFlow.decide(
            hasGrant: grants.consume(crewId: crewId, tool: toolName),
            hasPendingRequest: pending) {
        case .allowConsumingGrant:
            // 票已经在上面的 `consume` 里作废了 —— 一次同意 = 一次放行。
            return hookOutput(decision: "allow", toolName: toolName)
        case .denyWithoutFiling:
            // **这一支是承重点**：同一个工具已经有一条挂着的请求，再提就是往人的账上
            // 灌垃圾（agent 每重试一次加一条）。只拒，不写。
            return hookOutput(decision: "deny", toolName: toolName)
        case .denyAndFile:
            guard let todos, let item = todos.add(
                crewId: crewId, text: "我要跑 `\(toolName)`：\(summary)\n同意的话回复这条（回「可以 / 同意」即放行一次），不同意就说不行。",
                bySessionId: sessionId, bySenderName: nil,
                resumeNote: "人同意之后重跑 \(toolName)：\(summary)",
                expectsResume: true,
                permissionTool: toolName) else {
                // 账本没落盘：如实说，并保守判 deny 让 turn 继续。
                board?.appendSessionMessage(
                    crewId: crewId, sessionId: sessionId,
                    text: "要跑 `\(toolName)` 需要你放行，但这条没能记进人类 Todo"
                        + "（账本这次读不出来或漏读，原有内容没被动过），已代为回绝：\(summary)",
                    category: "question",
                    mentions: [LocalWhiteboardMention(kind: "human", targetId: nil),
                               LocalWhiteboardMention(kind: "captain", targetId: nil)])
                return hookOutput(decision: "deny", toolName: toolName)
            }
            board?.appendSessionMessage(
                crewId: crewId, sessionId: sessionId,
                text: "人类 To do +1: #\(item.number) 要跑 `\(toolName)` 需要你放行：\(summary)",
                category: "question",
                mentions: [LocalWhiteboardMention(kind: "human", targetId: nil),
                           LocalWhiteboardMention(kind: "captain", targetId: nil)])
            return hookOutput(decision: "deny", toolName: toolName)
        }
    }

    // `awaitDecision` 与那条一小时 long-poll 已随 #75 ② 一起删掉。
    // 它存在的意义是「阻塞到人来审」，而现在权限走 Todo、当场返回 —— 留着就是
    // 一条永远没人调的等待路径，还会让下一个人以为这里仍然会等人。


    /// 待审批摘要：工具名 + 关键入参（command / url / path 之一，截断）。
    private func permissionSummary(toolName: String, toolInput: Any?) -> String {
        var detail = ""
        if let inp = toolInput as? [String: Any] {
            for k in ["command", "url", "path", "file_path", "prompt"] {
                if let v = inp[k] as? String, !v.isEmpty { detail = " · \(k)=\(String(v.prefix(120)))"; break }
            }
        }
        return "请求使用 \(toolName)\(detail)"
    }

    /// PreToolUse hook 输出（spike 6 实测形状）。
    private func hookOutput(decision: String, toolName: String) -> String {
        let reason = decision == "allow" ? "人类已批准 \(toolName)" : "人类拒绝 \(toolName)"
        let dict: [String: Any] = ["hookSpecificOutput": [
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        ]]
        guard let d = try? JSONSerialization.data(withJSONObject: dict),
              let s = String(data: d, encoding: .utf8) else { return "" }
        return s
    }
}
