import XCTest

/// Todo 落地剧本（Todo #62）—— 顺序、失败回执、群里那行挂什么 @，只有这一份。
///
/// 这一族守的是**「什么都没发生」必须响**。同族两次事故：#577「没落盘就宣布」，
/// 以及 helper 里 fire-and-forget 的 `Task` 一声不吭永远不跑（实测：主线程卡在
/// `readLine`，`@MainActor` 的 async hop 直接挂死）。两次都是同一个形状 ——
/// 失败长得像成功。所以下面每一条都在钉「没成的时候，回执说没说实话」。
final class TodoLandingFlowTests: XCTestCase {

    // ── 顺序 ──────────────────────────────────────────────────────────────

    /// 落账 → 发群 → 唤醒。**先落账再宣布**，颠倒就是 #577。
    func testStepOrderIsPersistThenAnnounceThenWake() {
        XCTAssertEqual(TodoLandingFlow.order, [.persisted, .announced, .woke])
        XCTAssertTrue(TodoLandingFlow.Step.nothing < .persisted)
        XCTAssertTrue(TodoLandingFlow.Step.persisted < .announced)
        XCTAssertTrue(TodoLandingFlow.Step.announced < .woke)
    }

    /// 两个动作的终点不同：agent 加一条不用叫醒谁；人类拍完板必须把提问者叫回来。
    func testTerminalStepDiffersByAction() {
        XCTAssertEqual(TodoLandingFlow.terminal(.added), .announced)
        XCTAssertEqual(TodoLandingFlow.terminal(.responded), .woke)
    }

    // ── 第一步就没成：绝不许说得像成了 ─────────────────────────────────────

    /// 落账失败的回执里**不许出现**「已记入」，也**不许给号** —— 账上根本没有那条，
    /// 给了号等于凭空造一个出来。
    func testNotPersistedReceiptNeverClaimsSuccess() {
        let r = TodoLandingFlow.receipt(ledger: .human, action: .added,
                                        number: 7, reached: .nothing)
        XCTAssertFalse(r.contains("已记入"), "实得：\(r)")
        XCTAssertFalse(r.contains("#7"), "账上没有这条，回执不许给号；实得：\(r)")
        XCTAssertTrue(r.contains("没有记下"))
    }

    /// 号给了、但 `reached` 是 `.nothing` —— 一样按没落账处理（拿意图当结果这条路堵死）。
    func testNumberIsIgnoredWhenNothingPersisted() {
        XCTAssertEqual(
            TodoLandingFlow.receipt(ledger: .human, action: .added,
                                    number: 7, reached: .nothing),
            TodoLandingFlow.notPersistedReceipt(ledger: .human, action: .added))
    }

    /// 人类回应那条也一样：没落上就别说「已回应」，也别让人以为群里有了。
    func testRespondNotPersistedReceipt() {
        let r = TodoLandingFlow.receipt(ledger: .human, action: .responded,
                                        number: 3, reached: .nothing)
        XCTAssertFalse(r.contains("已回应"), "实得：\(r)")
        XCTAssertTrue(r.contains("没有记下"))
        XCTAssertTrue(r.contains("群里也不会出现"))
    }

    // ── 中途没成：账在、话没说出去 ─────────────────────────────────────────

    /// 落了账、群里没发成 → 必须同时说清两件：记下了 **和** 群里没人看得见。
    func testPersistedButNotAnnouncedTellsBoth() {
        let r = TodoLandingFlow.receipt(ledger: .human, action: .added, number: 4,
                                        reached: .persisted, detail: "磁盘满了")
        XCTAssertTrue(r.contains("已记入人类 Todo #4"))
        XCTAssertTrue(r.contains("群里那行没发出去"))
        XCTAssertTrue(r.contains("磁盘满了"))
    }

    /// 发群成了、没叫醒 → 只有 `.responded` 会走到这一档（`.added` 的终点就是发群）。
    func testAnnouncedButNotWokenOnlyWarnsForRespond() {
        let respond = TodoLandingFlow.receipt(ledger: .human, action: .responded, number: 4,
                                              reached: .announced, detail: "session 拉不起来")
        XCTAssertTrue(respond.contains("没能叫醒"))
        XCTAssertTrue(respond.contains("session 拉不起来"))

        // 同一步对 `.added` 就是终点 —— 不许无中生有报一条「没叫醒」的警。
        let added = TodoLandingFlow.receipt(ledger: .human, action: .added, number: 4,
                                            reached: .announced, detail: "不该被用上")
        XCTAssertFalse(added.contains("没能叫醒"), "实得：\(added)")
        XCTAssertFalse(added.contains("⚠️"), "实得：\(added)")
    }

    /// 全都成了 —— 一句警示都不许有。
    func testFullySuccessfulReceiptCarriesNoWarning() {
        let r = TodoLandingFlow.receipt(ledger: .human, action: .responded, number: 9,
                                        reached: .woke)
        XCTAssertEqual(r, "已回应人类 Todo #9。")
    }

    /// 新增的回执里那句预告，用的就是 `TodoLedger` 那一份文案 —— 两处别各写各的。
    func testAddedReceiptQuotesTheLedgerWording() {
        let r = TodoLandingFlow.receipt(ledger: .human, action: .added, number: 5,
                                        reached: .announced)
        XCTAssertTrue(r.contains(TodoLedger.human.responseAnnouncement(number: 5, text: "…")),
                      "实得：\(r)")
    }

    // ── 群里那行挂什么 @ ──────────────────────────────────────────────────

    /// 新增那行只标 `@human`：讲给人听的、别为它叫醒 agent，但队友照样看得见。
    func testAddedAnnouncementMentionsHumanOnly() {
        XCTAssertEqual(TodoLandingFlow.mentions(.added),
                       [CrewMention(kind: "human", targetId: nil)])
    }

    /// 回应那行走 `HumanTodoWakePlan`：全组可见 + 只叫醒提问者。
    func testRespondedAnnouncementUsesWakePlanMentions() {
        let plan = HumanTodoWakePlan.plan(createdBySessionId: "w-1",
                                          runningSessionIds: ["w-1"],
                                          captainSessionId: "cap")
        XCTAssertEqual(TodoLandingFlow.mentions(.responded, wake: plan),
                       [.broadcast, .session("w-1")])
    }

    /// 拿不到计划（不该发生）→ 退回**纯广播**，宁可多让人看见，也不静默变私信。
    func testRespondedWithoutPlanFallsBackToBroadcast() {
        XCTAssertEqual(TodoLandingFlow.mentions(.responded, wake: nil), [.broadcast])
    }
}
