import Foundation

/// 排序用的**原料**（Todo #102 / 人类 #113 追问）。
///
/// 人类原话：「不要按什么算 而是把各个指标拿出来 还有总机长的群聊信息 作为上下文
/// 让总机长自行判断顺序 什么应该放前面」。
///
/// 所以这里**不排序、不打分、不加权**。三列各自成列，原样交出去。
/// 「①×0.6 + ②×0.4」那种东西是**还在替他算，只是算得更花哨** —— 明令不做。
///
/// ## 三列各自的边界，必须跟着数一起交出去
/// 一个没有边界的读数不是更干净，是**这栏没被要过**。所以渲染时每一列都带上
/// 它自己的毛病：
/// - **① 最近有动静**（任何人发的最后一条）—— 覆盖最全、有分辨率，
///   但它量的是「哪儿刚有动静」，而动静大多是 agent 发的。
/// - **② 人类自己最后发言** —— 语义最贴「他手头在用的」，但**没有分辨率**：
///   他习惯只在机长群里说话、不挨个进子 crew，于是第二名往往就掉到几天前。
/// - **③ 人类最后打开** —— 语义最贴，2026-09-08 才开始埋点，**早期一定很稀疏**。
///   缺了照排，别把它当成非它不可的依赖。
enum CrewOrderingSignals {
    /// ③ 那一列此刻的状态。
    enum OpenedColumn: Equatable, Sendable {
        /// 读到了，而且里面有记录。
        case hasData
        /// 读到了，里面**确实**一条都没有（埋点刚加，还没攒到）。
        case emptySoFar
        /// **根本读不出来**（镜像文件不在 / 坏了）—— 这一列此刻是「我看不出来」，
        /// 不是「他没打开过」。
        case unreadable
    }

    struct Row: Equatable, Sendable {
        let crewId: String
        let title: String
        /// ① 任何人最后一条消息。
        let lastAnyMessageAt: Date?
        /// ② 人类自己最后一条消息（`senderKind == "user"`）。
        let lastHumanMessageAt: Date?
        /// ③ 人类最后一次打开这个 crew。
        let lastOpenedAt: Date?
    }

    /// 排出一张给 agent 读的表。**行序只是为了稳定，不是建议顺序** ——
    /// 判断谁在前是它的活，不是这张表的活。
    ///
    /// - Parameters:
    ///   - rows: 每个 crew 一行。
    ///   - now: 现在（算「多久之前」用；测试注入）。
    ///   - openedColumn: ③ 这一列的状态。**三态，不许压成两态** ——
    ///     「有数据」「读出来了但一条没有」「根本读不出来」在下游要说成不同的话。
    ///     把「读不出来」说成「一条没有」，就是把「我看不出来」报成「确实没有」，
    ///     这台机器上栽过（`SessionOutputEvidence` 那条同族的账）。
    static func render(rows: [Row], now: Date, openedColumn: OpenedColumn) -> String {
        guard !rows.isEmpty else { return "（这台机器上没有 crew）" }
        let sorted = rows.sorted { lhs, rhs in
            switch (lhs.lastAnyMessageAt, rhs.lastAnyMessageAt) {
            case let (l?, r?): if l != r { return l > r }
            case (nil, _?): return false
            case (_?, nil): return true
            case (nil, nil): break
            }
            return lhs.crewId < rhs.crewId
        }
        var lines: [String] = []
        lines.append("排序原料（三列各自成列，没有加权、没有推荐顺序 —— 判断是你的活）：")
        lines.append("① 最近有动静 / ② 人类最后发言 / ③ 人类最后打开 · 都是「多久之前」")
        for row in sorted {
            let a = ago(row.lastAnyMessageAt, now: now)
            let h = ago(row.lastHumanMessageAt, now: now)
            let o = openedColumn == .hasData ? ago(row.lastOpenedAt, now: now) : "—"
            lines.append("- \(row.title)｜① \(a)｜② \(h)｜③ \(o)｜id=\(row.crewId)")
        }
        lines.append("")
        lines.append("每一列的边界（**跟数一起看，别单看数**）：")
        lines.append("· ① 覆盖最全、有分辨率，但它量的是「哪儿刚有动静」，而动静大多是 agent 发的。")
        lines.append("· ② 语义最贴「他手头在用的」，但**分辨率很低** —— 他习惯只在机长群里说话、"
                     + "不挨个进子 crew，第二名往往就掉到几天前。")
        switch openedColumn {
        case .hasData:
            lines.append("· ③ 2026-09-08 才开始埋点，**早期很稀疏**；有就当加分项，没有别硬凑。")
        case .emptySoFar:
            lines.append("· ③ **读到了，但里面确实还一条都没有**（埋点 2026-09-08 才加）。"
                         + "空白是「还没攒到」，**不是「他谁都没打开过」** —— 别拿它当证据。")
        case .unreadable:
            lines.append("· ③ **这一列这次读不出来**（那份镜像不在或坏了）——"
                         + "所以它现在的空白是「我看不出来」，**不是「一条都没有」**。"
                         + "这两件事别混着用；这一轮就当没有这一列。")
        }
        return lines.joined(separator: "\n")
    }

    /// 「多久之前」。没有值 → `—`（明确的空，不是 0）。
    static func ago(_ date: Date?, now: Date) -> String {
        guard let date else { return "—" }
        let seconds = now.timeIntervalSince(date)
        if seconds < 0 { return "刚刚" }
        if seconds < 60 { return "\(Int(seconds)) 秒前" }
        if seconds < 3600 { return "\(Int(seconds / 60)) 分钟前" }
        if seconds < 86_400 { return "\(Int(seconds / 3600)) 小时前" }
        return "\(Int(seconds / 86_400)) 天前"
    }
}
