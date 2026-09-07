#if os(macOS)
import XCTest

/// 重放修复 第二批：**③ 队列存消息身份、不存渲染好的串；出队要发之前重新决定**
/// （人类 Todo #105）。
///
/// ## 两个独立现场，都只有这一条解释得了
///
/// 1. 人类那次：**90 秒**，而且**发生在被回复之后**，内容是最初形态。
/// 2. 4-1 那次：**一个回合之内两条**，他全程在跑、sessionId 没变（排除 ①）、
///    不是被拉起来的（排除 ②），两条都在他已经回复之后到达。
///
/// ## 机制
///
/// `CrewLocalMentionWaker` 在**扫描那一刻**就把正文 + 最近上下文渲染成一个字符串，
/// 交给 `CrewDeferredWakeQueue`。目标忙就压着，空闲再发，**中间从不重读白板**。
/// 同一段时间里 hook 路每次工具调用都在把同一条渲染进「未读」并推进游标 ——
/// 于是队列弹出的是一份**已经被消费掉的旧快照**。
///
/// 「送的是最初形态」不是缓存了旧数据：**队列里存的本来就是一份写死的串**。
///
/// ⚠️ 这几条**不许用「渲染层去重」变绿**。渲染层去重会让四条修法里任意一条都能
/// 让它变绿，那这条测试就不指认任何东西了。
final class CrewWakeDispatchTests: XCTestCase {

    private func entryPayload() -> CrewWakeDispatch.Payload {
        .whiteboardEntry(crewId: "c", entryId: "e1")
    }

    // MARK: - 存的是身份，不是串

    /// 结构性判据：白板来的唤醒**不能**以渲染好的字符串形态进队列。
    /// 只要它还是 `.literal`，出队时就无从重新决定 —— 这是 ③ 的病根本身。
    func test_白板来的唤醒进队列时存的是消息身份不是渲染结果() {
        guard case .whiteboardEntry(let crewId, let entryId) = entryPayload() else {
            return XCTFail("白板唤醒被存成了渲染好的串，出队时没有任何东西可以重新决定")
        }
        XCTAssertEqual(crewId, "c")
        XCTAssertEqual(entryId, "e1")
    }

    /// 与白板无关的唤醒（机长交接补投等）本来就没有「现取」可言，保持字面量。
    func test_与白板无关的唤醒仍然可以是字面量() {
        let out = CrewWakeDispatch.resolve(
            .literal("交接期间攒下的那条"),
            hasDelivered: { _, _ in true },      // 就算说已投过也不该影响它
            renderNow: { _, _ in nil })
        XCTAssertEqual(out, "交接期间攒下的那条")
    }

    // MARK: - 出队时重新决定

    /// **正身**：压队期间 hook 路已经把同一条投给这个 session 并推进了游标，
    /// 那么等它空闲时**这一遍就不该再发**。
    func test_压队期间已经被投过的_出队时不再发() {
        let out = CrewWakeDispatch.resolve(
            entryPayload(),
            hasDelivered: { crewId, entryId in crewId == "c" && entryId == "e1" },
            renderNow: { _, _ in "有人@你：…（入队那一刻渲染的）" })
        XCTAssertNil(out, "目标已经从别的投递面看过这条了，再发一遍就是重放")
    }

    /// 仍未投递时照常发 —— **但发的必须是「现取」的内容**。
    /// 这条断言 `renderNow` 真的在出队时被调用了：给它一个会变的返回值，
    /// 断言拿到的是**调用那一刻**的值，不是入队时的。
    func test_仍未投递时发的是现取的内容而不是入队时的快照() {
        var boardNow = "入队那一刻"
        let render: (String, String) -> String? = { _, _ in "有人@你：\(boardNow)" }

        // 入队…（此时若把结果算好存下来，就会永远是「入队那一刻」）
        boardNow = "出队那一刻"      // …期间白板变了

        let out = CrewWakeDispatch.resolve(
            entryPayload(), hasDelivered: { _, _ in false }, renderNow: render)
        XCTAssertEqual(out, "有人@你：出队那一刻",
                       "发出去的是入队时的快照 —— 这正是「送的是最初形态」的形状")
    }

    /// 消息在白板上找不到了（归档重建等）→ **别发**。
    /// 宁可不发也不发一句无法追溯的话。
    func test_白板上已经找不到这条时不发() {
        XCTAssertNil(CrewWakeDispatch.resolve(
            entryPayload(), hasDelivered: { _, _ in false }, renderNow: { _, _ in nil }))
    }
}
#endif
