import Foundation

/// Crew 成员 / 作者 → 显示名 + 头像 sender 的统一收口。
///
/// 收在一处的理由:session 的兜底名字过去散落在 roster、inspector、聊天气泡、
/// @-mention、relay 五处,各写各的(`"session"` / `"session · xxx"` /
/// `"session xxx"`),既不一致也难看。统一走 `sessionFallback`;并把 roster 与
/// inspector 里一字不差复制的 `groupSenderFor` 收成单一 `groupSender(for:)`,
/// 让成员列表和聊天页的头像/名字逐字节一致。
enum CrewSenderNaming {
    /// session 没有真实昵称/任务名时的兜底显示名。调用方负责"有真名优先",
    /// 这里只兜底:`"会话 · 4f2ab1"`(取 sessionId 前 6 位);无 id → `"会话"`。
    static func sessionFallback(_ sessionId: String?) -> String {
        guard let sid = sessionId, !sid.isEmpty else { return "会话" }
        return "会话 · \(sid.prefix(6))"
    }

    /// 一条**本地白板消息**映射到 wire `sender_display_name` 的取值。
    ///
    /// relay 搬进来的消息带远端名(`relayName`),要显示;但**本机人类自己发的消息
    /// (senderKind=="user")绝不能**把本地兜底名("人")塞进这个字段 —— 中栏
    /// `CrewSenderResolver` 把 `senderDisplayName != nil` 当"远端他人"的 relay 守卫,
    /// 折进去会让自己的消息被误判成 relay → 渲染到左侧(#3)。session 进展折
    /// `localName`(如"机长")作兜底显示名(relay 名优先)。
    static func localWireDisplayName(
        senderKind: String, relayName: String?, localName: String?) -> String? {
        senderKind == "user" ? relayName : (relayName ?? localName)
    }

    /// 从 crew 成员构造气泡/roster 头像 sender。`CrewRosterBar` 与
    /// `CrewSessionWindowView` 共用,成员列表和 inspector 的头像/名字保持一致。
    static func groupSender(for m: CrewMember, captainBotId: String?) -> GroupBubbleSender {
        let isCaptain = m.memberKind == "captain" || (m.botId != nil && m.botId == captainBotId)
        let isSession = m.memberKind == "code_session"
        let isHuman   = m.memberKind == "human"
        // emoji + 柔色都用这个 seed:bot=botId、human=userId、session=codeSessionId。
        // 与气泡(senderSessionId)、本地 run(sessionId)取同一值 → 同一 session 一张脸。
        let seed = m.botId ?? m.userId ?? m.codeSessionId ?? m.id
        let name: String = {
            if let n = m.displayName, !n.isEmpty { return n }
            if isCaptain { return "机长" }
            if isHuman   { return "人" }
            if isSession { return sessionFallback(m.codeSessionId) }
            return "bot"
        }()
        return GroupBubbleSender(
            kind: isHuman ? .user : .bot, id: seed, displayName: name,
            avatarPath: nil, avatarSeed: seed, isCaptain: isCaptain,
            sessionStatus: m.sessionStatus, isSession: isSession)
    }
}
