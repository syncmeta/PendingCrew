import Foundation

/// 新建 crew 时用过的工作目录 MRU 列表（Todo #29）。
///
/// 旧版只在 `pendingcrew.lastWorkingDirectory` 存**一个**最近目录，sheet 上只能
/// 「用上次」。现在改成最多 `limit` 条的最近列表：新用的置顶去重、超出截断、
/// 旧的单值 key 迁移进列表（别丢用户上次用的那个）、路径已经不存在的不显示。
///
/// 纯逻辑（不碰 AppKit / 不读磁盘 —— 存在性判定由调用方注入），三端可编、可单测。
enum RecentWorkingDirectories {
    static let key = "pendingcrew.recentWorkingDirectories"
    /// 旧的单值 key（v0.1.5 及以前）。读时迁移，不再写。
    static let legacyKey = "pendingcrew.lastWorkingDirectory"
    static let limit = 5

    /// 把 `path` 置顶到列表最前，去重（同一路径只留最新那次），超出 `limit` 截断。
    /// 空路径原样返回，不污染列表。
    static func promoting(_ path: String, into list: [String], limit: Int = limit) -> [String] {
        guard !path.isEmpty else { return Array(list.prefix(limit)) }
        var out = [path]
        out.append(contentsOf: list.filter { $0 != path && !$0.isEmpty })
        return Array(out.prefix(limit))
    }

    /// 读出来的列表 + 旧单值 key 合并：列表在前，旧值补在尾巴（列表里已有就不重复）。
    static func migrated(list: [String]?, legacy: String?, limit: Int = limit) -> [String] {
        var out = (list ?? []).filter { !$0.isEmpty }
        if let legacy, !legacy.isEmpty, !out.contains(legacy) {
            out.append(legacy)
        }
        return Array(out.prefix(limit))
    }

    /// 过滤掉已经不存在的路径（目录被删/改名后不该还挂在 sheet 上）。
    static func existing(_ list: [String], exists: (String) -> Bool) -> [String] {
        list.filter(exists)
    }

    // MARK: - UserDefaults 通道

    /// 从 defaults 读 → 迁移旧 key → 过滤不存在 → 返回可直接显示的列表。
    static func load(from defaults: UserDefaults,
                     exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> [String] {
        let stored = defaults.stringArray(forKey: key)
        let merged = migrated(list: stored, legacy: defaults.string(forKey: legacyKey))
        return existing(merged, exists: exists)
    }

    /// 记一次「刚用了这个目录」：置顶去重后写回列表；旧单值 key 同步写一份，
    /// 便于回滚到旧版本时不丢。
    static func record(_ path: String, in defaults: UserDefaults) {
        let current = migrated(list: defaults.stringArray(forKey: key),
                               legacy: defaults.string(forKey: legacyKey))
        defaults.set(promoting(path, into: current), forKey: key)
        defaults.set(path, forKey: legacyKey)
    }
}
