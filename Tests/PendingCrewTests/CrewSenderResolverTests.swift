#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

// ─────────────────────────────────────────────────────────────────────────────
// Task 11 — multi-human sender identity (real user id + strict mine check).
//
// Task 10 landed relay pull carrying the real remote `senderUserId` +
// `senderDisplayName` into local whiteboard entries. Before this fix,
// `CrewSenderResolver` treated ANY entry with a non-nil-but-mismatched-only-
// via-nil-guard `senderUserId` loosely — the old rule was:
//
//   mine = senderDisplayName == nil && (uid != nil ? uid == localUserId : true)
//
// which meant a relayed message whose `senderUserId` happened to equal
// `localUserId` (e.g. this same logged-in human posting from iOS, later
// relayed down to Mac) was *never* mine, because `senderDisplayName` was
// always non-nil on relay rows. That's wrong for multi-device use of the
// same account: it should render as "me" (right-aligned) no matter which
// device it was typed on.
//
// The fix: when `senderUserId != nil`, ignore `senderDisplayName` entirely
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

    // ── 2. 远端他人（senderUserId ≠ localUserId）→ not mine，显示其 displayName ──

    func testRemoteOtherHuman_differentUid_notMine() throws {
        let entry = try makeEntry(
            senderKind: "user",
            senderUserId: "uid-someone-else",
            senderDisplayName: "小明")
        let sender = CrewSenderResolver.resolve(
            entry, members: [], captainBotId: nil, localUserId: "auth-real-id")
        XCTAssertFalse(sender.isMine, "a different human's relayed message must not be mine")
        XCTAssertEqual(sender.displayName, "小明",
            "different remote humans are distinguished by senderDisplayName")
    }

    // ── 3. 远端的我自己（同一账号从 iOS 发言，relay 回流到 Mac）→ mine ──────────
    //     旧规则：senderDisplayName != nil 就永不是我 —— 这条测试锁住新规则:
    //     uid == localUserId 时严格判我，不再被 displayName 兜底否决。

    func testOwnMessageRelayedBackFromAnotherDevice_isMine() throws {
        let entry = try makeEntry(
            senderKind: "user",
            senderUserId: "auth-real-id",         // 同一登录账号
            senderDisplayName: "我 (iOS)")         // relay 落地时带的真实显示名
        let sender = CrewSenderResolver.resolve(
            entry, members: [], captainBotId: nil, localUserId: "auth-real-id")
        XCTAssertTrue(sender.isMine,
            "same account's message relayed back from another device must be mine, regardless of senderDisplayName")
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
