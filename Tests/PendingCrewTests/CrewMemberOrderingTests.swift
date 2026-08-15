import XCTest
// CrewMemberOrdering + CrewModels 直接编进 PendingCrewTests target，无需 import。

/// 成员列表排序（Todo #15）—— 钉住「新建的在最上、机长/人类保持置顶」，
/// 防止以后 UI 重排时悄悄回退。
final class CrewMemberOrderingTests: XCTestCase {
    private func d(_ s: String) -> Date {
        let f = ISO8601DateFormatter()
        return f.date(from: s)!
    }

    func testSessionsSortedNewestFirst() {
        let keys = [
            CrewMemberOrdering.Key(id: "old", isPinned: false, createdAt: d("2026-07-01T10:00:00Z")),
            CrewMemberOrdering.Key(id: "new", isPinned: false, createdAt: d("2026-07-26T10:00:00Z")),
            CrewMemberOrdering.Key(id: "mid", isPinned: false, createdAt: d("2026-07-10T10:00:00Z")),
        ]
        XCTAssertEqual(CrewMemberOrdering.sortedIds(keys), ["new", "mid", "old"])
    }

    func testPinnedRowsStayOnTopInOriginalOrder() {
        let keys = [
            // 机长/人类创建最早 —— 纯倒序会把它们压到最底，这里必须仍在最上。
            CrewMemberOrdering.Key(id: "captain", isPinned: true, createdAt: d("2026-01-01T00:00:00Z")),
            CrewMemberOrdering.Key(id: "human", isPinned: true, createdAt: d("2026-01-02T00:00:00Z")),
            CrewMemberOrdering.Key(id: "s1", isPinned: false, createdAt: d("2026-07-01T00:00:00Z")),
            CrewMemberOrdering.Key(id: "s2", isPinned: false, createdAt: d("2026-07-20T00:00:00Z")),
        ]
        XCTAssertEqual(CrewMemberOrdering.sortedIds(keys), ["captain", "human", "s2", "s1"])
    }

    func testMissingDateSinksToBottomAndTiesAreStable() {
        let keys = [
            CrewMemberOrdering.Key(id: "unknown-b", isPinned: false, createdAt: nil),
            CrewMemberOrdering.Key(id: "unknown-a", isPinned: false, createdAt: nil),
            CrewMemberOrdering.Key(id: "tie-b", isPinned: false, createdAt: d("2026-07-05T00:00:00Z")),
            CrewMemberOrdering.Key(id: "tie-a", isPinned: false, createdAt: d("2026-07-05T00:00:00Z")),
        ]
        XCTAssertEqual(
            CrewMemberOrdering.sortedIds(keys), ["tie-a", "tie-b", "unknown-a", "unknown-b"])
    }

    func testParseDateAcceptsPlainAndFractionalISO8601() {
        XCTAssertNotNil(CrewMemberOrdering.parseDate("2026-07-26T10:00:00Z"))
        XCTAssertNotNil(CrewMemberOrdering.parseDate("2026-07-26T10:00:00.123Z"))
        XCTAssertNil(CrewMemberOrdering.parseDate(nil))
        XCTAssertNil(CrewMemberOrdering.parseDate(""))
        XCTAssertNil(CrewMemberOrdering.parseDate("not a date"))
    }

    func testSortedMembersKeepsCaptainAndHumanOnTop() {
        func member(_ id: String, kind: String, at: String?) -> CrewMember {
            CrewMember(
                id: id, memberKind: kind, userId: nil, botId: kind == "captain" ? "bot-1" : nil,
                codeSessionId: kind == "code_session" ? id : nil, displayName: id, role: nil,
                status: "active", representsCrewId: nil, sessionStatus: nil, createdAt: at)
        }
        let members = [
            member("cap", kind: "captain", at: "2026-01-01T00:00:00Z"),
            member("me", kind: "human", at: nil),
            member("s-old", kind: "code_session", at: "2026-07-01T00:00:00Z"),
            member("s-new", kind: "code_session", at: "2026-07-25T00:00:00Z"),
        ]
        XCTAssertEqual(
            CrewMemberOrdering.sortedMembers(members, captainBotId: "bot-1").map(\.id),
            ["cap", "me", "s-new", "s-old"])
    }
}
