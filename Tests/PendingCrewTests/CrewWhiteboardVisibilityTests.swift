import XCTest
// CrewWhiteboardVisibility.swift + LocalWhiteboardStore.swift 编进 test bundle（见 project.yml）。

/// 白板消息 → session 注入面可见性的单一判定单测（#543 定向 @ 扩散根因）。
final class CrewWhiteboardVisibilityTests: XCTestCase {

    private func msg(
        sessionId: String? = "s-sender",
        kind: String = "session",
        mentions: [LocalWhiteboardMention]? = nil
    ) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: kind, senderUserId: nil,
            senderSessionId: sessionId, category: nil, text: "去把 X 做了",
            createdAt: "2027-01-15T00:00:00Z", senderName: "机长", mentions: mentions)
    }

    // ── 广播：唤醒/可见面不变 ─────────────────────────────────────────────

    func testBroadcastVisibleToEveryone() {
        let m = msg()
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-2"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    func testEmptyMentionsTreatedAsBroadcast() {
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg(mentions: []), to: "w-1"))
    }

    // ── 定向 @：只进被点名者的注入面 ───────────────────────────────────────

    func testDirectedSessionMentionOnlyVisibleToTarget() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-2"),
                       "定向派给 w-1 的活不该出现在 w-2 的注入面（#543 事故）")
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-3"))
    }

    func testCaptainMentionOnlyVisibleToCaptain() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
    }

    /// 2026-08-23 修的正主：`@人类` 过去落在 `default: return false` 上，于是一条
    /// 只 @ 了人类的消息对**每一个** agent 隐身 —— 队友「@人 我做完了」发出去，全
    /// crew 没人看得见。human 是附加标记，不收窄可见范围。
    func testHumanOnlyMentionStillVisibleToEveryAgent() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"),
                      "只 @ 人类的消息不该从第三方 session 眼前消失")
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-2"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true),
                      "机长也不该看不见「@人 汇报…」")
    }

    /// 显式 broadcast mention 同样不收窄（mentions 非空 ≠ 定向）。
    func testBroadcastMentionStillVisibleToEveryone() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "broadcast", targetId: nil)])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    /// #543 不许回退：混了 human 也不放宽 session 的排他性。
    func testSessionPlusHumanStaysSessionExclusive() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
            LocalWhiteboardMention(kind: "human", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-2"),
                       "@session 配 @human 仍按 session 排他（#543 一个字不松）")
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    /// 同理 `@captain + @human`。
    func testCaptainPlusHumanStaysCaptainExclusive() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "captain", targetId: nil),
            LocalWhiteboardMention(kind: "human", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
    }

    func testSenderAlwaysSeesOwnDirectedMessage() {
        let m = msg(sessionId: "s-sender",
                    mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "s-sender"),
                      "自己发的定向 @ 在自己上下文里不该凭空消失")
    }

    func testMultiMentionVisibleToEachTarget() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
            LocalWhiteboardMention(kind: "captain", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-2"))
    }

    func testSessionMentionWithoutTargetIdVisibleToNobody() {
        let m = msg(sessionId: nil,
                    mentions: [LocalWhiteboardMention(kind: "session", targetId: nil)])
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-1"),
                       "缺 target_id 的定向 @ 不该退化成广播")
    }

    // ── 显式 broadcast = 放宽器（#62）─────────────────────────────────────

    /// `[broadcast, session(X)]` = **全组可见 + 只叫醒 X**（#62 的正主）。
    /// 唤醒面照旧只命中 X —— 那条钉在下面的 `CrewBroadcastWakeAndDisambiguationTests`。
    func testBroadcastWidensPastSessionNarrowing() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"), "目标当然看得见")
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-2"),
                      "显式 broadcast 放宽：非目标也看得见（治「容易漏」）")
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    /// 顺序无关：broadcast 写在后面也一样放宽。
    func testBroadcastWidensRegardlessOfMentionOrder() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-2"))
    }

    /// `@captain` 也一样被放宽。
    func testBroadcastWidensPastCaptainNarrowing() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "captain", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    /// **#543 不许回退**：放宽是显式 opt-in。没写 broadcast 就还是老的排他行为 ——
    /// 漏写只会「不够宽」，绝不会「悄悄扩散」。
    func testWithoutExplicitBroadcastSessionStaysExclusive() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-2"),
                       "#62 放宽的是显式写了 broadcast 的那些，别的一个字不松")
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    // ── 注入面消歧（#62 硬要求：治 #543 的病根）────────────────────────────

    /// 真广播没有「派给谁」这回事 —— 不标注（含只 @ 人类 / 只写 broadcast）。
    func testDirectedNoteNilForRealBroadcast() {
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(msg(), to: "w-2"))
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(
            msg(mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)]), to: "w-2"))
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(
            msg(mentions: [LocalWhiteboardMention(kind: "broadcast", targetId: nil)]), to: "w-2"))
    }

    /// 就是点我的 → 不标注（别多此一举）。
    func testDirectedNoteNilForTheActualTarget() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
        ])
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(m, to: "w-1"))
    }

    /// 旁观者 → 标注，且写出目标的**显示名**（花名册解得出时）。
    /// 这就是 #543 病根的解法：看得见，而且一眼看得出不是自己的活。
    func testDirectedNoteNamesTargetForBystander() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
        ])
        XCTAssertEqual(
            CrewWhiteboardVisibility.directedNote(
                m, to: "w-2", displayName: { $0 == "w-1" ? "小王" : nil }),
            "（发给 小王 的）")
    }

    /// 花名册解不出名字 → 退回短 id，宁可标个 id 也不能不标。
    func testDirectedNoteFallsBackToShortSessionId() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "session", targetId: "worker-0c569d1e"),
        ])
        XCTAssertEqual(CrewWhiteboardVisibility.directedNote(m, to: "w-2"),
                       "（发给 session:worker 的）")
    }

    func testDirectedNoteForCaptainMention() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "captain", targetId: nil),
        ])
        XCTAssertEqual(CrewWhiteboardVisibility.directedNote(m, to: "w-2"), "（发给 机长 的）")
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(m, to: "cap", isCaptain: true),
                     "机长看 @机长 的条目不该被标成「发给机长的」")
    }

    /// 多目标 → 全写出来，保序去重。
    func testDirectedNoteListsEveryTarget() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
            LocalWhiteboardMention(kind: "captain", targetId: nil),
        ])
        XCTAssertEqual(
            CrewWhiteboardVisibility.directedNote(
                m, to: "w-2", displayName: { $0 == "w-1" ? "小王" : nil }),
            "（发给 小王、机长 的）")
    }

    /// 收窄型 mention 在场但一个名字都解不出（`@session` 缺 target_id）—— 仍要标：
    /// 读的人得知道「这条不是冲我来的」，只是说不出是冲谁。
    func testDirectedNoteStillMarksUnnameableTarget() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
            LocalWhiteboardMention(kind: "session", targetId: nil),
        ])
        XCTAssertEqual(CrewWhiteboardVisibility.directedNote(m, to: "w-2"), "（不是发给你的）")
    }

    // ── 批量过滤保序 ──────────────────────────────────────────────────────

    func testVisibleKeepsOrderAndDropsOthers() {
        let broadcast = msg()
        let toMe = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")])
        let toOther = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-9")])
        let out = CrewWhiteboardVisibility.visible([broadcast, toOther, toMe], to: "w-1")
        XCTAssertEqual(out.map(\.id), [broadcast.id, toMe.id])
    }
}

#if os(macOS)
/// 「可见」与「该叫醒」是两件事 —— 2026-08-23 放宽 human 的**可见性**时，这两条钉住
/// 唤醒面 / 收听面**一个字没变**。它们打在别的判据上（`CrewWhiteboardVisibility` 不参与），
/// 放这里是因为它们守的是同一条语义线：看得见 ≠ 该被它叫醒。
final class CrewHumanMentionWakeBoundaryTests: XCTestCase {

    private func whiteboardMsg(
        mentions: [LocalWhiteboardMention]?, senderKind: String = "session",
        sessionId: String? = "s-sender"
    ) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: senderKind, senderUserId: nil,
            senderSessionId: sessionId, category: nil, text: "@人 我做完了",
            createdAt: "2027-01-15T00:00:00Z", senderName: "worker", mentions: mentions)
    }

    /// 只 @ 人类 → 不唤醒任何 run（本地直投路）。
    func testHumanOnlyMentionWakesNobody() {
        let runs = [
            CrewLocalMentionInjectLogic.RunState(sessionId: "w-1", isBusy: false),
            CrewLocalMentionInjectLogic.RunState(sessionId: "cap", isBusy: false),
        ]
        let out = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: [CrewMention(kind: "human", targetId: nil)],
            runs: runs, messageText: "@人 我做完了", senderName: "worker",
            captainSessionId: "cap")
        XCTAssertTrue(out.isEmpty, "@人类 只是标记，不该叫醒任何 run（可见 ≠ 该叫醒）")
    }

    /// 只 @ 人类 → 也不该把缺席的 session / 机长拉起来。
    func testHumanOnlyMentionSpawnsNobody() {
        let t = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: [CrewMention(kind: "human", targetId: nil)],
            runningSessionIds: [], captainRunning: false)
        XCTAssertFalse(t.needCaptain)
        XCTAssertTrue(t.sessionIds.isEmpty)
    }

    /// 只 @ 人类 → 不投给收听者。这是**刻意的降噪**（人类没有直投链，去重的理由对它
    /// 不成立）：`@人 汇报…` 不值得把全 crew 正在 listen 的 session 全叫醒一遍。
    /// 谁若本着「human 不再是定向」的精神把 `deliverable` 那一支顺手删了，这条会红。
    func testHumanOnlyMentionNotDeliveredToListener() {
        let l = CrewListenLogic.Listener(
            sessionId: "s-listen", until: Date(timeIntervalSince1970: 1_800_000_600),
            senders: nil)
        let m = whiteboardMsg(mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)])
        XCTAssertFalse(CrewListenLogic.deliverable(m, to: l),
                       "human-only 不投给收听者是刻意降噪，不许被顺手改掉")
        // 对照：纯广播照送，证明上面那条 false 来自 human 那一支而不是别的门。
        XCTAssertTrue(CrewListenLogic.deliverable(whiteboardMsg(mentions: nil), to: l))
    }
}
#endif

#if os(macOS)
/// #62 的另外两半，打在**别的**判据上，所以单独一个类：
///   * **唤醒面一行没改** —— `[broadcast, session(X)]` 放宽了可见性，但只有 X 被叫醒。
///   * **注入面消歧** —— 放宽进来的条目在**真实注入文本**里必须一眼看得出不是自己的活。
///     这条必须打在渲染上（`HookEmitter` 每轮未读注入 + `CrewLocalMentionInjectLogic`
///     组装的「近期群聊」），不是只在 app 界面里 —— #543 的病就在注入面。
final class CrewBroadcastWakeAndDisambiguationTests: XCTestCase {

    private let widenedToW1: [CrewMention] = [.broadcast, .session("w-1")]

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("bc-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func wbMsg(_ mentions: [LocalWhiteboardMention]?) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: "user", senderUserId: "u", senderSessionId: nil,
            category: nil, text: "这件事就这么定了", createdAt: "2027-01-15T00:00:00Z",
            senderName: "人", mentions: mentions)
    }

    // ── 唤醒面：放宽可见性没让任何人多醒一次 ────────────────────────────────

    /// `[broadcast, session(X)]` → 只叫醒 X。`plannedInjections` 的 `switch m.kind`
    /// 照旧只认 session/captain，broadcast 落 `default: break`。
    func testWidenedDirectedMessageStillWakesOnlyTheTarget() {
        let runs = [
            CrewLocalMentionInjectLogic.RunState(sessionId: "w-1", isBusy: false),
            CrewLocalMentionInjectLogic.RunState(sessionId: "w-2", isBusy: false),
            CrewLocalMentionInjectLogic.RunState(sessionId: "cap", isBusy: false, isCaptain: true),
        ]
        let out = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: widenedToW1, runs: runs, messageText: "这件事就这么定了",
            senderName: "人", captainSessionId: "cap")
        XCTAssertEqual(out.map(\.sessionId), ["w-1"],
                       "全组看得见 ≠ 全组被叫醒；broadcast 只放宽可见面")
    }

    /// 同理不该把别的缺席 session / 机长拉起来。
    func testWidenedDirectedMessageSpawnsOnlyTheTarget() {
        let t = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: widenedToW1, runningSessionIds: [], captainRunning: false)
        XCTAssertFalse(t.needCaptain, "显式 broadcast 不该顺手把机长拉起来")
        XCTAssertEqual(t.sessionIds, ["w-1"])
    }

    /// 只写 broadcast（真广播）→ 谁都不叫醒，语义不变。
    func testBroadcastAloneWakesNobody() {
        let runs = [CrewLocalMentionInjectLogic.RunState(sessionId: "w-1", isBusy: false)]
        XCTAssertTrue(CrewLocalMentionInjectLogic.plannedInjections(
            mentions: [.broadcast], runs: runs, messageText: "大家看一下",
            senderName: "人", captainSessionId: "cap").isEmpty)
    }

    // ── 注入面消歧①：每轮未读注入（HookEmitter，#543 事故的原发地）──────────

    /// 同一条 `[broadcast, session(w-1)]`：w-2 的注入文本**带标注**、w-1 的**不带**。
    /// 走真 store + 真游标，渲染的是线上那条路本身。
    func testHookInjectionMarksWidenedMessageForBystanderOnly() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        // 花名册在场 → 标注写显示名而不是裸 id。
        var snapshot = CrewSessionsSnapshot()
        snapshot.updatedAt = "2027-01-15T00:00:00Z"
        snapshot.crews["c"] = [CrewSessionsSnapshot.Entry(
            sessionId: "w-1", name: "小王", role: "worker", brief: "", state: "idle")]
        try? JSONEncoder().encode(snapshot)
            .write(to: dir.appendingPathComponent(CrewSessionsSnapshot.fileName))

        // 一条真广播打底 + 一条放宽了的定向消息 —— 对照就在同一块注入文本里。
        store.appendUserMessage(crewId: "c", text: "都看一下这个决定", senderName: "人")
        store.appendSessionMessage(
            crewId: "c", sessionId: "cap", text: "这件事就这么定了", senderName: "机长",
            mentions: [LocalWhiteboardMention(kind: "broadcast", targetId: nil),
                       LocalWhiteboardMention(kind: "session", targetId: "w-1")],
            senderKind: "captain")

        let forBystander = HookEmitter(
            store: store, crewId: "c", sessionId: "w-2", cursorDir: dir).emitContextAndAdvance()
        let forTarget = HookEmitter(
            store: store, crewId: "c", sessionId: "w-1", cursorDir: dir).emitContextAndAdvance()

        // 钉**整段**而不只是一个子串：这块就是 agent 眼里的原样，多一行少一行都该在这里红。
        // （临时目录没有 local-crews.json → 没有「本 crew 当前名」那行。）
        XCTAssertEqual(
            forBystander,
            "群聊白板·未读：\n- 人: 都看一下这个决定\n- 机长: （发给 小王 的）这件事就这么定了",
            "旁观者的注入面必须一眼看出这不是派给自己的（#543 病根）")
        XCTAssertEqual(
            forTarget,
            "群聊白板·未读：\n- 人: 都看一下这个决定\n- 机长: 这件事就这么定了",
            "目标自己的注入面不该被标注")
    }

    /// 没写 broadcast 的定向消息对旁观者**根本不可见** —— 标注是给「可见但不是我的」
    /// 那一档用的，不是把 #543 藏起来的那道墙换成一句提示。
    func testPlainDirectedMessageStillInvisibleToBystander() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(
            crewId: "c", sessionId: "cap", text: "去把 X 做了", senderName: "机长",
            mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")],
            senderKind: "captain")
        XCTAssertNil(
            HookEmitter(store: store, crewId: "c", sessionId: "w-2", cursorDir: dir)
                .emitContextAndAdvance(),
            "#543 不回退：没显式放宽的定向消息仍然不进旁观者的注入面")
    }

    /// 真广播不带标注 —— 别给每条群消息都糊一个前缀。
    func testPlainBroadcastCarriesNoNote() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "大家看一下")
        let out = HookEmitter(store: store, crewId: "c", sessionId: "w-2", cursorDir: dir)
            .emitContextAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertFalse(out!.contains("发给"))
    }

    // ── 注入面消歧②：被 @ 唤醒时前置的「近期群聊」块 ─────────────────────────

    /// 被叫醒的 w-2 在「近期群聊」里读到那条派给 w-1 的消息 → 带标注。
    func testRecentContextBlockMarksWidenedMessageForBystander() {
        let recent = [wbMsg([LocalWhiteboardMention(kind: "broadcast", targetId: nil),
                             LocalWhiteboardMention(kind: "session", targetId: "w-1")])]
        let text = CrewLocalMentionInjectLogic.renderInjection(
            messageText: "换你接手", senderName: "机长", recent: recent,
            viewer: "w-2", displayName: { $0 == "w-1" ? "小王" : nil })
        XCTAssertTrue(text.contains("- 人: （发给 小王 的）这件事就这么定了"),
                      "近期群聊块也是注入面；实得：\n\(text)")
        // 「有人@你」那一行本来就是定向给 viewer 的，不该被标注。
        XCTAssertTrue(text.contains("有人@你：\n- 机长: 换你接手"))
    }

    /// 目标自己看同一块 → 不带标注。
    func testRecentContextBlockLeavesTargetUnmarked() {
        let recent = [wbMsg([LocalWhiteboardMention(kind: "broadcast", targetId: nil),
                             LocalWhiteboardMention(kind: "session", targetId: "w-1")])]
        let text = CrewLocalMentionInjectLogic.renderInjection(
            messageText: "接着干", senderName: "机长", recent: recent,
            viewer: "w-1", displayName: { $0 == "w-1" ? "小王" : nil })
        XCTAssertFalse(text.contains("发给"), "实得：\n\(text)")
    }

    /// 老调用方不传 viewer → 一个字不变（默认不标注）。
    func testRecentContextBlockUnchangedWithoutViewer() {
        let recent = [wbMsg([LocalWhiteboardMention(kind: "session", targetId: "w-1")])]
        XCTAssertEqual(CrewRecentContextRender.block(recent),
                       "近期群聊：\n- 人: 这件事就这么定了")
    }

    // ── composer 的「全体 + @某人」：整条链走通 ──────────────────────────────

    /// 人类在 composer 里同时选「全体」和「@小王」——`mentionsToSend` 只去重、
    /// **没有互斥**，两个都发出去。过去 `broadcast` 在可见面上等于不存在，于是
    /// 「人点了全体，系统当没看见」；现在它真的放宽。
    func testComposerBroadcastPlusSessionWidensAndStillWakesOnlyTarget() {
        let sent = CrewComposerMentionParser.mentionsToSend([
            CrewStagedMention(token: "@全体", mention: .broadcast),
            CrewStagedMention(token: "@小王", mention: .session("w-1")),
        ])
        XCTAssertEqual(sent, [.broadcast, .session("w-1")], "两个能同时选中，没有互斥")

        let stored = wbMsg(sent.map { LocalWhiteboardMention(kind: $0.kind, targetId: $0.targetId) })
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-2"),
                      "人点了「全体」，就得真的全体可见")
        XCTAssertEqual(
            CrewLocalMentionInjectLogic.plannedInjections(
                mentions: sent,
                runs: [.init(sessionId: "w-1", isBusy: false),
                       .init(sessionId: "w-2", isBusy: false)],
                messageText: "这件事就这么定了", senderName: "人").map(\.sessionId),
            ["w-1"], "只叫醒小王")
    }
}
#endif
