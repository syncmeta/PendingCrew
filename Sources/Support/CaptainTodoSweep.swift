import Foundation

/// 机长空闲时被提醒回头看 Todo 账，**直到它给出一份对得上的账才停**（驾驶舱计划 #71）。
///
/// # 人类原话，以及它推翻了什么
///
/// > 不要新给 todo 的时候看。如果机长休眠不干活，pendingcrew 就提醒一次看 todo，
/// > 直到机长确认 都做完了 或者卡在人类这边 明确输出确认应该停止 再停
///
/// 被推翻的方案是「新 Todo 进来时顺手回头看老账」。**新活进来是最差的触发时刻** ——
/// 那时机长最忙，会敷衍地扫一眼就过。而**空闲是对的时刻**：没有别的事跟它抢，而且
/// 空闲本身就意味着机长以为没事干了 —— 那正是该被质问的一刻。
///
/// 还有一层：人类要的是**产品去提醒**，不是让机长养成习惯。机长的习惯会被下一条
/// 消息冲掉，这一点当天就被证明过。
///
/// # 为什么确认必须带账
///
/// 现场（2026-09-08，机长自述）：它一整天以为自己在推进，直到人类问、把账拉出来，
/// 才发现 **23 条未完成里有 9 条是「做完了没翻牌」**，其中两条早在 0.1.26 就发出去了。
/// 它自己的结论是：**「如果确认只需要说一句『都做完了』，我今天会毫不犹豫地说出口。」**
///
/// 所以 `validate` 要求把**每一条未完成的 #N 都归进一个桶**，并集必须跟真账本逐个对上。
/// **让敷衍的成本高于真查** —— 这是整个机制唯一的承重点。归不进任何桶的那一条，
/// 正是「其实早就做完了、只是没翻牌」的那条。
///
/// # 跟 `SupervisionLease` 的边界（为什么不是复用它）
///
/// 两者**共享纪律，不共享代码**：
///
/// - `SupervisionLease` 的轴是**持有关系**，键是 `(crewId, planNumber)` 单条计划，
///   触发靠 `LocalWakeupStore` 的**定时器**，解除靠把那条计划翻到 done/blocked。
/// - 这一条的轴是**整本 Todo 账**，没有单条键，触发是**事件**（机长从忙变闲），
///   解除靠一份能跟账本对上的结构化确认。
///
/// 硬塞进租约模型就得编一个假的 planNumber 和一个假的到期时间 —— 那不是复用，
/// 是把两件事伪装成一件。**真正复用的是那条纪律**：
/// **没有「我知道了 / 已查看 / 顺延」任何一种出口**（有测试钉着）。一旦给了那个出口，
/// 机制必然退化成「看一眼就算办完」，那正是 `SupervisionLease` 里点名要避免的东西。
///
/// # 「永远在响的提醒会变成背景噪音」怎么处理
///
/// 机长几乎总有未完成 Todo，那是常态。所以**闭嘴的条件不是一个时长，是一个集合**：
/// 确认覆盖的是**那一批具体条目**，只要没冒出没被覆盖过的新条目，隔多久都不再响
/// （`decide` 用 superset 判定，不是相等）。条目变少（做完翻牌了）也不重新响 ——
/// 否则就成了「翻牌反而挨骂」。
///
/// 纯 Foundation、无 IO、无 actor —— 所以上面每一条都能脱离 app 直接单测。
enum CaptainTodoSweep {

    /// 一次被接受的确认。**记下它覆盖了哪些条目** —— 下次判断「这批变没变」全靠它。
    struct Confirmation: Equatable, Codable {
        let confirmedAt: String
        /// 确认那一刻账上所有未完成条目的 #N。
        let openNumbers: [Int]
    }

    enum Decision: Equatable {
        /// 不提醒。附一句人话的理由 —— 这条会进日志/回执，说不清为什么闭嘴的机制
        /// 没法被信任，也没法被调试。
        case silent(String)
        case remind(String)

        var isSilent: Bool { if case .silent = self { return true }; return false }
    }

    /// 确认被拒的三种原因。**每一种都要能原样说给机长听**，不许含糊成一句「失败」——
    /// 它得知道该去看哪几条。
    enum Refusal: Equatable {
        /// 有未完成条目没被归进任何桶。**空确认走的就是这条。**
        case missing([Int])
        /// 报了账上没有、或已经完成了的 #N。
        case unknown([Int])
        /// 同一条被放进了两个桶。
        case overlapping([Int])

        var summary: String {
            switch self {
            case let .missing(ns):
                return "这几条未完成的还没被归进任何一桶：\(Self.list(ns))。"
                    + "每一条都得说清它在跑、卡人类、还是排队——**归不进去的那条，多半是你早就做完了却没翻牌**。"
            case let .unknown(ns):
                return "这几个 #N 不在未完成之列（不存在、或已经是完成态）：\(Self.list(ns))。先 respond_todo 核一下。"
            case let .overlapping(ns):
                return "这几条被同时放进了两个桶：\(Self.list(ns))。一条只能在一个桶里。"
            }
        }

        private static func list(_ ns: [Int]) -> String {
            ns.map { "#\($0)" }.joined(separator: "、")
        }
    }

    struct ValidationResult: Equatable {
        var confirmation: Confirmation?
        var refusal: Refusal?
    }

    // MARK: - 提醒不提醒

    /// - `open`: 这本账此刻所有**未完成**条目的 #N。
    /// - `confirmation`: 上一次被接受的确认（没有 = 从没确认过）。
    /// - `lastRemindedAt`: 上一次真的提醒过的时刻（防抖用）。
    /// - `minimumInterval`: 两次提醒之间的地板间隔。
    static func decide(open: Set<Int>, confirmation: Confirmation?, lastRemindedAt: Date?,
                       now: Date, minimumInterval: TimeInterval) -> Decision {
        // ① 一条未完成都没有 —— 没什么可问的。**一条永远报「已知没事」的提醒会训练
        // 人忽略整个通道**，所以这里必须真闭嘴，而不是发一条「都做完了，很好」。
        guard !open.isEmpty else { return .silent("这本账没有未完成条目") }

        // ② 确认覆盖了当下这批 —— 闭嘴，**与过去多久无关**。
        // 用 superset 而不是相等：条目变少是「做完翻牌了」，那是好事，
        // 相等判定会让翻牌反而招来一次提醒。
        if let confirmation, Set(confirmation.openNumbers).isSuperset(of: open) {
            return .silent("已确认过这批，且没有新的未完成条目")
        }

        // ③ 地板间隔 —— 机长在忙/闲之间抖一下不该被刷屏。
        if let lastRemindedAt, now.timeIntervalSince(lastRemindedAt) < minimumInterval {
            return .silent("刚提醒过，还没到最短间隔")
        }

        let uncovered = confirmation.map { open.subtracting($0.openNumbers) } ?? open
        return .remind(text(open: open, uncovered: uncovered.sorted()))
    }

    private static func text(open: Set<Int>, uncovered: [Int]) -> String {
        let newlyLine = uncovered.count == open.count
            ? ""
            : "\n- 其中**没被你上次确认覆盖过**的是：\(uncovered.map { "#\($0)" }.joined(separator: "、"))。"
        return """
        你停下来了，但这本 Todo 账上还有 \(open.count) 条没完成。\(newlyLine)

        **空闲正是该核账的一刻** —— 现在没有别的事跟它抢；而「我以为没事干了」恰恰是最该被质问的那一刻。

        去把这 \(open.count) 条逐条看一遍（`respond_todo` 那本，人类派给你的活），然后调 `confirm_todo_sweep`，把每一条都归进一个桶：
        - `running`：正在跑 / 有人在做
        - `blocked_on_human`：卡在人类那边，你推不动
        - `queued`：还没开始，排着

        **三个桶的并集必须等于这 \(open.count) 条，一条不多一条不少**，否则会被拒。
        这不是形式：**归不进任何桶的那一条，多半是你早就做完了、只是没翻牌**（有过 23 条里 9 条是这种的先例）。

        确认之前它还会再提醒你。这里没有「我知道了」「顺延」这种动作。
        """
    }

    // MARK: - 确认：承重点

    /// 三个桶必须**恰好**铺满 `open`。任何一种对不上都拒绝，并说清是哪几条。
    static func validate(running: [Int], blockedOnHuman: [Int], queued: [Int],
                         open: Set<Int>) -> ValidationResult {
        let buckets = running + blockedOnHuman + queued

        var seen = Set<Int>()
        var duplicated = Set<Int>()
        for n in buckets where !seen.insert(n).inserted { duplicated.insert(n) }
        if !duplicated.isEmpty {
            return ValidationResult(confirmation: nil, refusal: .overlapping(duplicated.sorted()))
        }

        let unknown = seen.subtracting(open)
        if !unknown.isEmpty {
            return ValidationResult(confirmation: nil, refusal: .unknown(unknown.sorted()))
        }

        let missing = open.subtracting(seen)
        if !missing.isEmpty {
            return ValidationResult(confirmation: nil, refusal: .missing(missing.sorted()))
        }

        return ValidationResult(
            confirmation: Confirmation(
                confirmedAt: ISO8601DateFormatter().string(from: Date()),
                openNumbers: open.sorted()),
            refusal: nil)
    }

    // MARK: - 参数

    /// 两次提醒之间的地板间隔。比这更密就是刷屏 —— 机长在忙/闲之间抖一下会触发好几次。
    static let minimumRemindInterval: TimeInterval = 15 * 60
}
