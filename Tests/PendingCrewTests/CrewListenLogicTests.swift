#if os(macOS)
import XCTest
// 待测源码直接编进 PendingCrewTests bundle（standalone，无 app module）——
// CrewListenLogic / CrewLocalMentionInjectLogic / LocalWhiteboardStore 均见 project.yml。

/// `CrewListenLogic` 纯决策核心单测（#465 群聊收听）。
final class CrewListenLogicTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func msg(
        id: String = UUID().uuidString,
        kind: String = "user",
        sessionId: String? = nil,
        text: String = "hello",
        name: String? = nil,
        mentions: [LocalWhiteboardMention]? = nil
    ) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: kind, senderUserId: kind == "user" ? "u1" : nil,
            senderSessionId: sessionId, category: nil, text: text,
            createdAt: "2027-01-15T00:00:00Z", senderName: name, mentions: mentions)
    }

    private func listener(
        _ sessionId: String = "s-listen",
        untilOffset: TimeInterval = 600,
        senders: [String]? = nil
    ) -> CrewListenLogic.Listener {
        .init(sessionId: sessionId, until: now.addingTimeInterval(untilOffset), senders: senders)
    }

    private func idleRun(_ id: String = "s-listen") -> CrewLocalMentionInjectLogic.RunState {
        .init(sessionId: id, isBusy: false)
    }

    // ── 基本投递 ──────────────────────────────────────────────────────────

    func testBroadcastDeliveredToIdleListener() {
        let out = CrewListenLogic.decide(
            entries: [msg(text: "有人看下这个吗")],
            listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].sessionId, "s-listen")
        XCTAssertTrue(out[0].text.contains("有人看下这个吗"))
        XCTAssertTrue(out[0].text.contains("群聊收听"), "短标头点明这是收听送达")
        XCTAssertFalse(out[0].text.contains("prompt injection"), "长免责说明不再进注入面(#484)")
    }

    func testBusyListenerSkipped() {
        let entry = msg()
        let runs = [CrewLocalMentionInjectLogic.RunState(sessionId: "s-listen", isBusy: true)]
        let out = CrewListenLogic.decide(
            entries: [entry], listeners: [listener()], runs: runs, now: now)
        XCTAssertTrue(out.isEmpty, "纯决策仍不立即打断 busy")
        let planned = CrewListenLogic.plannedInjections(
            entries: [entry], listeners: [listener()], runs: runs, now: now)
        XCTAssertEqual(planned.count, 1,
                       "busy 只能延迟收听投递，不能丢掉计划或等待下一条消息")
    }

    func testExpiredListenerSkipped() {
        let out = CrewListenLogic.decide(
            entries: [msg()], listeners: [listener(untilOffset: -1)],
            runs: [idleRun()], now: now)
        XCTAssertTrue(out.isEmpty)
    }

    func testNoRunForListenerSkipped() {
        let out = CrewListenLogic.decide(
            entries: [msg()], listeners: [listener()], runs: [], now: now)
        XCTAssertTrue(out.isEmpty)
    }

    // ── 消息过滤 ──────────────────────────────────────────────────────────

    func testOwnMessageNotDelivered() {
        let out = CrewListenLogic.decide(
            entries: [msg(kind: "session", sessionId: "s-listen")],
            listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertTrue(out.isEmpty)
    }

    func testSystemNoticeNotDelivered() {
        let out = CrewListenLogic.decide(
            entries: [msg(kind: "session", sessionId: "system", name: "系统")],
            listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertTrue(out.isEmpty)
    }

    func testMentionToOthersFilteredOut() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "someone-else")])
        let out = CrewListenLogic.decide(
            entries: [m], listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertTrue(out.isEmpty, "@ 了别人的定向消息不进收听者视野")
    }

    func testMentionToListenerHandledByDirectedWakeNotListen() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "s-listen")])
        let out = CrewListenLogic.decide(
            entries: [m], listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertTrue(out.isEmpty, "定向 @ 统一走 mention 链，listen 不重复投")
    }

    func testExplicitBroadcastMentionStillDelivered() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "broadcast", targetId: nil)])
        let out = CrewListenLogic.decide(
            entries: [m], listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertEqual(out.count, 1)
    }

    /// #543：收听的语义是「广播也叫醒我」，**不是**「别人的定向 @ 也灌给我」——
    /// 开着 listen 的非目标不该被机长的定向派工唤醒（同一批里的广播照送）。
    func testListeningNonTargetNotWokenByDirectedMention() {
        let directed = msg(kind: "captain", sessionId: "cap-1", text: "去修 X", name: "机长",
                           mentions: [LocalWhiteboardMention(kind: "session", targetId: "s-other")])
        let broadcast = msg(kind: "captain", sessionId: "cap-1", text: "全员周知", name: "机长")
        let out = CrewListenLogic.decide(
            entries: [directed, broadcast],
            listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertEqual(out.count, 1)
        XCTAssertFalse(out[0].text.contains("去修 X"), "@ 别人的定向不进收听者视野")
        XCTAssertTrue(out[0].text.contains("全员周知"), "无 @ 的广播照常送达")
    }

    /// 整批都是 @ 别人的 → 收听者一次都不该被叫醒。
    func testListeningNonTargetGetsNothingWhenBatchAllDirected() {
        let out = CrewListenLogic.decide(
            entries: [msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "s-a")]),
                      msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "s-b")])],
            listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertTrue(out.isEmpty)
    }

    /// `@captain` 同样统一走定向唤醒，机长开着 listen 也不能再投一份。
    func testCaptainMentionNotDuplicatedThroughListen() {
        let m = msg(kind: "session", sessionId: "w-9", text: "机长看下",
                    mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        var capListener = listener("s-cap")
        capListener.isCaptain = true
        let toCaptain = CrewListenLogic.decide(
            entries: [m], listeners: [capListener], runs: [idleRun("s-cap")], now: now)
        XCTAssertTrue(toCaptain.isEmpty)
        let toWorker = CrewListenLogic.decide(
            entries: [m], listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertTrue(toWorker.isEmpty)
    }

    /// 人类无 @ 默认路由机长；机长自己开 listen 不能收到第二份。普通 worker
    /// 主动 listen 仍能收到这条广播。
    func testHumanBroadcastDefaultsToCaptainWithoutListenDuplicate() {
        let m = msg(kind: "user", text: "有人在吗")
        var captain = listener("s-cap")
        captain.isCaptain = true
        XCTAssertTrue(CrewListenLogic.decide(
            entries: [m], listeners: [captain], runs: [idleRun("s-cap")], now: now
        ).isEmpty)
        XCTAssertEqual(CrewListenLogic.decide(
            entries: [m], listeners: [listener("s-worker")], runs: [idleRun("s-worker")], now: now
        ).count, 1)
    }

    // ── 发送者过滤 ────────────────────────────────────────────────────────

    func testSenderFilterHumanOnly() {
        let humanMsg = msg(kind: "user", text: "主人指令")
        let peerMsg = msg(kind: "session", sessionId: "s-peer", text: "同事进展", name: "worker")
        let out = CrewListenLogic.decide(
            entries: [humanMsg, peerMsg],
            listeners: [listener(senders: ["human"])], runs: [idleRun()], now: now)
        XCTAssertEqual(out.count, 1)
        XCTAssertTrue(out[0].text.contains("主人指令"))
        XCTAssertFalse(out[0].text.contains("同事进展"))
    }

    /// listen 的 sender 过滤只控制主动收听，不能反过来挡住人类无定向消息的
    /// 默认 captain wake。现场 captain 即使只听 session，也仍由 mention waker 接。
    func testListenSenderFilterCannotBlockDefaultHumanCaptainWake() {
        let human = msg(kind: "user", text: "刚刚那个列表是什么？")
        var captain = listener("s-cap", senders: ["some-worker"])
        captain.isCaptain = true

        XCTAssertTrue(CrewListenLogic.plannedInjections(
            entries: [human], listeners: [captain], runs: [idleRun("s-cap")], now: now
        ).isEmpty, "sender filter 可以不命中主动 listen")
        let justAfterMessage = ISO8601DateFormatter().date(from: "2027-01-15T00:00:01Z")!
        XCTAssertEqual(
            CrewLocalMentionWakeLogic.pending(entries: [human], now: justAfterMessage).first?.mentions,
            [.captain],
            "默认 captain wake 是独立链，不能被 listen sender filter 吃掉")
    }

    func testSenderFilterCaptain() {
        let capMsg = msg(kind: "captain", sessionId: "s-cap", text: "机长安排", name: "机长")
        XCTAssertTrue(CrewListenLogic.senderMatches(capMsg, filter: ["captain"]))
        XCTAssertFalse(CrewListenLogic.senderMatches(capMsg, filter: ["human"]))
    }

    func testSenderFilterBySessionIdPrefix() {
        let peer = msg(kind: "session", sessionId: "abc123def", name: "worker")
        XCTAssertTrue(CrewListenLogic.senderMatches(peer, filter: ["abc123"]))
        XCTAssertFalse(CrewListenLogic.senderMatches(peer, filter: ["zzz"]))
    }

    func testSenderFilterByDisplayName() {
        let peer = msg(kind: "session", sessionId: "abc123def", name: "Claude Code · abc123")
        XCTAssertTrue(CrewListenLogic.senderMatches(peer, filter: ["Claude Code · abc123"]))
    }

    // ── 多收听者 / 打包 ───────────────────────────────────────────────────

    func testMultipleListenersEachGetOwnInjection() {
        let out = CrewListenLogic.decide(
            entries: [msg()],
            listeners: [listener("s-a"), listener("s-b")],
            runs: [idleRun("s-a"), idleRun("s-b")], now: now)
        XCTAssertEqual(Set(out.map(\.sessionId)), ["s-a", "s-b"])
    }

    func testBatchBundledIntoOneInjection() {
        let out = CrewListenLogic.decide(
            entries: [msg(text: "第一条"), msg(text: "第二条")],
            listeners: [listener()], runs: [idleRun()], now: now)
        XCTAssertEqual(out.count, 1, "同批多条消息打包成一次注入")
        XCTAssertTrue(out[0].text.contains("第一条"))
        XCTAssertTrue(out[0].text.contains("第二条"))
    }
}
#endif
