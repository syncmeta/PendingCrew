import Foundation

/// 成员列表排序（Todo #15）——**唯一真值**：右栏成员富列表、顶部 roster 头像条、
/// @ 选择器都过这里，别在某处再自己排一遍。
///
/// 规则：
/// 1. 置顶行（机长 / 人类成员）保持原有置顶语义与相对顺序 —— 它们创建最早，
///    纯倒序会把机长压到最底，那不是人要的「谁在干活一眼看到」。
/// 2. 其余 session 成员按创建时刻**倒序**（最新建的在最上面）。
/// 3. 拿不到创建时刻的排在有时刻的之后；同刻/都没时刻时按 `id` 稳定排。
enum CrewMemberOrdering {
    /// 排序输入键 —— 与 UI 类型解耦，方便单测钉住。
    struct Key: Equatable {
        let id: String
        /// true = 机长/人类等固定置顶行（不参与倒序）。
        let isPinned: Bool
        /// 创建时刻；nil = 未知。
        let createdAt: Date?

        init(id: String, isPinned: Bool, createdAt: Date?) {
            self.id = id
            self.isPinned = isPinned
            self.createdAt = createdAt
        }
    }

    /// ISO8601 字符串 → Date（本地 session 成员的 `createdAt` 就是这个格式）。
    ///
    /// **转发给 `CrewTimestamp.parse`，不在这里自己建格式器**（人类 Todo #140 ①）。
    /// 改动前这里每次调用都新建一到两个 `ISO8601DateFormatter`，而调用方
    /// `CrewSessionWindowView.memberRowItems` 是计算属性 —— 每次 body 求值把全部成员
    /// 重排一遍，于是现场 sample 里 `libicucore` 那 7.1% 全在这条路上。
    ///
    /// `CrewTimestamp` 本来就是「全 app 一份口径」的那一份，尝试顺序（先带小数秒、
    /// 再不带）与改动前这两行逐字相同 —— 所以这是复用既有的那份，不是第二套。
    static func parseDate(_ raw: String?) -> Date? {
        guard let raw, !raw.isEmpty else { return nil }
        return CrewTimestamp.parse(raw)
    }

    /// 通用排序：调用方给出每个元素的 `Key`，返回排好序的元素。
    static func sorted<T>(_ items: [T], key: (T) -> Key) -> [T] {
        let keyed = items.map { (item: $0, key: key($0)) }
        let pinned = keyed.filter { $0.key.isPinned }.map(\.item)
        let rest = keyed.filter { !$0.key.isPinned }.sorted { a, b in
            switch (a.key.createdAt, b.key.createdAt) {
            case let (l?, r?):
                if l != r { return l > r }          // 新的在前
            case (nil, _?):
                return false                        // 无时刻的沉到后面
            case (_?, nil):
                return true
            case (nil, nil):
                break
            }
            return a.key.id < b.key.id              // 稳定兜底
        }.map(\.item)
        return pinned + rest
    }

    /// roster 成员（顶部头像条 / @ 选择器 / 富列表的 server 行）统一排序。
    static func sortedMembers(_ members: [CrewMember], captainBotId: String?) -> [CrewMember] {
        sorted(members) { m in
            Key(
                id: m.id,
                isPinned: m.memberKind == "human" || m.memberKind == "captain"
                    || (m.botId != nil && m.botId == captainBotId),
                createdAt: parseDate(m.createdAt))
        }
    }

    /// 单测用的薄壳：只排 `Key` 本身，返回 id 顺序。
    static func sortedIds(_ keys: [Key]) -> [String] {
        sorted(keys) { $0 }.map(\.id)
    }
}
