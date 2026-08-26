#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

// ─────────────────────────────────────────────────────────────────────────────
// Task 11 — sender identity resolves by user id, never by display name.
//
// The rule: when `senderUserId != nil`, ignore `senderDisplayName` entirely
// and go strictly by id equality — but "id equality" must recognize BOTH:
//   - the real logged-in id (`localUserId` param, from `AppModel.currentUserId`)
//   - the macOS BYOK local sentinel (`LocalWhiteboardStore.localUserId`),
//     because `LocalWhiteboardStore.appendUserMessage` always stamps the
//     sentinel on locally-composed rows regardless of login state — it has
//     no way to know the logged-in account id. Without the sentinel
//     fallback, a logged-in Mac user's own freshly-typed messages would
//     start rendering as NOT mine the moment `CrewChatView.localUserId`
//     switches from the sentinel to the real id (#lookalike regression this
//     test guards against).
//
// #63 第二期删掉跨端遥控整层时，本文件里两条只有 relay 才能触发的用例
// （「不同远端人类靠 displayName 区分」「同账号从另一台设备 relay 回来仍是我」）
// 一并删除 —— 本地 `appendUserMessage` 恒写 BYOK 哨兵，产不出那两种行。
// ─────────────────────────────────────────────────────────────────────────────

private func ts() -> String { "2026-01-01T00:00:00Z" }

/// Decode a `CrewWhiteboardEntry` from a minimal JSON dict — same helper
/// pattern as `CrewChatAdapterTests.makeEntry` (kept local/private to avoid
/// cross-file coupling between XCTest files in the same bundle).
private func makeEntry(
    id: String = "e1",
    senderKind: String = "user",
    senderUserId: String? = nil,
    senderDisplayName: String? = nil,
    summary: String = "hello"
) throws -> CrewWhiteboardEntry {
    var d: [String: Any] = [
        "id": id,
        "sender_kind": senderKind,
        "message_kind": "announcement",
        "summary": summary,
        "created_at": ts()
    ]
    if let v = senderUserId { d["sender_user_id"] = v }
    if let v = senderDisplayName { d["sender_display_name"] = v }
    let data = try JSONSerialization.data(withJSONObject: d)
    return try JSONDecoder().decode(CrewWhiteboardEntry.self, from: data)
}

final class CrewSenderResolverTests: XCTestCase {

    // ── 1. 本地我（legacy 消息，senderUserId 字段引入前落盘，解码为 nil）→ mine ──

    func testLegacyLocalMessage_nilSenderUserId_isMine() throws {
        let entry = try makeEntry(senderKind: "user", senderUserId: nil)
        let sender = CrewSenderResolver.resolve(
            entry, members: [], captainBotId: nil, localUserId: "auth-real-id")
        XCTAssertTrue(sender.isMine,
            "legacy local row with no senderUserId must still resolve as mine")
    }

    // ── 1b. 本地我（当前格式：composer 行恒标 BYOK 哨兵），登录态下也仍是我 ────
    //     这是 #445-451 从常量切到 appModel.currentUserId 后必须守住的场景：
    //     LocalWhiteboardStore.appendUserMessage 恒写 sentinel，不随登录态变，
    //     resolver 必须把 sentinel 当"我"来兜，不然登录后自己发的话就变"别人"。

    func testLocalComposerRow_sentinelUid_isMineEvenWhenLoggedIn() throws {
        let entry = try makeEntry(
            senderKind: "user",
            senderUserId: LocalWhiteboardStore.localUserId,   // composer 恒写的哨兵
            senderDisplayName: nil)
        let sender = CrewSenderResolver.resolve(
            entry, members: [], captainBotId: nil, localUserId: "auth-real-id")  // 已登录
        XCTAssertTrue(sender.isMine,
            "sentinel-tagged composer row must resolve as mine even when localUserId is the real logged-in id")
    }

    // ── 4. senderUserId != nil 且不等于任何"我" id，displayName 缺省 → 兜底"人" ──

    func testRemoteOtherHuman_noDisplayName_fallsBackToGenericLabel() throws {
        let entry = try makeEntry(
            senderKind: "user",
            senderUserId: "uid-someone-else",
            senderDisplayName: nil)
        let sender = CrewSenderResolver.resolve(
            entry, members: [], captainBotId: nil, localUserId: "auth-real-id")
        XCTAssertFalse(sender.isMine)
        XCTAssertEqual(sender.displayName, "人")
    }
}
#endif
