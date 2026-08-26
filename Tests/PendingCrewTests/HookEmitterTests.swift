import XCTest
// HookEmitter.swift + LocalWhiteboardStore.swift 编进 test bundle（见 project.yml）。

final class HookEmitterTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("hook-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }
    private func emitter(_ store: LocalWhiteboardStore, _ dir: URL, session: String = "s") -> HookEmitter {
        HookEmitter(store: store, crewId: "c", sessionId: session, cursorDir: dir)
    }

    func testEmitsUnreadAsAdditionalContext() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "@session 去做 X")
        let out = emitter(store, dir).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("additionalContext"))
        XCTAssertTrue(out!.contains("去做 X"))
        // #484 微信式精简：注入面只留短标头，trust 教学在 world-model 系统提示。
        XCTAssertTrue(out!.contains("群聊白板·未读"), "短标头点明这是群聊未读注入")
        XCTAssertFalse(out!.contains("prompt injection"), "长免责说明不再进注入面(#484)")
    }

    func testSecondCallAfterReadReturnsNil() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "msg")
        XCTAssertNotNil(emitter(store, dir).emitAndAdvance())
        // 游标已推进，已读完 → nil（不重复注入）。
        XCTAssertNil(emitter(store, dir).emitAndAdvance())
    }

    func testOnlyNewMessagesAfterCursor() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "first")
        _ = emitter(store, dir).emitAndAdvance()
        store.appendUserMessage(crewId: "c", text: "second")
        let out = emitter(store, dir).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("second"))
        XCTAssertFalse(out!.contains("first"), "first 已读，不再注入")
    }

    func testEmptyWhiteboardReturnsNil() {
        let dir = tempDir()
        XCTAssertNil(emitter(LocalWhiteboardStore(directory: dir), dir).emitAndAdvance())
    }

    // MARK: - 机长自知力纯判定

    func testNamingReminderOnlyForMaturePlaceholder() {
        XCTAssertFalse(CaptainAwarenessLogic.shouldRemindToRename(
            titleSource: .placeholder,
            whiteboardMessageCount: CaptainAwarenessLogic.namingMessageThreshold - 1))
        XCTAssertTrue(CaptainAwarenessLogic.shouldRemindToRename(
            titleSource: .placeholder,
            whiteboardMessageCount: CaptainAwarenessLogic.namingMessageThreshold))
        XCTAssertFalse(CaptainAwarenessLogic.shouldRemindToRename(
            titleSource: .human, whiteboardMessageCount: 99))
        XCTAssertFalse(CaptainAwarenessLogic.shouldRemindToRename(
            titleSource: .captain, whiteboardMessageCount: 99))
    }

    func testSplitThresholdsUseEitherHardSignalAndRenderActualCounts() {
        XCTAssertNil(CaptainAwarenessLogic.splitSignal(
            activeSessionCount: CaptainAwarenessLogic.parallelSessionThreshold - 1,
            recentMessageCount: CaptainAwarenessLogic.densityMessageThreshold - 1))
        let sessions = CaptainAwarenessLogic.splitSignal(
            activeSessionCount: CaptainAwarenessLogic.parallelSessionThreshold,
            recentMessageCount: 1)
        XCTAssertNotNil(sessions)
        XCTAssertTrue(CaptainAwarenessLogic.renderSplitHint(sessions!).contains("4 个"))
        XCTAssertTrue(CaptainAwarenessLogic.renderSplitHint(sessions!).contains("可以考虑"))
        let density = CaptainAwarenessLogic.splitSignal(
            activeSessionCount: 1,
            recentMessageCount: CaptainAwarenessLogic.densityMessageThreshold)
        XCTAssertTrue(CaptainAwarenessLogic.renderSplitHint(density!).contains("12 条"))
    }

    func testRecentMessageDensityUsesFifteenMinuteWindow() {
        let now = Date(timeIntervalSince1970: 10_000)
        let timestamps = [now, now.addingTimeInterval(-899), now.addingTimeInterval(-901)]
        XCTAssertEqual(CaptainAwarenessLogic.recentMessageCount(timestamps: timestamps, now: now), 2)
    }

    func testSplitHintCooldownAndDuplicateSuppression() {
        let now = Date(timeIntervalSince1970: 10_000)
        let signal = CaptainAwarenessLogic.SplitSignal(activeSessionCount: 4, recentMessageCount: 3)
        XCTAssertTrue(CaptainAwarenessLogic.shouldEmitSplitHint(
            signal: signal, previousSignature: nil, previousDate: nil, now: now))
        XCTAssertFalse(CaptainAwarenessLogic.shouldEmitSplitHint(
            signal: signal, previousSignature: signal.signature,
            previousDate: now.addingTimeInterval(-CaptainAwarenessLogic.splitCooldown), now: now),
            "相同快照即使过基础冷却也不应重复")
        XCTAssertTrue(CaptainAwarenessLogic.shouldEmitSplitHint(
            signal: .init(activeSessionCount: 5, recentMessageCount: 3),
            previousSignature: signal.signature,
            previousDate: now.addingTimeInterval(-CaptainAwarenessLogic.splitCooldown), now: now),
            "压力数字变化且过基础冷却后可以重提")
        XCTAssertTrue(CaptainAwarenessLogic.shouldEmitSplitHint(
            signal: signal, previousSignature: signal.signature,
            previousDate: now.addingTimeInterval(-CaptainAwarenessLogic.identicalSplitReminderInterval),
            now: now))
    }

    func testPerSessionCursorsIndependent() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "广播给全员")
        _ = emitter(store, dir, session: "s1").emitAndAdvance()   // s1 读了
        let out = emitter(store, dir, session: "s2").emitAndAdvance()  // s2 仍未读
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("广播给全员"))
    }

    // MARK: - 定向 @ 不扩散到非目标（#543 根因回归）

    /// 事故复现：机长 `post_to_crew(mentions: [session:worker-1])` 派一件活 ——
    /// 之前非目标 worker 的 hook 注入里照样出现这条、且与广播不可区分，于是全 crew
    /// 都当成自己的活认领。现在非目标注入面看不到它。
    func testDirectedMentionNotInjectedToNonTarget() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(
            crewId: "c", sessionId: "cap-1", text: "去把 X 修了", senderName: "机长",
            mentions: [LocalWhiteboardMention(kind: "session", targetId: "worker-1")],
            senderKind: "captain")
        XCTAssertNil(emitter(store, dir, session: "worker-2").emitAndAdvance(),
                     "定向派给 worker-1 的活不该进 worker-2 的注入面")
        let target = emitter(store, dir, session: "worker-1").emitAndAdvance()
        XCTAssertNotNil(target)
        XCTAssertTrue(target!.contains("去把 X 修了"), "目标照常收到")
    }

    /// 被滤掉的条目游标照常推进：对本 session 就是「已阅」，不该攒着下轮再来。
    func testFilteredOutEntryStillAdvancesCursor() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(
            crewId: "c", sessionId: "cap-1", text: "定向活", senderName: "机长",
            mentions: [LocalWhiteboardMention(kind: "session", targetId: "worker-1")],
            senderKind: "captain")
        XCTAssertNil(emitter(store, dir, session: "worker-2").emitAndAdvance())
        store.appendUserMessage(crewId: "c", text: "后来的广播")
        let out = emitter(store, dir, session: "worker-2").emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("后来的广播"))
        XCTAssertFalse(out!.contains("定向活"), "已消费的定向条目不会在下一轮补回来")
    }

    /// 广播（无 mentions）注入面不变 —— 修定向扩散不能顺手收窄广播。
    func testBroadcastStillInjectedToEveryone() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(crewId: "c", sessionId: "cap-1", text: "全员周知",
                                   senderName: "机长", senderKind: "captain")
        for s in ["worker-1", "worker-2", "worker-3"] {
            let out = emitter(store, dir, session: s).emitAndAdvance()
            XCTAssertNotNil(out, "\(s) 应收到广播")
            XCTAssertTrue(out!.contains("全员周知"))
        }
    }

    func testCaptainMentionOnlyInjectedToCaptain() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(
            crewId: "c", sessionId: "w-9", text: "机长看下这个", senderName: "worker",
            mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        XCTAssertNil(emitter(store, dir, session: "worker-2").emitAndAdvance())
        let cap = HookEmitter(store: store, crewId: "c", sessionId: "cap-1",
                              cursorDir: dir, isCaptain: true).emitAndAdvance()
        XCTAssertNotNil(cap)
        XCTAssertTrue(cap!.contains("机长看下这个"))
    }

    func testSenderStillSeesOwnDirectedMessage() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(
            crewId: "c", sessionId: "cap-1", text: "派给你", senderName: "机长",
            mentions: [LocalWhiteboardMention(kind: "session", targetId: "worker-1")],
            senderKind: "captain")
        let own = emitter(store, dir, session: "cap-1").emitAndAdvance()
        XCTAssertNotNil(own, "自己发的定向 @ 在自己上下文里不该凭空消失")
        XCTAssertTrue(own!.contains("派给你"))
    }

    // MARK: - 机长视野：全机 crew 组织树概览（#24）

    /// base/local-crews.json + base/whiteboard/ 的目录布局（与 Application Support
    /// 真实布局一致：白板目录的父级放 crew 文件）。
    @MainActor
    private func makeOrgFixture() -> (wbDir: URL, currentCrewId: String) {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("hooktree-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        let crewStore = LocalCrewStore(baseDirectory: base)
        let req = { (t: String) in CreateCrewRequest.make(
            responsibleSubjectId: "s", title: t, machineId: nil, workingDirectory: "/tmp/x",
            captainAgentKind: "codex", captain: .systemGenerated(templateName: nil)) }
        let hq = crewStore.createCrew(req("总部")).crewId
        let mine = crewStore.createCrew(req("鉴权重构")).crewId
        try? crewStore.adopt(crewId: mine, underParent: hq)
        let wbDir = base.appendingPathComponent("whiteboard", isDirectory: true)
        try? FileManager.default.createDirectory(at: wbDir, withIntermediateDirectories: true)
        return (wbDir, mine)
    }

    @MainActor
    func testCaptainGetsOrgTreeOverview() {
        let (wbDir, mine) = makeOrgFixture()
        let store = LocalWhiteboardStore(directory: wbDir)
        store.appendUserMessage(crewId: mine, text: "触发注入")
        let out = HookEmitter(store: store, crewId: mine, sessionId: "cap",
                              cursorDir: wbDir, isCaptain: true).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("crew 组织树"), "机长注入应带全机组织树概览")
        XCTAssertTrue(out!.contains("总部"))
        XCTAssertTrue(out!.contains("（本 crew）"), "应标注本 crew")
        XCTAssertTrue(out!.contains("adopt_crew"), "概览应带架构调整工具提示")
    }

    @MainActor
    func testWorkerGetsNoOrgTree() {
        let (wbDir, mine) = makeOrgFixture()
        let store = LocalWhiteboardStore(directory: wbDir)
        store.appendUserMessage(crewId: mine, text: "触发注入")
        let out = HookEmitter(store: store, crewId: mine, sessionId: "w",
                              cursorDir: wbDir).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertFalse(out!.contains("crew 组织树"), "worker 注入不带组织树,保持精简")
    }

    @MainActor
    func testPlaceholderRenameTodoIsImmediatelyAfterCurrentTitle() {
        let base = tempDir()
        let crewStore = LocalCrewStore(baseDirectory: base)
        let request = CreateCrewRequest.make(
            responsibleSubjectId: "s", title: "Lisbon", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "codex",
            initialTitleSource: .placeholder,
            captain: .systemGenerated(templateName: nil))
        let id = crewStore.createCrew(request).crewId
        let wbDir = base.appendingPathComponent("whiteboards", isDirectory: true)
        let store = LocalWhiteboardStore(directory: wbDir)
        for i in 1...CaptainAwarenessLogic.namingMessageThreshold {
            store.appendUserMessage(crewId: id, text: "消息 \(i)")
        }
        let out = HookEmitter(store: store, crewId: id, sessionId: "cap",
                              cursorDir: wbDir, isCaptain: true).emitAndAdvance()!
        let lines = out.components(separatedBy: "\\n")
        let titleIndex = lines.firstIndex { $0.contains("本 crew 当前名") }!
        XCTAssertTrue(lines[titleIndex + 1].contains("命名待办"))
        XCTAssertTrue(lines[titleIndex + 1].contains("rename_crew"))
        XCTAssertTrue(lines[titleIndex + 1].contains("4 条"))
    }

    @MainActor
    func testHumanTitleNeverGetsRenameTodo() {
        let base = tempDir()
        let crewStore = LocalCrewStore(baseDirectory: base)
        let request = CreateCrewRequest.make(
            responsibleSubjectId: "s", title: "人类定名", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "codex",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))
        let id = crewStore.createCrew(request).crewId
        let wbDir = base.appendingPathComponent("whiteboards", isDirectory: true)
        let store = LocalWhiteboardStore(directory: wbDir)
        for i in 1...20 { store.appendUserMessage(crewId: id, text: "消息 \(i)") }
        let out = HookEmitter(store: store, crewId: id, sessionId: "cap",
                              cursorDir: wbDir, isCaptain: true).emitAndAdvance()!
        XCTAssertFalse(out.contains("命名待办"))
    }

    @MainActor
    func testOrgTreeMarksPlaceholderChild() throws {
        let base = tempDir()
        let crewStore = LocalCrewStore(baseDirectory: base)
        func request(_ title: String, _ source: LocalCrewTitleSource) -> CreateCrewRequest {
            .make(responsibleSubjectId: "s", title: title, machineId: nil,
                  workingDirectory: "/tmp/x", captainAgentKind: "codex",
                  initialTitleSource: source,
                  captain: .systemGenerated(templateName: nil))
        }
        let parent = crewStore.createCrew(request("总部", .human)).crewId
        let child = crewStore.createCrew(request("Lisbon", .placeholder)).crewId
        try crewStore.adopt(crewId: child, underParent: parent)
        let wbDir = base.appendingPathComponent("whiteboards", isDirectory: true)
        let store = LocalWhiteboardStore(directory: wbDir)
        store.appendUserMessage(crewId: parent, text: "触发")
        let out = HookEmitter(store: store, crewId: parent, sessionId: "cap",
                              cursorDir: wbDir, isCaptain: true).emitAndAdvance()!
        XCTAssertTrue(out.contains("Lisbon〔占位名·待子机长改名〕"))
        XCTAssertFalse(out.contains("总部〔占位名"))
    }

    @MainActor
    func testDensitySplitHintRendersOnceThenCoolsDown() {
        let base = tempDir()
        let crewStore = LocalCrewStore(baseDirectory: base)
        let request = CreateCrewRequest.make(
            responsibleSubjectId: "s", title: "主群", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "codex",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))
        let id = crewStore.createCrew(request).crewId
        let wbDir = base.appendingPathComponent("whiteboards", isDirectory: true)
        let store = LocalWhiteboardStore(directory: wbDir)
        for i in 1...CaptainAwarenessLogic.densityMessageThreshold {
            store.appendUserMessage(crewId: id, text: "密集 \(i)")
        }
        let first = HookEmitter(store: store, crewId: id, sessionId: "cap",
                                cursorDir: wbDir, isCaptain: true).emitAndAdvance()!
        XCTAssertTrue(first.contains("最近 15 分钟白板 12 条"))
        store.appendUserMessage(crewId: id, text: "冷却内的新消息")
        let second = HookEmitter(store: store, crewId: id, sessionId: "cap",
                                 cursorDir: wbDir, isCaptain: true).emitAndAdvance()!
        XCTAssertFalse(second.contains("拆组信号"))
    }

    @MainActor
    func testParallelSessionSplitHintReadsLiveSnapshotAndRendersActualCount() throws {
        let base = tempDir()
        let crewStore = LocalCrewStore(baseDirectory: base)
        let request = CreateCrewRequest.make(
            responsibleSubjectId: "s", title: "并行开发", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "codex",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))
        let id = crewStore.createCrew(request).crewId
        let wbDir = base.appendingPathComponent("whiteboards", isDirectory: true)
        let store = LocalWhiteboardStore(directory: wbDir)
        store.appendUserMessage(crewId: id, text: "触发")
        // 时间只从这一处进：写进快照的 `updatedAt` 与传给 emit 的 `now` 是**同一个
        // 时刻**（先格式化再解回来，消掉秒以下截断），所以 `activeSessionCount` 里那道
        // 15 秒新鲜度闸算出的差恒为 0，与本机跑多少用例、跑多慢完全无关。
        // 曾经这里取 `Date()` 后还要经历编码/写盘/构造/emit，一旦这段墙上时间超过
        // 15 秒（跑全量时真的发生过）count 就归 0，提示整句消失 → 飘红。
        let stamp = ISO8601DateFormatter().string(from: Date())
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: stamp))
        var snapshot = CrewSessionsSnapshot()
        snapshot.updatedAt = stamp
        snapshot.crews[id] = [
            .init(sessionId: "cap", name: "机长", role: "captain", brief: "", state: "idle"),
            .init(sessionId: "w1", name: "一号", role: "worker", brief: "A", state: "working"),
            .init(sessionId: "w2", name: "二号", role: "worker", brief: "B", state: "awaitingDecision"),
            .init(sessionId: "w3", name: "三号", role: "worker", brief: "C", state: "rateLimited"),
            .init(sessionId: "old", name: "旧", role: "worker", brief: "D", state: "exited"),
        ]
        try JSONEncoder().encode(snapshot)
            .write(to: wbDir.appendingPathComponent(CrewSessionsSnapshot.fileName))
        let out = try XCTUnwrap(HookEmitter(store: store, crewId: id, sessionId: "cap",
                                            cursorDir: wbDir, isCaptain: true)
            .emitAndAdvance(now: now))
        XCTAssertTrue(out.contains("当前活跃 session 4 个"))
        XCTAssertFalse(out.contains("5 个"), "已退出 session 不算当前并行")
    }

    // MARK: - 发送者名（Phase 3：白板看得见是谁发的）

    func testSessionMessageWithNameRendersName() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(crewId: "c", sessionId: "sess-abcdef12",
                                   text: "我开始干活了", senderName: "小绿")
        let out = emitter(store, dir).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("小绿"), "有 senderName 时应渲染名字")
        XCTAssertFalse(out!.contains("session:sess-abcdef12"), "有名字时不再裸 uuid")
    }

    func testSessionMessageWithoutNameFallsBackToSessionId() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendSessionMessage(crewId: "c", sessionId: "sess-9", text: "无名发言")
        let out = emitter(store, dir).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("session:sess-9"), "无 senderName 退回旧格式 session:<id>")
    }

    func testUserMessageWithNameRendersName() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "人类留言", senderName: "阿强")
        let out = emitter(store, dir).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("阿强"))
    }

    func testUserMessageWithoutNameFallsBackToHuman() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "人类留言")
        let out = emitter(store, dir).emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("人类"), "无 senderName 退回旧格式「人类」")
    }
}
