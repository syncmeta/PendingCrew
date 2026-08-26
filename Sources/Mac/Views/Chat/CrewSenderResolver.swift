import Foundation

/// 一条消息的作者解析结果，供 CrewChatAdapter → BubbleView / CrewAvatarBadges 渲染。
struct CrewSender: Equatable {
    enum Kind: Equatable { case bot, session, human }
    let kind: Kind
    let displayName: String
    let avatarSeed: String          // 喂给 CrewColorHash 的稳定 id
    let isCaptain: Bool
    let sessionStatus: String?      // 仅 session：running/queued/…
    let isMine: Bool                // 我自己 → 右对齐、无头像
}

enum CrewSenderResolver {
    /// `entry` 的作者。`localUserId` = 当前登录用户（用于判断"我"）。
    static func resolve(
        _ entry: CrewWhiteboardEntry,
        members: [CrewMember],
        captainBotId: String?,
        localUserId: String?
    ) -> CrewSender {
        switch entry.senderKind {
        case "user", "human":
            let uid = entry.senderUserId
            // 多人身份修复（Task 11）：严格按 uid 判"我"，不用 senderDisplayName
            // 兜底守卫。
            // uid == localUserId：同一账号。
            // uid == LocalWhiteboardStore.localUserId：macOS composer 本地行恒标
            // 的 BYOK 哨兵常量，不随登录态变（`appendUserMessage` 不知道登录 id）——
            // 登录后 `localUserId` 换成真实 id，composer 行仍是哨兵，两者都得算我，
            // 否则登录用户自己刚发的话会突然变成"别人"（右对齐→左对齐回归）。
            // uid == nil：字段引入前的历史白板消息，legacy fallback 仍当我。
            let mine: Bool
            if let uid {
                mine = uid == localUserId || uid == LocalWhiteboardStore.localUserId
            } else {
                mine = true
            }
            let m = members.first { $0.userId != nil && $0.userId == uid }
            return CrewSender(
                kind: .human,
                displayName: entry.senderDisplayName ?? m?.displayName ?? "人",
                avatarSeed: uid ?? entry.senderDisplayName ?? "me",
                isCaptain: false, sessionStatus: nil, isMine: mine
            )

        case "captain":
            // 机长 run 的发言（本地 MCP helper 标 senderKind "captain"）。头像种子
            // 用稳定的 captainBotId —— 与 roster/`CrewSenderNaming.groupSender` 的
            // captain 成员同源，成员列表和气泡才是同一张脸；run sessionId 每次启动
            // 都变，不能当身份种子。
            let cap = members.first { $0.memberKind == "captain" }
            return CrewSender(
                kind: .bot,
                displayName: entry.senderDisplayName ?? cap?.displayName ?? "机长",
                avatarSeed: cap?.botId ?? captainBotId ?? entry.senderSessionId ?? entry.id,
                isCaptain: true, sessionStatus: nil, isMine: false
            )

        case "session":
            let m = members.first { $0.codeSessionId != nil && $0.codeSessionId == entry.senderSessionId }
            // 名字优先级:roster 成员名(登录态)→ entry.senderDisplayName(本地 session
            // 经 post_to_crew 带的 label,如「机长」/「Claude Code · abc」)→ 兜底「会话」。
            // 本地 session 不在 roster,过去只能落兜底「会话」;加 senderDisplayName 这层后
            // 本地 session 也显真名。
            return CrewSender(
                kind: .session,
                displayName: m?.displayName ?? entry.senderDisplayName
                    ?? CrewSenderNaming.sessionFallback(entry.senderSessionId),
                avatarSeed: entry.senderSessionId ?? entry.id,
                isCaptain: false, sessionStatus: m?.sessionStatus, isMine: false
            )

        case "bot":
            let bid = entry.senderBotId
            // captain 兜底只对 bid == nil 生效 —— relay 进来的远端 bot
            // （senderDisplayName 非 nil、bid 不在本地 members）不能误判成机长。
            let m = members.first { $0.botId != nil && $0.botId == bid }
                ?? (bid == nil && entry.senderDisplayName == nil
                    ? members.first { $0.memberKind == "captain" } : nil)   // null 兜底 captain
            let isCaptain = (bid != nil && bid == captainBotId) || m?.memberKind == "captain"
            return CrewSender(
                kind: .bot,
                displayName: entry.senderDisplayName ?? m?.displayName ?? (isCaptain ? "机长" : "bot"),
                avatarSeed: bid ?? captainBotId ?? "bot",
                isCaptain: isCaptain, sessionStatus: nil, isMine: false
            )

        default:
            return CrewSender(kind: .bot, displayName: entry.senderKind,
                              avatarSeed: entry.id, isCaptain: false, sessionStatus: nil, isMine: false)
        }
    }
}
