#if os(macOS)
import Foundation

/// 「近期群聊」上下文块的**纯渲染核心**（项8）。
///
/// 背景：claude session 被 @ 唤醒的第一拍，PTY 只写「有人@你：xxx」——白板要等
/// 它这一轮首次调工具触发 PostToolUse hook 才补；若它直接口头回应就只看到孤立
/// 一句。给两个注入渲染器（`CrewMailboxWakeLogic` / `CrewLocalMentionInjectLogic`）
/// 前置一段「近期群聊」，让被 @ 的 claude 一醒来就带着上下文，像人回群消息时
/// 面对整屏历史那样。**只对 claude 后端补**（codex 每轮 turn 自带未读白板
/// additionalContext，别重复塞）——claude-only 的判定在调用方。
///
/// 抽成纯函数（对齐 `CrewMailboxWakeLogic` 风格）便于单测：空 → nil、非空 → 渲染
/// 「近期群聊：\n- 发送者: 正文…」。取数（`LocalWhiteboardStore.shared.list`）留在
/// 调用方。
enum CrewRecentContextRender {

    /// 单条正文的软上限——群消息通常很短，但兜个底防某条超长把注入撑爆。
    private static let bodyCap = 200

    /// 把最近若干条白板消息渲染成一块「近期群聊」上下文。空 → nil（调用方据此
    /// 决定是否前置）。调用方已 `.suffix(n)` 截断，这里按传入的原序渲染。
    static func block(_ recent: [LocalWhiteboardMessage]) -> String? {
        guard !recent.isEmpty else { return nil }
        var lines = ["近期群聊："]
        for m in recent {
            // 附件路径提示行（Todo #3 群聊图片）不参与截断 —— 截半截的路径 Read
            // 不了；只对正文做软截断。被 @ 醒来的 claude 上下文里也带全路径。
            let hints = m.attachmentAgentHints.joined(separator: "\n  ")
            var body = truncate(m.text.trimmingCharacters(in: .whitespacesAndNewlines))
            if !hints.isEmpty {
                body = body.isEmpty ? hints : body + "\n  " + hints
            }
            guard !body.isEmpty else { continue }
            lines.append("- \(label(m)): \(body)")
        }
        // 全是空正文（理论上少见）→ 只剩标题头，无意义，返回 nil。
        guard lines.count > 1 else { return nil }
        return lines.joined(separator: "\n")
    }

    /// 发送者标注。优先本地显示名（`senderName`）/ relay 远端名（`senderDisplayName`），
    /// 再按 kind 兜底（user/human → 人类，captain → 机长，session → `session:<前6>`）。
    private static func label(_ m: LocalWhiteboardMessage) -> String {
        if let n = m.senderName, !n.isEmpty { return n }
        if let n = m.senderDisplayName, !n.isEmpty { return n }
        switch m.senderKind {
        case "user", "human": return "人类"
        case "captain": return "机长"
        case "session":
            if let sid = m.senderSessionId, !sid.isEmpty { return "session:\(sid.prefix(6))" }
            return "session"
        default: return m.senderKind
        }
    }

    /// 单行软截断——超 `bodyCap` 截断加省略号，别把某条超长消息把整块撑爆。
    private static func truncate(_ s: String) -> String {
        guard s.count > bodyCap else { return s }
        return String(s.prefix(bodyCap)) + "…"
    }
}
#endif
