import Foundation

/// 「只看 @ 我的消息」筛选器的**判定核心**（Todo #61）—— 一条群聊消息算不算
/// 点名了人类。纯函数，不碰 SwiftUI、不碰 @State，供中栏筛选开关直接喂数组。
///
/// ## 为什么不能只认结构化 mentions
///
/// `@人类` 在数据里有**两种**存法，两边都不是小数：
///
/// 1. **结构化** —— `CrewWhiteboardEntry.mentions` 里有一项 `kind == "human"`
///    （语义定义见 `CrewWhiteboardVisibility`；Mac 侧从 @-popover / 头像右键发出，
///    agent 侧从 `post_to_crew(mentions:)` 发出）。
/// 2. **只有正文里写了 `@<人类显示名>`** —— `McpServer.parseMentions` 只读
///    `mentions` 参数、**从不解析正文**。agent（包括机长）在正文顶上敲一句
///    「@人 ……」但没传 `mentions` 时，结构化字段是空的。
///
/// 本机真实白板全量统计（2026-08-23，52 个白板文件 5257 条）：带结构化 human
/// mention 578 条、正文里出现 `@人` 357 条、两者都有 223 条、**并集 712 条**。
/// 即：只认结构化会漏 134 条他真正被点名的消息，只认正文会漏 355 条。**必须取
/// 并集** —— 只做一半的话，人打开筛选器会发现「我明明被 @ 了却不在列表里」，
/// 比没有这个筛选器更糟。
///
/// ## 「@自己」在数据层面只能做到「@任何人类」
///
/// 结构化 human mention 在**实际数据里恒无 `target_id`**（上面那 578 条逐条查过，
/// 578/578 的 `target_id` 都是 nil —— agent 侧 `post_to_crew` 只传 `{kind:"human"}`）。
/// Mac 侧两条发送路（`CrewComposerMentions.candidates`、`CrewChatView.avatarMention`）
/// 确实会写 `target_id = userId`，但那只覆盖人自己在 app 里 @ 别人的那一小部分，
/// **按 target_id 收紧会把 agent 发的 578 条全滤掉**。所以这里一律认「任何
/// `kind == "human"`」。
///
/// 后果：多人类 crew 下区分不了「@我」和「@另一个人类」。当前每个 crew 只有一个
/// 人类成员，两者等价。**这是数据模型的限制，不是本实现的缺陷** —— 真要区分，得
/// 先让 agent 侧的 human mention 带上 target_id。
///
/// ## 正文匹配为什么要花名册
///
/// 不能硬编码「人」这个字：本机人类显示名恰好叫「人」（`CrewSenderNaming` 的兜底），
/// 换个 crew 就不是了。正文匹配一律拿**该 crew 人类成员的显示名**拼 `@`，与
/// `CrewChatView.avatarMention` 的 `"@" + sender.displayName` 同一套名字。
///
/// 而中文没有词边界，裸拼 `"@" + "人"` 会把 `@人机交互组` 这类**以人类名开头的更长
/// 成员名**一起命中。所以判据是：在每个 `@` 处取**花名册里能匹配的最长**成员名 ——
/// 最长那个是人类名才算命中。`@人机交互组`（花名册里真有这个成员）→ 最长匹配是
/// 「人机交互组」，不算命中。
///
/// 残留的已知取舍：`@人事`、而「人事」**不在**花名册时，最长匹配仍是「人」→ 算命中。
/// 花名册之外的词无从判定，这里选择**宁可多收一条也不漏**（漏 = 人以为没被 @ 到）。
enum CrewMentionFilter {

    /// 判定用的花名册快照：人类显示名 + 全员显示名（前缀消歧用）。
    ///
    /// 两份名字都在构造时归一化：去掉前导 `@`（relay 远端名可能自带）、去掉首尾
    /// 空白、丢掉空串、去重。`allNames` 恒包含 `humanNames`。
    struct Roster: Equatable {
        /// 本 crew 人类成员的显示名（已归一化）。空 = 正文匹配整条关掉。
        let humanNames: [String]
        /// 本 crew 全体成员显示名（已归一化），用于「@X 到底 @ 的是谁」的最长匹配。
        let allNames: [String]

        init(humanNames: [String], otherNames: [String] = []) {
            let humans = Roster.normalize(humanNames)
            self.humanNames = humans
            self.allNames = Roster.normalize(humans + otherNames)
        }

        /// 从 crew 成员列表构造。显示名取 `CrewSenderNaming.groupSender` 那一份
        /// （气泡 / 成员列表 / @-菜单共用的单一真值，含「人」「机长」这些兜底名）。
        static func from(members: [CrewMember], captainBotId: String?) -> Roster {
            var humans: [String] = []
            var others: [String] = []
            for m in members {
                let name = CrewSenderNaming.groupSender(for: m, captainBotId: captainBotId).displayName
                if m.memberKind == "human" { humans.append(name) } else { others.append(name) }
            }
            return Roster(humanNames: humans, otherNames: others)
        }

        /// 去前导 `@` + trim + 去空 + 去重（保序）。
        private static func normalize(_ names: [String]) -> [String] {
            var seen = Set<String>()
            var out: [String] = []
            for raw in names {
                let n = String(raw.drop(while: { $0 == "@" }))
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                guard !n.isEmpty else { continue }
                let key = n.lowercased()
                guard seen.insert(key).inserted else { continue }
                out.append(n)
            }
            return out
        }
    }

    // MARK: - 判定

    /// 判定核心：这条消息算不算点名了人类。**结构化与正文取并集**（命中一次即可，
    /// 两边都有也只是 true，不重复计数）。
    ///
    /// - Parameters:
    ///   - mentionKinds: 这条消息结构化 mentions 的 `kind` 列表（无 mentions 传空）。
    ///   - text:         这条消息的正文（`CrewWhiteboardEntry.displayText`）。
    ///   - roster:       花名册快照。
    static func isHumanMention(mentionKinds: [String], text: String, roster: Roster) -> Bool {
        if mentionKinds.contains("human") { return true }
        return bodyMentionsHuman(text: text, roster: roster)
    }

    /// 一条白板条目算不算点名了人类。
    static func isHumanMention(_ entry: CrewWhiteboardEntry, roster: Roster) -> Bool {
        isHumanMention(
            mentionKinds: (entry.mentions ?? []).map(\.kind),
            text: entry.displayText, roster: roster)
    }

    /// 这条是不是 `localUserId` 本人发的。
    static func isFromSelf(_ entry: CrewWhiteboardEntry, localUserId: String?) -> Bool {
        guard let me = localUserId, !me.isEmpty else { return false }
        return entry.senderUserId == me
    }

    /// 「给我一个数组、还我筛过的数组」—— 筛选开关直接喂 `timelineEntries`。
    /// 保持输入序。`roster.humanNames` 为空时正文那一半自动失效，结构化那一半照常。
    ///
    /// **自己发的消息一并留下**（`localUserId` 非 nil 时）：只留「@ 我」的话，人会
    /// 看到一串回应却看不到自己说了什么，读起来是断的 —— 一问一答里只剩答。传 nil
    /// 可关掉这一条（纯粹只看被 @ 的）。
    static func onlyHumanMentions(
        _ entries: [CrewWhiteboardEntry], roster: Roster, includingFrom localUserId: String? = nil
    ) -> [CrewWhiteboardEntry] {
        entries.filter {
            isHumanMention($0, roster: roster) || isFromSelf($0, localUserId: localUserId)
        }
    }

    // MARK: - 正文匹配

    /// 正文里有没有一处 `@` 指向人类成员。
    ///
    /// 逐个扫 `@`，在每个 `@` 之后取**花名册里能匹配的最长**成员名（大小写不敏感）：
    /// 最长那个是人类名 → 命中。没有任何成员名匹配上 → 这个 `@` 不算数（`@人`
    /// 这种「人」在花名册里时仍会匹配到「人」本身）。
    static func bodyMentionsHuman(text: String, roster: Roster) -> Bool {
        guard !roster.humanNames.isEmpty, !text.isEmpty else { return false }
        let humans = Set(roster.humanNames.map { $0.lowercased() })
        var cursor = text.startIndex
        while cursor < text.endIndex, let at = text[cursor...].firstIndex(of: "@") {
            let after = text.index(after: at)
            if after < text.endIndex {
                let rest = text[after...]
                var best: String?
                for name in roster.allNames
                where rest.range(of: name, options: [.caseInsensitive, .anchored]) != nil {
                    if best == nil || name.count > best!.count { best = name }
                }
                if let best, humans.contains(best.lowercased()) { return true }
            }
            cursor = after
        }
        return false
    }
}
