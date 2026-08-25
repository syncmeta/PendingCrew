#if os(macOS)
import Foundation

/// Local直投唤醒的**纯决策核心**(Phase 6 单元 3)。
///
/// 背景:本地(LocalBackend)模式下,空闲的 session run **不会自己醒** —— 白板
/// 每轮注入发生在「下一轮 turn 前」,但 idle 的 run 没有下一轮触发。所以人在本地
/// @ 某 session 时,要**直接把这条消息注入目标 run** 唤醒它。
///
/// 这与 Phase 4b 的 `CrewMailboxWakeLogic` 区别:那是远端 mailbox(hub→inbox→
/// waker,有 deliver 状态机);**这里是本地直投** —— 无 mailbox、无 deliver 标记,
/// 人按了发送就把消息塞给目标 run。
///
/// 输入:本次发送的 staged `[CrewMention]`、每个本地 run 的「身份 + 是否空闲」
/// 快照(`RunState`)、消息正文 + 发送者名、以及 captain 本地 run 的 sessionId
/// (`captainSessionId`,调用方解析后传入)。输出:一组 `Injection`(给哪个 run
/// 注入什么文本)。busy 的 run **不打断**；活体调用方用 `plannedInjections`
/// 保留投递计划，并交给 runner 在 idle 后自动补投。
/// `@session` 按目标 sessionId 唤醒;`@captain` 解析到 captain 本地 run 的
/// sessionId 后**同样唤醒**(#1:之前 captain 被忽略 → @机长 机长不醒);
/// broadcast / human 在本地直投路径**不**唤醒具体 run(broadcast 靠白板每轮注入覆盖)。
///
/// 抽成纯函数让「哪些 run 该醒 / 注入什么」可在不起真 PTY / app-server 的前提下
/// 单测(对齐 `CrewMailboxWakeLogic` 的风格)。活体「找 run → run.send」的 IO
/// 编排留在 `CrewChatView.send()` 的本地分支 + 手动验证。
enum CrewLocalMentionInjectLogic {

    /// 一个本地 run 的决策所需快照。`sessionId` 用来跟 `@session` mention 的
    /// targetId 配对;`isBusy` 来自 `SessionBackend.isBusy`;`isClaude` = 后端是否
    /// claude(`run.kind == .claudeCode`)——仅 claude 目标前置近期群聊上下文(项8),
    /// codex 每轮 turn 自带未读白板 additionalContext,别重复塞。默认 false 兼容
    /// 旧调用方 / 单测。
    struct RunState: Equatable {
        let sessionId: String
        let isBusy: Bool
        var isClaude: Bool = false
        /// 该 run 是本 crew 机长 —— 只用于**注入面消歧**（#62）：机长自己看
        /// `@captain` 的条目不该被标成「（发给 机长 的）」。唤醒面不读它
        /// （`@captain` 走 `captainSessionId` 解析），默认 false 兼容旧调用方 / 单测。
        var isCaptain: Bool = false
    }

    /// 一条注入指令:给 `sessionId` 对应的 run `send(text)`。
    struct Injection: Equatable {
        let sessionId: String
        let text: String
    }

    /// 核心决策。
    ///
    /// 对每个 `@session` / `@captain` mention,找到对应的本地 run:
    ///   * 命中且空闲(`!isBusy`)→ 产出一条 `Injection`(注入定向文本唤醒)。
    ///   * 命中但 busy → 本方法跳过；活体调用方用 `plannedInjections` 留账补投。
    ///   * 没有对应本地 run(远端 session / 已结束)→ 跳过(本地无可投目标)。
    /// `@captain` 解析到 `captainSessionId`(调用方从在跑的 captain run 取);为 nil
    /// (本地没在跑 captain)则忽略。broadcast / human 一律忽略(见类型注释)。
    ///
    /// 同一目标被 @ 多次 → 去重,只注入一次。
    ///
    /// `recent` = 近期群聊上下文(项8):仅前置给 **claude** 目标(`RunState.isClaude`),
    /// 在定向文本之前渲染一块「近期群聊」;codex 目标不塞。
    /// #490:改成**按 sessionId 现取**的闭包 —— 每个目标 session 有自己的未读游标,
    /// 上下文取该 session 游标之后的未读(调用方接 `WhiteboardCursor`),而不是所有目标
    /// 共用一份「白板最近 15 条」重发已注入过的历史。默认空闭包(单测/无上下文调用)。
    /// `imStyle` = IM 式渲染(项10):无具体 @ 的默认给机长时,注入文本用「名:正文」
    /// 而**不套**「有人@你：」壳(因为不是定向 @)。
    static func decide(
        mentions: [CrewMention],
        runs: [RunState],
        messageText: String,
        senderName: String,
        captainSessionId: String? = nil,
        recent: (String) -> [LocalWhiteboardMessage] = { _ in [] },
        imStyle: Bool = false,
        displayName: (String) -> String? = { _ in nil }
    ) -> [Injection] {
        let busyBySession = Dictionary(
            runs.map { ($0.sessionId, $0.isBusy) }, uniquingKeysWith: { a, _ in a })
        return plannedInjections(
            mentions: mentions, runs: runs, messageText: messageText,
            senderName: senderName, captainSessionId: captainSessionId,
            recent: recent, imStyle: imStyle, displayName: displayName
        ).filter { busyBySession[$0.sessionId] == false }
    }

    /// 解析本条消息命中的全部在跑目标，不因 busy 丢掉计划。`decide` 继续保留
    /// “只返回当前可立即投递目标”的纯决策语义；IO 调用方使用本方法，再统一交给
    /// `CrewSessionRunner.deliverOrDeferWake` 做 busy 门禁。
    static func plannedInjections(
        mentions: [CrewMention],
        runs: [RunState],
        messageText: String,
        senderName: String,
        captainSessionId: String? = nil,
        recent: (String) -> [LocalWhiteboardMessage] = { _ in [] },
        imStyle: Bool = false,
        displayName: (String) -> String? = { _ in nil }
    ) -> [Injection] {
        // 目标 session id 集合(保序去重)。@session → 目标 sessionId;
        // @captain → captain 本地 run 的 sessionId(调用方解析后传入)。
        var targets: [String] = []
        for m in mentions {
            switch m.kind {
            case "session":
                if let sid = m.targetId, !targets.contains(sid) { targets.append(sid) }
            case "captain":
                if let cid = captainSessionId, !targets.contains(cid) { targets.append(cid) }
            default:
                break   // broadcast / human:本地直投不唤醒具体 run
            }
        }
        guard !targets.isEmpty else { return [] }

        let runBySession = Dictionary(runs.map { ($0.sessionId, $0) }, uniquingKeysWith: { a, _ in a })

        var out: [Injection] = []
        for sid in targets {
            guard let run = runBySession[sid] else { continue }
            // 近期群聊上下文只前置给 claude 目标(项8);codex 传空。#490:按 sessionId
            // 现取该目标自己的未读上下文。
            let text = renderInjection(
                messageText: messageText, senderName: senderName,
                recent: run.isClaude ? recent(sid) : [], imStyle: imStyle,
                viewer: sid, viewerIsCaptain: run.isCaptain, displayName: displayName)
            out.append(Injection(sessionId: sid, text: text))
        }
        return out
    }

    /// @ 目标**不在跑** → 该拉起谁（用户点名「@某个人就要能唤醒它」——不在跑
    /// 不能只留白板,要真把进程拉起来）。与 `decide` 互补:decide 只管在跑且
    /// 空闲的注入,这里只管完全没在跑的:
    ///   * `@captain` 且本地没有在跑的 captain → `needCaptain = true`
    ///   * `@session` 且该 sessionId 没有对应在跑的 run → 进 `sessionIds`(保序去重)
    /// 在跑的目标(无论 busy/idle)不在此列。broadcast / human 不触发拉起。
    static func wakeTargets(
        mentions: [CrewMention],
        runningSessionIds: Set<String>,
        captainRunning: Bool
    ) -> (needCaptain: Bool, sessionIds: [String]) {
        var needCaptain = false
        var out: [String] = []
        for m in mentions {
            switch m.kind {
            case "captain":
                if !captainRunning { needCaptain = true }
            case "session":
                if let sid = m.targetId, !runningSessionIds.contains(sid),
                   !out.contains(sid) {
                    out.append(sid)
                }
            default:
                break
            }
        }
        return (needCaptain, out)
    }

    /// 把人类发的这条消息渲染成注入给目标 run 的文本。
    ///   * 定向 @(默认):单行前导「有人@你：」+ 保留发送者身份(CC-P3)的正文行,
    ///     尾随一句「先吱一声」提醒(#530:被 @ 先 post_to_crew 简短确认再干活)。
    ///   * `imStyle`(项10 无 @ 默认给机长):IM 式「发送者：正文」,不套「有人@你」壳。
    /// `recent` 非空 → 在定向/IM 文本**之前**前置一块「近期群聊」上下文(项8)。
    /// `viewer` / `viewerIsCaptain` / `displayName` 只作用在前置的「近期群聊」块上
    /// （注入面消歧，#62）——「有人@你」那一行本来就是定向给 viewer 的，不需要标注。
    static func renderInjection(
        messageText: String, senderName: String,
        recent: [LocalWhiteboardMessage] = [], imStyle: Bool = false,
        viewer: String? = nil, viewerIsCaptain: Bool = false,
        displayName: (String) -> String? = { _ in nil }
    ) -> String {
        let body = messageText.trimmingCharacters(in: .whitespacesAndNewlines)
        let directed = imStyle
            ? "\(senderName)：\(body)"
            : "有人@你：\n- \(senderName): \(body)\n（先 post_to_crew 吱一声「收到/我看看」再干活）"
        guard let ctx = CrewRecentContextRender.block(
            recent, viewer: viewer, viewerIsCaptain: viewerIsCaptain,
            displayName: displayName) else { return directed }
        return ctx + "\n\n" + directed
    }
}
#endif
