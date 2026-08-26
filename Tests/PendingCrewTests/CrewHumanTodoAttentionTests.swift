import XCTest

/// 侧栏黄点读**两个源**（Todo #62 ④）：机长 `raise_attention` 的理由，**或**
/// 人类 Todo 那本里还有没回应的条目。
///
/// 为什么不合成一个源：`attentionReason` 是单槽 last-write-wins —— 混进去会互相
/// 冲掉（人类 Todo 覆盖机长的理由；机长 `clear_attention` 顺手把待拍板熄了）。
/// 并联之后两边互不干扰，「人类不回应也能按灭」也自然成立。
final class CrewHumanTodoAttentionTests: XCTestCase {

    private let idle = [CrewSessionStatusSignal(isAlive: true, isWorking: false,
                                                hasHealthIssue: false)]

    // ── 两个源各自能点亮，互不干扰 ─────────────────────────────────────────

    /// 只有机长的 attention → 黄（老行为，一个字没变）。
    func testAttentionAloneStillLightsYellow() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: idle, attentionReason: "看一眼这个"),
            .yellow)
    }

    /// 只有未回应的人类 Todo → 也黄。机长那边一个字没写。
    func testUnansweredHumanTodoAloneLightsYellow() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: idle, attentionReason: nil,
                                      humanTodoUnanswered: 3),
            .yellow)
    }

    /// 机长熄了自己的灯，人类 Todo 那盏**不受影响** —— 这正是不合并成一个源的理由。
    func testClearingCaptainAttentionDoesNotKillTodoDot() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: idle, attentionReason: nil,
                                      humanTodoUnanswered: 1),
            .yellow, "clear_attention 不该把「还有 1 条没人拍板」一起熄掉")
    }

    /// 反过来也一样：人类把 Todo 全回应完了，机长那盏还亮着。
    func testAnsweringAllTodosDoesNotKillCaptainAttention() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: idle, attentionReason: "还有别的事",
                                      humanTodoUnanswered: 0),
            .yellow)
    }

    /// 两个源都空 → 不黄（有 session 记录 → 灰）。
    func testNeitherSourceMeansNoYellow() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: idle, attentionReason: nil,
                                      humanTodoUnanswered: 0),
            .gray)
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

    /// 老调用方不传新形参 → 一个字不变。
    func testLegacyCallSitesUnchanged() {
        XCTAssertEqual(CrewStatusAggregation.dot(sessions: idle, attentionReason: nil), .gray)
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
}
