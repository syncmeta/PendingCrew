#if os(macOS)
import XCTest
// CrewLocalMentionInjectLogic + CrewModels 直接编进 PendingCrewTests target，无需 import。

/// 本地直投唤醒纯决策核心(`CrewLocalMentionInjectLogic.decide`)的单测
/// (Phase 6 单元 3):「@某 session + 空闲 → 注入唤醒」「busy → 不打断」
/// 「无对应本地 run / 非 @session → 不注入」「同 session 多 @ 去重」。
final class CrewLocalMentionInjectLogicTests: XCTestCase {

    private func run(_ sid: String, busy: Bool, claude: Bool = false) -> CrewLocalMentionInjectLogic.RunState {
        .init(sessionId: sid, isBusy: busy, isClaude: claude)
    }

    private func wbMsg(_ kind: String, _ text: String, name: String? = nil) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: kind, senderUserId: nil, senderSessionId: nil,
            category: nil, text: text, createdAt: "2026-07-13T00:00:00Z", senderName: name)
    }

    func testIdleSessionGetsInjected() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("sess-a")],
            runs: [run("sess-a", busy: false)],
            messageText: "跑下测试",
            senderName: "我")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out.first?.sessionId, "sess-a")
        XCTAssertTrue(out.first!.text.contains("有人@你："))
        XCTAssertTrue(out.first!.text.contains("我: 跑下测试"))
    }

    func testBusySessionNotInterrupted() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("sess-a")],
            runs: [run("sess-a", busy: true)],
            messageText: "在吗",
            senderName: "我")
        XCTAssertTrue(out.isEmpty, "busy run 不打断")
    }

    /// 回归：机长 busy 时只来这一条消息；随后自己变 idle，期间没有第二条消息。
    /// idle 边沿必须补投一次，重复 idle / 重复扫描都不能再投。
    func testBusyCaptainMessageDeliveredOnceAfterIdleWithoutSecondMessage() {
        let planned = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: [.captain],
            runs: [run("captain-abc", busy: true)],
            messageText: "为什么没有审批卡？",
            senderName: "人",
            captainSessionId: "captain-abc",
            imStyle: true)
        XCTAssertEqual(planned.count, 1, "busy 只延迟投递，不能丢掉计划")

        var queue = CrewDeferredWakeQueue()
        let delivery = CrewDeferredWakeQueue.Delivery(
            key: "whiteboard:msg-1|target:captain-abc",
            targetSessionId: "captain-abc",
            text: planned[0].text)
        XCTAssertEqual(queue.submit(delivery, isBusy: true), .deferred)
        XCTAssertEqual(queue.pendingCount(sessionId: "captain-abc"), 1)

        // 没有第二次 submit，仅 busy -> idle 事件就拿到这条。
        XCTAssertEqual(queue.popWhenIdle(sessionId: "captain-abc"), delivery)
        XCTAssertNil(queue.popWhenIdle(sessionId: "captain-abc"),
                     "重复 idle 信号不能重复投递")
        XCTAssertEqual(queue.submit(delivery, isBusy: false), .duplicate,
                       "同一消息被其它观察路重扫也不能重复投递")
    }

    /// 多条消息在同一个 busy 窗口积压时，每次 idle 只拿一条。第一条 send 起的新
    /// turn 不会被第二条紧接着打断；下一次 turn 完成再拿下一条。
    func testDeferredQueuePopsOneMessagePerIdleTransition() {
        var queue = CrewDeferredWakeQueue()
        let first = CrewDeferredWakeQueue.Delivery(
            key: "m1|target:s", targetSessionId: "s", text: "first")
        let second = CrewDeferredWakeQueue.Delivery(
            key: "m2|target:s", targetSessionId: "s", text: "second")
        XCTAssertEqual(queue.submit(first, isBusy: true), .deferred)
        XCTAssertEqual(queue.submit(second, isBusy: true), .deferred)
        XCTAssertEqual(queue.popWhenIdle(sessionId: "s"), first)
        XCTAssertEqual(queue.pendingCount(sessionId: "s"), 1)
        XCTAssertEqual(queue.popWhenIdle(sessionId: "s"), second)
        XCTAssertEqual(queue.pendingCount(sessionId: "s"), 0)
    }

    func testNoLocalRunForSessionIsSkipped() {
        // @ 了一个本地没有对应 run 的 session(远端 / 已结束) → 不注入。
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("sess-remote")],
            runs: [run("sess-a", busy: false)],
            messageText: "x",
            senderName: "我")
        XCTAssertTrue(out.isEmpty)
    }

    func testBroadcastAndCaptainAndHumanIgnored() {
        // 非 @session 的 mention 在本地直投路径不唤醒具体 run。
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.broadcast, .captain, CrewMention(kind: "human", targetId: "u1")],
            runs: [run("sess-a", busy: false)],
            messageText: "大家好",
            senderName: "我")
        XCTAssertTrue(out.isEmpty)
    }

    func testMixedTargetsOnlyInjectIdleSessions() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("sess-a"), .session("sess-b"), .broadcast],
            runs: [run("sess-a", busy: false), run("sess-b", busy: true)],
            messageText: "看下",
            senderName: "我")
        // sess-a idle → inject; sess-b busy → skip; broadcast ignored.
        XCTAssertEqual(out.map(\.sessionId), ["sess-a"])
    }

    // ── #543: 定向 @ 只唤醒目标，广播唤醒面不变 ─────────────────────────────

    /// 事故形状复现：同 crew 三个空闲 worker，机长只 @ 了其中一个 —— 唤醒面必须
    /// 只有那一个（唤醒路本就正确，这条钉死它不被后续改动带歪）。
    func testDirectedMentionWakesOnlyTargetAmongIdlePeers() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("worker-1")],
            runs: [run("worker-1", busy: false),
                   run("worker-2", busy: false),
                   run("worker-3", busy: false)],
            messageText: "去修 X",
            senderName: "机长")
        XCTAssertEqual(out.map(\.sessionId), ["worker-1"],
                       "定向派给 worker-1 的活不该唤醒 worker-2/3（#543 三方撞车事故）")
    }

    /// 广播（无 mentions）语义原样：本地直投路径一个 run 都不叫醒，靠白板每轮
    /// 注入覆盖全员 —— 修定向扩散不能顺手把广播也收窄或放宽。
    func testBroadcastWakeSurfaceUnchanged() {
        let idle = [run("worker-1", busy: false), run("worker-2", busy: false)]
        XCTAssertTrue(CrewLocalMentionInjectLogic.decide(
            mentions: [], runs: idle, messageText: "大家看下", senderName: "机长").isEmpty)
        XCTAssertTrue(CrewLocalMentionInjectLogic.decide(
            mentions: [.broadcast], runs: idle, messageText: "大家看下", senderName: "机长").isEmpty)
        // 拉起面同理：广播不拉起任何缺席目标。
        let wake = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: [.broadcast], runningSessionIds: [], captainRunning: true)
        XCTAssertFalse(wake.needCaptain)
        XCTAssertTrue(wake.sessionIds.isEmpty)
    }

    func testSameSessionMentionedTwiceInjectsOnce() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("sess-a"), .session("sess-a")],
            runs: [run("sess-a", busy: false)],
            messageText: "x",
            senderName: "我")
        XCTAssertEqual(out.count, 1)
    }

    // ── #1: @机长 唤醒 captain run ───────────────────────────────────────────

    func testCaptainMentionWakesCaptainRun() {
        // @机长 → 调用方把 captain 本地 run 的 sessionId 传入 → 空闲则注入唤醒(#1)。
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.captain],
            runs: [run("captain-abc", busy: false)],
            messageText: "汇报进度",
            senderName: "我",
            captainSessionId: "captain-abc")
        XCTAssertEqual(out.map(\.sessionId), ["captain-abc"])
        XCTAssertTrue(out.first!.text.contains("汇报进度"))
    }

    func testCaptainMentionBusyNotInterrupted() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.captain],
            runs: [run("captain-abc", busy: true)],
            messageText: "x", senderName: "我",
            captainSessionId: "captain-abc")
        XCTAssertTrue(out.isEmpty, "captain busy 不打断")
    }

    func testCaptainMentionWithoutRunningCaptainIgnored() {
        // 本地没在跑 captain(captainSessionId nil)→ @机长 不注入(不崩)——
        // 该场景由 wakeTargets 接手真拉起(见下)。
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.captain],
            runs: [run("sess-a", busy: false)],
            messageText: "x", senderName: "我",
            captainSessionId: nil)
        XCTAssertTrue(out.isEmpty)
    }

    // MARK: - wakeTargets（@ 目标不在跑 → 拉起谁）

    func testWakeCaptainWhenNotRunning() {
        let w = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: [.captain], runningSessionIds: [], captainRunning: false)
        XCTAssertTrue(w.needCaptain)
        XCTAssertTrue(w.sessionIds.isEmpty)
    }

    func testNoCaptainWakeWhenRunning() {
        let w = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: [.captain], runningSessionIds: ["captain-abc"], captainRunning: true)
        XCTAssertFalse(w.needCaptain, "在跑的机长归注入路径,不重复拉起")
    }

    func testWakeExitedSessionAndSkipRunning() {
        let w = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: [.session("sess-dead"), .session("sess-live"), .session("sess-dead")],
            runningSessionIds: ["sess-live"], captainRunning: true)
        XCTAssertEqual(w.sessionIds, ["sess-dead"], "在跑的跳过、重复 @ 去重")
        XCTAssertFalse(w.needCaptain)
    }

    func testBroadcastAndHumanNeverWake() {
        let w = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: [.broadcast, CrewMention(kind: "human", targetId: "u1")],
            runningSessionIds: [], captainRunning: false)
        XCTAssertFalse(w.needCaptain)
        XCTAssertTrue(w.sessionIds.isEmpty)
    }

    // MARK: - 项8：近期群聊上下文（仅 claude 目标）

    func testRecentContextOnlyForClaudeTarget() {
        let recent = [wbMsg("captain", "前情提要")]
        // claude 目标 → 前置近期群聊；codex 目标 → 不塞（它自带 additionalContext）。
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("claude-run"), .session("codex-run")],
            runs: [run("claude-run", busy: false, claude: true),
                   run("codex-run", busy: false, claude: false)],
            messageText: "看下", senderName: "我", recent: { _ in recent })
        let byId = Dictionary(uniqueKeysWithValues: out.map { ($0.sessionId, $0.text) })
        XCTAssertTrue(byId["claude-run"]!.contains("近期群聊："), byId["claude-run"]!)
        XCTAssertTrue(byId["claude-run"]!.contains("机长: 前情提要"))
        XCTAssertFalse(byId["codex-run"]!.contains("近期群聊："), "codex 目标不前置上下文")
    }

    func testRecentContextPrependedBeforeDirected() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.session("a")], runs: [run("a", busy: false, claude: true)],
            messageText: "跑测试", senderName: "我", recent: { _ in [wbMsg("user", "早")] })
        let t = out.first!.text
        XCTAssertTrue(t.range(of: "近期群聊：")!.lowerBound < t.range(of: "有人@你：")!.lowerBound)
    }

    // MARK: - 项10：无 @ 默认给机长 + IM 式渲染

    func testImStyleRendersSenderColonBodyNoAtShell() {
        // 无具体 @ → 默认 @机长 + IM 式「名：正文」，不套「有人@你」壳。
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.captain], runs: [run("captain-abc", busy: false)],
            messageText: "在忙吗", senderName: "小明",
            captainSessionId: "captain-abc", imStyle: true)
        let t = out.first!.text
        XCTAssertEqual(t, "小明：在忙吗")
        XCTAssertFalse(t.contains("有人@你"), "IM 式不套定向壳")
    }

    func testImStyleWithRecentContextForClaudeCaptain() {
        let out = CrewLocalMentionInjectLogic.decide(
            mentions: [.captain], runs: [run("cap", busy: false, claude: true)],
            messageText: "在吗", senderName: "小明",
            captainSessionId: "cap", recent: { _ in [wbMsg("user", "早")] }, imStyle: true)
        let t = out.first!.text
        XCTAssertTrue(t.hasPrefix("近期群聊："), t)
        XCTAssertTrue(t.hasSuffix("小明：在吗"), t)
        XCTAssertFalse(t.contains("有人@你"))
    }

    // MARK: - kind 反推（重启已退成员时用）

    func testKindInferredFromMemberDisplayName() {
        XCTAssertEqual(LocalCodingAgentKind.inferred(fromDisplayName: "Claude Code · ab12cd"), .claudeCode)
        XCTAssertEqual(LocalCodingAgentKind.inferred(fromDisplayName: "Codex · ef34gh"), .codex)
        XCTAssertNil(LocalCodingAgentKind.inferred(fromDisplayName: "机长"))
    }
}
#endif
