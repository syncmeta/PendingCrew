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
/// ## 这里判的是「看得见吗」，不是「该叫醒吗」
///
/// 两件事，全项目分开实现，别混：
///   * **可见**（本类型）—— 这条要不要渲染进该 session 的上下文。四个调用点全是这个
///     语义：`HookEmitter`（每轮未读注入）、`CrewLocalMentionWaker` /
///     `CrewLocalMentionDelivery` / `CrewMailboxWaker`（一次已决定要发生的唤醒，附带的
///     「近期群聊上下文」）。
///   * **该叫醒** —— 由 `CrewLocalMentionInjectLogic.plannedInjections` / `.wakeTargets`
///     和 `CrewMailboxWakeLogic` 决定，它们只认 `kind == "session"` / `"captain"`。
///     放宽本类型的可见性**不会**让任何人被多叫醒一次。
///
/// ## 规则
///   * **没有收窄型 mention = 广播** → 谁都看得到。「收窄型」只有两种：
///     `kind == "session"`（配 sessionId）和 `kind == "captain"`（配机长 session）。
///   * **`kind == "human"` 是附加标记，不收窄可见范围**。`@人类` 说的是「这条是讲给人
///     听的、别为它叫醒 agent」，**不是**「对 agent 隐身」。
///     2026-08-23 修：过去 human 落在 `default: return false` 上，于是一条只 @ 了人类的
///     消息对**每一个** agent 隐身（包括机长）—— 队友「@人 我做完了」发出去，全 crew
///     没人看得见，本机白板约 1/10 的消息处在这个状态。别叫醒 ≠ 看不见，写成了同一句
///     就是这个 bug 的来源。「别叫醒」在上面那两个唤醒判据里，本来就已经实现了。
///   * **有收窄型 mention = 定向** → 只有被点到的看得到；同时带 human 也不放宽
///     （`@session + @human` 仍按 session 排他，#543 一个字不松）。
///   * 自己发的那条永远对自己可见（否则自己的定向 @ 在自己上下文里凭空消失）。
///
/// 「看不到」只针对 agent 注入面：人类在 app 白板上仍看得到全部消息，session 想看全景
/// 也随时能主动调 `read_whiteboard`（那是显式拉取，不是被当成派给自己的活塞进来）。
/// 与 session world-model §9 的措辞保持一致 —— 那里也已改成「@ 了**别的 session / 机长**
/// 的部分才会被过滤掉；@ 人类的照样看得到」。
enum CrewWhiteboardVisibility {

    /// 收窄可见范围的 mention 种类。**只有这两种**能把一条消息变成定向。
    /// `human` / `broadcast` 不在此列 —— 它们是附加标记，不动可见面。
    private static func isNarrowing(_ kind: String) -> Bool {
        kind == "session" || kind == "captain"
    }

    /// 这条消息对 `sessionId` 是否可见。`isCaptain` = 该 session 是本 crew 机长
    /// （吃 `@captain` 的定向）。
    static func isVisible(
        _ m: LocalWhiteboardMessage, to sessionId: String, isCaptain: Bool = false
    ) -> Bool {
        // 判据是「**有没有收窄型 mention**」，不是「mentions 空不空」：只 @ 了人类
        // （或只有显式 broadcast）的消息 mentions 非空，但它仍然是广播。
        let narrowing = (m.mentions ?? []).filter { isNarrowing($0.kind) }
        guard !narrowing.isEmpty else { return true }                            // 广播
        if let sender = m.senderSessionId, sender == sessionId { return true }   // 自己发的
        return narrowing.contains { mention in
            switch mention.kind {
            case "session": return mention.targetId == sessionId
            case "captain": return isCaptain
            default: return false   // isNarrowing 已挡住，走不到
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
