import Foundation

/// claude 的 **Stop** hook（agent 结束一轮时触发）—— 人类 Todo #25 层 1 的 claude 半边。
///
/// 官方 payload（code.claude.com/docs/en/hooks，2026-08 核对，**不是凭印象写的字段名**）：
/// `{ hook_event_name: "Stop", session_id, prompt_id, transcript_path, cwd, permission_mode,
///    last_assistant_message, … }`。
/// - `last_assistant_message` = 本轮最后一条 assistant 正文，文档明确要求**用它而不是读
///   `transcript_path`** —— transcript 异步落盘，会滞后于内存里的当前轮。拿不到正文时
///   本 hook 直接不发（宁可漏一次，别瞎发）。
/// - `prompt_id` 唯一标识这一轮（v2.1.196+），拿它做同轮去重；老版本没有就退化成靠
///   「代发的那条自己就是痕迹」兜着。
/// - 输出：本 hook **永远不阻塞** —— 不吐 `decision: "block"`，只做副作用后正常退出。
///   把 agent 强行留在轮内会把「等人」变成「空转」，与本功能的目的正相反。
///
/// codex 半边挂在 `CodexAppServerBackend` 的 `turn/completed` 上，共用 `SessionTurnTrace`。
struct McpTurnHook {
    let board: LocalWhiteboardStore
    let crewId: String
    /// **本地** session id（`--session` argv 传入）—— 不是 stdin 里 claude 自己的
    /// `session_id`（另一个 uuid 空间，用了群里这条就归错人、@ 也点不到）。
    let sessionId: String
    let sessionLabel: String?
    let isCaptain: Bool
    let markerDirectory: URL

    /// 处理一条 Stop hook stdin JSON。返回是否代发了一条群消息（供单测/调试）。
    @discardableResult
    func handle(_ stdinJson: String) -> Bool {
        guard let data = stdinJson.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return false }
        // 子 agent 结束走的是 SubagentStop，不该按 session 留痕算账（它没有自己的群身份）。
        if let ev = obj["hook_event_name"] as? String, ev != "Stop" { return false }
        if obj["agent_id"] != nil { return false }

        let marker = SessionTurnMarker(directory: markerDirectory, crewId: crewId, sessionId: sessionId)
        let prev = marker.read()
        let messages = board.list(crewId: crewId)
        let turnId = obj["prompt_id"] as? String

        let lastAgentText = obj["last_assistant_message"] as? String ?? ""
        let post = SessionTurnTrace.decide(.init(
            messages: messages,
            sessionId: sessionId,
            sessionName: (sessionLabel?.isEmpty == false) ? sessionLabel! : sessionId,
            isCaptain: isCaptain,
            sinceMessageId: prev.lastMessageId,
            lastHandledTurnId: prev.lastTurnId,
            turnId: turnId,
            lastAssistantMessage: lastAgentText))

        if let post {
            board.appendSessionMessage(
                crewId: crewId, sessionId: sessionId, text: post.text, category: "question",
                senderName: sessionLabel,
                mentions: post.mentionKinds.map { LocalWhiteboardMention(kind: $0, targetId: nil) })
        }

        // 记账无论发没发都推进：本轮边界 = 现在的白板末条（代发的那条已在其中）。
        // `awaitingQuestion` 每轮重写（层 2）——这轮不是停在问句上就写 nil，红点自然熄。
        marker.write(.init(lastMessageId: board.list(crewId: crewId).last?.id ?? prev.lastMessageId,
                           lastTurnId: turnId ?? prev.lastTurnId,
                           lastAssistantMessage: lastAgentText,
                           awaitingQuestion: SessionTurnTrace.trailingQuestion(from: lastAgentText)))
        // The tool only arms. Stop is the authoritative boundary that makes the
        // promise runnable, so directory activity during tool execution cannot
        // start a second prompt inside the current turn.
        SessionContinuationStore(directory: markerDirectory).finishTurn(
            crewId: crewId, sessionId: sessionId, outcome: .continuing)
        return post != nil
    }
}
