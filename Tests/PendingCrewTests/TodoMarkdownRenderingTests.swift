#if os(macOS)
import AppKit
import SwiftUI
import XCTest

/// Todo 页面渲染 markdown（人类 Todo #119），以及**卡片不许被撑爆**。
///
/// ## 动手前量到的读数（决定了实现方式，不是写完再调的）
///
/// 列表卡片宽 300pt、`.lineLimit(3)`，同一批样本三种渲染的高度：
///
/// | 样本 | 纯 `Text` | `.article` | `.chat` |
/// |---|---|---|---|
/// | 短·纯文本 | 14 | 21 | 17 |
/// | 长·纯文本无换行（#109 形状） | 42 | 77 | 51 |
/// | **多 block·标题+列表+粗体** | **42** | **527** | **245** |
/// | 带围栏代码块 | 42 | 275 | 162 |
///
/// 两件事被这组数钉死：
/// 1. **`.lineLimit` 管不住多 block 的 markdown。** 纯 `Text` 恒定封在 42pt（3 行），
///    同一段内容换成 markdown 直接冲到 527pt —— **12.5 倍**，一条长 Todo 就能把整个
///    面板顶垮。`.lineLimit` 是**逐 block** 生效的，不是「整段最多 3 行」。
///    → 所以卡片必须在**源文本层**先截断（`TodoListPresentation.cardMarkdown`），
///      光靠视图层的 lineLimit 不行。
/// 2. **`.article` 会改字号**（17pt 衬线），`.chat` 是 16pt —— 而两处正文现在都是
///    `Theme.Fonts.footnote`（13pt）。人类明说「ui 格式要和外面的…一样」，
///    → 所以不能借用现成 variant，得加一个 13pt 的 `.todo`（`.chat` 一个字不动）。
///
/// ## 这里量得到什么、量不到什么
///
/// **量得到**：卡片高度会不会随内容里的 markdown 结构变化（离屏 `NSHostingView`
/// 真布局）、两处有没有都接上同一套 variant、字号有没有被动过、截断纯逻辑的边界。
///
/// **量不到**：好不好看。渲染出来的观感、两处像不像、截断处断得体面不体面，
/// **只有人眼能验** —— 本文件全绿不构成「视觉没问题」。
final class TodoMarkdownRenderingTests: XCTestCase {

    /// 右栏 Todo 面板卡片的内宽（detail 列 ideal 400 − 面板边距 − 卡片 12×2）。
    private let cardWidth: CGFloat = 300
    private var layout: TodoListPresentation.OverviewLayout { TodoListPresentation.overviewLayout }

    // MARK: - ① 卡片高度必须与「内容里有多少 markdown 结构」无关

    /// **这条是 #119 的主判据。**
    ///
    /// 判据刻意跟**同一趟里的另一个样本**比，不跟一个记下来的 pt 数比：绝对值会随
    /// 字体/行距/机器变，而「一段纯散文和一段带标题列表代码块的内容，在同一张卡片里
    /// 应该一样高」这件事不会变。
    func testCardHeightDoesNotGrowWithMarkdownStructure() {
        let prose = cardHeight(Self.longProse)
        XCTAssertGreaterThan(prose, 0, "散文基准没量出来，测试本身失效了")

        for sample in Self.samples {
            let height = cardHeight(sample.text)
            print(String(format: "[卡片高度] %@ = %.0f（散文基准 %.0f）", sample.name, height, prose))
            XCTAssertLessThanOrEqual(
                height, prose * 1.25,
                """
                「\(sample.name)」在列表卡片里量到 \(Int(height))pt，而同宽同预算的纯散文只有 \
                \(Int(prose))pt —— 卡片高度跟着内容里的 markdown 结构涨了。\
                `.lineLimit` 是逐 block 生效的，管不住多 block；卡片必须在**源文本层**先截断。
                """)
        }
    }

    /// 反面：截断必须真的发生 —— 同一条内容，卡片一定比详情窗口矮。
    /// 少了这条，上面那条可以靠「把卡片渲染成空的」作弊通过。
    func testCardIsActuallyTruncatedRelativeToDetail() {
        for sample in Self.samples where sample.isLong {
            let card = cardHeight(sample.text)
            let detail = detailHeight(sample.text)
            XCTAssertLessThan(
                card, detail,
                "「\(sample.name)」卡片(\(Int(card))pt)没有比详情(\(Int(detail))pt)矮 —— 截断没生效")
            XCTAssertGreaterThan(card, 0, "「\(sample.name)」卡片渲染成空的了，那不叫截断")
        }
    }

    // MARK: - ② 两处都要接上，而且是同一套样式

    func testBothSurfacesRenderMarkdownWithTheSameVariant() throws {
        let panel = try Self.text(of: "CrewTodoPanel.swift")
        let detail = try Self.text(of: "CrewTodoDetailWindow.swift")

        for (name, source) in [("列表卡片", panel), ("详情窗口", detail)] {
            XCTAssertTrue(
                source.contains("MarkdownText("),
                "\(name)还没渲染 markdown —— 人类要的是「todo 页面」整页，不是只改一处")
            XCTAssertTrue(
                source.contains("variant: .todo"),
                """
                \(name)用的不是 `.todo` variant。两处必须同一套 —— 只给详情加、\
                让点进去变成另一副样子，正是人类说「要和外面一样」时在防的那件事。
                """)
        }
    }

    func testResponsesAreRenderedToo() throws {
        let panel = Self.codeOnly(try Self.text(of: "CrewTodoPanel.swift"))
        let detail = Self.codeOnly(try Self.text(of: "CrewTodoDetailWindow.swift"))
        XCTAssertFalse(
            detail.contains("Text(resp.text)"),
            "详情窗口的回应还是纯 Text —— 回应也在「todo 页面」里，要一起渲染")
        XCTAssertTrue(
            panel.contains("MarkdownText(") && panel.contains("overviewResponse"),
            "列表卡片的末条回应没走 markdown")
    }

    // MARK: - ③ 字号不许动

    func testTodoVariantKeepsTheExistingFootnoteSize() throws {
        let markdown = try Self.text(of: "MarkdownText.swift")
        XCTAssertTrue(
            markdown.contains("case chat, codexTranscript, article, todo, todoNote"),
            "MarkdownText 没有加 .todo / .todoNote variant（也别去改 .chat）")
        XCTAssertTrue(
            markdown.contains("AppTheme.Fonts.scaled(13)"),
            """
            .todo 主题没有钉在 13pt。两处正文现在都是 `Theme.Fonts.footnote`（=13pt），\
            人类说「ui 格式要和外面的一样」—— 借 `.article`(17pt 衬线) 或 `.chat`(16pt) \
            都会把 Todo 卡片撑大变样。
            """)
        XCTAssertTrue(
            markdown.contains("AppTheme.Fonts.scaled(12)"),
            ".todoNote 主题没有钉在 12pt（回应现在是 Theme.Fonts.caption）")
    }

    func testChatVariantIsUntouched() throws {
        let markdown = try Self.text(of: "MarkdownText.swift")
        XCTAssertTrue(
            markdown.contains("FontSize(AppTheme.Fonts.scaled(16))"),
            "聊天气泡的 16pt 被动了 —— 加 Todo 的样式不许动 .chat")
    }

    /// 「已完成只变灰、不加删除线」是人类明确要求过的老行为。换成 markdown 之后
    /// **祖先的 `.foregroundStyle` 压不动它** —— 主题里 `.text { ForegroundColor(…) }`
    /// 是显式写死的。这条钉住变灰改走了主题那一侧，而不是被静默丢掉。
    func testCompletedTodosStillDimAfterSwitchingToMarkdown() throws {
        let markdown = try Self.text(of: "MarkdownText.swift")
        XCTAssertTrue(markdown.contains("var dimmed: Bool = false"),
                      "MarkdownText 没有变灰这一档")
        XCTAssertTrue(markdown.contains("todoDimTheme"), ".todo 没有对应的变灰主题")
        for (name, file) in [("列表卡片", "CrewTodoPanel.swift"), ("详情窗口", "CrewTodoDetailWindow.swift")] {
            let source = Self.codeOnly(try Self.text(of: file))
            XCTAssertTrue(
                source.contains("dimmed: icon.dimsText"),
                """
                \(name)没有把「已完成」接到变灰上 —— 换 markdown 之后外层 \
                `.foregroundStyle` 已经压不动主题里写死的颜色，已完成的 Todo 会跟未完成一样黑。
                """)
            XCTAssertFalse(
                source.contains(".strikethrough("),
                "\(name)加了删除线 —— 人类明确要求过只变灰、不划线")
        }
    }

    // MARK: - ④ 截断纯逻辑的边界

    func testShortProseIsLeftExactlyAlone() {
        let text = "把侧栏那个排序改一下"
        XCTAssertEqual(TodoListPresentation.cardMarkdown(text, lineBudget: 3), text,
                       "短的纯文本被动过了 —— 绝大多数 Todo 是这种，不该有任何变化")
    }

    func testEmptyStaysEmpty() {
        XCTAssertEqual(TodoListPresentation.cardMarkdown("", lineBudget: 3), "")
        XCTAssertEqual(TodoListPresentation.cardMarkdown("   \n\n  ", lineBudget: 3), "")
    }

    func testOverBudgetIsCutAndMarked() {
        let cut = TodoListPresentation.cardMarkdown(Self.multiBlock, lineBudget: 3)
        XCTAssertTrue(cut.hasSuffix("…"), "截断了却没留省略号，人看不出下面还有")
        XCTAssertLessThan(cut.count, Self.multiBlock.count, "根本没截")
        XCTAssertFalse(cut.contains("2026-06-15"), "第三段还在，预算没起作用")
    }

    func testNeverEmitsAnUnclosedCodeFence() {
        let cut = TodoListPresentation.cardMarkdown(Self.fenced, lineBudget: 3)
        XCTAssertEqual(
            cut.components(separatedBy: "```").count % 2, 1,
            """
            截出来的片段里围栏数是奇数 —— 有一个 ``` 没闭合。\
            MarkdownUI 会把它后面所有内容都吞成代码块，卡片当场变成一坨。
            截出来的是：\(cut)
            """)
    }

    func testFirstLineSurvivesEvenWhenItAloneExceedsBudget() {
        let oneHugeLine = String(repeating: "很长的一句话没有任何换行", count: 40)
        let cut = TodoListPresentation.cardMarkdown(oneHugeLine, lineBudget: 3)
        XCTAssertFalse(cut.isEmpty, "一行就超预算时被截成空的了 —— 卡片会整个空白")
        XCTAssertTrue(oneHugeLine.hasPrefix(String(cut.dropLast())),
                      "截断改写了原文，而不是取前缀")
    }

    // MARK: - ⑤ 点进去要落在那一条上（人类 Todo #122，与 #119 同一块地方）

    /// 人类原话：「在外面的没放大的 todo 列表 点进去之后 要能直接显示这个 Todo 的详情
    /// 而不是展示全文」。现状是点进去看到**又一份完整列表**，他得在里面重新找。
    ///
    /// 形状选的是「单条详情 + 一个回全部列表的入口」而不是「滚动定位 + 高亮」：
    /// 详细窗口里最大的一本账有 **125 条**（本机实测），列表必须留 `LazyVStack`，
    /// 而往 `LazyVStack` 里 `scrollTo` 一个还没实体化的 id 是出了名的会飘 ——
    /// **一个我没法目视验证的定位，飘了也不会有人告诉我**。单条详情由构造保证落点，
    /// 没有定位可言，也就没有飘的余地。列表能力用「‹ 全部」原样留着。
    func testFocusShowsOnlyThatOneTodo() {
        let rows = Self.ledgerRows
        let focused = TodoListPresentation.focusedRows(rows, focus: 7)
        XCTAssertEqual(focused.map(\.number), [7],
                       "点第 7 条进去，看到的不是这一条 —— 人还得在里面重新找")
    }

    func testNoFocusKeepsTheWholeList() {
        let rows = Self.ledgerRows
        XCTAssertEqual(TodoListPresentation.focusedRows(rows, focus: nil).map(\.number),
                       rows.map(\.number),
                       "没指定条目时列表被砍了 —— 他没说不要列表")
    }

    func testMissingFocusFallsBackToTheWholeList() {
        let rows = Self.ledgerRows
        XCTAssertEqual(
            TodoListPresentation.focusedRows(rows, focus: 999).map(\.number),
            rows.map(\.number),
            """
            指向一条不存在的 #N（开着窗口时那条被删了 / 换了本账号码对不上）时给了空白。\
            空窗口没有任何出路，人只能关掉重开 —— 回落到全列表。
            """)
    }

    func testTappingARowCarriesItsNumberIntoTheWindow() throws {
        let panel = Self.codeOnly(try Self.text(of: "CrewTodoPanel.swift"))
        XCTAssertTrue(
            panel.contains("openDetail(focus: item.number)"),
            "点某一行没有把它的 #N 带进详细窗口 —— 那窗口就不知道该显示哪条")
        XCTAssertTrue(
            panel.contains("openDetail(focus: nil)"),
            "顶部「放大看」按钮该开的是全列表（那是列表入口，不是某一条）")
    }

    func testDetailWindowKeepsAWayBackToTheList() throws {
        let detail = Self.codeOnly(try Self.text(of: "CrewTodoDetailWindow.swift"))
        XCTAssertTrue(
            detail.contains("focusedRows("),
            "详细窗口没有接单条聚焦 —— 点进去还是一整份列表")
        XCTAssertTrue(
            detail.contains("focus = nil"),
            "详细窗口里没有回到全列表的出路 —— 那就是把列表能力砍掉了")
    }

    private static let ledgerRows: [LocalTodoItem] = [
        LocalTodoItem(id: "a", number: 9, text: "九", status: "pending", createdAt: "2026-09-03"),
        LocalTodoItem(id: "b", number: 7, text: "七", status: "pending", createdAt: "2026-09-02"),
        LocalTodoItem(id: "c", number: 2, text: "二", status: "completed", createdAt: "2026-09-01"),
    ]

    // MARK: - 量法（离屏真布局）

    private func cardHeight(_ text: String) -> CGFloat {
        height(AnyView(
            MarkdownText(text: TodoListPresentation.cardMarkdown(text, lineBudget: layout.bodyLineLimit),
                         variant: .todo)
                .lineLimit(layout.bodyLineLimit)))
    }

    private func detailHeight(_ text: String) -> CGFloat {
        height(AnyView(MarkdownText(text: text, variant: .todo)))
    }

    private func height(_ view: AnyView) -> CGFloat {
        let host = NSHostingView(rootView: AnyView(view.frame(width: cardWidth)))
        host.frame = CGRect(x: 0, y: 0, width: cardWidth, height: 10)
        host.layoutSubtreeIfNeeded()
        _ = host.fittingSize
        host.layoutSubtreeIfNeeded()
        return host.fittingSize.height
    }

    // MARK: - 样本（形状取自本机账本里的真条目）

    private struct Sample { let name: String; let text: String; let isLong: Bool }

    /// 基准：长、但**一点 markdown 结构都没有**（#109 就是这个形状 —— 几百字、零换行）。
    private static let longProse = String(repeating:
        "我希望总机长是相当于一个汇总的和层级、时间流视图并列的并且有自己的群聊、todo和session等等一个机组应该有的。", count: 3)

    private static let multiBlock = """
    **你记得的是对的，而且我把整条时间线挖出来了。**

    时间线（每一条都有提交号）：

    - **2026-06-03/04** `f93e90c9`：做了群钱包页（实缴池 + 认缴），两端都接了入口。
    - **2026-06-09** `73faafbe`：Mac 改接共享 Crew tab，那个入口没了。
    - **2026-06-15** `095dfc2b`：删掉 CrewDetailV2View，iOS 那个入口也没了。

    **所以真正的原因是：它的新家在 crew 详情页里，而那套在 6-15 被当作死代码清掉了。**
    """

    private static let fenced = """
    这条报错反复出现，先把复现步骤记下：

    ```swift
    let host = NSHostingView(rootView: root)
    host.layoutSubtreeIfNeeded()
    ```

    另外表格也要看：

    | 列 | 值 |
    | --- | --- |
    | a | 1 |
    """

    private static let samples: [Sample] = [
        Sample(name: "短·纯文本", text: "把侧栏那个排序改一下", isLong: false),
        Sample(name: "长·纯文本无换行(#109 形状)", text: longProse, isLong: true),
        Sample(name: "多block·标题+列表+粗体", text: multiBlock, isLong: true),
        Sample(name: "带围栏代码块+表格", text: fenced, isLong: true),
    ]

    // MARK: - 源码扫描小工具

    /// 只扫代码，不扫注释 —— 注释里正该写「这里以前是纯 Text」。
    private static func codeOnly(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    private static func text(of fileName: String) throws -> String {
        guard let hit = try sourceFiles().first(where: { $0.0.lastPathComponent == fileName })
        else { throw XCTSkip("找不到源码文件 \(fileName)") }
        return hit.1
    }

    private static func sourceFiles() throws -> [(URL, String)] {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录 \(root.path)（不在开发机上跑）") }
        return walker.compactMap { any in
            guard let url = any as? URL, url.pathExtension == "swift",
                  let text = try? String(contentsOf: url, encoding: .utf8) else { return nil }
            return (url, text)
        }
    }
}
#endif
