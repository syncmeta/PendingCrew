import Foundation

/// 群消息的**分类**，以及它要落进哪本账（人类 Todo #115）。
///
/// 人类原话：「我希望 在群里的发言直接分类 不然第一 todo、cockpit等等不能及时更新
/// 第二 群里消息很乱」。**两件事是同一个病**：消息是自由文本，账是结构，中间靠 agent
/// 自觉去搬 —— **自觉会漏**。所以分类必须**真的驱动落账**：做成一个标签颜色 = 白做。
///
/// 判据是「**这条该落进哪本账**」，不是「它讲什么」。
///
/// ## 不新造第三本账
/// 三本账都已经存在，各有 store 和写入口，这一层只负责把分类**路由**过去：
/// - 人类 Todo → `LocalTodoStore.shared(.human)`（+ 共享剧本 `TodoLandingFlow`）
/// - Agent Todo → `LocalTodoStore.shared(.agent)`
/// - 驾驶舱 → `CockpitPlanStore`
enum CrewMessageCategory: String, CaseIterable {
    // MARK: A 组 —— 发这条 = 同时改了一本账

    /// 要人拍板 / 只有人能做的事 → 人类 Todo 面板新增一条。
    case humanTodo = "human_todo"
    /// 回应人类派下来的 Todo → Agent Todo 面板翻牌。
    case todoResponse = "todo_response"
    /// 我要开始做的一件事 → 驾驶舱新增一条计划。
    case plan
    /// 某条计划推进了 → 驾驶舱追加进度。
    case progress
    /// 某条计划卡住了 → 驾驶舱翻 blocked + 指出卡点。
    case blocked
    /// 某条计划完成了 → 驾驶舱翻 done。
    case done

    // MARK: 记录，但不驱动任何动作

    /// 把一件事交给某个 session / 子 crew。
    ///
    /// **只落这条记录，不起任何进程、不发任何跨 crew 消息。** 交接不是写一行账，
    /// 是起一个进程 —— 一条标错分类的消息就能凭空多一个 session 在跑，**而且撤不回来**，
    /// 跟「落账必须可撤」直接冲突。真派活仍然走 `start_session` / `message_child_crew`，
    /// 那是显式动作，本来就该显式。
    case handoff

    // MARK: B 组 —— 不落账

    case ack, question, finding, note

    // MARK: C 组 —— 系统写的，agent 不许选

    /// 重启、额度警戒、投递回执、超时告警。
    case system

    /// 这条要落哪本账。
    enum Ledger: Equatable { case humanTodo, agentTodo, cockpit, none }

    var ledger: Ledger {
        switch self {
        case .humanTodo: return .humanTodo
        case .todoResponse: return .agentTodo
        case .plan, .progress, .blocked, .done: return .cockpit
        case .handoff, .ack, .question, .finding, .note, .system: return .none
        }
    }

    /// agent 可以自己选的分类（`system` 不在内）。
    static var agentSelectable: [CrewMessageCategory] {
        allCases.filter { $0 != .system }
    }
}

/// 分类 → 落账的**纯路由判定**。不碰 IO，不写任何账 —— 只回答「该不该落、够不够、
/// 不够时说什么」。
enum CrewCategoryRouting {

    enum Decision: Equatable {
        /// 不落账，照常发消息。
        case noLedger
        /// 参数齐了，按这个分类去落账。
        case land(CrewMessageCategory)
        /// 拒绝，并把这句话原样回给调用方。
        case refuse(String)
    }

    /// - Parameters:
    ///   - category: `post_to_crew` 传进来的分类。**第一步它仍是可选的** ——
    ///     `--mcp-serve` 一个 session 一个进程、长期存活，改了 enum 对在跑的 session
    ///     不生效（见 `LocalSessionLaunch.prepareLocalCommsConfig` 的注释）。
    ///     此刻把它翻成必填，会让一批在跑的 session 的 `post_to_crew` 开始失败，
    ///     而**有的 agent 会把失败读成「这条不该发」然后静默咽掉** ——
    ///     咽掉的正是人类最需要看到的汇报。收口留到装版之后。
    ///   - args: 同一次工具调用里的其它参数（`plan` / `todo` / `blocked_by_number`）。
    static func decide(category: String?, args: [String: Any]) -> Decision {
        let raw = (category ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else { return .noLedger }

        // 旧 enum 的三个值仍在跑。`progress` 在新表里也有、语义没变；
        // `question` 也有；`milestone` 新表没有 —— **映射到不落账，不映射到 done**：
        // done 要计划号，老调用方不可能带，映射过去只会让它们全部开始 refuse。
        if raw == "milestone" { return .noLedger }

        guard let category = CrewMessageCategory(rawValue: raw) else {
            return .refuse(unknownCategoryMessage(raw))
        }
        if category == .system {
            return .refuse("category `system` 不归 agent 选 —— 它留给重启、额度警戒、"
                + "投递回执这类系统通告。你要说的多半是 `finding`（值得知道的事实）"
                + "或 `note`（其它）。")
        }
        for req in requirements(of: category) where args[req.key] == nil {
            return .refuse(req.message)
        }
        // 不落账的那几类到此为止 —— 红测就是在这儿逮到我的：原来它们也走到了
        // `.land`，等于把 `ack` / `note` / `handoff` 也接上了写账那条路。
        // **`handoff` 一旦 land 就意味着起进程**，那正是这一单最不该发生的事。
        guard category.ledger != .none else { return .noLedger }
        return .land(category)
    }

    // MARK: - 缺什么，以及**该往哪走**

    /// 一条必填参数。
    ///
    /// `message` **必须给出路，不能只报缺什么**：要参数这件事让「顺手报一条 progress」
    /// 变贵了，而人类要的恰恰是账**能及时更新**。如果翻不到计划号就干脆改标 `note`，
    /// 这一单等于没做。**读到这句话的是一个正在写下一句话的 agent，它需要的是下一步，
    /// 不是诊断** —— 所以出路写在错误信息里，不写在文档里。
    private struct Requirement {
        let key: String
        let message: String
    }

    private static func requirements(of category: CrewMessageCategory) -> [Requirement] {
        switch category {
        case .progress, .done:
            return [Requirement(key: "plan", message: planNumberMessage(category))]
        case .blocked:
            return [Requirement(key: "plan", message: planNumberMessage(category)),
                    Requirement(key: "blocked_by_number", message: blockedByMessage)]
        case .todoResponse:
            return [Requirement(key: "todo", message: todoNumberMessage)]
        case .humanTodo, .plan, .handoff, .ack, .question, .finding, .note, .system:
            return []
        }
    }

    private static func planNumberMessage(_ category: CrewMessageCategory) -> String {
        "category `\(category.rawValue)` 要 `plan`（驾驶舱里那条计划的 #N，plan_list 看得到）。"
            + "\n**没有计划号，说明这条报的是一件还没排上计划的事** —— 两条出路："
            + "\n① 先 `plan_add` 排一条，再报它的进度；"
            + "\n② 它本来就不是某条计划的进度，那它是 `finding`（值得知道的事实，不构成待办）。"
    }

    private static let blockedByMessage =
        "category `blocked` 还要 `blocked_by_number`（卡在哪条**人类 Todo** 的 #N）。"
        + "\n**「卡住」的意思就是卡在人身上** —— 不指出是哪一条，人看到板也不知道该推什么。"
        + "\n出路：先 `add_human_todo` 把要人拍的那件事提出来，拿到 #N 再回来标 blocked。"

    private static let todoNumberMessage =
        "category `todo_response` 要 `todo`（群消息「To do +1: #N」里的那个 N）。"
        + "\n**没有 N 就翻不动牌** —— 出路：去 Todo 面板「Agent 的」那本找到它的号；"
        + "\n如果这条根本不是在回应谁派的活，它多半是 `progress` 或 `finding`。"

    private static func unknownCategoryMessage(_ raw: String) -> String {
        let list = CrewMessageCategory.agentSelectable.map { "`\($0.rawValue)`" }
            .joined(separator: " / ")
        return "category `\(raw)` 不认识。可选的是：\(list)。"
            + "\n判据是「**这条该落进哪本账**」，不是「它讲什么」："
            + "\n落账的 —— `human_todo`(要人拍板) / `todo_response`(回应派下来的活) /"
            + " `plan`(要开始做一件事) / `progress`(某条计划推进了) /"
            + " `blocked`(某条计划卡住了) / `done`(某条计划完成了)；"
            + "\n不落账的 —— `handoff`(交给谁了，只记录) / `ack`(收到) /"
            + " `question`(要答复才能继续) / `finding`(值得知道的事实) / `note`(其它)。"
    }
}


/// 群消息**顺带更新一条 Todo**（人类 Todo #120）的纯判定。
///
/// 人类原话：「群消息多一个参数：对应着哪一条todo 如果对应上了 就要写最新的状态
/// 也就是强制更新一下todo的状态」。它是 #115 那句根的具体化：**发消息的同时账就更新了**，
/// 不靠 agent 记得再调一次 `respond_todo`。
///
/// ## 为什么单独一路，不塞进 `category`
///
/// 一条消息可能**既是进度、又对应一条 Todo**；也可能是 `blocked`，而卡的正是那条 Todo。
/// 绑在某个分类上，写的人会卡在「这算 progress 还是算 todo_response」上纠结 ——
/// **而它其实两者都是**。分类回答「落哪本账」，Todo 号回答「同时挂在哪条 Todo 上」，
/// 这是两个问题，所以是两个参数、两个判定函数。
enum CrewMessageTodoLink {

    enum Decision: Equatable {
        /// 没挂 Todo。
        case none
        /// 把这条 Todo 翻到这个状态。
        case update(number: Int, status: String)
        case refuse(String)
    }

    /// `respond_todo` 认的三档，照抄它不另立一套。
    static let validStatuses = ["pending", "in_progress", "completed"]

    static func decide(args: [String: Any]) -> Decision {
        let number = (args["todo"] as? Int) ?? (args["todo"] as? NSNumber)?.intValue
        let status = (args["todo_status"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)

        guard let number else {
            // 填了状态却没给号 —— 那个状态哪儿也去不了，**别静默丢掉它**。
            if let status, !status.isEmpty {
                return .refuse("给了 `todo_status`（\(status)）却没给 `todo` —— 这个状态哪条 Todo 都没挂上，"
                    + "等于白填。\n出路：补上 `todo`（群消息「To do +1: #N」里的 N）；"
                    + "如果这条本来就不对应任何 Todo，把 `todo_status` 去掉。")
            }
            return .none
        }
        guard number > 0 else {
            return .refuse("`todo` 需为正整数（群消息「To do +1: #N」里的 N）。")
        }
        guard let status, !status.isEmpty else {
            // 人类原话是「**强制**更新一下 todo 的状态」。跟「要参数」同一个形状：
            // 填不出状态的那条，本来就不该挂这个号。
            return .refuse("挂了 `todo` #\(number) 就必须同时给 `todo_status` —— 人类要的就是"
                + "「对应上了就强制更新状态」，只挂号不更新等于账还是旧的。"
                + "\n三档：`pending`（还没开始）/ `in_progress`（在做）/ `completed`（做完了）。"
                + "\n**出路**：说不准是哪一档，多半说明这条消息跟这条 Todo 其实没有强对应 —— "
                + "把 `todo` 去掉，它就是一条普通的进度或发现。")
        }
        guard validStatuses.contains(status) else {
            return .refuse("`todo_status` 只能是 " + validStatuses.map { "`\($0)`" }.joined(separator: " / ")
                + "，收到的是 `\(status)`。")
        }
        if status == "completed" {
            let evidence = ((args["evidence"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let commit = ((args["evidence_commit"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            // ⚠️ **这里只检查「有没有给」，不检查「给的对不对」。**
            //
            // 真正的闸在 `McpServer.judgeCompletionEvidence(args:)`：它会**当场解析**
            // `evidence_commit`，并分出 5 种结局（格式不对 / 仓库里不存在 / 环境验不了 /
            // 散文凭据 / 解析成功）。2026-09-07 挖出的那笔假账带着一个**根本不存在的
            // hash** 挂了 191 小时 —— 病根不是「没带凭据」，是**那个凭据从来没有被解析过**。
            //
            // 所以：**这一层返回 `.update` 不等于凭据验过了**，接线时必须再过那道闸，
            // 不许拿这一层的绿去替代它。写在这儿是因为下一个人很容易把这段读成
            // 「凭据校验在纯层做完了」。
            guard !evidence.isEmpty || !commit.isEmpty else {
                // `respond_todo` 现在就有这道闸，**不许在这条新路上放宽** ——
                // 一条记成「已完成」而其实没做的账，没有任何人会回来看。
                return .refuse("把 Todo #\(number) 翻成 `completed` 必须带凭据："
                    + "`evidence_commit`（产出所在的 commit）或 `evidence`（产出不是 commit 时，"
                    + "一句话写清是什么）。\n**这道闸是故意的**：宣布完成贵一点点，"
                    + "因为一条记成「已完成」而其实没做的账，没有任何人会回来看。"
                    + "\n出路：还没做完就先翻 `in_progress`，做完拿到凭据再来。")
            }
        }
        return .update(number: number, status: status)
    }
}
