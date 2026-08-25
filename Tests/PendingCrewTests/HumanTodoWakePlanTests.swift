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

/// **实测**：`HumanTodoWakePlan` 组出来的 mentions，喂进 A 线那套真判定
/// （`CrewWhiteboardVisibility` / `CrewLocalMentionInjectLogic`），是不是真的
/// 「全组可见 + 只叫醒提问者 + 别人看得出不是自己的活」。
///
/// 这一族不测我自己的枚举，测的是**两条线接上以后成不成立** —— brief 明写
/// 「rebase 之后要实测这条真的成立，不成立就报，别自己另造机制」。
final class HumanTodoWakePlanAgainstRealMentionLogicTests: XCTestCase {

    /// 把计划里的 mentions 装成一条真白板消息（人类发的那条回应）。
    private func message(_ mentions: [CrewMention]) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: "m1", senderKind: "user", senderUserId: "local-byok-user",
            senderSessionId: nil, category: nil,
            text: "回应 人类 To Do #3：选 A",
            createdAt: "2026-08-25T00:00:00Z",
            senderName: "人",
            mentions: mentions.map { LocalWhiteboardMention(kind: $0.kind, targetId: $0.targetId) })
    }

    func testAskerRunningPlanIsVisibleToEveryoneButWakesOnlyTheAsker() {
        let plan = HumanTodoWakePlan.plan(createdBySessionId: "w-1",
                                          runningSessionIds: ["w-1", "w-2"],
                                          captainSessionId: "cap-1")
        let msg = message(plan.mentions)

        // 全组可见 —— 提问者、旁观 worker、机长，一个都不被挡。
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg, to: "w-2"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg, to: "cap-1", isCaptain: true))

        // 只叫醒提问者 —— 旁观 worker 和机长都不在唤醒名单里。
        let injections = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: plan.mentions,
            runs: [.init(sessionId: "w-1", isBusy: false),
                   .init(sessionId: "w-2", isBusy: false),
                   .init(sessionId: "cap-1", isBusy: false)],
            messageText: msg.text, senderName: "人", captainSessionId: "cap-1")
        XCTAssertEqual(injections.map(\.sessionId), ["w-1"])
    }

    /// 旁观者看得见，但注入面上标着「不是给你的」—— #543 的病根靠标注治，
    /// 不是靠藏起来。
    func testBystanderSeesItButIsToldItIsNotTheirs() {
        let plan = HumanTodoWakePlan.plan(createdBySessionId: "w-1",
                                          runningSessionIds: ["w-1"],
                                          captainSessionId: "cap-1")
        let msg = message(plan.mentions)
        XCTAssertEqual(
            CrewWhiteboardVisibility.directedNote(msg, to: "w-2",
                                                  displayName: { $0 == "w-1" ? "小王" : nil }),
            "（发给 小王 的）")
        // 提问者本人不带标注 —— 这条本来就是冲他来的。
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(msg, to: "w-1"))
    }

    /// 回落到机长的那版同样成立：全组可见、只叫醒机长。
    func testCaptainFallbackPlanIsVisibleToEveryoneButWakesOnlyCaptain() {
        let plan = HumanTodoWakePlan.plan(createdBySessionId: "w-1",
                                          runningSessionIds: [],   // 提问者已退出
                                          captainSessionId: "cap-1")
        let msg = message(plan.mentions)
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg, to: "w-2"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg, to: "cap-1", isCaptain: true))

        let injections = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: plan.mentions,
            runs: [.init(sessionId: "w-2", isBusy: false),
                   .init(sessionId: "cap-1", isBusy: false)],
            messageText: msg.text, senderName: "人", captainSessionId: "cap-1")
        XCTAssertEqual(injections.map(\.sessionId), ["cap-1"])
    }

    /// 反证：**不写 broadcast** 的话这条就是排他的 —— 说明「全组可见」是
    /// `.broadcast` 挣来的，不是碰巧。漏写它，旁观者就看不见了。
    func testWithoutBroadcastTheSameMessageWouldBeExclusive() {
        let msg = message([.session("w-1")])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(msg, to: "w-2"))
    }

    /// 提问者不在跑时不产生注入 —— 由调用方按 `wakeTargets` 把它拉起来。
    /// 回落到机长的计划里，机长没在跑同样进 `needCaptain`，不会静默丢。
    func testCaptainFallbackStillAsksToLaunchCaptainWhenNotRunning() {
        let plan = HumanTodoWakePlan.plan(createdBySessionId: nil,
                                          runningSessionIds: [],
                                          captainSessionId: nil)
        let targets = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: plan.mentions, runningSessionIds: [], captainRunning: false)
        XCTAssertTrue(targets.needCaptain)
        XCTAssertTrue(targets.sessionIds.isEmpty)
    }
}
