#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

/// 人类 Todo #132 / #133：群消息下面那一排**可点的引用胶囊**。
///
/// 这一层只回答一个问题：**这颗胶囊现在点得动吗**。
///
/// 立这些用例的理由是那条硬口径 —— 「点了没反应的胶囊，比不长这颗胶囊更糟」。
/// 一颗跳不到任何地方的胶囊不报错、不变灰，它就是一颗看起来正常的死件；人点了
/// 以为是自己没点准，再点一次。所以「目标不可达 → 一颗都不长」必须是**被断言的
/// 行为**，不能只是实现里顺手写的一个 guard。
final class CrewMessageReferencePillsTests: XCTestCase {

    // MARK: - Fixtures

    private func ref(_ kind: CrewMessageReference.Kind, _ target: String) -> CrewMessageReference {
        CrewMessageReference(kind, target)
    }

    /// 什么都可达的上下文（用来把「不长」的原因锁死在被测的那一个条件上）。
    private var openContext: CrewMessageReferencePills.Context {
        .init(
            selfMessageId: "self",
            loadedMessageIds: ["m1", "m2", "self"],
            openableSessions: ["s1": "小绿"],
            crewTargets: [
                "7": .init(crewId: "crew-7", title: "总机长功能", sessionId: nil),
                "7-1": .init(crewId: "crew-7", title: "总机长功能 · 机长", sessionId: "cap-7"),
            ])
    }

    // MARK: - 没有引用就一个字都不变

    func test_没有引用的老消息一颗胶囊都不长() {
        XCTAssertTrue(CrewMessageReferencePills.pills(nil, in: openContext).isEmpty)
        XCTAssertTrue(CrewMessageReferencePills.pills([], in: openContext).isEmpty)
    }

    // MARK: - 两本账分开

    func test_两本Todo的同一个号是两颗不同的胶囊() {
        let pills = CrewMessageReferencePills.pills(
            [ref(.humanTodo, "12"), ref(.agentTodo, "12")], in: openContext)
        XCTAssertEqual(pills.count, 2)
        XCTAssertEqual(pills[0].action, .todo(ledger: .human, number: 12))
        XCTAssertEqual(pills[1].action, .todo(ledger: .agent, number: 12))
        // 标签也必须分得开 —— 两颗都写「Todo #12」时人没法判断点哪颗。
        XCTAssertNotEqual(pills[0].label, pills[1].label)
        XCTAssertTrue(pills[0].label.contains("人类"))
    }

    func test_计划的号解析成计划胶囊() {
        let pills = CrewMessageReferencePills.pills([ref(.plan, "3")], in: openContext)
        XCTAssertEqual(pills.map(\.action), [.plan(number: 3)])
        XCTAssertEqual(pills.first?.label, "计划 #3")
    }

    func test_不是正整数的号一律不长() {
        // `+5` / 全角 `５` / 前后带空白的这几条是**故意留的**：`Int(_:)` 自己就
        // 认它们（或认一半），只有那道「必须全是 ASCII 数字」的字符判定拦得住。
        // 少了它们，那道判定就是一条**永远不触发的检查** —— 删掉它测试照样全绿。
        for bad in ["0", "-1", "abc", "", " ", "1.5", "12x", "+5", "５", " 5", "5 "] {
            XCTAssertTrue(
                CrewMessageReferencePills.pills([ref(.humanTodo, bad)], in: openContext).isEmpty,
                "「\(bad)」不该长出胶囊")
            XCTAssertTrue(
                CrewMessageReferencePills.pills([ref(.plan, bad)], in: openContext).isEmpty,
                "「\(bad)」不该长出胶囊")
        }
    }

    // MARK: - 消息：滚不到就不长

    func test_指向已加载的消息才长胶囊() {
        let pills = CrewMessageReferencePills.pills([ref(.message, "m1")], in: openContext)
        XCTAssertEqual(pills.map(\.action), [.message(id: "m1")])
    }

    func test_指向没加载进来的消息不长胶囊() {
        // 这条正是「死件」的典型：id 是真的，但这个群此刻的时间线里没有它，
        // 点下去滚不到任何地方。
        XCTAssertTrue(
            CrewMessageReferencePills.pills([ref(.message, "m404")], in: openContext).isEmpty)
    }

    func test_指向自己那条消息不长胶囊() {
        XCTAssertTrue(
            CrewMessageReferencePills.pills([ref(.message, "self")], in: openContext).isEmpty)
    }

    // MARK: - session：这台机器上没这个 session 就不长

    func test_能打开的session才长胶囊并用它的名字() {
        let pills = CrewMessageReferencePills.pills([ref(.session, "s1")], in: openContext)
        XCTAssertEqual(pills.map(\.action), [.session(sessionId: "s1")])
        XCTAssertEqual(pills.first?.label, "小绿")
    }

    func test_打不开的session不长胶囊() {
        XCTAssertTrue(
            CrewMessageReferencePills.pills([ref(.session, "s404")], in: openContext).isEmpty)
    }

    // MARK: - 机组号码：解析不出就不长

    func test_机组号码解析得出才长胶囊() {
        let pills = CrewMessageReferencePills.pills(
            [ref(.crew, "7"), ref(.crew, "7-1")], in: openContext)
        XCTAssertEqual(pills.map(\.action), [
            .crew(crewId: "crew-7", sessionId: nil),
            .crew(crewId: "crew-7", sessionId: "cap-7"),
        ])
        XCTAssertEqual(pills.map(\.label), ["7 · 总机长功能", "7-1 · 总机长功能 · 机长"])
    }

    func test_查无此号不长胶囊() {
        XCTAssertTrue(
            CrewMessageReferencePills.pills([ref(.crew, "99")], in: openContext).isEmpty)
    }

    // MARK: - 认不出的种类

    func test_不认识的种类只丢那一颗别的照长() {
        // 老数据里冒出新种类时，解码那层已经保住了整条消息（存的是字符串不是 enum）；
        // 这一层要保住的是**同一条消息上其它胶囊**别被一起拖掉。
        let json = #"{"kind":"wormhole","target_id":"x"}"#.data(using: .utf8)!
        let unknown = try! JSONDecoder().decode(CrewMessageReference.self, from: json)
        let pills = CrewMessageReferencePills.pills(
            [unknown, ref(.plan, "3")], in: openContext)
        XCTAssertEqual(pills.map(\.action), [.plan(number: 3)])
    }

    // MARK: - 顺序与去重

    func test_顺序照引用给的顺序重复的只留一颗() {
        let pills = CrewMessageReferencePills.pills(
            [ref(.plan, "3"), ref(.message, "m1"), ref(.plan, "3")], in: openContext)
        XCTAssertEqual(pills.map(\.action), [.plan(number: 3), .message(id: "m1")])
    }
}
#endif
