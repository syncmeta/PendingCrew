#if os(macOS)
import Foundation

/// 旧的 composer 直投唤醒 + 缺席目标拉起原语。它原先从 `CrewChatView` 抽出，
/// 供人类 composer 与 Todo 共用。人类群消息现已统一由 `CrewLocalMentionWaker` 读取白板
/// message id 后投递，产品入口不得再调用本类型，否则会绕过去重产生双重唤醒。
/// 暂留源码只为兼容仍引用其纯拼装接口的历史分支；新代码不要接入。
/// 决策核心仍是纯函数 `CrewLocalMentionInjectLogic`（已有单测）；
/// 这里只做「拿 run 快照 → 决策 → IO」。
@MainActor
enum CrewLocalMentionDelivery {
    /// 对 `mentions` 点名的本地 run 直投唤醒；`mentions` 为空按「无定向 @」兜底
    /// `@captain`（IM 式「名：正文」渲染，不套「有人@你」壳 —— 人类广播 / Todo
    /// 回执走这条，项10）。命中但没在跑的目标另走缺席拉起（真启动进程）。
    static func injectAndWake(
        crewId: String,
        mentions: [CrewMention],
        text: String,
        senderName: String,
        sessionRunner: CrewSessionRunner,
        backend: PendingCrewBackend?,
        onError: ((String) -> Void)? = nil
    ) {
        let defaultToCaptain = mentions.isEmpty
        let effectiveMentions = defaultToCaptain ? [CrewMention.captain] : mentions

        // 项8/#490/#543：近期群聊上下文（仅前置给 claude 目标）从每个目标 session
        // 自己的未读游标现取，且只放对该 session 可见的（@ 别人的定向不当「近期
        // 群聊」灌进来）。要的是「**看得见吗**」：唤醒名单由下面的 `plannedInjections`
        // 从 `effectiveMentions` 里解（只认 session/captain），跟这一段无关。
        let store = LocalWhiteboardStore.shared
        let cursorDir = LocalWhiteboardStore.defaultDirectory
        var unreadBySession: [String: [LocalWhiteboardMessage]] = [:]
        for r in sessionRunner.runs where r.status == .running && r.kind == .claudeCode {
            let unread = WhiteboardCursor(
                directory: cursorDir, crewId: crewId, sessionId: r.sessionId).unread(in: store)
            unreadBySession[r.sessionId] = CrewWhiteboardVisibility.visible(
                unread, to: r.sessionId, isCaptain: r.role == .captain)
        }
        let runStates: [CrewLocalMentionInjectLogic.RunState] = sessionRunner.runs
            .filter { $0.status == .running }
            .map { .init(sessionId: $0.sessionId, isBusy: $0.backend.isBusy,
                         isClaude: $0.kind == .claudeCode,
                         isCaptain: $0.role == .captain) }
        // 注入面消歧（#62）的花名册：这里直接从在跑的 run 取显示名（比读快照更准，
        // 也不依赖 2 秒一次的落盘节奏）。
        let roster = Dictionary(
            sessionRunner.runs.map { ($0.sessionId, $0.displayName) },
            uniquingKeysWith: { a, _ in a })
        // @机长 → captain 是本地 run（role==.captain），解析出它的 sessionId 让
        // 决策核心一视同仁按 session 唤醒。限本 crew —— 多 crew 各自跑机长时别把
        // 别群的机长当目标。
        let captainSessionId = sessionRunner.runs
            .first { $0.crewId == crewId && $0.role == .captain && $0.status == .running }?
            .sessionId
        let injections = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: effectiveMentions, runs: runStates, messageText: text, senderName: senderName,
            captainSessionId: captainSessionId,
            recent: { sid in Array((unreadBySession[sid] ?? []).dropLast().suffix(15)) },
            imStyle: defaultToCaptain,
            displayName: { roster[$0] })
        for inj in injections {
            guard let run = sessionRunner.runs
                .first(where: { $0.sessionId == inj.sessionId && $0.status == .running })
            else { continue }
            // 一次本地 composer / Todo 落地只调用这里一次，UUID 只负责给 runner
            // 这条投递一个唯一身份；busy 时正文与消费回调一起留账，idle 后再真 send。
            sessionRunner.deliverOrDeferWake(
                sourceKey: "local:" + UUID().uuidString,
                to: run,
                text: inj.text
            ) {
                // #490：claude 目标真注入后才推进游标到刚 append 的这条（该 session
                // 未读尾）；排队阶段不消费。codex 走自己的 turn provider 游标，不推。
                if run.kind == .claudeCode, let last = unreadBySession[inj.sessionId]?.last {
                    WhiteboardCursor(directory: cursorDir, crewId: crewId, sessionId: inj.sessionId)
                        .advance(to: last, in: store)
                }
            }
        }
        wakeAbsentMentionTargets(
            crewId: crewId, mentions: effectiveMentions, text: text,
            sessionRunner: sessionRunner, backend: backend, onError: onError)
    }

    /// @ 了但本地没在跑的目标 → 拉起进程（用户点名「@某个人就要能唤醒它」，不能
    /// 只留白板）。机长 → `startCaptain`（带 wakeText 开场）；已退成员 →
    /// `restartMember`（复用原 sessionId，白板游标延续）。目标既不在跑、也不在
    /// 本地持久成员登记（如登录态 edge session）→ 跳过，那是 mailbox 唤醒的辖区。
    private static func wakeAbsentMentionTargets(
        crewId: String,
        mentions: [CrewMention],
        text: String,
        sessionRunner: CrewSessionRunner,
        backend: PendingCrewBackend?,
        onError: ((String) -> Void)?
    ) {
        let runningIds = Set(sessionRunner.runs
            .filter { $0.status == .running }.map(\.sessionId))
        let captainRunning = sessionRunner.runs.contains {
            $0.crewId == crewId && $0.role == .captain && $0.status == .running
        }
        let wake = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: mentions, runningSessionIds: runningIds, captainRunning: captainRunning)
        guard wake.needCaptain || !wake.sessionIds.isEmpty,
              let backend else { return }
        let members = LocalCrewStore.shared.sessionMembers(crewId: crewId)
        Task {
            do {
                let detail = try await backend.getCrew(crewId)
                if wake.needCaptain {
                    try await sessionRunner.startCaptain(
                        detail: detail, backend: backend, wakeText: text)
                }
                for sid in wake.sessionIds {
                    guard let m = members.first(where: { $0.sessionId == sid }) else { continue }
                    try await sessionRunner.restartMember(
                        detail: detail, backend: backend, member: m, wakeText: text)
                }
            } catch {
                onError?("@ 唤醒失败：\(error.localizedDescription)")
            }
        }
    }
}
#endif
