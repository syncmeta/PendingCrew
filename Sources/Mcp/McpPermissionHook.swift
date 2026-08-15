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

    init(approvals: LocalApprovalStore, crewId: String, sessionId: String = "",
         gates: [String], board: LocalWhiteboardStore? = nil) {
        self.approvals = approvals
        self.crewId = crewId
        self.sessionId = sessionId
        self.gates = gates
        self.board = board
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

        // 归档用**本地** sessionId（self.sessionId），不是 stdin 里 claude 的 session_id。
        let summary = permissionSummary(toolName: toolName, toolInput: obj["tool_input"])
        guard let reqId = approvals.raise(
            crewId: crewId, kind: "permission", sessionId: sessionId, summary: summary) else {
            // 审批账本没落盘（读不出来 / 漏读，白板上有系统警示）。再去 long-poll
            // 只会对着一条磁盘上不存在的待审批干等一小时。保守判 deny 让 turn 继续，
            // 并把真正的原因说出来（#577）。措辞不说「已归档」—— 读不出来时原件
            // 一字未动、不产生归档（2026-08-12）。
            board?.appendSessionMessage(
                crewId: crewId, sessionId: sessionId,
                text: "待审批没能记进审批账本（文件这次读不出来或漏读，原有内容没被动过），已代为回绝："
                    + "\(summary)\n审批列表这会儿不可用，修好后让它重试。",
                category: "question",
                mentions: [LocalWhiteboardMention(kind: "human", targetId: nil),
                           LocalWhiteboardMention(kind: "captain", targetId: nil)])
            return hookOutput(decision: "deny", toolName: toolName)
        }
        // 通知半边（spec §6）：贴到本地群聊白板 + **@ 到能处理的人**。审批只有人类能
        // allow/deny，所以 @human 是主（人默认不进 session 详情，全靠群里这条 @ 才会注意到、
        // 去详情里的审批卡处理）；@captain 兜底让机长知道有 session 卡在审批、可催人。
        board?.appendSessionMessage(crewId: crewId, sessionId: sessionId,
                                    text: "待审批：\(summary)\n（去该 session 详情的审批卡 allow / deny）",
                                    category: "question",
                                    mentions: [LocalWhiteboardMention(kind: "human", targetId: nil),
                                               LocalWhiteboardMention(kind: "captain", targetId: nil)])
        let decision = awaitDecision(reqId: reqId, pollInterval: pollInterval, maxWaits: maxWaits)
        if decision == "timedOut" {
            // 到点没人审 —— **说出来**（Todo #6）。此前这里是无限干等：没人审就永远
            // 挂着，群里也没有第二句话，这个 session 就此静默死掉。现在保守判 deny
            // 让 turn 继续（agent 拿到「被拒绝」自己决定绕路还是求助），并把这件事
            // 亮到群里，别让「等到没人管」变成一次无人知晓的失踪。
            board?.appendSessionMessage(
                crewId: crewId, sessionId: sessionId,
                text: "等了 \(Int(Double(Self.defaultMaxWaits) * pollInterval / 60)) 分钟没人审批，已代为回绝："
                    + "\(summary)\n它会带着「被拒绝」继续跑，要放行就让它重试。",
                category: "question",
                mentions: [LocalWhiteboardMention(kind: "human", targetId: nil),
                           LocalWhiteboardMention(kind: "captain", targetId: nil)])
            return hookOutput(decision: "deny", toolName: toolName)
        }
        return hookOutput(decision: decision, toolName: toolName)
    }

    /// 不设 `maxWaits` 时的兜底上限（× `pollInterval` 0.5s = 1 小时）。
    /// 「一直等人审」听着稳妥，实际是让 session 无限期静默挂死 —— 有个上限 +
    /// 到点亮出来，比永远悬着诚实。
    static let defaultMaxWaits = 7200

    /// 阻塞 long-poll 直到 permission 待审批被 decide。
    /// 返回 "allow" / "deny"，或到点未决的 "timedOut"（调用方负责亮出来 + 判 deny）。
    /// `maxWaits` 单测可传 0 立即到点；生产不传则走 `defaultMaxWaits`。
    func awaitDecision(reqId: String, pollInterval: TimeInterval = 0.5, maxWaits: Int? = nil) -> String {
        let cap = maxWaits ?? Self.defaultMaxWaits
        var waits = 0
        while true {
            if let it = approvals.item(crewId: crewId, id: reqId), it.status == "answered" {
                return it.decision == "allow" ? "allow" : "deny"
            }
            if waits >= cap { return "timedOut" }
            waits += 1
            Thread.sleep(forTimeInterval: pollInterval)
        }
    }

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
