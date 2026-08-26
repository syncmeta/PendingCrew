import Foundation

/// 驾驶舱「任务列表」段的**模型 + 纯逻辑**（人类 Todo #66）。
///
/// 人类原话：「再出一个任务列表视图，就是你把要做的事情列出来，一条条列出来，然后把
/// 进度标上去，及时更新这个进度。这和 todo 的功能其实差不多，但是 todo 因为是人自己
/// 说的，比较琐碎；而任务列表是你自己整理的，可以比较有条理。」
///
/// # 这是第六本账 —— 先说清它跟另外五本的关系，否则半年后没人分得清
///
/// 驾驶舱里同时存在这些账，**它们不是重复，是不同的人写的**：
/// 1. **活跃 task 账**（`~/.claude/tasks/<id>/*.json`）—— coding agent 自己的执行清单，
///    驾驶舱**只读**（`CockpitTaskLedger`）。
/// 2. **仓库 markdown task 账**（`docs/tasks/*.md`）—— 已退役，只作 ① 的回落。
/// 3. **人类 Todo `.agent`**（`<crewId>.todos.json`）—— 人类派给 agent，agent 只能回应。
/// 4. **人类 Todo `.human`**（`<crewId>.human-todos.json`，Todo #62）—— 方向反过来：
///    agent 请人类拍板。
/// 5. **路线账**（`docs/roadmap.md`）—— 人定阶段骨架，agent 填条目。
/// 6. **本文件：机长作战板**（`<crewId>.plan.json`）—— **只有机长能写**，人类只读。
///
/// **命名就是分界线**：`CockpitTask*` = **从别处读来的**活（聚合器的展示模型，谁写的都有）；
/// `CockpitPlan*` = **机长自己排的**活（这一本，第一手）。看到 `Plan` 就知道「这是机长
/// 自己写下的」，看到 `Task` 就知道「这是从别人的账上读来的」。
///
/// **和 `CockpitTasksGlance` 那个聚合段的关系**：那一段是「读遍所有账的时态摘要」
/// （在做什么 / 接下来做什么 / 做了什么），本账**也会被它读进去**（`CockpitTaskItem.Origin.captainPlan`）
/// —— 不进聚合器才会变成一座孤岛。两者是**被读者与读者**，不是并列的两个 tab：
/// 聚合段是 glance，本段是这本账的**编辑台**（四档进度、点进去看进度描述、标卡住）。
///
/// # 为什么不塞进 `TodoLedger`（Todo #62 的两本账）
///
/// 那套参数化的是 **Todo 的形状**：三档状态 + `responses` + 「谁加谁回应」。本账是
/// **四档 + 卡住引用**，硬塞进那个枚举会把两边都搞脏。**共用的是更下面那一层**
/// —— `MultiProcessJSONStore`（flock / 逐条 lenient 解码 / corrupt 归档 / 「读失败 ≠
/// 内容损坏」）。**不 fork 的是基座，不共用的是账本形状。**

// MARK: - 四档进度

/// 机长作战板的进度档位。**故意不复用 Todo 的三档**（pending / in_progress / completed）：
/// 本账多一档「卡住」，而且那一档带**指向人类 Todo 的引用**——语义不同的东西共用一个
/// 枚举，迟早有人把两边的判断写串。
enum CockpitPlanStatus: String, Codable, CaseIterable, Sendable {
    /// 排上了但还没动。
    case notStarted = "not_started"
    /// 在做。点进去看进度描述（追加式）。
    case inProgress = "in_progress"
    /// **卡在人身上** —— 必须带一条指向人类 Todo 的引用（`CockpitPlanBlocker`）。
    /// 这是本账最有价值的一处：一点就知道卡在哪条待人拍板的事上。
    case blocked
    /// 做完了。
    case done

    /// 界面上的说法（人类原话就是这四个词）。
    var title: String {
        switch self {
        case .notStarted: return "没做"
        case .inProgress: return "进行中"
        case .blocked: return "卡住"
        case .done: return "完成"
        }
    }
}

// MARK: - 卡住引用

/// 「卡住」指向的那条人类 Todo。
///
/// **为什么连账本一起存**（`ledger` 而不是光一个 `number`）：Todo #62 之后有两本账、
/// **各自从 #1 起自增**，所以裸 `#7` 是有歧义的——群聊里那行都被迫加了「人类」二字才
/// 分得清（`人类 To do +1: #7` vs `To do +1: #7`）。今天只指得到 `.human` 那本，但哪天
/// 有人想指向 agent 那本（完全合理），**现在多存一个标签是零成本，以后补是数据迁移**
/// ——那时字段已经落在别人机器上的 JSON 里了。
///
/// **为什么 `ledger` 是 `String` 而不是 `TodoLedger`**：`TodoLedger` 是 Todo #62 的产物，
/// 本文件落地时它还没合进 main。依赖一个尚不存在的类型就是「假装它已经在」。字面量与
/// 它的 `rawValue` 一字不差（`"agent"` / `"human"`），#62 合 main 后接线只是一次映射。
struct CockpitPlanBlocker: Codable, Equatable, Sendable {
    /// `"human"`（agent 请人类拍板那本）/ `"agent"`（人类派给 agent 那本）。
    var ledger: String
    /// 那本账里的 #N。
    var number: Int

    /// 说给人听的引用（群聊里的措辞与之一致）。
    var label: String {
        ledger == "human" ? "人类 Todo #\(number)" : "Todo #\(number)"
    }
}

/// 引用的那条**还在不在** —— 由调用方（MCP 层 / UI 层）去另一本账里查了之后告诉本层。
///
/// **本层不自己去查**：跨账硬依赖是灾难——那本账读不出来的时候，本账连写都写不进去。
/// 所以这里只表达三种结论，其中 `.unverified` 是**如实承认没查**，不是默认「在」。
enum CockpitPlanBlockerState: Equatable, Sendable {
    /// 查过，还在。
    case present
    /// 查过，**人类把它删了**（`LocalTodoStore.delete` 是软删，人类随时能删）。
    case missing
    /// 没查成（那本账还没接线 / 读不出来）。附一句为什么。
    case unverified(reason: String)
}

// MARK: - 条目

/// 一条计划。
///
/// `status` 存**字符串**而不是枚举：与 Todo 那本同一套 lenient 姿态——将来多一档
/// （或者手改坏了一个字）时，坏的是这一条的显示，不是整份文件解码失败。
struct CockpitPlanItem: Codable, Equatable, Identifiable, Sendable {
    let id: String
    /// crew 内自增、唯一。**软删也占号** —— 号码是对外说过的。
    let number: Int
    /// 一句话说清这条活是什么。机长可改。
    var title: String
    /// `CockpitPlanStatus.rawValue`。解不出来的值按「没做」渲染但**原样留在文件里**。
    var status: String
    /// 只有 `blocked` 才有。离开 `blocked` 时清空（见 `CockpitPlan.validate`）。
    var blockedBy: CockpitPlanBlocker? = nil
    /// 进度描述，**追加式**（照 Todo 的回应那样只加不覆盖）。
    var updates: [CockpitPlanUpdate] = []
    let createdAt: String
    /// 谁排的这条 —— 恒为机长，但记下来才追得回（多机长 / 换 session 之后）。
    var createdBySessionId: String? = nil
    var createdByName: String? = nil
    /// **最后一次真正改动的时间**（改标题 / 推状态 / 追加进度都算；读不算）。
    ///
    /// 这个字段是机长给自己装的**照妖镜**：界面上直说「进行中 · 最后更新 3 天前」。
    /// 这块板的价值全看机长更不更新，而**一个没人能验证的承诺等于没有** —— 一张全是
    /// 「进行中 · 最后更新 6 天前」的板，比空板更能说明问题。所以它**不做成提醒、不弹、
    /// 不变红**，只是让「我三天没碰过这条」这件事一眼可见，藏起来就等于把镜子扣过来。
    var updatedAt: String
    /// 软删墓碑。理由同 Todo：号码对外说过，物理删会让自增把同一个号发第二遍。
    var deletedAt: String? = nil

    var isDeleted: Bool { deletedAt != nil }
}

/// 一条进度描述。`status` = 这条更新把条目推到了哪一档（nil = 只写进度、没动状态）。
struct CockpitPlanUpdate: Codable, Equatable, Identifiable, Sendable {
    let id: String
    let text: String
    var status: String? = nil
    let createdAt: String
    /// 谁写的（机长 session）。历史上只会是机长，但记下来才追得回。
    var bySessionId: String? = nil
    var byName: String? = nil
}

// MARK: - 纯逻辑（可单测，不碰文件系统）

enum CockpitPlan {

    // MARK: 状态

    /// 宽松解析。认不出来 → nil（调用方按「没做」渲染，但**不改写**文件里的原值）。
    static func status(_ raw: String) -> CockpitPlanStatus? {
        CockpitPlanStatus(rawValue: raw.trimmingCharacters(in: .whitespaces).lowercased())
    }

    /// 写入前的守卫结论。`nil` = 放行。
    enum Refusal: Error, Equatable {
        /// 想翻成「卡住」却没给引用。
        case blockedWithoutBlocker
        /// 状态词根本不认识。
        case unknownStatus(String)

        /// 说给调用方（MCP 回执 / 单测）听的一句话。
        var summary: String {
            switch self {
            case .blockedWithoutBlocker:
                return "翻成「卡住」必须指明卡在哪条人类 Todo（#N）——「卡住」的意思就是卡在人身上，不写就没人知道去推哪一条"
            case let .unknownStatus(raw):
                return "不认识的进度档「\(raw)」——只有 not_started / in_progress / blocked / done 四档"
            }
        }
    }

    /// **本账唯一的硬约束**：`blocked` ⇔ 有引用。
    ///
    /// - `next`: 想写成的状态（nil = 这次不动状态）。
    /// - `incomingBlocker`: 这次带来的引用（nil = 没带）。
    /// - `existingBlocker`: 条目上已有的引用（已经是 blocked、这次只追加进度时用得上）。
    ///
    /// 返回放行后**应该落盘的引用**：离开 `blocked` 时是 nil（引用跟着状态一起清，
    /// 否则一条「完成」的活上挂着「卡在 #7」，下一个人只能靠猜）。
    static func validate(next: CockpitPlanStatus?,
                         incomingBlocker: CockpitPlanBlocker?,
                         existingBlocker: CockpitPlanBlocker?) -> Result<CockpitPlanBlocker?, Refusal> {
        guard let next else {
            // 不动状态：带了新引用就更新，没带就保持原样。
            return .success(incomingBlocker ?? existingBlocker)
        }
        guard next == .blocked else { return .success(nil) }
        guard let blocker = incomingBlocker ?? existingBlocker else {
            return .failure(.blockedWithoutBlocker)
        }
        return .success(blocker)
    }

    /// 字符串入口（MCP 层拿到的是原始字符串）。
    static func validate(nextRaw: String?,
                         incomingBlocker: CockpitPlanBlocker?,
                         existingBlocker: CockpitPlanBlocker?) -> Result<CockpitPlanBlocker?, Refusal> {
        guard let nextRaw, !nextRaw.trimmingCharacters(in: .whitespaces).isEmpty else {
            return validate(next: nil, incomingBlocker: incomingBlocker, existingBlocker: existingBlocker)
        }
        guard let next = status(nextRaw) else { return .failure(.unknownStatus(nextRaw)) }
        return validate(next: next, incomingBlocker: incomingBlocker, existingBlocker: existingBlocker)
    }

    // MARK: 照妖镜（多久没碰过）

    /// 「最后更新 3 天前」。`nil` 时间戳 → 空串（调用方就不显示这一截）。
    ///
    /// 粒度**故意粗**：分钟级以下一律「刚刚」。这行字的用途是让「久没动」显出来，
    /// 不是秒表——写细了反而像个在跑的计时器，把注意力从「6 天没碰」引开。
    static func lastUpdatedLabel(_ updated: Date?, now: Date) -> String {
        guard let updated else { return "" }
        let seconds = now.timeIntervalSince(updated)
        if seconds < 60 { return "最后更新 刚刚" }
        if seconds < 3600 { return "最后更新 \(Int(seconds / 60)) 分钟前" }
        if seconds < 86400 { return "最后更新 \(Int(seconds / 3600)) 小时前" }
        return "最后更新 \(Int(seconds / 86400)) 天前"
    }

    /// 行尾那一整句：「进行中 · 最后更新 3 天前」。
    static func statusLine(statusRaw: String, updated: Date?, now: Date) -> String {
        let name = status(statusRaw)?.title ?? CockpitPlanStatus.notStarted.title
        let stale = lastUpdatedLabel(updated, now: now)
        return stale.isEmpty ? name : "\(name) · \(stale)"
    }

    // MARK: 引用还在不在

    /// 判定一条卡住引用的存在状态。**查账的动作由调用方注入** —— 本层不认识
    /// `LocalTodoStore`，也不该认识：跨账硬依赖会让「那本账读不出来」升级成
    /// 「这本账连写都写不进去」。
    ///
    /// - `agentTodoExists`: 人类派给 agent 那本（`.agent`）里有没有这条 #N。
    /// - `humanTodoExists`: 请人类拍板那本（`.human`，Todo #62）。**`nil` = 那本还没
    ///   接线**，此时如实返回 `.unverified`，绝不默认「在」—— 假装查过比没查更坏。
    static func blockerState(_ blocker: CockpitPlanBlocker,
                             agentTodoExists: (Int) -> Bool,
                             humanTodoExists: ((Int) -> Bool)?) -> CockpitPlanBlockerState {
        switch blocker.ledger {
        case "agent":
            return agentTodoExists(blocker.number) ? .present : .missing
        case "human":
            guard let humanTodoExists else {
                return .unverified(reason: "这个调用点没接人类 Todo 那本账，查不了")
            }
            return humanTodoExists(blocker.number) ? .present : .missing
        default:
            return .unverified(reason: "不认识的账本「\(blocker.ledger)」")
        }
    }

    // MARK: 卡住引用怎么说

    /// 「卡在」那一行。**悬空引用要显式说出来，不许静默降级**——
    /// 既不偷偷把状态改回「进行中」（那是替人类改机长的状态），也不显示一个点不开的
    /// #N。静默降级比报错难查得多，这一族 bug 已经吃过好几次亏了。
    static func blockerLine(_ blocker: CockpitPlanBlocker?, state: CockpitPlanBlockerState) -> String {
        guard let blocker else { return "" }
        switch state {
        case .present:
            return "卡在 \(blocker.label)"
        case .missing:
            return "卡在 \(blocker.label) —— 那条现在找不到了（被删掉了，或那本账这次读不出来），卡点需要重新指认"
        case let .unverified(reason):
            return "卡在 \(blocker.label)（未核实：\(reason)）"
        }
    }

    // MARK: 排序

    /// 从新到旧（与 Todo 面板同一套姿态：新排的活在最上面）。
    static func newestFirst(_ items: [CockpitPlanItem]) -> [CockpitPlanItem] {
        items.sorted { $0.number > $1.number }
    }
}
