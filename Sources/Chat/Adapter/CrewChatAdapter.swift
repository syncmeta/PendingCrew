import Foundation

/// Maps a `CrewWhiteboardEntry` into the `(CrewChatMessage, GroupBubbleSender?)`
/// pair that `BubbleView` consumes.
///
/// `groupSender == nil` ⇒ the local user's own message → right-aligned,
/// no avatar, no display name.
enum CrewChatAdapter {

    /// Adapt one whiteboard entry into a BubbleView-compatible pair.
    ///
    /// - Parameters:
    ///   - e:             The entry to adapt.
    ///   - members:       Current crew roster (for display-name + role lookup).
    ///   - captainBotId:  The crew's captain bot id (for the isCaptain badge).
    ///   - localUserId:   Logged-in user's id (single source of truth for mine-ness
    ///                    via `CrewSenderResolver`).
    static func adapt(
        _ e: CrewWhiteboardEntry,
        members: [CrewMember],
        captainBotId: String?,
        localUserId: String?
    ) -> (CrewChatMessage, GroupBubbleSender?) {
        let s = CrewSenderResolver.resolve(e, members: members,
                                           captainBotId: captainBotId,
                                           localUserId: localUserId)

        // Map CrewAttachment → Attachment (same field layout, no-op rename).
        let attachments: [Attachment]? = e.attachments.map { crewAtts in
            crewAtts.map { a in
                Attachment(
                    id: a.id,
                    kind: a.kind,
                    mime: a.mime,
                    size: a.size,
                    width: a.width,
                    height: a.height,
                    url: a.url,
                    filename: a.filename
                )
            }
        }

        let msg = CrewChatMessage(
            id: e.id,
            // sender_type is used by BubbleView for the bot-vs-user branch on
            // the *own-message* (right-aligned) path; for mine messages we
            // always pass "user" so the green bubble renders. For peer bots we
            // pass "bot" to unlock BotAvatar rendering if BubbleView ever falls
            // through to the non-group sender path.
            sender_type: s.isMine ? "user" : (s.kind == .bot ? "bot" : "user"),
            sender_id: e.senderUserId ?? e.senderBotId ?? e.senderSessionId ?? e.id,
            content: e.displayText,
            attachments: attachments,
            status: nil,
            mine: s.isMine          // single source of truth — CrewSenderResolver
        )

        // Own messages: no group-sender overlay (right-aligned, no avatar/name).
        guard !s.isMine else { return (msg, nil) }

        // Map CrewSender.Kind → GroupBubbleSender.Kind.
        // Sessions have no `GroupBubbleSender.Kind.session` — map to .bot and
        // set `isSession = true` so the A7 badge overlay can render a terminal icon.
        let gbKind: GroupBubbleSender.Kind
        switch s.kind {
        case .bot, .session: gbKind = .bot
        case .human:         gbKind = .user
        }

        var sender = GroupBubbleSender(
            kind: gbKind,
            // emoji 种子与色种子同用 resolver 的 avatarSeed —— roster 侧
            // （CrewSenderNaming.groupSender）两个种子也是同一个值，气泡和
            // 成员列表才恒同脸。此前 id 独立取 senderBotId/SessionId 兜底链，
            // captain（senderKind "captain"/bot 且 botId 为空）会跟 roster 的
            // captainBotId 种子岔开 → 两张脸。
            id: s.avatarSeed,
            displayName: s.displayName,
            avatarPath: nil,        // PendingCrew has no per-user attachment avatar yet
            avatarSeed: s.avatarSeed
        )
        sender.isCaptain     = s.isCaptain
        sender.sessionStatus = s.sessionStatus
        sender.isSession     = (s.kind == .session)

        return (msg, sender)
    }
}
