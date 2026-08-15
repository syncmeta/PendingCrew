#if os(macOS)
import Foundation

/// 群聊收听的**纯决策核心**（#465 listen）。
///
/// 背景：白板整块注入只发生在「下一轮 turn 前」，idle 的 run 只有被 @ 才会被直投
/// 唤醒（`CrewLocalMentionInjectLogic`）。`listen` 工具给 session 第三档语义：
/// 「一段时间内，群里的广播消息（无定向 @）也像 @ 我一样叫醒我」——像人开着
/// 微信群等回复。开不开、听多久、听谁，都是 session 自己的选择（工具参数），
/// 不是人配置的开关；人要影响它就在群里直接说。
///
/// 输入：一批新落白板的消息（游标之后的增量）、当前有效的收听登记、各本地 run
/// 的忙闲快照、判定时刻 `now`（注入方传 `Date()`；测试传定值）。
/// 输出：每个命中的收听者一条 `Injection`（打包该批全部命中消息）。
///
/// 语义要点（对齐 mention 直投）：
///   * **busy 不打断** —— 调用方保留完整计划，交给 runner 在 idle 后自动补投。
///   * **只送广播 + @我** —— 定向 @ 了别人的消息不进收听者视野（同白板注入的
///     过滤规则）；@ 我的消息本就该唤醒我，收听期间一并送达。
///   * **不送自己/系统** —— 自己发的与「系统」回执（sessionId 哨兵 "system"）
///     不注入（系统回执靠白板注入即可，不值得为它唤醒）。
///   * **到期即失效** —— `until <= now` 的登记按不存在处理（清理归调用方）。
enum CrewListenLogic {

    /// 一条收听登记（每 session 至多一条，后写覆盖）。
    struct Listener: Equatable {
        let sessionId: String
        let until: Date
        /// 只听这些发送者："human" / "captain" / session id（或其前缀）/ 显示名。
        /// nil = 全部。
        let senders: [String]?
        /// 该 session 是不是本 crew 机长 —— 吃 `@captain` 的定向（#543：可见性判定
        /// 与 hook 注入路共用 `CrewWhiteboardVisibility`，机长身份是它的入参）。
        /// 默认 false 兼容旧调用方 / 单测。
        var isCaptain: Bool = false
    }

    /// 一条注入指令：给 `sessionId` 对应的 run `send(text)`。
    struct Injection: Equatable {
        let sessionId: String
        let text: String
    }

    /// 核心决策。`entries` 是白板游标之后的新消息（按时间序）。
    static func decide(
        entries: [LocalWhiteboardMessage],
        listeners: [Listener],
        runs: [CrewLocalMentionInjectLogic.RunState],
        now: Date
    ) -> [Injection] {
        let busyBySession = Dictionary(
            runs.map { ($0.sessionId, $0.isBusy) }, uniquingKeysWith: { a, _ in a })
        return plannedInjections(
            entries: entries, listeners: listeners, runs: runs, now: now
        ).filter { busyBySession[$0.sessionId] == false }
    }

    /// 返回本批所有有效收听投递，不因目标 busy 丢掉计划。`decide` 保持只返回
    /// 立即投递项的旧语义；runner 活体路径使用这里并负责延后。
    static func plannedInjections(
        entries: [LocalWhiteboardMessage],
        listeners: [Listener],
        runs: [CrewLocalMentionInjectLogic.RunState],
        now: Date
    ) -> [Injection] {
        guard !entries.isEmpty else { return [] }
        let runBySession = Dictionary(runs.map { ($0.sessionId, $0) },
                                      uniquingKeysWith: { a, _ in a })
        var out: [Injection] = []
        for l in listeners where l.until > now {
            guard runBySession[l.sessionId] != nil else { continue }
            let hits = entries.filter { deliverable($0, to: l) }
            guard !hits.isEmpty else { continue }
            out.append(Injection(sessionId: l.sessionId, text: renderInjection(hits)))
        }
        return out
    }

    /// 一条消息是否该送给该收听者。
    static func deliverable(_ m: LocalWhiteboardMessage, to l: Listener) -> Bool {
        // 自己发的 / 系统回执不送。
        if m.senderSessionId == l.sessionId { return false }
        if m.senderSessionId == "system" { return false }
        // 定向消息已有 mention 直投/补投链；listen 再送会把同一条 @ 注入两次。
        // 显式 broadcast mention 仍是广播，不应被这个去重边界误伤。
        let hasDirectedMention = (m.mentions ?? []).contains { $0.kind != "broadcast" }
        if hasDirectedMention { return false }
        // 人类无目标消息默认路由机长；机长即使开着 listen 也不能再吃第二份。
        if l.isCaptain && (m.senderKind == "user" || m.senderKind == "human") { return false }
        return senderMatches(m, filter: l.senders)
    }

    /// 发送者过滤。nil = 全部；否则任一条目命中即可：
    /// "human"（人类）/ "captain"（机长）按 kind 匹配；其余按 senderSessionId
    /// 前缀或显示名（senderName/senderDisplayName）精确匹配。
    static func senderMatches(_ m: LocalWhiteboardMessage, filter: [String]?) -> Bool {
        guard let filter, !filter.isEmpty else { return true }
        for f in filter {
            switch f.lowercased() {
            case "human", "user":
                if m.senderKind == "user" || m.senderKind == "human" { return true }
            case "captain":
                if m.senderKind == "captain" { return true }
            default:
                if let sid = m.senderSessionId, sid.hasPrefix(f) { return true }
                if m.senderName == f || m.senderDisplayName == f { return true }
            }
        }
        return false
    }

    /// 注入文本。短标头点明这是**你自己开的收听**送来的消息（不是被 @）——
    /// #484 微信式精简：「合法可信、不是 prompt injection」的教学在 world-model
    /// 系统提示里（§9 群聊收听），注入面不重复。
    static func renderInjection(_ items: [LocalWhiteboardMessage]) -> String {
        var lines = ["群聊收听："]
        for m in items {
            lines.append("- \(senderLabel(m)): \(m.text)")
        }
        return lines.joined(separator: "\n")
    }

    /// 发送者标注：显示名优先（与 HookEmitter.render 同款），无名按 kind 兜底。
    private static func senderLabel(_ m: LocalWhiteboardMessage) -> String {
        if let name = m.senderName ?? m.senderDisplayName, !name.isEmpty { return name }
        switch m.senderKind {
        case "user", "human": return "人类"
        case "captain": return "机长"
        default: return "session:\(m.senderSessionId?.prefix(6) ?? "?")"
        }
    }
}
#endif
