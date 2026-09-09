import XCTest

/// 群聊引用（人类 Todo #132 / #133）的**纯收集层**。
///
/// 判据只有一条：**引用必须来自结构化字段，不来自正文**。
/// 所以这批用例里没有一条给 `build` 喂过正文 —— 喂不进去，是设计。
final class CrewMessageReferenceTests: XCTestCase {

    func test_没有任何结构化字段就一颗胶囊都不长() {
        XCTAssertTrue(CrewMessageReferences.build(.init()).isEmpty)
    }

    /// **正文里写满 `#123` 也长不出引用** —— 这条是这一单的判据本身。
    ///
    /// `build` 的入参里根本没有正文这一项，所以这条用例只能这样写：
    /// 把一堆号码放进**别的**地方，确认出来的引用只有真给了字段的那些。
    func test_引用只从字段来_号码写在别处一律不认() {
        let refs = CrewMessageReferences.build(.init(agentTodoNumber: 7))
        XCTAssertEqual(refs, [CrewMessageReference(.agentTodo, "7")],
                       "多认出来的那颗，必然是从别处猜的：\(refs)")
    }

    func test_四类引用各自认得出来() {
        let refs = CrewMessageReferences.build(.init(
            agentTodoNumber: 2, humanTodoNumber: 3, planNumber: 4,
            inReplyTo: "msg-1", mentionedSessionIds: ["sess-a"], crewNumber: "7-1"))
        XCTAssertEqual(refs, [
            CrewMessageReference(.humanTodo, "3"),
            CrewMessageReference(.agentTodo, "2"),
            CrewMessageReference(.plan, "4"),
            CrewMessageReference(.message, "msg-1"),
            CrewMessageReference(.session, "sess-a"),
            CrewMessageReference(.crew, "7-1"),
        ], "顺序也是判据：胶囊排布不稳定，看起来就像界面在闪")
    }

    /// **两本 Todo 各自从 #1 起，裸 `#N` 有歧义** —— 所以种类必须分开存，
    /// 不能合成一个 `todo`。同一个 3，一本指「请人类拍板的第 3 条」，
    /// 另一本指「人类派下来的第 3 条」。
    func test_两本todo的同一个号是两颗不同的胶囊() {
        let refs = CrewMessageReferences.build(.init(agentTodoNumber: 3, humanTodoNumber: 3))
        XCTAssertEqual(refs.count, 2)
        XCTAssertNotEqual(refs[0], refs[1])
    }

    func test_号码非正数不长胶囊() {
        XCTAssertTrue(CrewMessageReferences.build(
            .init(agentTodoNumber: 0, humanTodoNumber: -1, planNumber: 0)).isEmpty,
            "0 和负数不是「没给」，是给错了 —— 长出一颗点进去什么也没有的胶囊比不长更糟")
    }

    func test_同一个东西被两条路指到只留一颗() {
        let refs = CrewMessageReferences.build(
            .init(mentionedSessionIds: ["sess-a", "sess-a", "sess-b"]))
        XCTAssertEqual(refs, [CrewMessageReference(.session, "sess-a"),
                              CrewMessageReference(.session, "sess-b")])
    }

    func test_空白目标不算给了() {
        XCTAssertTrue(CrewMessageReferences.build(
            .init(inReplyTo: "  ", mentionedSessionIds: [""], crewNumber: "")).isEmpty)
    }

    /// 存的是字符串不是 enum：老数据里冒出没见过的种类时，**解码不许整条炸掉**。
    func test_不认识的种类解得出来只是渲染端不认() throws {
        let json = #"{"kind":"quasar","target_id":"x"}"#.data(using: .utf8)!
        let ref = try JSONDecoder().decode(CrewMessageReference.self, from: json)
        XCTAssertEqual(ref.kind, "quasar")
        XCTAssertNil(ref.resolvedKind, "认不出来就该是 nil，让渲染端不长这颗胶囊")
    }

    /// 落盘键名用 snake_case，跟白板上其它字段（`target_id`）对齐。
    func test_落盘键名是snake_case() throws {
        let data = try JSONEncoder().encode(CrewMessageReference(.humanTodo, "9"))
        let s = String(data: data, encoding: .utf8) ?? ""
        XCTAssertTrue(s.contains("\"target_id\""), s)
        XCTAssertTrue(s.contains("\"human_todo\""), s)
    }
}
