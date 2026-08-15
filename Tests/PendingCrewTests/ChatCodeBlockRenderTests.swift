#if os(macOS)
import AppKit
import SwiftUI
import XCTest

/// 「群聊里三反引号代码块掉回普通正文」（2026-08-11）的回归守卫。
///
/// ## 根因（这个测试守的到底是什么）
///
/// `MarkdownText` 原来这么写：
/// ```
/// Markdown(text)
///     .markdownTheme(chatTheme)                  // 里层
///     .markdownBlockStyle(\.codeBlock) { … }     // 外层
/// ```
/// MarkdownUI 里这两个都是环境值写入：`markdownTheme(t)` 是
/// `.environment(\.theme, t)`（整个 theme 换掉），`markdownBlockStyle(kp)` 是
/// `.environment(\.theme[kp], …)`（读外层 theme、改一个字段、写回）。SwiftUI 环境值
/// **离视图越近的越赢**，于是里层的 `markdownTheme` 把外层那个补丁整个盖回去，
/// `ChatCodeBlock` 从来没被实例化过 —— 围栏块一路退到 MarkdownUI `Theme()` 的默认
/// `codeBlock`（`BlockStyle { $0.label }`，拿正文字体直接画），所以没边框、没底色、
/// 不等宽、没复制按钮，还跟正文一起按字宽折行。行内 `code` 不受影响（它本来就在
/// theme 里），这正是「行内好、围栏坏」的成因。
///
/// ## 为什么能自动测（不用起 GUI）
///
/// 这里不截图、不比像素，只**量高度**：离屏 `NSHostingView` 布局出来的高度，能把
/// 「围栏成了块」和「围栏退回正文」这两种状态分开，而且分得很开：
///
/// - `ChatCodeBlock` 有工具条 + 内边距 + 描边 ⇒ 同样一行内容，块比裸段落**高一截**。
/// - `ChatCodeBlock` 把内容装在 `ScrollView(.horizontal)` 里 ⇒ 超长单行**横向滚动**，
///   高度维持在一行；退回正文则按字宽硬折成很多行，高度暴涨。
///
/// 两条合起来，只有「`ChatCodeBlock` 真的被实例化了」能同时解释。
///
/// ## 守不住的部分（别误读这个测试的绿）
///
/// 边框/底色的**观感**、等宽字体好不好看、「复制」按钮**点下去**是否真进剪贴板，
/// 都只有人眼能验 —— 挂在 QA 尾巴（task #443 / docs/tech-debt.md）。
/// 本测试绿只证明：围栏走的是 `ChatCodeBlock` 这条路，且长行不再按正文折行。
final class ChatCodeBlockRenderTests: XCTestCase {

    /// 中栏气泡的典型宽度，跟 `CrewChatOpenCostTests` 对齐。
    private let bubbleWidth: CGFloat = 520

    override func setUp() {
        super.setUp()
        // SwiftUI / MarkdownUI 首次使用有一次性初始化成本与懒加载，先热一遍，
        // 免得第一条被测量的高度带上噪声。
        _ = height(of: "warm **up** `x`")
    }

    /// 离屏把一段 markdown 布局出来，返回它要占的高度。
    private func height(of markdown: String) -> CGFloat {
        let host = NSHostingView(
            rootView: MarkdownText(text: markdown, allowCodeRun: true, citations: [])
                .fixedSize(horizontal: false, vertical: true)
                .frame(width: bubbleWidth, alignment: .leading)
        )
        host.frame = CGRect(x: 0, y: 0, width: bubbleWidth, height: 10)
        host.layoutSubtreeIfNeeded()
        return host.fittingSize.height
    }

    // MARK: - 验收 1：围栏独立成块（有工具条 + 内边距的 chrome）

    /// 同样一行内容，围栏版必须明显高于裸段落版 —— 高出来的就是
    /// `ChatCodeBlock` 的工具条（含「复制」）+ 上下内边距 + 描边。
    ///
    /// 修之前这两个高度几乎一样（都是一行正文），差值≈0，所以这条会红。
    func test_围栏比同内容的裸段落高出一截_说明chrome真的画了() {
        let paragraph = height(of: "swift build")
        let fenced = height(of: "```\nswift build\n```")
        let delta = fenced - paragraph

        XCTAssertGreaterThan(
            delta, 25,
            """
            围栏块只比裸段落高 \(delta)pt —— chrome 没画出来，围栏多半又退回普通段落了。
            段落 \(paragraph)pt / 围栏 \(fenced)pt。
            先看 MarkdownText.swift 里 codeBlock 样式是不是又被 `.markdownTheme` 盖掉了。
            """
        )
    }

    /// 验收 3：**不带**语言标签的裸围栏也要成块（人类实际发的那条就是裸围栏）。
    func test_裸围栏和带语言标签的围栏都成块() {
        let paragraph = height(of: "swift build")
        let bare = height(of: "```\nswift build\n```")
        let tagged = height(of: "```sh\nswift build\n```")

        XCTAssertGreaterThan(bare - paragraph, 25, "裸围栏没成块")
        XCTAssertGreaterThan(tagged - paragraph, 25, "带语言标签的围栏没成块")
    }

    // MARK: - 验收 4：长行横向滚动，不许按正文字宽硬折

    /// 一条远超气泡宽度的单行命令：
    /// - 成块 ⇒ 装在 `ScrollView(.horizontal)` 里，高度维持一行；
    /// - 退回正文 ⇒ 按字宽折成一坨，高度跟同样内容的裸段落一样暴涨。
    ///
    /// 所以判据是「围栏版必须显著矮于同内容的裸段落版」。修之前两者相当，这条会红。
    func test_超长单行在围栏里不折行() {
        let long = String(repeating: "pendingcrew-xcodebuild-destination ", count: 16)
        let asParagraph = height(of: long)
        let asFenced = height(of: "```\n\(long)\n```")

        XCTAssertGreaterThan(
            asParagraph, 100,
            "前提没成立：这条长行在裸段落里就该折成好几行（实测 \(asParagraph)pt），" +
            "否则这个对比没有意义，请把 long 加长。"
        )
        XCTAssertLessThan(
            asFenced, asParagraph * 0.7,
            """
            超长单行在围栏里占了 \(asFenced)pt，裸段落是 \(asParagraph)pt —— 差不多说明
            围栏内容仍在按正文字宽折行，没走 ChatCodeBlock 的横向 ScrollView。
            """
        )
    }

    // MARK: - 别把守卫做成「只要够高就行」

    /// 上面几条都在比高度差，一个「把所有块都撑高」的错误实现也能骗过它们。
    /// 这条反向钉住：普通段落**不许**长出代码块的 chrome —— 正文还是正文。
    func test_普通段落不许被当成代码块() {
        let oneLine = height(of: "这是一句普通正文。")
        let fenced = height(of: "```\n这是一句普通正文。\n```")
        XCTAssertLessThan(
            oneLine, fenced,
            "普通段落长得跟代码块一样高，说明 chrome 被套到了所有块上，不是只套围栏。"
        )
    }
}
#endif
