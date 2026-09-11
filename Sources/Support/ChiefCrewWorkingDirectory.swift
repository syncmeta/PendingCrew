import Foundation

/// **总机组那一层的工作目录该填什么**（人类 Todo #141 / #145 的前提）。
///
/// ## 为什么这件事需要一个判据，而不是一行赋值
/// 总机组进名册时 `workingDirectory` 写死为 nil，于是 `CrewSessionRunner` 那四道
/// guard（无 cwd 一律不许起 session）把它挡死 —— **总机组永远起不了机长**，
/// 连带「总机长排序/写摘要」整条线都不成立。2026-09-12 实测：往总机组发一条汇报，
/// 群里出现的是「自动拉起机长失败：这个 crew 没有工作目录」。
///
/// guard 是对的，不放宽：session 是要落地干活的，没有 cwd 本来就不该起。
/// 要补的是**给它一个目录**。
///
/// ## 为什么不是硬编码，也不是「最近用过的目录」
/// - **硬编码绝对路径**：那是某一台机器的事实，不是产品的事实。
/// - **`RecentWorkingDirectories`（UserDefaults 的 MRU）**：仓里唯一一条现成的
///   「本机默认工作目录」，但 2026-09-12 实测它第一项是 `~/Untitled/tv-transfer`
///   —— 它记的是「人最近一次建 crew 挑了哪儿」，**不是「这台机器的活主要在哪儿」**。
///   拿它当默认，总机长会被扔进一个跟 crew 工具链毫不相干的目录。
///
/// 用的是**名册里现有 crew 的工作目录取众数**：它直接回答「这台机器的活主要在
/// 哪个目录」，且完全由真实数据推出。本机实测第一名 19 次、第二名 7 次。
///
/// ## 拿不出答案时**回 nil，不编**
/// 全新机器上一个 crew 都没有 → 没有众数 → nil。那时总机组照旧没有工作目录，
/// 四道 guard 会如实说「这个 crew 没有工作目录」，人再 `change_workdir` 指一个。
/// **编一个不存在的路径出来，会把「还没设」变成「设了但是错的」——后者更难查。**
enum ChiefCrewWorkingDirectory {

    /// - Parameters:
    ///   - existing: 名册里**其它** crew 的工作目录（nil / 空串已经可以直接传进来）。
    ///   - exists: 判断某个路径此刻还在不在（注入，便于测试）。
    /// - Returns: 出现次数最多、且此刻仍然存在的那个目录；并列时取字典序最小的
    ///   那个（**全序**，别让同一份输入两次给出不同答案）。没有候选 → nil。
    static func resolve(
        existing: [String?],
        exists: (String) -> Bool
    ) -> String? {
        var counts: [String: Int] = [:]
        for raw in existing {
            guard let path = raw?.trimmingCharacters(in: .whitespaces), !path.isEmpty
            else { continue }
            guard exists(path) else { continue }
            counts[path, default: 0] += 1
        }
        // 全序：先比次数，次数相同比路径 —— 同一份输入每次都给同一个答案。
        return counts.max { lhs, rhs in
            if lhs.value != rhs.value { return lhs.value < rhs.value }
            return lhs.key > rhs.key
        }?.key
    }
}
