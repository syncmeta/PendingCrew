import Foundation

/// **督办租约**（人类 Todo #107：别人停了我不知道）。
///
/// 现场：后台重启带走了三条子 crew 的机长，**半小时没人管**，靠人工核账才发现 ——
/// session 只有被 @ 才会醒。人类原话：「如果一个会话把事情交给了某个人，要做一个
/// 超时，超时还没动静要自己唤醒去看看怎么回事，要定期监督，直到不需要再醒了、
/// 做完了，再把这个唤醒解除。**保持监督和警觉是常态，而休息下来不是常态。**」
///
/// # 它跟世界观里「别空转轮询、静默是正常状态」并不打架
///
/// 打架的错觉来自按**身份**分（机长该醒、worker 该静）—— 那既漏又多：一个把子问题
/// 交出去的 worker 同样持有委托；一个手上没有任何在途委托的机长定时醒来就是纯空转。
/// 轴不是身份，是**持有关系**：
///
/// > 醒来的理由必须是「我手上有一笔没有结果的委托，且它超期了」，不是「我是机长」。
/// > **有委托 → 警觉是常态；无委托 → 静默是常态。**
///
/// 于是两条规矩成了同一条规则的两侧，世界观那句一个字都不用改。
///
/// # 解除条件是这整件事的命门
///
/// **唯一**能让督办消音的路径，是把那条计划翻到 `done` 或 `blocked`
/// （`CockpitPlanStatus`）。这里**故意不提供「我知道了 / 已查看 / 顺延」任何一种
/// 动作**，`plan_update` 的 schema 上也不许出现这类参数（有一条测试钉着：
/// `testPlanToolsExposeNoAcknowledgeOrSnoozeAction`）。理由：一旦给了那个出口，
/// 督办必然退化成「看一眼就算办完」的仪式，那正是人类点名要避免的东西。
///
/// **副作用是故意的**：想让闹钟停就必须更新板子 —— 于是 #107 的机制顺手治了 #108
/// （板子没人更）的病根。**别把这当疏漏「补」上。**
///
/// # 四条硬约束落在哪
///
/// 1. 到期**只叫醒持有那笔委托的那一个 session**，绝不广播 —— `CrewSessionRunner`
///    的 `fireSupervisionLease`。
/// 2. 到期**不写白板**。白板是给人看的，不是闹钟 —— 同上，那条路径一个字都不往
///    白板写，连「叫不到人」时也不写。
/// 3. 同一笔委托同一时刻**最多一个在途唤醒** —— 靠 `id(crewId:planNumber:)` 的
///    确定性 id + `LocalWakeupStore.register` 同 id no-op，不靠调用方自觉。
/// 4. worker **默认不挂** —— 参数是可选的，而且 `plan_add` / `plan_update` 本来就
///    只有机长调得动。
///
/// # ⚠️ 一个**已知的、有意留下的**缺口（2026-09-07，机长明示，不是没想到）
///
/// 上面那条轴是「持有关系」，按它推到底：**一个 worker 把子问题 `contact` 出去、
/// 或者交接给别人，它同样持有一笔委托，本该也能挂督办。** 而现在唯一的挂载路是
/// 机长的 plan 工具 —— 也就是说这一版实际落地的仍是「机长能挂、worker 不能挂」，
/// **形状上等于把被否掉的「按身份分」偷偷编了回来。这是偏离，不是设计。**
///
/// 这一轮不做的理由**只有顺序**：整套机制还没上过真机（活验条目见
/// `docs/qa/2026-09-07-supervision-lease-device-qa.md` 的 L1–L6），
/// 在一个未验证的机制上先加第二条挂载路，出事时分不清是哪条路的问题。
///
/// **补的时候要补成什么样**：`contact` / 交接那条路上给一个同样的可选参数，
/// 落到同一个 `SupervisionLease`、同一本 `LocalWakeupStore`、同一套解除条件 ——
/// **别为 worker 另写一套语义**，尤其别给它一个机长没有的「取消/顺延」出口。
enum SupervisionLease {

    // MARK: - 身份

    /// 一笔委托的确定性 id。**同一条计划恒等** —— 「最多一个在途唤醒」这条硬约束
    /// 是靠它 + 账本的同 id 去重实现的，不是靠调用方记得先删再加。
    static func id(crewId: String, planNumber: Int) -> String { "lease:\(crewId):\(planNumber)" }

    // MARK: - 到期后的决定

    enum DischargeReason: Equatable {
        /// 委托有结果了（`done` / `blocked`）—— 唯一的正门。
        case resolved(CockpitPlanStatus)
        /// 这条计划已经不在板上了（撤下 / 查不到）。**不是后门**：撤下会把这条从
        /// 板面上整个拿掉，跟「看一眼就算办完」是两回事；留着一个指不到任何计划的
        /// 闹钟才是纯噪音。
        case goneFromBoard
    }

    enum Decision: Equatable {
        /// 不叫醒、不重排、不写白板。
        case discharge(DischargeReason)
        /// 叫醒持有人（正文），随后按退避重排。
        case remind(String)
    }

    /// - `planStatusRaw`: 这条计划当下的状态；**nil = 板上查不到这条**。
    /// - `leaseSince`: 挂上督办的那一刻。nil（老数据 / 时间戳解不开）→ 照常提醒，
    ///   但**不编一个时长出来**。
    static func decide(planNumber: Int, planTitle: String?, planStatusRaw: String?,
                       leaseSince: Date?, now: Date) -> Decision {
        guard let planStatusRaw else { return .discharge(.goneFromBoard) }
        if let status = CockpitPlan.status(planStatusRaw), status == .done || status == .blocked {
            return .discharge(.resolved(status))
        }
        let title = (planTitle?.isEmpty == false) ? planTitle! : "（这条计划没有标题）"
        let elapsed = leaseSince.map {
            "你把它交出去已经 \(CockpitPlan.elapsedLabel(now.timeIntervalSince($0)))，到现在没有结果"
        } ?? "它到现在没有结果"
        let statusName = CockpitPlan.status(planStatusRaw)?.title ?? planStatusRaw
        return .remind("""
        督办到期 · 计划 #\(planNumber)「\(title)」
        - \(elapsed) —— 板上这条仍是「\(statusName)」。
        - 现在去把它推到有结果：找持有人要一句现状（看有没有 commit、有没有报，别看它显示忙还是闲），或者自己接回来做完。
        - 解除督办只有一条路：把 #\(planNumber) 翻到 done 或 blocked（plan_update）。这里没有「我知道了」这种动作，所以它还会再叫你，间隔按 1×→2×→4×→8× 退避。
        """)
    }

    // MARK: - 退避（不是固定间隔连环叫）

    /// 退避封顶倍数。1×→2×→4×→8× 之后不再翻倍。
    ///
    /// **为什么封顶**：不封顶的指数退避等于「叫几次没人理就永远闭嘴」，那正好复刻
    /// #107 —— 一笔没有结果的委托必须一直有人盯着，只是节奏可以放缓。以常用的
    /// 40 分钟基础间隔算，封顶后约 5 小时一次，仍在「半天内一定会被过问」的量级里。
    static let maxBackoffMultiplier: Double = 8

    /// 账本里没记基础间隔时（理论上不该发生）用它，跟旧 `schedule_wakeup` 的典型
    /// 量级同阶，宁可多叫一次也不静默失约。
    static let fallbackBaseSeconds: TimeInterval = 30 * 60

    struct Reschedule: Equatable {
        let after: TimeInterval
        let step: Int
    }

    /// - `delivered`: 这一次**真的叫到人了**吗。没叫到（持有人不在跑、这一刻没有
    ///   任何可叫的目标）**不算叫过一次**：档位不推进、间隔不翻倍。否则一次 app
    ///   重启就能把督办退避到几小时以后 —— 那恰好是 #107 的现场。
    static func reschedule(baseSeconds: TimeInterval, step: Int, delivered: Bool) -> Reschedule {
        let nextStep = delivered ? step + 1 : step
        let multiplier = min(pow(2, Double(max(0, nextStep))), maxBackoffMultiplier)
        return Reschedule(after: max(1, baseSeconds) * multiplier, step: nextStep)
    }

    /// 到期后的下一条租约。**id 与挂单时刻不动** —— 时长得从最初交出去那一刻算，
    /// 不是从上一次响铃算；id 不动才保得住「最多一个在途唤醒」。
    static func next(_ w: LocalWakeupStore.PendingWakeup, delivered: Bool, now: Date)
        -> LocalWakeupStore.PendingWakeup {
        let base = w.leaseBaseSeconds ?? fallbackBaseSeconds
        let plan = reschedule(baseSeconds: base, step: w.leaseStep ?? 0, delivered: delivered)
        var next = w
        next.fireAt = ISO8601DateFormatter().string(from: now.addingTimeInterval(plan.after))
        next.leaseStep = plan.step
        return next
    }

    // MARK: - 参数卫生

    /// 督办间隔的下限。**比这更短就是空转轮询**，与世界观「别空转轮询」直接打架 ——
    /// 那条并没有被推翻，被推翻的只是「没有委托时也该安静」这半。
    static let minMinutes: Double = 5
    /// 上限同 `schedule_wakeup`：超过一天的「督办」已经不是督办了。
    static let maxMinutes: Double = 1440

    /// 三态，**不许压成 `Double?`**：「没给」和「给了但不合法」是两回事，压成一个
    /// nil 就会把一次拒绝静默说成「你没要求过」。
    enum ParsedMinutes: Equatable {
        case none
        case minutes(Double)
        case refused(String)
    }

    static func parseMinutes(_ raw: Any?) -> ParsedMinutes {
        guard let raw, !(raw is NSNull) else { return .none }
        let value: Double
        if let d = raw as? Double { value = d }
        else if let i = raw as? Int { value = Double(i) }
        else {
            return .refused("supervise_after_minutes 需要一个数字（分钟）。")
        }
        guard value >= minMinutes, value <= maxMinutes else {
            return .refused("supervise_after_minutes 需在 \(Int(minMinutes))–\(Int(maxMinutes)) 分钟之间 —— 比 \(Int(minMinutes)) 分钟更短的督办就是空转轮询。")
        }
        return .minutes(value)
    }
}
