import XCTest

/// 群消息分类 → 落账路由的**纯判定层**（人类 Todo #115）。
///
/// 人类原话：「我希望 在群里的发言直接分类 不然第一 todo、cockpit等等不能及时更新
/// 第二 群里消息很乱」。**两件事是同一个病**：消息是自由文本，账是结构，中间靠
/// agent 自觉去搬 —— **自觉会漏**。所以分类必须**真的驱动落账**；
/// 做成一个标签颜色 = 这一单白做。
///
/// 这一层只回答两个问题，不碰任何 IO：
/// 1. 这条分类要落**哪本账**（三本都已存在：人类 Todo / Agent Todo / 驾驶舱）；
/// 2. 参数够不够，不够时**该说什么**。
final class CrewMessageCategoryTests: XCTestCase {

    private func decide(_ category: String?, _ args: [String: Any] = [:])
        -> CrewCategoryRouting.Decision {
        CrewCategoryRouting.decide(category: category, args: args)
    }

    // MARK: - A 组：参数齐就落账

    func test_人类todo只要正文就能落账() {
        XCTAssertEqual(decide("human_todo"), .land(.humanTodo))
    }

    func test_计划只要正文就能落账() {
        XCTAssertEqual(decide("plan"), .land(.plan))
    }

    func test_进度给了计划号才落账() {
        XCTAssertEqual(decide("progress", ["plan": 3]), .land(.progress))
    }

    func test_卡住要计划号也要卡在哪条人类todo() {
        XCTAssertEqual(decide("blocked", ["plan": 3, "blocked_by_number": 7]), .land(.blocked))
    }

    func test_完成要计划号() {
        XCTAssertEqual(decide("done", ["plan": 3]), .land(.done))
    }

    func test_回应todo要todo号() {
        XCTAssertEqual(decide("todo_response", ["todo": 5]), .land(.todoResponse))
    }

    // MARK: - 缺参数时：**错误信息必须给出路**，不能只报缺什么

    /// 4-1 的硬要求：要参数有个副作用 —— 填 `#N` 让「顺手报一条 progress」变贵了，
    /// 而人类要的恰恰是账**能及时更新**。如果翻不到计划号就干脆标 `note`，
    /// 这一单等于没做。
    ///
    /// **所以出路要写进错误信息本身，不写进文档** —— 读到它的是一个正在写下一句话的
    /// agent，它需要的是下一步，不是诊断。
    /// ⚠️ `progress` 是**旧 enum 里本来就有的值**，在跑的 session 正在用它。
    /// 所以缺参数时它**降级成「不落账 + 提醒」，而不是失败** —— 第一步不许炸任何人。
    /// （第一版直接 refuse，当场打挂了 `McpServerTests.testPostToCrewWritesWhiteboard`，
    /// 那条用例发的就是 `category: "progress"` 且不带计划号 —— **它替所有在跑的
    /// session 挡了这一下**。）
    func test_旧值progress缺计划号时降级不失败() {
        guard case let .skipped(hint) = decide("progress") else {
            return XCTFail("旧值因为缺参数失败了 —— 在跑的 session 会开始咽掉汇报")
        }
        XCTAssertTrue(hint.contains("没有落进账本"), hint)
        XCTAssertTrue(hint.contains("finding"), "降级也要给出路：\(hint)")
    }

    /// 新值（旧 enum 里没有的）缺参数就该直接拒 —— 没有在跑的 session 在用它们。
    func test_新值缺参数时错误信息要给出路而不只是报缺什么() {
        guard case let .refuse(msg) = decide("done") else {
            return XCTFail("缺 plan 号却放行了 —— 那条进度会落到不知道哪条计划上")
        }
        XCTAssertTrue(msg.contains("plan"), "没说缺哪个参数：\(msg)")
        // 出路两条，缺一不可：先排一条计划，或者它本来就不是进度。
        XCTAssertTrue(msg.contains("plan_add") || msg.contains("先"),
                      "没给出路①（先排一条计划再报进度）：\(msg)")
        XCTAssertTrue(msg.contains("finding"),
                      "没给出路②（它本来就是 finding，不是进度）：\(msg)")
    }

    func test_卡住缺卡点时也要给出路() {
        guard case let .refuse(msg) = decide("blocked", ["plan": 3]) else {
            return XCTFail("没指出卡在哪条人类 Todo 却放行了 —— 人看到板也不知道该推什么")
        }
        XCTAssertTrue(msg.contains("blocked_by_number"), msg)
        XCTAssertTrue(msg.contains("add_human_todo") || msg.contains("人类 Todo"),
                      "没给出路（卡在人身上就得先有一条人类 Todo）：\(msg)")
    }

    func test_回应todo缺号时给出路() {
        guard case let .refuse(msg) = decide("todo_response") else {
            return XCTFail("没给 Todo 号却放行了")
        }
        XCTAssertTrue(msg.contains("todo"), msg)
    }

    // MARK: - handoff：**只落记录，不起进程**（4-1 拍的）

    /// 交接不是写一行账，是**起一个进程 / 派一个跨 crew 任务**。一条标错分类的消息
    /// = 凭空多一个 session 在跑，**而且撤不回来** —— 跟「落账可撤」这条口径直接冲突。
    /// 所以它留在分类表里（「谁把什么交给了谁」是有价值的事实），但**不驱动任何动作**。
    func test_交接不落账也不起任何进程() {
        XCTAssertEqual(decide("handoff"), .noLedger,
                       "handoff 一旦驱动 start_session，一条标错的消息就能凭空起一个 session")
    }

    // MARK: - B 组：不落账

    func test_不落账的四类() {
        for c in ["ack", "question", "finding", "note"] {
            XCTAssertEqual(decide(c), .noLedger, c)
        }
    }

    // MARK: - 第一步不许炸任何人（4-1 改的方案）

    /// `post_to_crew` 失败对一个 agent 意味着什么**取决于它当时在干嘛** —— 有的重试，
    /// **有的会把错读成「这条不该发」然后静默咽掉**，而咽掉的正是人类最需要看到的汇报。
    /// 所以第一步：分类**仍可选**，不给就是 `note`，一条都不许失败。
    func test_不给分类不算错只当note() {
        XCTAssertEqual(decide(nil), .noLedger)
        XCTAssertEqual(decide(""), .noLedger)
    }

    /// 旧 enum 的三个值（`progress` / `question` / `milestone`）在跑的 session 还在用 ——
    /// **`--mcp-serve` 一个 session 一个进程、长期存活，改了 enum 对它们不生效**
    /// （见 `LocalSessionLaunch.prepareLocalCommsConfig` 的注释）。所以旧值必须还认。
    func test_旧值不许炸() {
        XCTAssertEqual(decide("question"), .noLedger)
        // `milestone` 新表里没有。**映射到不落账那档，不映射到 done** ——
        // done 要计划号，老调用方不可能带；映射过去只会让它们全部开始 refuse。
        XCTAssertEqual(decide("milestone"), .noLedger)
    }

    /// 真的写错了（不是旧值）→ 拒，**但错误信息要把合法值列全**，
    /// 而不是一句「category 非法」。
    func test_未知分类要把合法值列全() {
        guard case let .refuse(msg) = decide("progres") else {
            return XCTFail("拼错的分类被放过了")
        }
        for c in ["human_todo", "plan", "progress", "blocked", "done", "todo_response",
                  "handoff", "ack", "question", "finding", "note"] {
            XCTAssertTrue(msg.contains(c), "错误信息里没列出 \(c)：\(msg)")
        }
    }

    /// `system` 不归 agent 选（重启、额度警戒、投递回执都是系统写的）。
    func test_system不许agent自己用() {
        guard case let .refuse(msg) = decide("system") else {
            return XCTFail("agent 把自己的话标成了 system —— 那条会被当成系统通告")
        }
        XCTAssertTrue(msg.contains("system"), msg)
    }

    // MARK: - #120：顺带更新一条 Todo（**独立一路，跟 category 正交**）

    private func todo(_ args: [String: Any]) -> CrewMessageTodoLink.Decision {
        CrewMessageTodoLink.decide(args: args)
    }

    /// 一条消息可能**既是进度、又对应一条 Todo**。绑在某个分类上，写的人会卡在
    /// 「这算 progress 还是算 todo_response」上纠结 —— 而它其实两者都是。
    func test_挂todo跟分类互不干涉() {
        XCTAssertEqual(decide("progress", ["plan": 1]), .land(.progress))
        XCTAssertEqual(todo(["todo": 7, "todo_status": "in_progress"]),
                       .update(number: 7, status: "in_progress"))
    }

    func test_没挂todo就什么都不做() {
        XCTAssertEqual(todo([:]), .none)
    }

    /// 人类原话是「对应上了**就要**写最新的状态」。
    func test_挂了号就必须给状态_而且要给出路() {
        guard case let .refuse(msg) = todo(["todo": 7]) else {
            return XCTFail("只挂号不给状态却放行了 —— 账还是旧的，等于没更新")
        }
        XCTAssertTrue(msg.contains("todo_status"), msg)
        XCTAssertTrue(msg.contains("pending") && msg.contains("in_progress")
                      && msg.contains("completed"), "没把三档列全：\(msg)")
        XCTAssertTrue(msg.contains("出路") || msg.contains("去掉"),
                      "没给出路（说不准是哪一档就说明没有强对应，把 todo 去掉）：\(msg)")
    }

    /// 填了状态却没给号 —— 那个状态哪儿也去不了，**不许静默丢掉**。
    func test_给了状态却没给号要报错不能静默丢() {
        guard case let .refuse(msg) = todo(["todo_status": "in_progress"]) else {
            return XCTFail("状态没挂上任何 Todo 却被静默吞了")
        }
        XCTAssertTrue(msg.contains("todo"), msg)
    }

    /// **翻 completed 必须带凭据，这道闸不许在新路上放宽。**
    /// `respond_todo` 现在就有它：一条记成「已完成」而其实没做的账，没有任何人会回来看。
    func test_翻完成必须带凭据() {
        guard case let .refuse(msg) = todo(["todo": 7, "todo_status": "completed"]) else {
            return XCTFail("没凭据就把 Todo 翻成了 completed —— 销号那道闸被绕过去了")
        }
        XCTAssertTrue(msg.contains("evidence"), msg)
        XCTAssertTrue(msg.contains("in_progress"), "没给出路（先翻 in_progress）：\(msg)")

        XCTAssertEqual(todo(["todo": 7, "todo_status": "completed", "evidence_commit": "abc1234"]),
                       .update(number: 7, status: "completed"))
        XCTAssertEqual(todo(["todo": 7, "todo_status": "completed", "evidence": "在真机上验过"]),
                       .update(number: 7, status: "completed"))
    }

    func test_状态只认三档() {
        guard case let .refuse(msg) = todo(["todo": 7, "todo_status": "doing"]) else {
            return XCTFail("状态写错了却放行")
        }
        XCTAssertTrue(msg.contains("pending"), msg)
    }

}
