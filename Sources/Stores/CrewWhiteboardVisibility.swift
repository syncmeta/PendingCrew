import Foundation

/// 一条白板消息该不该进**某个 session 的 agent 注入面** —— 全项目唯一判定（#543）。
///
/// 2026-07-26 事故：机长 `post_to_crew(mentions: [{kind:"session", target_id:"worker-…"}])`
/// 定向派一件活，同 crew 三个 worker 全把它当成「派给我的」认领，三方撞车。根因**不在**
/// 唤醒路 —— `CrewLocalMentionWakeLogic` / `CrewLocalMentionInjectLogic` 一直按 targetId
/// 精确过滤，只叫醒目标。病在**白板未读注入路**（`HookEmitter`，每轮 PostToolUse /
/// codex turn 把未读塞进上下文）：它把游标之后的**全部**未读原样渲染给每个 session，
/// 定向 @ 与广播长得一模一样，非目标 worker 从注入面上根本看不出这条不是给自己的。
/// 于是「唤醒面正确、注入面扩散」，三条路（唤醒 / 收听 / 白板注入）判定标准不一致。
///
/// 本类型把标准收成一份，三条路共用（`CrewListenLogic.deliverable` 早已是这个语义，
/// 现在改成调它，不再各写一遍）：
///   * **无 mentions = 广播** → 谁都看得到（唤醒面不变，广播语义原样）。
///   * **有 mentions = 定向** → 只有被点到的看得到：`kind == "session"` 配 sessionId，
///     `kind == "captain"` 配机长 session。`kind == "human"` 是给人看的标记，不进任何
///     agent 的注入面。
///   * 自己发的那条永远对自己可见（否则自己的定向 @ 在自己上下文里凭空消失）。
///
/// 「看不到」只针对 agent 注入面：人类在 app 白板上仍看得到全部消息，session 想看全景
/// 也随时能主动调 `read_whiteboard`（那是显式拉取，不是被当成派给自己的活塞进来）。
/// 与 session world-model §9 的承诺一致：「@ 别人的部分会被过滤掉 —— 你看不到，也不
/// 需要关心」。
enum CrewWhiteboardVisibility {

    /// 这条消息对 `sessionId` 是否可见。`isCaptain` = 该 session 是本 crew 机长
    /// （吃 `@captain` 的定向）。
    static func isVisible(
        _ m: LocalWhiteboardMessage, to sessionId: String, isCaptain: Bool = false
    ) -> Bool {
        guard let mentions = m.mentions, !mentions.isEmpty else { return true }  // 广播
        if let sender = m.senderSessionId, sender == sessionId { return true }   // 自己发的
        return mentions.contains { mention in
            switch mention.kind {
            case "session": return mention.targetId == sessionId
            case "captain": return isCaptain
            default: return false   // human：人看的标记，不进 agent 注入面
            }
        }
    }

    /// 批量过滤（保持输入序）。
    static func visible(
        _ msgs: [LocalWhiteboardMessage], to sessionId: String, isCaptain: Bool = false
    ) -> [LocalWhiteboardMessage] {
        msgs.filter { isVisible($0, to: sessionId, isCaptain: isCaptain) }
    }
}
