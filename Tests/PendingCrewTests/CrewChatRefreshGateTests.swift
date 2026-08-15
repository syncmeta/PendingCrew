import XCTest
// 源码直接编进 test bundle（见 project.yml），无需 import module。

/// #443 第二道闸：群聊 refresh 在内容真的没变时不许碰 `@State`。
///
/// 这里钉两件事：
/// 1. 闸本身的语义（相等 → nil）。
/// 2. **`CrewWhiteboardEntry` / `CrewMember` 的 Equatable 真的是逐字段内容比较** ——
///    这才是闸能不能生效的前提。哪天有人往模型里塞了个每次重建都不同的字段
///    （时间戳 / 随机 id），闸会静默失效、退回整表重排，这组断言会先红。
final class CrewChatRefreshGateTests: XCTestCase {

    // MARK: - 闸语义

    func testEqualContentYieldsNil() throws {
        let a = try entries(texts: ["一", "二", "三"])
        let b = try entries(texts: ["一", "二", "三"])
        XCTAssertNil(CrewChatRefreshGate.changed(current: a, fresh: b),
                     "同样的内容不该触发赋值 —— 那会让整条消息列表重新测量一遍")
    }

    func testAppendedMessageYieldsNewValue() throws {
        let a = try entries(texts: ["一", "二"])
        let b = try entries(texts: ["一", "二", "三"])
        let changed = CrewChatRefreshGate.changed(current: a, fresh: b)
        XCTAssertEqual(changed?.count, 3, "真的多了一条就必须刷 —— 收敛不能把该刷的刷没了")
    }

    func testEditedTextYieldsNewValue() throws {
        let a = try entries(texts: ["一", "二"])
        let b = try entries(texts: ["一", "改过了"])
        XCTAssertNotNil(CrewChatRefreshGate.changed(current: a, fresh: b))
    }

    func testEmptyToEmptyYieldsNil() throws {
        let a: [CrewWhiteboardEntry] = []
        XCTAssertNil(CrewChatRefreshGate.changed(current: a, fresh: []))
    }

    // MARK: - Equatable 是内容比较（闸生效的前提）

    /// 同一份字节解两遍必须相等 —— 每个 tick 重拉的就是这同一份字节。
    func testDecodingSameBytesTwiceIsEqual() throws {
        let json = try entryJSON(id: "e1", text: "hello")
        let first = try JSONDecoder().decode(CrewWhiteboardEntry.self, from: json)
        let second = try JSONDecoder().decode(CrewWhiteboardEntry.self, from: json)
        XCTAssertEqual(first, second)
    }

    /// 附件（本地图片路径）也在比较范围内 —— LED驱动板那 5 张图正是这条路。
    func testAttachmentDifferenceIsDetected() throws {
        let a = try JSONDecoder().decode(
            CrewWhiteboardEntry.self,
            from: try entryJSON(id: "e1", text: "看图", attachmentURL: "file:///tmp/a.png"))
        let b = try JSONDecoder().decode(
            CrewWhiteboardEntry.self,
            from: try entryJSON(id: "e1", text: "看图", attachmentURL: "file:///tmp/b.png"))
        XCTAssertNotEqual(a, b)
        XCTAssertNotNil(CrewChatRefreshGate.changed(current: [a], fresh: [b]))
    }

    func testMemberListEqualityIsContentBased() throws {
        let a = try members(names: ["机长", "人"])
        let b = try members(names: ["机长", "人"])
        XCTAssertNil(CrewChatRefreshGate.changed(current: a, fresh: b))

        let c = try members(names: ["机长", "人", "修转圈"])
        XCTAssertNotNil(CrewChatRefreshGate.changed(current: a, fresh: c))
    }

    /// `captainBotId` 走同一道闸（`String?`）。
    func testOptionalScalarGate() {
        XCTAssertNil(CrewChatRefreshGate.changed(current: String?.none, fresh: String?.none))
        XCTAssertNotNil(CrewChatRefreshGate.changed(current: String?.none, fresh: "bot-1"))
        XCTAssertNil(CrewChatRefreshGate.changed(current: "bot-1", fresh: "bot-1"))
    }

    // MARK: - fixtures

    private func entryJSON(id: String, text: String, attachmentURL: String? = nil) throws -> Data {
        var d: [String: Any] = [
            "id": id,
            "sender_kind": "user",
            "message_kind": "instruction",
            "summary": text,
            "created_at": "2026-08-11T00:00:00Z",
            "payload": ["text": text]
        ]
        if let attachmentURL {
            d["attachments"] = [[
                "id": "a1", "mime": "image/png", "url": attachmentURL
            ]]
        }
        return try JSONSerialization.data(withJSONObject: d)
    }

    private func entries(texts: [String]) throws -> [CrewWhiteboardEntry] {
        try texts.enumerated().map { idx, text in
            try JSONDecoder().decode(
                CrewWhiteboardEntry.self, from: try entryJSON(id: "e\(idx)", text: text))
        }
    }

    private func members(names: [String]) throws -> [CrewMember] {
        try names.enumerated().map { idx, name in
            let d: [String: Any] = [
                "id": "m\(idx)",
                "member_kind": "code_session",
                "display_name": name,
                "status": "active"
            ]
            return try JSONDecoder().decode(
                CrewMember.self, from: try JSONSerialization.data(withJSONObject: d))
        }
    }
}
