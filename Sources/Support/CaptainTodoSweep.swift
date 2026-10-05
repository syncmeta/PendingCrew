import Foundation

/// Idle reconciliation is a checkpoint, never permanent forgiveness of open work.
/// IO faults remain system faults and must not create recurring model turns.
enum CaptainTodoSweep {

    /// 一次被接受的确认。**记下它覆盖了哪些条目** —— 下次判断「这批变没变」全靠它。
    struct Confirmation: Equatable, Codable {
        var confirmedAt: String
        /// 确认那一刻账上所有未完成条目的 #N。
        let openNumbers: [Int]
        var followUps: [FollowUp]? = nil
        var blockers: [Blocker]? = nil
        var revisions: [String: String]? = nil
    }

    struct FollowUp: Codable, Equatable {
        let number: Int
        let ownerSessionId: String
        let nextAction: String
        let checkAt: String
        var stage: String? = nil
        var evidence: String? = nil
    }

    struct Blocker: Codable, Equatable {
        let number: Int
        let humanTodoNumber: Int
    }

    struct Item: Equatable {
        let number: Int
        let status: String
        let revision: String
        let acceptedOwner: String?

        init(_ row: LocalTodoItem) {
            number = row.number
            status = row.status
            // Include responses as well as the item stamp: human replies and
            // reopened tasks must invalidate an older accounting checkpoint.
            revision = row.effectiveUpdatedAt + "|" + row.status + "|" + String(row.responses.count)
            acceptedOwner = row.responses.last(where: { $0.status != nil })
                .flatMap { $0.status == "in_progress" ? $0.sessionId : nil }
        }
    }

    /// 账本这一眼读到了什么。**三态（有 / 真空 / 读不到），不许压成 `Set<Int>`。**
    ///
    /// 第一版这里就是 `Set<Int>`，于是「读不出来」只能被当成空集，判定照着空集一算
    /// 就**安静地说没事** —— 一条存在意义就是「不让沉默发生」的通道，自己在读失败时
    /// 沉默了。那是自我否定，不是保守失败。
    enum LedgerSnapshot: Equatable {
        case read(Set<Int>) // Legacy callers; confirmations have bounded grace.
        case items([Item], humanOpen: Set<Int>, liveOwners: Set<String>)
        case unreadable
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
        case invalidCheckpoint(String)

        var summary: String {
            switch self {
            case .invalidCheckpoint(let detail): return detail
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
    static func decide(open snapshot: LedgerSnapshot, confirmation: Confirmation?,
                       lastRemindedAt: Date?, unreadableStreak: Int,
                       now: Date, minimumInterval: TimeInterval) -> Decision {
        // 账本 IO 故障不是 agent 能修复的待办：错误留给系统告警/界面，不能
        // 借空闲核账把同一事故周期性塞给 LLM（人类 Todo #157）。读得回来后，
        // 正常未完成条目的核账提醒仍照旧。
        if case .unreadable = snapshot {
            return .silent("账本读不出来；已由系统错误通道报告，不唤醒 agent")
        }
        let open: Set<Int>
        let items: [Item]
        let humanOpen: Set<Int>
        let liveOwners: Set<String>
        switch snapshot {
        case .read(let numbers):
            open = numbers; items = []; humanOpen = []; liveOwners = []
        case .items(let rows, let humans, let owners):
            open = Set(rows.map(\.number)); items = rows
            humanOpen = humans; liveOwners = owners
        case .unreadable: return .silent("账本读不出来")
        }
        guard !open.isEmpty else { return .silent("这本账没有未完成条目") }
        if let confirmation, Set(confirmation.openNumbers).isSuperset(of: open) {
            if let followUps = confirmation.followUps,
               let blockers = confirmation.blockers,
               let revisions = confirmation.revisions, !items.isEmpty {
                let covered = items.allSatisfy { item in
                    guard revisions[String(item.number)] == item.revision else { return false }
                    if let blocker = blockers.first(where: { $0.number == item.number }) {
                        return item.status == "blocked_on_human"
                            && humanOpen.contains(blocker.humanTodoNumber)
                    }
                    guard let next = followUps.first(where: { $0.number == item.number }),
                          liveOwners.contains(next.ownerSessionId),
                          let date = parseDate(next.checkAt) else { return false }
                    return date > now
                }
                if covered { return .silent("未到检查点；负责人或人类阻塞仍有效") }
            } else if let at = parseDate(confirmation.confirmedAt),
                      now.timeIntervalSince(at) < minimumInterval {
                return .silent("旧确认暂缓一次；到期重新核账")
            }
        }

        // ③ 地板间隔 —— 机长在忙/闲之间抖一下不该被刷屏。
        if let lastRemindedAt, now.timeIntervalSince(lastRemindedAt) < minimumInterval {
            return .silent("刚提醒过，还没到最短间隔")
        }

        let uncovered = confirmation.map { open.subtracting($0.openNumbers) } ?? open
        return .remind(text(open: open, uncovered: uncovered.sorted()))
    }

    /// 对可读且有未完成条目的账，生成给机长的一次核账提醒。
    private static func text(open: Set<Int>, uncovered: [Int]) -> String {
        let newlyLine = uncovered.isEmpty ? "" : "\n需要核对：" + uncovered.map { "#\($0)" }.joined(separator: "、")
        return """
        你停下来了，但这本 Todo 账上还有 \(open.count) 条没完成。\(newlyLine)
        核对真实产出和负责人回执，继续推进可执行事项；做完了没翻牌的用 respond_todo 清账。
        调 confirm_todo_sweep 把全部未完成事项归入 running、blocked_on_human 或 queued。
        running 必须有该负责人 respond_todo(in_progress) 的受理回执和当前在线证据。
        running / queued 每条都要 follow_ups：number、owner_session_id、next_action、check_at（ISO8601，未来一小时内）。
        blocked_on_human 每条都要 blockers：number、human_todo_number，指向仍在等人答复的真实问题。
        派出消息只算排队；检查点到期、负责人退出或阻塞解除后会重新核账。没有「我知道了 / 顺延」出口。
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

    /// Runtime validation checks receipts, live ownership and actual blockers.
    static func validate(running: [Int], blockedOnHuman: [Int], queued: [Int],
                         items: [Item], followUps: [FollowUp], blockers: [Blocker],
                         humanOpen: Set<Int>, liveOwners: Set<String>, now: Date) -> ValidationResult {
        let partition = validate(running: running, blockedOnHuman: blockedOnHuman,
                                 queued: queued, open: Set(items.map(\.number)))
        guard var confirmation = partition.confirmation else { return partition }
        func reject(_ detail: String) -> ValidationResult {
            .init(confirmation: nil, refusal: .invalidCheckpoint(detail))
        }
        guard Set(followUps.map(\.number)) == Set(running + queued),
              followUps.count == running.count + queued.count,
              Set(blockers.map(\.number)) == Set(blockedOnHuman),
              blockers.count == blockedOnHuman.count else {
            return reject("running / queued 每条必须有且仅有一个 follow_ups；blocked_on_human 每条必须有且仅有一个 blockers。")
        }
        for next in followUps {
            let stage = next.stage ?? (running.contains(next.number) ? "running" : "queued")
            guard ["queued", "running", "awaiting_review", "blocked_on_system"].contains(stage),
                  !(stage == "running" && !running.contains(next.number)),
                  !(stage == "queued" && running.contains(next.number)) else {
                return reject("#\(next.number) stage 与归桶不符；可选 queued / running / awaiting_review / blocked_on_system。")
            }
            if stage == "awaiting_review" || stage == "blocked_on_system" {
                guard !(next.evidence ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                    return reject("#\(next.number) 等待验收须有产物位置；系统阻塞须有错误证据。")
                }
            }
            guard !next.nextAction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  liveOwners.contains(next.ownerSessionId),
                  let check = parseDate(next.checkAt), check > now,
                  check.timeIntervalSince(now) <= 3600 else {
                return reject("#\(next.number) 缺少在线负责人、具体下一动作，或未来一小时内的有效检查点。未接活的 worker 应归 queued，由机长负责下一步。")
            }
            if running.contains(next.number),
               !items.contains(where: { $0.number == next.number && $0.status == "in_progress"
                    && $0.acceptedOwner == next.ownerSessionId }) {
                return reject("#\(next.number) 没有该负责人的 respond_todo(in_progress) 受理回执；派出消息不算开工，请归 queued。")
            }
        }
        for blocker in blockers {
            guard humanOpen.contains(blocker.humanTodoNumber),
                  items.contains(where: { $0.number == blocker.number && $0.status == "blocked_on_human" }) else {
                return reject("#\(blocker.number) 的人类阻塞无效；请指向仍未答复的人类 Todo，或改回 queued / running 并继续。")
            }
        }
        confirmation.confirmedAt = ISO8601DateFormatter().string(from: now)
        confirmation.followUps = followUps.map { next in
            var row = next
            row.stage = next.stage ?? (running.contains(next.number) ? "running" : "queued")
            return row
        }
        confirmation.blockers = blockers
        confirmation.revisions = Dictionary(uniqueKeysWithValues: items.map { (String($0.number), $0.revision) })
        return .init(confirmation: confirmation, refusal: nil)
    }

    static func progressLine(item: LocalTodoItem, confirmation: Confirmation?,
                             liveOwners: Set<String>, now: Date,
                             ownerNames: [String: String] = [:], humanOpen: Set<Int>? = nil) -> String? {
        guard !item.isSettled, !item.isDeleted else { return nil }
        guard let confirmation, confirmation.revisions?[String(item.number)] == Item(item).revision else {
            return "待核账 · 尚无有效负责人和检查点"
        }
        if let blocker = confirmation.blockers?.first(where: { $0.number == item.number }) {
            let phase = humanOpen.map { $0.contains(blocker.humanTodoNumber) ? "等人答复" : "答复已到 · 待接续" } ?? "待核对人类阻塞"
            return "\(phase) · 人类 Todo #\(blocker.humanTodoNumber)"
        }
        guard let next = confirmation.followUps?.first(where: { $0.number == item.number }) else { return nil }
        let phase: String
        if !liveOwners.contains(next.ownerSessionId) { phase = "负责人不在线 · 待接续" }
        else if parseDate(next.checkAt).map({ $0 <= now }) ?? true { phase = "检查点已到 · 待核账" }
        else {
            switch next.stage {
            case "running": phase = "已接活 · 执行中"
            case "awaiting_review": phase = "已有产物 · 等待验收"
            case "blocked_on_system": phase = "系统阻塞"
            default: phase = "排队 · 等待接活"
            }
        }
        let check = parseDate(next.checkAt).map { $0.formatted(date: .omitted, time: .shortened) } ?? next.checkAt
        let owner = ownerNames[next.ownerSessionId] ?? "待核对成员"
        return "\(phase) · 负责人 \(owner) · 检查 \(check)\n下一步：\(next.nextAction)"
    }

    static func parseDate(_ stamp: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        if let date = formatter.date(from: stamp) { return date }
        formatter.formatOptions.insert(.withFractionalSeconds)
        return formatter.date(from: stamp)
    }

    // MARK: - 读不出来时的退避（计划 #98）

    /// 这一拍之后「连续几次因为读不出来而提醒」该记成几。
    ///
    /// - 读得回来 → **0**，不管这一拍提不提醒。读得回来之后恢复正常判定，
    ///   下一窗故障是一件新情况，不许被上一窗的档位压着。
    /// - 读不出来、真叫出去了 → +1。
    /// - 读不出来、这一拍闭嘴 → 不动。**没叫出去不算叫过一次**（与
    ///   `SupervisionLease.reschedule` 的 `delivered` 同一条纪律），否则间隔会在
    ///   没人听见的时候越拉越长。
    static func nextUnreadableStreak(after decision: Decision, snapshot: LedgerSnapshot,
                                     previous: Int) -> Int {
        guard case .unreadable = snapshot else { return 0 }
        if case .remind = decision { return previous + 1 }
        return previous
    }

    /// 已经连续提醒了 `streak` 次之后，下一次至少要隔多久：地板间隔 × 1、1、2、4、8、8…
    ///
    /// **倍数走 `SupervisionLease.backoffMultiplier`，不在这里另写一份。**
    /// 封顶 8× 的理由也是它那条：不封顶的指数退避等于「叫几次没人理就永远闭嘴」。
    /// 以 15 分钟地板算封顶是 2 小时 —— 一窗 9 小时的故障叫 7 次（固定地板是 37 次），
    /// 而且任何时刻离上一次被问都不超过 2 小时。
    static func unreadableGap(streak: Int, minimumInterval: TimeInterval) -> TimeInterval {
        minimumInterval * SupervisionLease.backoffMultiplier(step: streak - 1)
    }

    // MARK: - 参数

    /// 两次提醒之间的地板间隔。比这更密就是刷屏 —— 机长在忙/闲之间抖一下会触发好几次。
    static let minimumRemindInterval: TimeInterval = 15 * 60
}
