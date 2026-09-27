import XCTest

/// 「仅@你」挪到三个按钮左边（人类 Todo #128）；#155 将默认值改为关闭。
///
/// ## 默认状态与空态出路
///
/// `CrewCenterView` 里有 Todo #61 定的行为：**切 crew 时筛选归位**，理由原文是
/// 「换个群还挂着『只看 @ 我』，新群大概率筛成空」。
///
/// #128 曾要求默认点亮；#155 新口径只将默认值改为关闭，点击后的筛选判定不变。
///
/// 手动打开筛选后，空状态仍须可解释、可一键退出。所以这里除了钉
/// 「默认关闭」「位置在最左」，还钉死那条出路 —— 筛完一条不剩时，得有一个明说
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

    // MARK: - ① 默认关闭

    func testFilterDefaultsToOff() throws {
        let center = Self.codeOnly(try Self.text(of: "CrewCenterView.swift"))
        XCTAssertTrue(
            center.contains("@State private var onlyMentions = CrewMentionFilter.defaultOnlyMentions"),
            """
            「仅@你」的默认值不是从 `CrewMentionFilter.defaultOnlyMentions` 来的。\
            这个默认值有两个读者（初值 + 切群归位），写死两处迟早会分叉 —— \
            那时冷启动与切群后的默认状态可能分叉，而且没有任何报错。
            """)
        XCTAssertFalse(
            CrewMentionFilter.defaultOnlyMentions,
            "新口径要求「仅@你」默认不点亮")
    }

    /// 切群归位**保留**（那是 Todo #61 的意思：筛选状态不跨群带走），
    /// 且仍通过共享默认值归位，不另写一份常量。
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
            切群归位没有归到 `defaultOnlyMentions`，默认状态可能与冷启动分叉，\
            或失去 Todo #61 要求的「筛选状态不跨群带走」。
            """)
        XCTAssertFalse(
            handler.contains("onlyMentions = false"),
            "切群时还在把筛选写死成 false")
    }

    /// 反面守卫：**搜索时仍然强制看全部**。
    ///
    /// 这条是上面那把尺子第一版咬到的东西，单独钉住：
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
            群里有消息、手动打开筛选后却全筛没了，\
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

    /// #61/#128 的历史与 #155 的当前默认值，都要留在切群归位处。
    func testTheConflictIsWrittenDownWhereTheNextPersonWillLook() throws {
        let center = try Self.text(of: "CrewCenterView.swift")
        XCTAssertTrue(
            center.contains("#61") && center.contains("#128") && center.contains("#155"),
            """
            切群归位的说明缺少 #61、#128 或 #155，下一次修改可能误判默认状态。
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
