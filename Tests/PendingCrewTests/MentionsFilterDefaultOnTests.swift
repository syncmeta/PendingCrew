import XCTest

/// 「仅@你」挪到三个按钮左边、并且**默认点亮**（人类 Todo #128）。
///
/// ## 这条改动跟一条既有设计正面冲突，本文件的一半是为它立的
///
/// `CrewCenterView` 里有 Todo #61 定的行为：**切 crew 时筛选归位**，理由原文是
/// 「换个群还挂着『只看 @ 我』，新群大概率筛成空」。
///
/// 默认点亮 + 切群归位 ⇒ **每次进群都是点亮的** ⇒ 按 #61 自己的说法，大概率一进群
/// 就是一片空白。人类要的正是 #61 当初要防的那件事。
///
/// 机长的裁定：照人类说的做，但**空状态必须可解释、可一键退出**。所以这里除了钉
/// 「默认点亮」「位置在最左」，还钉死那条出路 —— 筛完一条不剩时，得有一个明说
/// 「这个群没有 @ 你的消息」和一个点一下就看全部的按钮。
///
/// ## 量得到什么、量不到什么
///
/// **量得到**：默认值、toolbar 里的声明顺序、切群归位的口径、以及「什么时候该给
/// 那条出路」的纯判定（真跑，不是扫文本）。
///
/// **量不到**：它长什么样、按钮在人眼里是不是真的在那三个按钮左边。
/// macOS 的 toolbar 最终排布由系统决定，源码顺序只是**必要条件**。
/// **本文件全绿不构成「位置对了」** —— 那一条只有人眼能验。
final class MentionsFilterDefaultOnTests: XCTestCase {

    // MARK: - ① 默认点亮

    func testFilterDefaultsToOn() throws {
        let center = Self.codeOnly(try Self.text(of: "CrewCenterView.swift"))
        XCTAssertTrue(
            center.contains("@State private var onlyMentions = CrewMentionFilter.defaultOnlyMentions"),
            """
            「仅@你」的默认值不是从 `CrewMentionFilter.defaultOnlyMentions` 来的。\
            这个默认值有两个读者（初值 + 切群归位），写死两处迟早会分叉 —— \
            那时「默认点亮」在冷启动时成立、切一次群就不成立了，而且没有任何报错。
            """)
        XCTAssertTrue(
            CrewMentionFilter.defaultOnlyMentions,
            "默认不是点亮的 —— 人类原话「并且默认点亮」")
    }

    /// 切群归位**保留**（那是 Todo #61 的意思：筛选状态不跨群带走），
    /// 但归到的是**新的默认值**，不再是写死的 false。
    ///
    /// 判据只切**切群那一段**，不扫全文件 —— 第一版扫全文件，被
    /// 「搜索时强制看全部」那条合法的 `onlyMentions = false` 咬红了。
    /// 一把会逼人改坏别处的尺子，红的是尺子不是代码。
    func testSwitchingCrewResetsToTheDefaultNotToFalse() throws {
        let handler = try Self.onChangeBlock(
            of: "crewStore.selectedCrewId",
            in: Self.codeOnly(try Self.text(of: "CrewCenterView.swift")))
        XCTAssertTrue(
            handler.contains("onlyMentions = CrewMentionFilter.defaultOnlyMentions"),
            """
            切群归位没有归到 `defaultOnlyMentions`。它要么还写死着 false —— 那「默认点亮」\
            只在冷启动那一次成立、切一次群就灭了；要么整条归位被删了 —— 那 Todo #61 \
            「筛选状态不跨群带走」就没了。两种都不会报错，只有人自己觉得不对劲。
            """)
        XCTAssertFalse(
            handler.contains("onlyMentions = false"),
            "切群时还在把筛选写死成 false")
    }

    /// 反面守卫：**搜索时仍然强制看全部**。
    ///
    /// 这条是上面那把尺子第一版咬到的东西，单独钉住 —— 它和「默认点亮」不冲突：
    /// 人打字搜东西时，再叠一层「只看 @ 我」会把结果筛得莫名其妙地少。
    func testTypingASearchStillForcesShowingEverything() throws {
        let handler = try Self.onChangeBlock(
            of: "searchQuery",
            in: Self.codeOnly(try Self.text(of: "CrewCenterView.swift")))
        XCTAssertTrue(
            handler.contains("onlyMentions = false"),
            "搜索时不再强制看全部了 —— 搜索结果会被「仅@你」再筛一道，人会以为搜不到")
    }

    // MARK: - ② 位置：在那三个按钮左边

    func testFilterIsDeclaredBeforeTheThreeButtons() throws {
        let center = Self.codeOnly(try Self.text(of: "CrewCenterView.swift"))
        guard let filter = center.range(of: "仅@你"),
              let info = center.range(of: "info.circle"),
              let cockpit = center.range(of: "speedometer"),
              let refresh = center.range(of: "arrow.clockwise")
        else { return XCTFail("toolbar 里那四项找不齐 —— 这条测试的锚点没了，先修测试") }

        for (name, other) in [("crew 详情", info), ("驾驶舱", cockpit), ("刷新", refresh)] {
            XCTAssertLessThan(
                filter.lowerBound, other.lowerBound,
                "「仅@你」声明在「\(name)」后面 —— 人类要的是放在这三个按钮的**左侧**")
        }
    }

    func testFilterNoLongerPinnedToTheTrailingEdge() throws {
        let center = Self.codeOnly(try Self.text(of: "CrewCenterView.swift"))
        XCTAssertFalse(
            center.contains("ToolbarItem(placement: .primaryAction)"),
            """
            「仅@你」还挂在 `.primaryAction` 上 —— 那个 placement 就是把它推到最右的原因\
            （Todo #79 当初特意这么放的）。要挪到最左就不能再用它。
            """)
    }

    // MARK: - ③ 筛成空时必须有出路（机长裁定的落点）

    func testEscapeHatchShowsWhenTheFilterHidEverything() {
        XCTAssertTrue(
            CrewMentionFilter.showsClearFilterEscape(
                onlyMentions: true, isSearching: false,
                hasAnyEntries: true, filteredIsEmpty: true),
            """
            群里有消息、筛选把它们全筛没了 —— 这正是「默认点亮」最常见的样子，\
            必须给一句解释和一个看全部的出口，否则人看到的就是一个坏掉的界面。
            """)
    }

    func testNoEscapeHatchWhenTheGroupItselfIsEmpty() {
        XCTAssertFalse(
            CrewMentionFilter.showsClearFilterEscape(
                onlyMentions: true, isSearching: false,
                hasAnyEntries: false, filteredIsEmpty: true),
            """
            群本身就是空的，却给了「看全部」—— 点下去还是空。\
            那颗按钮会把「这个群没人说过话」误说成「是筛选挡住了」。
            """)
    }

    func testNoEscapeHatchWhileSearching() {
        XCTAssertFalse(
            CrewMentionFilter.showsClearFilterEscape(
                onlyMentions: true, isSearching: true,
                hasAnyEntries: true, filteredIsEmpty: true),
            "搜索没结果是另一种空，出路是搜索框本身，不该再冒一颗「看全部」")
    }

    func testNoEscapeHatchWhenFilterIsOff() {
        XCTAssertFalse(
            CrewMentionFilter.showsClearFilterEscape(
                onlyMentions: false, isSearching: false,
                hasAnyEntries: true, filteredIsEmpty: true),
            "筛选没开着，空就是真的空 —— 不该说是筛选挡的")
    }

    func testNoEscapeHatchWhenSomethingSurvivedTheFilter() {
        XCTAssertFalse(
            CrewMentionFilter.showsClearFilterEscape(
                onlyMentions: true, isSearching: false,
                hasAnyEntries: true, filteredIsEmpty: false),
            "还有内容显示着，不该有空态出路")
    }

    func testChatViewActuallyWiresTheEscapeHatch() throws {
        let chat = Self.codeOnly(try Self.text(of: "CrewChatView.swift"))
        XCTAssertTrue(
            chat.contains("showsClearFilterEscape("),
            "群聊没接那条判定 —— 判定写好了没装到车上，人看到的还是一片纯空白")
        XCTAssertTrue(
            chat.contains("看全部"),
            "空态里没有「看全部」这颗按钮")
    }

    /// #61 那段注释是这次冲突**唯一的线索来源**，不许删掉了事 —— 要改写成现在的口径。
    func testTheConflictIsWrittenDownWhereTheNextPersonWillLook() throws {
        let center = try Self.text(of: "CrewCenterView.swift")
        XCTAssertTrue(
            center.contains("#61") && center.contains("#128"),
            """
            切群归位那段注释没有同时留下 #61 和 #128。\
            「默认点亮」和「切群归位」是一对会互相解释的设定，\
            只写现在这条、把当初为什么归位删掉，下一个人会把这个坑重走一遍。
            """)
    }

    // MARK: - 小工具

    /// 只扫代码，不扫注释 —— 注释里正该写「这里以前是 false」。
    private static func codeOnly(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    /// 取某个 `.onChange(of: X)` 闭包那一段（到下一个 `.onChange(` / `.task(` 为止）。
    private static func onChangeBlock(of key: String, in text: String) throws -> String {
        guard let start = text.range(of: ".onChange(of: \(key))") else {
            throw XCTSkip("找不到 .onChange(of: \(key)) —— 这条测试的锚点没了，先修测试")
        }
        let rest = text[start.upperBound...]
        let end = [".onChange(", ".task(", ".sheet("]
            .compactMap { rest.range(of: $0)?.lowerBound }
            .min() ?? rest.endIndex
        return String(rest[..<end])
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
