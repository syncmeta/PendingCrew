import XCTest

/// Todo #71：侧栏黄点只表示「人类 Todo 那本里还有没回应的条目」。
/// `attentionReason` 继续持久化供其它提醒渠道使用，但不再控制这颗状态点。
final class CrewHumanTodoAttentionTests: XCTestCase {

    private let idle = [CrewSessionStatusSignal(isAlive: true, isWorking: false,
                                                hasHealthIssue: false)]

    // ── 黄点只有一个来源：给人类的 Todo ───────────────────────────────────

    /// 只有机长 attention、没有人类 Todo → 静止时不画点。
    func testAttentionAloneDoesNotLightYellow() {
        XCTAssertNil(CrewStatusAggregation.dot(
            sessions: idle, attentionReason: "看一眼这个"))
    }

    /// 只有未回应的人类 Todo → 也黄。机长那边一个字没写。
    func testUnansweredHumanTodoAloneLightsYellow() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: idle, attentionReason: nil,
                                      humanTodoUnanswered: 3),
            .yellow)
    }

    /// 机长 attention 是否存在都不影响 Todo 黄点。
    func testClearingCaptainAttentionDoesNotKillTodoDot() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: idle, attentionReason: nil,
                                      humanTodoUnanswered: 1),
            .yellow, "clear_attention 不该把「还有 1 条没人拍板」一起熄掉")
    }

    /// 人类把 Todo 全回应完后，即使还留着 attentionReason，静止 crew 也不画点。
    func testAnsweringAllTodosClearsYellowEvenWithAttentionReason() {
        XCTAssertNil(CrewStatusAggregation.dot(
            sessions: idle, attentionReason: "还有别的事",
            humanTodoUnanswered: 0))
    }

    /// 没有 Todo 且静止 → 完全不画指示。
    func testNoTodoAndIdleMeansNoIndicator() {
        XCTAssertNil(CrewStatusAggregation.dot(
            sessions: idle, attentionReason: nil,
            humanTodoUnanswered: 0))
    }

    /// 红压过黄 —— 优先级没被这一笔动到。
    func testRedStillOutranksTodoYellow() {
        let stuck = [CrewSessionStatusSignal(isAlive: true, isWorking: false,
                                             hasHealthIssue: true)]
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: stuck, attentionReason: nil,
                                      humanTodoUnanswered: 5),
            .red)
    }

    /// 老调用方不传 Todo 数 → 视为没有 Todo，静止不画。
    func testDefaultTodoCountMeansNoIndicator() {
        XCTAssertNil(CrewStatusAggregation.dot(sessions: idle, attentionReason: nil))
    }

    // ── 「不回应也能按灭」：判据在条目上 ───────────────────────────────────

    /// `dismissedAt` 一打就不再算未回应 —— 人看过、决定不办，黄点就该灭。
    func testDismissedItemStopsCountingAsUnanswered() {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("human-todo-attn-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = LocalTodoStore(directory: dir, ledger: .human)
        _ = store.add(crewId: "c", text: "A 还是 B？", bySessionId: "w-1")

        XCTAssertEqual(store.list(crewId: "c").filter(\.isUnanswered).count, 1)
        XCTAssertTrue(store.setDismissed(crewId: "c", number: 1))
        XCTAssertEqual(store.list(crewId: "c").filter(\.isUnanswered).count, 0,
                       "不回应也能按灭")
        // 反悔 → 重新算作未回应。
        XCTAssertTrue(store.setDismissed(crewId: "c", number: 1, dismissed: false))
        XCTAssertEqual(store.list(crewId: "c").filter(\.isUnanswered).count, 1)
    }

    /// 回应过的条目同样不再算未回应（正常那条路）。
    func testRespondedItemStopsCountingAsUnanswered() {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("human-todo-attn-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = LocalTodoStore(directory: dir, ledger: .human)
        _ = store.add(crewId: "c", text: "A 还是 B？", bySessionId: "w-1")
        XCTAssertNotNil(store.respond(crewId: "c", number: 1, sessionId: "human",
                                      senderName: "人", text: "选 A"))
        XCTAssertEqual(store.list(crewId: "c").filter(\.isUnanswered).count, 0)
    }

    /// 完成态是终态；即使旧数据没有回应行，也不能继续点灯。
    func testCompletedItemIsNotUnansweredEvenWithoutResponse() {
        let item = LocalTodoItem(
            id: "done", number: 1, text: "已经办完", status: "completed",
            createdAt: "2026-08-27T00:00:00Z")
        XCTAssertFalse(item.isUnanswered)
    }

    /// 软删墓碑不触发；判据自身也要 fail-safe，不能只依赖 list 过滤。
    func testSoftDeletedItemIsNotUnanswered() {
        let item = LocalTodoItem(
            id: "deleted", number: 2, text: "已删除", status: "pending",
            createdAt: "2026-08-27T00:00:00Z", deletedAt: "2026-08-27T00:01:00Z")
        XCTAssertFalse(item.isUnanswered)
    }

    // ── Todo #73：后代 Todo 向祖先传播 ────────────────────────────────────

    func testMultiLevelDescendantTodoPropagatesToEveryAncestor() {
        let result = CrewHumanTodoAttentionCache.aggregate(
            directCounts: ["root": 1, "leaf": 2],
            parentsByCrew: ["root": [], "middle": ["root"], "leaf": ["middle"]])

        XCTAssertEqual(result["leaf"], .init(ownUnanswered: 2, descendantUnanswered: 0))
        XCTAssertEqual(result["middle"], .init(ownUnanswered: 0, descendantUnanswered: 2))
        XCTAssertEqual(result["root"], .init(ownUnanswered: 1, descendantUnanswered: 2))
    }

    /// DAG 有两条路径抵达同一祖先时，同一后代的条目只算一次。
    func testDiamondHierarchyDoesNotDoubleCountDescendantTodo() {
        let result = CrewHumanTodoAttentionCache.aggregate(
            directCounts: ["leaf": 1],
            parentsByCrew: [
                "root": [], "left": ["root"], "right": ["root"],
                "leaf": ["left", "right"],
            ])

        XCTAssertEqual(result["root"]?.descendantUnanswered, 1)
        XCTAssertEqual(result["left"]?.descendantUnanswered, 1)
        XCTAssertEqual(result["right"]?.descendantUnanswered, 1)
    }

    /// 脏数据成环时有限返回，且不能把来源 crew 自己再算成自己的后代。
    func testCycleFailsSafeWithoutSelfCounting() {
        let result = CrewHumanTodoAttentionCache.aggregate(
            directCounts: ["b": 1],
            parentsByCrew: ["a": ["c"], "b": ["a"], "c": ["b"]])

        XCTAssertEqual(result["b"], .init(ownUnanswered: 1, descendantUnanswered: 0))
        XCTAssertEqual(result["a"]?.descendantUnanswered, 1)
        XCTAssertEqual(result["c"]?.descendantUnanswered, 1)
    }

    /// 父 id 不在本次 crew 列表时停在边界，不造幽灵祖先、不丢本 crew 自身语义。
    func testMissingParentFailsSafeAtKnownCrewBoundary() {
        let result = CrewHumanTodoAttentionCache.aggregate(
            directCounts: ["child": 1],
            parentsByCrew: ["child": ["missing"]])

        XCTAssertEqual(result, [
            "child": .init(ownUnanswered: 1, descendantUnanswered: 0),
        ])
    }

    func testOwnAndDescendantAttentionHaveDistinctAccessibleSemantics() {
        let own = CrewHumanTodoAttention(ownUnanswered: 2, descendantUnanswered: 0)
        let descendant = CrewHumanTodoAttention(ownUnanswered: 0, descendantUnanswered: 3)

        XCTAssertEqual(own.scope, .own)
        XCTAssertEqual(own.accessibilityLabel, "本 crew 有 2 条人类 Todo 等你拍板")
        XCTAssertEqual(descendant.scope, .descendant)
        XCTAssertEqual(descendant.accessibilityLabel, "下属 crew 有 3 条人类 Todo 等你拍板")

        let both = CrewHumanTodoAttention(ownUnanswered: 1, descendantUnanswered: 4)
        XCTAssertEqual(both.scope, .ownAndDescendant)
        XCTAssertEqual(
            both.accessibilityLabel,
            "本 crew 有 1 条人类 Todo 等你拍板；下属 crew 有 4 条人类 Todo 等你拍板")
    }

    /// 父 crew 自己在工作时，后代 Todo 黄仍压过绿，不能伪装成“正在工作”。
    func testDescendantTodoYellowOutranksWorkingGreen() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [CrewSessionStatusSignal(
                    isAlive: true, isWorking: true, hasHealthIssue: false)],
                attention: .init(ownUnanswered: 0, descendantUnanswered: 1)),
            .yellow)
    }

    // ── 快照：不许把 2026-08-17 那个形状再造一遍 ───────────────────────────

    /// 指纹没变 → **一次解码都不做**。这是「没在 body 路径上现读整份 Todo」的
    /// 验收口径：`decodeCount` 涨多少 = 本来要付几份整份解码。
    func testFingerprintGateSkipsDecodeWhenNothingChanged() {
        var fingerprint = FileChangeGate.Fingerprint(modified: 1, size: 10)
        var loads = 0
        let cache = CrewHumanTodoAttentionCache(
            fingerprintOf: { _ in fingerprint },
            loadUnansweredCount: { _ in loads += 1; return 2 })

        XCTAssertEqual(cache.refresh(crewIds: ["a", "b"]), ["a": 2, "b": 2])
        XCTAssertEqual(cache.decodeCount, 2)
        _ = cache.refresh(crewIds: ["a", "b"])
        _ = cache.refresh(crewIds: ["a", "b"])
        XCTAssertEqual(cache.decodeCount, 2, "指纹没变就一个字节都不该读")

        fingerprint = FileChangeGate.Fingerprint(modified: 2, size: 11)
        _ = cache.refresh(crewIds: ["a", "b"])
        XCTAssertEqual(cache.decodeCount, 4, "指纹变了才重新解码")
        XCTAssertEqual(loads, 4)
    }

    /// 文件不存在（这本账还没开张）→ 不读、键缺失（不是「没读」，是确实没有）。
    func testMissingLedgerFileIsNotDecoded() {
        let cache = CrewHumanTodoAttentionCache(
            fingerprintOf: { _ in nil },
            loadUnansweredCount: { _ in XCTFail("文件不存在还去读"); return nil })
        XCTAssertEqual(cache.refresh(crewIds: ["a"]), [:])
        XCTAssertEqual(cache.decodeCount, 0)
    }

    /// 只改父边、不改 Todo 文件时要立刻重算祖先，但不能重新解码账本。
    func testHierarchyChangeReaggregatesWithoutTodoDecode() {
        let cache = CrewHumanTodoAttentionCache(
            fingerprintOf: { _ in FileChangeGate.Fingerprint(modified: 1, size: 10) },
            loadUnansweredCount: { $0 == "child" ? 1 : 0 })

        let first = cache.refresh(
            crewIds: ["old-parent", "new-parent", "child"],
            parentsByCrew: [
                "old-parent": [], "new-parent": [], "child": ["old-parent"],
            ])
        XCTAssertEqual(first["old-parent"]?.descendantUnanswered, 1)
        XCTAssertEqual(first["new-parent"]?.descendantUnanswered, 0)
        XCTAssertEqual(cache.decodeCount, 3)

        let moved = cache.refresh(
            crewIds: ["old-parent", "new-parent", "child"],
            parentsByCrew: [
                "old-parent": [], "new-parent": [], "child": ["new-parent"],
            ])
        XCTAssertEqual(moved["old-parent"]?.descendantUnanswered, 0)
        XCTAssertEqual(moved["new-parent"]?.descendantUnanswered, 1)
        XCTAssertEqual(cache.decodeCount, 3, "只改父边不该重读 Todo 文件")
    }
}
