import XCTest


@MainActor
final class CodexTranscriptTests: XCTestCase {
    func testItemCompletedAppendsThenUpsertsById() {
        let t = CodexTranscript()
        t.apply(method: "item/completed", params: ["item": ["id": "i1", "type": "agentMessage", "text": "hi"]])
        XCTAssertEqual(t.items.count, 1)
        t.apply(method: "item/completed", params: ["item": ["id": "i1", "type": "agentMessage", "text": "hi (final)"]])
        XCTAssertEqual(t.items.count, 1)   // upsert, not duplicate
        guard case let .agentMessage(text, _) = t.items[0].kind else { return XCTFail() }
        XCTAssertEqual(text, "hi (final)")
    }
    func testTwoDistinctItemsBothAppend() {
        let t = CodexTranscript()
        t.apply(method: "item/completed", params: ["item": ["id": "a", "type": "agentMessage", "text": "1"]])
        t.apply(method: "item/completed", params: ["item": ["id": "b", "type": "agentMessage", "text": "2"]])
        XCTAssertEqual(t.items.count, 2)
    }
    func testTurnLifecycleTogglesActive() {
        let t = CodexTranscript()
        t.apply(method: "turn/started", params: ["turn": ["id": "t1"]])
        XCTAssertTrue(t.turnActive); XCTAssertEqual(t.activeTurnId, "t1")
        t.apply(method: "turn/completed", params: ["turn": ["id": "t1"], "status": "completed"])
        XCTAssertFalse(t.turnActive); XCTAssertNil(t.activeTurnId)
    }
    func testIgnoredMethodsAreNoops() {
        let t = CodexTranscript()
        t.apply(method: "item/started", params: ["item": ["id": "x", "type": "agentMessage"]])
        t.apply(method: "item/agentMessage/delta", params: [:])
        t.apply(method: "thread/tokenUsage/updated", params: [:])
        XCTAssertEqual(t.items.count, 0)
        XCTAssertFalse(t.turnActive)
    }
    func testMalformedItemIsSkippedNotCrash() {
        let t = CodexTranscript()
        t.apply(method: "item/completed", params: [:])              // no "item"
        t.apply(method: "item/completed", params: ["item": "nope"]) // wrong type
        XCTAssertEqual(t.items.count, 0)
    }

    // ── #4: codex 思考过程流式渲染 ──────────────────────────────────────────
    // 真实 reasoning item 在 ChatGPT 鉴权下 item/completed 带 summary:[](空)+
    // encrypted_content;可读思考只在 item/reasoning/summaryTextDelta 流里。

    func testReasoningSummaryDeltasAccumulateAndSurviveEmptyCompleted() {
        let t = CodexTranscript()
        t.apply(method: "item/reasoning/summaryTextDelta",
                params: ["itemId": "r1", "delta": "先看 ", "summaryIndex": 0])
        t.apply(method: "item/reasoning/summaryTextDelta",
                params: ["itemId": "r1", "delta": "仓库结构", "summaryIndex": 0])
        XCTAssertEqual(t.items.count, 1, "reasoning 行随首个 delta 出现并增长")
        guard case let .reasoning(s1, _) = t.items[0].kind else { return XCTFail() }
        XCTAssertEqual(s1, "先看 仓库结构")
        // item/completed 的 reasoning summary 是空数组 → 必须保留已累积的流式思考。
        t.apply(method: "item/completed",
                params: ["item": ["id": "r1", "type": "reasoning", "summary": [], "encrypted_content": "zzz"]])
        XCTAssertEqual(t.items.count, 1, "按 id upsert,不重复行")
        guard case let .reasoning(s2, _) = t.items[0].kind else { return XCTFail() }
        XCTAssertEqual(s2, "先看 仓库结构", "空 completed 不能抹掉流式累积的思考(#4)")
    }

    func testReasoningCompletedWithRealSummaryTakesPrecedence() {
        let t = CodexTranscript()
        t.apply(method: "item/reasoning/summaryTextDelta",
                params: ["itemId": "r1", "delta": "partial", "summaryIndex": 0])
        // 若 completed 自带非空 summary（如 PONG 那种简单回合）→ 以它为准。
        t.apply(method: "item/completed",
                params: ["item": ["id": "r1", "type": "reasoning", "summary": ["最终思考"], "content": []]])
        guard case let .reasoning(s, _) = t.items[0].kind else { return XCTFail() }
        XCTAssertEqual(s, "最终思考")
    }
}
