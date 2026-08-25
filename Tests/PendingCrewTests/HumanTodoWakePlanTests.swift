import XCTest

/// 人类回应「人类 Todo」后**该叫醒谁**（Todo #62 ②）。
///
/// 两条回落是写死的要求：提问的 session 已退出 → 回落机长转达；机长自己提的 →
/// 机长自己。**都不许静默丢** —— 人类拍完板没人接，等于这个功能没做。
final class HumanTodoWakePlanTests: XCTestCase {

    func testAskerStillRunningWakesTheAsker() {
        let t = HumanTodoWakePlan.resolve(
            createdBySessionId: "worker-42",
            runningSessionIds: ["worker-42", "worker-7"],
            captainSessionId: "cap-1")
        XCTAssertEqual(t, .session("worker-42"))
    }

    /// 提问的 session 已经退出 → 回落机长，不是无声无息。
    func testAskerGoneFallsBackToCaptain() {
        let t = HumanTodoWakePlan.resolve(
            createdBySessionId: "worker-42",
            runningSessionIds: ["worker-7"],
            captainSessionId: "cap-1")
        XCTAssertEqual(t, .captain)
    }

    /// 机长自己提的 → 叫醒机长自己（按「是不是机长」认，不按在不在跑）。
    func testCaptainAskedItselfWakesCaptain() {
        let t = HumanTodoWakePlan.resolve(
            createdBySessionId: "cap-1",
            runningSessionIds: ["cap-1"],
            captainSessionId: "cap-1")
        XCTAssertEqual(t, .captain)
    }

    /// 没记下提问者（老数据 / 写漏）→ 同样回落机长，绝不静默丢。
    func testMissingAskerFallsBackToCaptain() {
        XCTAssertEqual(
            HumanTodoWakePlan.resolve(createdBySessionId: nil,
                                      runningSessionIds: ["w1"], captainSessionId: "cap-1"),
            .captain)
        XCTAssertEqual(
            HumanTodoWakePlan.resolve(createdBySessionId: "",
                                      runningSessionIds: ["w1"], captainSessionId: "cap-1"),
            .captain)
    }

    /// 机长本地也没在跑时仍返回 `.captain` —— 由调用方按 `@captain` 把他拉起来，
    /// 而不是把这条答复丢掉。
    func testCaptainNotRunningStillTargetsCaptain() {
        XCTAssertEqual(
            HumanTodoWakePlan.resolve(createdBySessionId: "worker-42",
                                      runningSessionIds: [], captainSessionId: nil),
            .captain)
    }

    // MARK: - mentions 组装：全组可见 + 只叫醒该醒的那个

    func testPlanForRunningAskerIsBroadcastPlusThatSession() {
        let p = HumanTodoWakePlan.plan(createdBySessionId: "worker-42",
                                       runningSessionIds: ["worker-42"],
                                       captainSessionId: "cap-1")
        XCTAssertEqual(p.mentions, [.broadcast, .session("worker-42")])
        XCTAssertNil(p.captainReason)
        XCTAssertNil(p.fallbackNote)
    }

    func testPlanForGoneAskerIsBroadcastPlusCaptainWithRelayNote() throws {
        let p = HumanTodoWakePlan.plan(createdBySessionId: "worker-42",
                                       runningSessionIds: [],
                                       captainSessionId: "cap-1")
        XCTAssertEqual(p.mentions, [.broadcast, .captain])
        XCTAssertEqual(p.captainReason, .askerGone("worker-42"))
        // 回落时必须说清「为什么这条到了你手上」，否则机长不知道该转达。
        let note = try XCTUnwrap(p.fallbackNote)
        XCTAssertTrue(note.contains("worker-42"))
        XCTAssertTrue(note.contains("转达"))
    }

    /// 机长自己提的：叫醒机长，但**不**加转达提示 —— 那就是给他本人的答复。
    func testPlanForCaptainAskerHasNoRelayNote() {
        let p = HumanTodoWakePlan.plan(createdBySessionId: "cap-1",
                                       runningSessionIds: ["cap-1"],
                                       captainSessionId: "cap-1")
        XCTAssertEqual(p.mentions, [.broadcast, .captain])
        XCTAssertEqual(p.captainReason, .askedByCaptain)
        XCTAssertNil(p.fallbackNote)
    }

    func testPlanForUnknownAskerAsksCaptainToClaim() {
        let p = HumanTodoWakePlan.plan(createdBySessionId: nil,
                                       runningSessionIds: ["w1"],
                                       captainSessionId: "cap-1")
        XCTAssertEqual(p.mentions, [.broadcast, .captain])
        XCTAssertEqual(p.captainReason, .askerUnknown)
        XCTAssertNotNil(p.fallbackNote)
    }

    /// 每一种计划都带 `.broadcast` —— 人类原话是「直接在群里发消息」，
    /// 那条回应全组都该看得见，只是别把全组叫醒。
    func testEveryPlanIsVisibleToTheWholeCrew() {
        let plans = [
            HumanTodoWakePlan.plan(createdBySessionId: "w1", runningSessionIds: ["w1"],
                                   captainSessionId: "cap-1"),
            HumanTodoWakePlan.plan(createdBySessionId: "w1", runningSessionIds: [],
                                   captainSessionId: "cap-1"),
            HumanTodoWakePlan.plan(createdBySessionId: nil, runningSessionIds: [],
                                   captainSessionId: nil),
        ]
        for p in plans {
            XCTAssertTrue(p.mentions.contains(.broadcast), "\(p.target) 少了 broadcast")
            XCTAssertEqual(p.mentions.count, 2, "只该有 broadcast + 一个唤醒目标")
        }
    }
}
