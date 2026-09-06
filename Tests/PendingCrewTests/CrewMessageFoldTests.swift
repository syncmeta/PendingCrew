import XCTest

/// 群聊消息折叠的判定（人类 Todo #104）。
///
/// 人类的原话是「**消息太多太乱了**」——**乱不是长**。所以这里量的是：
/// 该收起的收得起来，而**收起态那一行必须是作者自己写的结论**，不是截断出来的开头。
///
/// 三条约束逐条有用例钉着（来自 4-1）：默认收起 / 摘要不是截断前 N 字 / 注入面不折。
final class CrewMessageFoldTests: XCTestCase {

    private func lines(_ n: Int, _ body: String = "细节") -> String {
        (1...n).map { "\(body)\($0)" }.joined(separator: "\n")
    }

    // MARK: - 该折的

    func test_超过阈值且有粗体结论_折起来并拿粗体当摘要() {
        let text = "**闸门全绿，包可以发了。**\n" + lines(20)
        let folded = CrewMessageFold.fold(text)
        XCTAssertNotNil(folded, "21 行、有粗体结论，应该折")
        XCTAssertEqual(folded?.summary, "闸门全绿，包可以发了。")
        XCTAssertEqual(folded?.lineCount, 21, "行数要留在收起条上——墙折起来仍看得出是墙")
    }

    /// 粗体常常不在第一段（先「收到，我看看」再给结论）。实测最近 3 天：第一个粗体
    /// 落在第 0/1/2 段的分别是 327 / 90 / 4 条。
    func test_粗体在第二段也认() {
        let text = "收到 @机长 —— 我看看。\n\n**结论：那条是重放，不用重做。**\n" + lines(15)
        XCTAssertEqual(CrewMessageFold.fold(text)?.summary, "结论：那条是重放，不用重做。")
    }

    func test_没有粗体但有标题_用标题() {
        let text = "## 发版闸门核对\n" + lines(20)
        XCTAssertEqual(CrewMessageFold.fold(text)?.summary, "发版闸门核对")
    }

    // MARK: - 不该折的

    func test_短消息不折() {
        XCTAssertNil(CrewMessageFold.fold("**收到**，我看看。"), "两行的消息折了反而更难读")
    }

    func test_正好等于阈值不折() {
        XCTAssertNil(CrewMessageFold.fold("**结论**\n" + lines(CrewMessageFold.lineThreshold - 1)))
        XCTAssertNotNil(CrewMessageFold.fold("**结论**\n" + lines(CrewMessageFold.lineThreshold)),
                        "越过阈值一行就该折——边界两侧都要量，只量一侧的判据不算判据")
    }

    /// **这条是 4-1 第 2 条约束的正身。** 一条长消息没有任何作者写下的结论时，
    /// 正确的行为是**摊开**，不是拿开头几十个字冒充摘要。
    func test_没有可当摘要的东西时宁可不折也不截断开头() {
        let text = "收到，我看看。\n" + lines(30)
        XCTAssertNil(
            CrewMessageFold.fold(text),
            "没有结论可摘时必须摊开。截出来的开头常常是「收到，我看看」——"
                + "那种摘要比不折更糟：它让人以为自己看过了。")
    }

    /// 贴了脚本/日志的消息：围栏里的 `**` 不是强调、`#` 不是标题。
    func test_围栏代码块里的星号和井号不当摘要() {
        let text = "```sh\n# 这是注释不是标题\necho '**这不是粗体**'\n```\n" + lines(20)
        XCTAssertNil(CrewMessageFold.fold(text),
                     "拿脚本里的字当摘要，等于给这条消息编了一个它没说过的结论")
    }

    // MARK: - 摘要本身的样子

    func test_摘要压平成一行并去掉markdown标记() {
        let text = "**闸门全绿，\n包可以发了**（`0.1.25`）\n" + lines(20)
        let s = CrewMessageFold.fold(text)?.summary
        XCTAssertEqual(s, "闸门全绿， 包可以发了")
        XCTAssertFalse(s?.contains("*") ?? true, "收起条是一行纯文本，留着星号只会看见星号")
    }

    func test_过长的摘要截断加省略号() {
        let long = String(repeating: "长", count: 200)
        let s = CrewMessageFold.fold("**\(long)**\n" + lines(20))?.summary
        XCTAssertEqual(s?.count, CrewMessageFold.summaryCap + 1)
        XCTAssertTrue(s?.hasSuffix("…") ?? false)
    }

    func test_显式摘要优先于推导() {
        let text = "**推导出来的那句**\n" + lines(20)
        XCTAssertEqual(
            CrewMessageFold.fold(text, explicitSummary: "作者自己写的那句")?.summary,
            "作者自己写的那句", "发送者显式给了就用它——那是最可靠的一级")
    }

    // MARK: - 边界：折叠只活在渲染层

    /// **4-1 第 3 条约束**：折叠是给人看的。给 agent 的注入面必须仍是全文 ——
    /// 我们已经有两道方向相反的截断了，别让它长成第三道。
    func test_折叠只活在渲染层_注入面拿到的仍是全文() {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("fold-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = LocalWhiteboardStore(directory: dir)
        let body = "**结论在这里。**\n" + lines(30, "只有展开才看得到的细节")
        store.appendUserMessage(crewId: "c", text: body)

        XCTAssertNotNil(CrewMessageFold.fold(body), "前置条件：这条在界面上确实会被折起来")

        let injected = HookEmitter(store: store, crewId: "c", sessionId: "s", cursorDir: dir)
            .emitAndAdvance()
        XCTAssertNotNil(injected)
        XCTAssertTrue(injected!.contains("只有展开才看得到的细节30"),
                      "注入面丢了正文最后一行 —— 折叠变成了第三道截断")
    }
}
