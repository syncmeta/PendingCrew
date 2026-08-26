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
///   * **可见**（本类型）—— 这条要不要渲染进该 session 的上下文。三个调用点全是这个
///     语义：`HookEmitter`（每轮未读注入）、`CrewLocalMentionWaker` /
///     `CrewLocalMentionDelivery`（一次已决定要发生的唤醒，附带的「近期群聊上下文」）。
///   * **该叫醒** —— 由 `CrewLocalMentionInjectLogic.plannedInjections` / `.wakeTargets`
///     决定，它们只认 `kind == "session"` / `"captain"`。
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
///   * **`kind == "broadcast"` 是显式放宽器，优先级高于收窄**（#62）。作者明写
///     「这条全组都该看见」→ `isVisible` 直接 true。于是
///     `[broadcast, session(X)]` = **全组可见 + 只叫醒 X**（唤醒面只认
///     session/captain，一行没改）—— 这正是「人类在 Todo 上回应」要的那一档：
///     全组都看得见（治容易漏），同时把当初提这件事的那个 session 叫醒。
///     放宽是**显式 opt-in**：不写 broadcast 就还是老的排他行为，漏写只会
///     「不够宽」，不会「悄悄扩散」。
///   * 自己发的那条永远对自己可见（否则自己的定向 @ 在自己上下文里凭空消失）。
///
/// 「看不到」只针对 agent 注入面：人类在 app 白板上仍看得到全部消息，session 想看全景
/// 也随时能主动调 `read_whiteboard`（那是显式拉取，不是被当成派给自己的活塞进来）。
/// 与 session world-model §9 的措辞保持一致 —— 那里也已改成「@ 了**别的 session / 机长**
/// 的部分才会被过滤掉；@ 人类的照样看得到」。
enum CrewWhiteboardVisibility {

    /// 收窄可见范围的 mention 种类。**只有这两种**能把一条消息变成定向。
    /// `human` 不在此列（附加标记，不动可见面）；`broadcast` 也不在此列，它走
    /// 反方向 —— 见 `isWidening`。
    private static func isNarrowing(_ kind: String) -> Bool {
        kind == "session" || kind == "captain"
    }

    /// 显式**放宽**可见范围的 mention 种类（#62）。只有 `broadcast` —— 它今天已经
    /// 是真实存在的 kind（`CrewMention.broadcast`，composer 的「全体」发的就是它），
    /// 过去在这里既不收窄也不放宽 = 等于不存在。现在给它定义语义：作者明写它，
    /// 就是明说「全组都该看见」，压过同一条上的任何收窄型 mention。
    private static func isWidening(_ kind: String) -> Bool {
        kind == "broadcast"
    }

    /// 这条消息对 `sessionId` 是否可见。`isCaptain` = 该 session 是本 crew 机长
    /// （吃 `@captain` 的定向）。
    static func isVisible(
        _ m: LocalWhiteboardMessage, to sessionId: String, isCaptain: Bool = false
    ) -> Bool {
        // 判据是「**有没有收窄型 mention**」，不是「mentions 空不空」：只 @ 了人类
        // （或只有显式 broadcast）的消息 mentions 非空，但它仍然是广播。
        let mentions = m.mentions ?? []
        // 显式放宽压过收窄（#62）：`[broadcast, session(X)]` 全组可见，唤醒面照旧只
        // 命中 X。注意**只放宽可见面** —— 谁该被叫醒不在本类型的辖区。
        if mentions.contains(where: { isWidening($0.kind) }) { return true }
        let narrowing = mentions.filter { isNarrowing($0.kind) }
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

    /// **注入面消歧**（#62 硬要求）—— 治的是 #543 真正的病根。
    ///
    /// #543 的病不是「看得见」，是「**看不出这不是给我的**」：本文件头那段事故里
    /// 写着「定向 @ 与广播长得一模一样，非目标 worker 从注入面上根本看不出这条
    /// 不是给自己的」。2026-07-26 的修法是把它藏起来 —— 那是绕过病根，不是治它。
    /// `broadcast` 放宽以后，`[broadcast, session(X)]` 又会让别人看见一条明明派给
    /// X 的消息，形状正是当年怕的那个。所以放宽的同时必须把病根治掉：
    ///
    /// > 一条消息对某个 session 可见、但收窄型 mention 里没有它时，**注入面上必须
    /// > 一眼看得出这条不是派给它的**。
    ///
    /// 本方法是那条判定的**纯函数**（输入：一条消息 + 观察者 + 花名册；输出：要不要
    /// 标注、标注写谁），渲染端只负责把返回值拼在正文前面，别把判定埋进渲染代码里。
    /// 两条注入面共用这一份：`HookEmitter`（每轮未读注入）与
    /// `CrewLocalMentionInjectLogic`（唤醒附带的「近期群聊」）。
    ///
    /// 返回 nil = 不用标注（真广播；或这条本来就点了我）。非 nil = 该前置的标注，
    /// 形如 `（发给 小王 的）`。
    ///
    /// 与 `isVisible` **正交**：本方法只答「该不该标注」，不管看不看得见。自己发的
    /// 定向消息在自己注入面上也会带标注 —— 那是事实（它确实是发给别人的），不是噪音。
    ///
    /// `displayName` = 花名册（sessionId → 显示名）。查不到就退回 `session:<前6>`，
    /// 宁可标个短 id 也不能不标。
    static func directedNote(
        _ m: LocalWhiteboardMessage, to sessionId: String, isCaptain: Bool = false,
        displayName: (String) -> String? = { _ in nil }
    ) -> String? {
        let narrowing = (m.mentions ?? []).filter { isNarrowing($0.kind) }
        // 真广播（含只 @ 人类 / 只写 broadcast）：没有「派给谁」这回事，不标。
        guard !narrowing.isEmpty else { return nil }
        let addressesMe = narrowing.contains { mention in
            switch mention.kind {
            case "session": return mention.targetId == sessionId
            case "captain": return isCaptain
            default: return false
            }
        }
        guard !addressesMe else { return nil }   // 就是点我的，别多此一举
        var names: [String] = []
        for mention in narrowing {
            let name: String
            switch mention.kind {
            case "captain":
                name = "机长"
            case "session":
                guard let tid = mention.targetId, !tid.isEmpty else { continue }
                name = displayName(tid) ?? "session:\(tid.prefix(6))"
            default:
                continue
            }
            if !names.contains(name) { names.append(name) }
        }
        // 收窄型 mention 在场但一个名字都解不出（如 `@session` 缺 target_id）——
        // 仍要标：读的人得知道「这条不是冲我来的」，只是说不出是冲谁。
        guard !names.isEmpty else { return "（不是发给你的）" }
        return "（发给 \(names.joined(separator: "、")) 的）"
    }

    /// 批量过滤（保持输入序）。
    static func visible(
        _ msgs: [LocalWhiteboardMessage], to sessionId: String, isCaptain: Bool = false
    ) -> [LocalWhiteboardMessage] {
        msgs.filter { isVisible($0, to: sessionId, isCaptain: isCaptain) }
    }
}
