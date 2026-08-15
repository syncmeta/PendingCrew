import XCTest

/// 群聊里三反引号代码块「掉回普通正文」那次翻车（2026-08-11）留下的守卫。
///
/// 那次的**真根因**是视图层的环境值覆盖（`.markdownTheme` 盖掉
/// `.markdownBlockStyle(\.codeBlock)`，见 MarkdownText.swift 的 SHIM 注释），
/// 不是这里测的字符串层 —— 排查时头号嫌疑是「`rewrittenText` 把围栏改坏了」，
/// 查下来 `MathMarkup` 是**围栏感知**的、清白。
///
/// 但清白不等于以后也清白：`MarkdownText` 渲染的从来不是原文 `text` 而是
/// `rewrittenText`，任何人往那条重写链上再加一道正则，都可能悄悄把围栏行改坏，
/// 而这种坏法只有人眼盯着 GUI 才看得出来。所以把「经过重写后围栏必须原样还在」
/// 钉成单测：这是这块唯一能自动守住的一层。
final class MathMarkupFenceTests: XCTestCase {

    // MARK: - 围栏必须原样穿过重写

    /// 裸围栏（无语言标签）—— 人类实际发的那条就是这种。
    func testBareFenceSurvivesRewrite() {
        let input = """
        前面一段正文。

        ```
        git log --oneline -10 main
        cd apps/pendingcrew && xcodebuild test
        ```

        后面一段正文。
        """
        XCTAssertEqual(MathMarkup.rewrite(input), input,
                       "裸围栏块必须原样穿过重写，一个字符都不许动")
    }

    /// 带语言标签的围栏。
    func testLanguageTaggedFenceSurvivesRewrite() {
        let input = """
        ```sh
        echo hi
        ```
        """
        XCTAssertEqual(MathMarkup.rewrite(input), input)
    }

    /// 围栏里的 `$` 必须留成字面量 —— shell 片段里 `$HOME` / `$(cmd)` 遍地都是，
    /// 被当成行内数学吃掉就等于把用户的命令改坏了。
    func testDollarsInsideFenceAreNotTreatedAsMath() {
        let input = """
        ```bash
        echo $HOME
        VER=$(date +%s) && echo "$VER"
        ```
        """
        XCTAssertTrue(MathMarkup.containsMath(input), "含 `$` 才会真的走进重写分支")
        XCTAssertEqual(MathMarkup.rewrite(input), input,
                       "围栏内的 $ 必须字面保留，不许被 inline math 吃掉")
    }

    /// 反斜杠数学定界符在围栏里同样不许生效（LaTeX 教程/正则片段会踩）。
    func testBackslashDelimitersInsideFenceAreLiteral() {
        let input = """
        ```tex
        \\(x^2\\) 和 \\[y\\]
        ```
        """
        XCTAssertEqual(MathMarkup.rewrite(input), input)
    }

    /// 波浪号围栏（`~~~`）也是 CommonMark 合法围栏。
    func testTildeFenceSurvivesRewrite() {
        let input = """
        ~~~
        cost = $5 + $10
        ~~~
        """
        XCTAssertEqual(MathMarkup.rewrite(input), input)
    }

    /// 缩进的闭合围栏（列表里贴代码常见）也要被认出来，否则闭合围栏之后的正文
    /// 会被误当成代码段的一部分。
    func testIndentedClosingFenceIsRecognised() {
        let input = "```\ncode\n  ```\n"
        XCTAssertEqual(MathMarkup.rewrite(input), input)
    }

    /// 行内代码同样是代码区，`$` 不许被吃。
    func testInlineCodeSpanIsLeftAlone() {
        let input = "行内 `echo $PATH` 后面还有字。"
        XCTAssertEqual(MathMarkup.rewrite(input), input)
    }

    // MARK: - 围栏之外的数学照常重写（别把守卫做成「什么都不改」）

    /// 上面那一堆断言都是「相等」，很容易被一个「rewrite 直接 return text」的
    /// 退化实现全部骗过。这条反向钉住：围栏**外**的数学必须真的被改写。
    func testMathOutsideFenceIsStillRewritten() {
        let input = "公式 \\(x^2\\) 在这里。"
        let out = MathMarkup.rewrite(input)
        XCTAssertNotEqual(out, input, "围栏外的数学必须被重写，否则守卫是假的")
        XCTAssertTrue(out.contains("![](\(MathMarkup.scheme)://i/"),
                      "应重写成行内 math 图片引用，实际：\(out)")
    }

    /// 混合场景：同一条消息里围栏内外都有 `$` —— 外面的算数学，里面的不算。
    /// 人类那条群聊消息正是「正文 + 行内代码 + 围栏」混排。
    func testMixedMessageRewritesOnlyOutsideFence() {
        let input = """
        行内代码 `foo` 和公式 $x^2$ 都在正文里。

        ```
        echo $NOT_MATH
        ```
        """
        let out = MathMarkup.rewrite(input)
        XCTAssertTrue(out.contains("```\necho $NOT_MATH\n```"),
                      "围栏块必须原样保留，实际：\(out)")
        XCTAssertTrue(out.contains("![](\(MathMarkup.scheme)://i/"),
                      "正文里的 $x^2$ 应被重写，实际：\(out)")
        XCTAssertTrue(out.contains("`foo`"), "行内代码应原样保留")
    }

    // MARK: - 无数学时不动原文

    /// `containsMath` 是重写的前置闸（MarkdownText.rewrittenText 只在它为真时
    /// 才调 rewrite）。不含数学定界符的围栏消息应当连门都不进。
    func testFenceWithoutMathDelimitersSkipsRewriteEntirely() {
        let input = "```\ngit status\n```"
        XCTAssertFalse(MathMarkup.containsMath(input))
        XCTAssertEqual(MathMarkup.rewrite(input), input)
    }
}
