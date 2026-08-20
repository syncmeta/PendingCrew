#if os(macOS)
import AppKit
import ImageIO
import MarkdownUI
import SwiftUI
import XCTest

/// 「点进 crew 群聊把整个 PendingCrew 卡死」（#443）的**量化**回归。
///
/// ## 用的是真数据，不是玩具
///
/// `Fixtures/LEDDriverCrew/` 是从人类机器上原样拷来的那份「LED驱动板」白板
/// （70 条、58,945 字节、最长单条 1,236 字）加它那 5 张真图（最大 1514×1376 / 369KB）。
/// 造数据量不出真问题 —— 本 crew（PendingCrew）338 条、82k 字、也有图，实测**比这
/// 70 条还快**，所以「消息多才卡」是错的，必须拿这一份特定数据跑。
///
/// **那 70 条是定标口径，不是「当天碰巧多少条」**（2026-08-18 钉死）：白板只增不删，
/// 这个 crew 今天已经 342 条。直接拿全量重取 fixture 会把本文件里所有绝对预算变成
/// 看运气 —— 实测同一个用例单独跑 73 ms、跟在那个「重复 5 倍」的大表用例后面跑
/// 139 ms（同进程做过大表布局之后，后续每次布局都贵近一倍），100 ms 预算于是时红
/// 时绿。所以 `make-chat-fixtures.sh` 默认只取**前 70 条**（append-only ⇒ 前 70 条
/// 就是当初那份快照本身，仍是真数据，且跨机器跨时间可复现）。要看今天的全量加
/// `--full`，但别拿它的绝对毫秒去对这里的预算。
///
/// ## 这个测量到什么、量不到什么（重要，别含糊）
///
/// **量得到**：白板 JSON 解码 → 消息模型构造 → Markdown 视图构建+布局（离屏
/// `NSHostingView`）→ 本地图解码。
///
/// **量不到**：真窗口里**重复**发生的那部分（见下）。
///
/// ~~**没有窗口时 SelectionOverlay 的 NSView 根本不创建**，所以离屏量到的 selection
/// 成本是下限。~~
/// **[2026-08-11 失效]** 这句是上一版的**推理**，实测把它否掉了：
/// `test_探针_离屏到底有没有创建SelectionOverlay的NSTextField` 数出来，离屏 30 条
/// 常开 selection 的视图树里有 **71 个真 NSTextField**（按需模式 0 个）。所以
/// **overlay 的创建成本本测量是覆盖到的**，D 的收益不再是"只能靠推理"。
///
/// 仍然覆盖不到的是**重复**那一维：真窗口里每次视图图更新都要把全部 overlay 的
/// `updateNSView` 再跑一遍（0.1.7 那份 68.68s hang 的头号热点），离屏只发生一次。
/// 2026-08-11 那两份 hang 报告：
///
/// - `PendingCrew_2026-08-11-165319_Starship.hang`（0.1.7，卡 **68.68s**）：主线程
///   4/11 采样在 `SelectionOverlay.updateNSView → -[NSTextField setAttributedStringValue:]`。
/// - `PendingCrew_2026-08-11-165332_Starship.hang`（0.1.8 build 3584，卡 **7.03s**）：
///   12/12 采样在 `LazyStack.measureEstimates → lengthAndSpacing → 逐行 sizeThatFits`。
///
/// 离屏量到的一次全表成本是 ~180ms，现场是 7s —— 中间那 35 倍是「同一下付了几十遍」
/// 加「真窗口里的行比这里量的 MarkdownText 更重（头像/回复引用/图/悬停按钮）」，
/// **本测量都覆盖不到**，只能靠 hang 栈指认。所以本测试绿 ≠ 「点进去不卡」，
/// 只证明「每次重排要付的钱」降到了预算内。
/// 真验收口径见 `docs/tech-debt.md` #443 与 task #443 第 10 条。
final class CrewChatOpenCostTests: XCTestCase {

    // MARK: - 预算

    /// 打开一个 crew，主线程**一次**把消息渲染出来的预算（毫秒）。
    ///
    /// 定在 100ms：一次重排就该在两三帧内做完。窗口化之前是整整 70 条 ≈ 180ms，
    /// 而且打开时这一下要付十几遍 —— 那正是 7 秒 hang 的来源。
    private let budgetMs: Double = 100

    // MARK: - 真数据

    private static let fixtureDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .appendingPathComponent("Fixtures/LEDDriverCrew", isDirectory: true)

    /// fixture 是人类真实的群聊内容 + 5 张真实截图，**故意不入 git**（见 .gitignore），
    /// 所以在一台干净机器上它是缺的。
    ///
    /// **2026-08-20 从「缺了就红」改成「缺了就 skip」**（开源准备时干净 clone 实测
    /// 撞上的）。原来的理由是「静默跳过的性能回归测试等于没有测试」—— 那个理由成立，
    /// 但 `XCTSkip` **不是静默的**：报告里是独立的 skipped 状态，并原样打出下面那段
    /// 带修复命令的 hint，绝不会被误读成「测过了、没问题」。
    ///
    /// 改的原因是它的代价落错了人：README 教所有人跑 `xcodebuild … test`，而任何一个
    /// 刚 clone 下来的贡献者手上都不可能有这份 fixture —— 他做的第一件事就会得到一片
    /// 红，然后去查一个根本不存在的故障。**「本机开发者忘了取数据」和「外部人第一次
    /// clone」需要不同的对待，而红/绿这一个比特分不出这两者，skip 能。**

    private static let fixtureHint = """

        ✗ #443 打开成本测试缺少真实数据：
          \(fixtureDir.path)

          这份 fixture 是人类真实的群聊内容 + 真实聊天截图，故意不提交进 git。
          在**仓库根目录**跑一次（脚本要一个 crew id）：

            scripts/make-chat-fixtures.sh <crew-id>

          crew id 从本机白板目录里挑（「LED驱动板」那份就是这套基线的来源）：

            ls "$HOME/Library/Application Support/PendingCrew/whiteboards"

          没有真数据就不测，而不是拿造的数据凑一个绿 —— 造的数据量不出真问题
          （本 crew 338 条比 LED 那 70 条还快）。所以这里是 skip，不是 pass。

        """

    /// 每个用例开头调一次：缺数据就带着修复办法跳过（不是静默跳过，见上）。
    private func requireFixtures() throws {
        let json = Self.fixtureDir.appendingPathComponent("whiteboard.json")
        guard FileManager.default.fileExists(atPath: json.path) else {
            throw XCTSkip(Self.fixtureHint)
        }
    }

    private struct FixtureAttachment: Codable {
        let id: String
        let path: String
        let mime: String
        let size: Int?
    }

    private struct FixtureEntry: Codable {
        let id: String
        let text: String?
        let senderKind: String?
        let senderName: String?
        let senderSessionId: String?
        let createdAt: String?
        let attachments: [FixtureAttachment]?
    }

    private func loadEntries() throws -> [FixtureEntry] {
        try requireFixtures()
        let data = try Data(contentsOf: Self.fixtureDir.appendingPathComponent("whiteboard.json"))
        return try JSONDecoder().decode([FixtureEntry].self, from: data)
    }

    private func fixtureImages() throws -> [URL] {
        try requireFixtures()
        return try FileManager.default
            .contentsOfDirectory(at: Self.fixtureDir, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension.lowercased() == "png" }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    // MARK: - 计时

    private func ms(_ block: () -> Void) -> Double {
        let t = DispatchTime.now().uptimeNanoseconds
        block()
        return Double(DispatchTime.now().uptimeNanoseconds - t) / 1_000_000
    }

    /// 气泡里 markdown 的可用宽度（中栏典型值）。宽度会显著影响换行行数，钉死它，
    /// 否则不同机器上的数字不可比。
    private let bubbleWidth: CGFloat = 520

    /// 离屏把一段消息渲染并布局出来的成本 —— 这就是 `LazyStack` 每量一行要付的钱。
    private func layoutCost(_ texts: [String], selectable: Bool) -> Double {
        var total = 0.0
        for text in texts {
            total += ms {
                let host = NSHostingView(rootView:
                    MarkdownText(text: text, allowCodeRun: true, citations: [])
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .frame(width: bubbleWidth, alignment: .leading)
                        .environment(\.crewBubbleSelectable, selectable))
                _ = host.fittingSize
                host.layoutSubtreeIfNeeded()
                _ = host.fittingSize
            }
        }
        return total
    }

    /// SwiftUI / MarkdownUI 第一次用有一次性初始化成本，别把它算进任何一档。
    private func warmUp() {
        _ = layoutCost(["warm **up**\n\n- a\n- b\n\n| x | y |\n|---|---|\n| 1 | 2 |"],
                       selectable: true)
    }

    // MARK: - 整表（一个 host 装整条时间线，不是把单行成本加起来）

    /// 整表布局的两种问法 —— **这两种问法量出来的东西天差地别，别混着说**。
    enum TimelineProbe {
        /// 固定高度的视口里布一次。`LazyVStack` 的惰性此时是生效的：只有一屏被构建，
        /// 成本与「总共多少条」**无关**。
        case viewport
        /// 问「整张表有多高」。`LazyVStack` 必须把每一行都量一遍才能回答 ——
        /// 这正是 0.1.8 那份 hang 里 12/12 采样所在的
        /// `measureEstimates → lengthAndSpacing → 逐行 sizeThatFits`。
        case fullHeight
    }

    private func timelineLayoutCost(
        _ texts: [String], selectable: Bool, probe: TimelineProbe, eager: Bool = false
    ) -> Double {
        let host = makeTimelineHost(texts, selectable: selectable, probe: probe, eager: eager)
        return ms {
            host.layoutSubtreeIfNeeded()
            _ = host.fittingSize
        }
    }

    private func makeTimelineHost(
        _ texts: [String], selectable: Bool, probe: TimelineProbe, eager: Bool = false
    ) -> NSHostingView<AnyView> {
        let stack = Group {
            if eager {
                VStack(alignment: .leading, spacing: 0) {
                    timelineProbeRows(texts)
                }
            } else {
                LazyVStack(alignment: .leading, spacing: 0) {
                    timelineProbeRows(texts)
                }
            }
        }
        let root: AnyView
        switch probe {
        case .viewport:
            root = AnyView(ScrollView { stack }
                .frame(width: bubbleWidth, height: viewportHeight)
                .environment(\.crewBubbleSelectable, selectable))
        case .fullHeight:
            // 不给高度 → 谁要 fittingSize，谁就逼着这张表把每一行都量出来。
            root = AnyView(stack
                .frame(width: bubbleWidth)
                .environment(\.crewBubbleSelectable, selectable))
        }
        let host = NSHostingView(rootView: root)
        host.frame = CGRect(x: 0, y: 0, width: bubbleWidth,
                            height: probe == .viewport ? viewportHeight : 10)
        return host
    }

    @ViewBuilder
    private func timelineProbeRows(_ texts: [String]) -> some View {
        ForEach(Array(texts.enumerated()), id: \.offset) { _, text in
            MarkdownText(text: text, allowCodeRun: true, citations: [])
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// 中栏视口高度的典型值。窗口一页 12 条；这个值只影响
    /// 「LazyVStack 认为自己要显示多少」，不影响结论的方向。
    private let viewportHeight: CGFloat = 700

    /// 数一数这棵离屏视图树里到底有没有 `SelectionOverlay` 的真 `NSTextField`。
    ///
    /// 前任说「没有窗口时那些 NSView 根本不创建，所以离屏量不到 selection 的成本」——
    /// 那是推理，不是实测。这个探针把它变成事实：数出来是几，报告里就写几。
    private func countTextFields(in view: NSView) -> Int {
        var n = view is NSTextField ? 1 : 0
        for sub in view.subviews { n += countTextFields(in: sub) }
        return n
    }

    // MARK: - 无辜的三段（候选清单里被数据否掉的）

    func test_白板解码不是瓶颈() throws {
        try requireFixtures()
        let data = try Data(contentsOf: Self.fixtureDir.appendingPathComponent("whiteboard.json"))
        var entries: [FixtureEntry] = []
        let t = ms {
            for _ in 0 ..< 10 {
                entries = (try? JSONDecoder().decode([FixtureEntry].self, from: data)) ?? []
            }
        }
        // 不钉死条数 —— 这个群还在长（重取 fixture 时从 70 变 72 很正常）。只要求它
        // 确实是那份「一屏装不下」的真白板，别悄悄退化成三五条的玩具数据。
        XCTAssertGreaterThanOrEqual(entries.count, 50,
                                    "fixture 必须是那份真白板；太短就量不出打开成本")
        let each = t / 10
        print(String(format: "[#443] 白板 JSON 解码 %d 条：%.2f ms", entries.count, each))
        XCTAssertLessThan(each, 5, "解码从来不是瓶颈；真超了说明 fixture 或解码路变了")
    }

    func test_Markdown解析本身不是瓶颈() throws {
        // 候选清单里「Markdown 解析对 70 条全量同步做」这一条 —— cmark 解析 18,855 字
        // 只要 ~1ms，加解析缓存实测 0 收益。这个断言把这条结论钉住，免得下次又有人
        // 顺着直觉去做「markdown 缓存」。
        let texts = try loadEntries().compactMap(\.text).filter { !$0.isEmpty }
        warmUp()
        let t = ms { for s in texts { _ = MarkdownContent(s) } }
        print(String(format: "[#443] cmark 解析 %d 条（%d 字）：%.2f ms",
                     texts.count, texts.reduce(0) { $0 + $1.count }, t))
        XCTAssertLessThan(t, 20, "解析不是瓶颈 —— 别再往『缓存解析结果』的方向修")
    }

    func test_本地图解码不是瓶颈且已降采样() throws {
        let available = try fixtureImages()
        XCTAssertGreaterThanOrEqual(available.count, 5, "基线那 5 张真图应当都在")
        // 这个基线从建立起量的就是 5 张；crew 后续继续发图不该改变历史预算口径。
        let urls = Array(available.prefix(5))
        var total = 0.0
        for url in urls {
            var image: NSImage?
            total += ms { image = CrewLocalImageCache.decode(url: url, maxPixel: 330) }
            let decoded = try XCTUnwrap(image)
            XCTAssertLessThanOrEqual(max(decoded.size.width, decoded.size.height), 330,
                                     "缩略图必须按格子尺寸降采样，别拿 4000px 原图填 110pt")
        }
        print(String(format: "[#443] 5 张真图解码到 330px：%.1f ms（且在后台线程）", total))
        XCTAssertLessThan(total, 200, "图解码在后台，200ms 内都不该成为问题")

        // 看大图那条路必须仍是原图 —— 不许为了跑分把清晰度也砍了。
        let biggest = urls.max { (try? Data(contentsOf: $0).count) ?? 0 < (try? Data(contentsOf: $1).count) ?? 0 }!
        let full = try XCTUnwrap(CrewLocalImageCache.decode(url: biggest, maxPixel: nil))
        XCTAssertGreaterThan(max(full.size.width, full.size.height), 330,
                             "maxPixel = nil 必须给原图 —— 点开看大图不许糊")
    }

    // MARK: - 真正的大头：主线程渲染成本

    /// 这就是那条红线。窗口化之前它跑的是全部 70 条 ≈ 180ms，超预算。
    func test_打开LED驱动板一次重排在预算内() throws {
        let entries = try loadEntries()
        warmUp()

        let allTexts = entries.compactMap(\.text).filter { !$0.isEmpty }

        // 线上真正交给 ForEach 的那一段。
        let windowed = CrewChatWindow.window(entries, limit: CrewChatWindow.pageSize)
        let windowedTexts = windowed.compactMap(\.text).filter { !$0.isEmpty }

        let costAll = layoutCost(allTexts, selectable: true)                 // 修之前
        let costWindow = layoutCost(windowedTexts, selectable: true)         // 只上 B
        let costWindowNoSel = layoutCost(windowedTexts, selectable: false)   // B + D

        print("""

        ╔══ [#443] 打开「LED驱动板」的主线程渲染成本（真数据 70 条 / 5 图）
        ║ 修之前：全部 \(allTexts.count) 条 · 每条都挂 textSelection   \(String(format: "%7.1f ms", costAll))
        ║ 只上 B：窗口 \(windowedTexts.count) 条 · 每条都挂 textSelection   \(String(format: "%7.1f ms", costWindow))
        ║ B + D ：窗口 \(windowedTexts.count) 条 · 只有指针底下那条挂     \(String(format: "%7.1f ms", costWindowNoSel))
        ║ 预算  \(String(format: "%7.1f ms", budgetMs))
        ╚══ D 的收益里含 overlay 的**创建**成本（实测离屏确实建了 NSTextField，见探针），
            但不含真窗口里每次更新把全部 overlay 再跑一遍的 updateNSView —— 现场只会更贵

        """)

        XCTAssertLessThan(costWindowNoSel, budgetMs,
                          "一次重排必须在预算内 —— 打开一个 crew 要付这一下十几遍")
        XCTAssertLessThan(costWindow, costAll,
                          "渲染窗口必须真的少渲染（B）")
        XCTAssertLessThanOrEqual(costWindowNoSel, costWindow,
                                 "按需 textSelection 不该更贵（D）")
    }

    /// **现场那条 hang 栈的在体外复现**：把整条时间线放进一个 `LazyVStack`，
    /// 然后问它「你总共多高」—— `LazyVStack` 必须逐行 `sizeThatFits` 才能回答，
    /// 这正是 0.1.8 那份卡 7.03s 的报告里 12/12 采样所在的位置。
    ///
    /// 顺带钉住一个**反直觉但重要**的事实：只要没人问总高度（`.viewport`），
    /// `LazyVStack` 的惰性是生效的，成本与总条数无关。所以真正要治的不是
    /// 「列表长」，是「打开时有人反复问这张表的总高度」。窗口化（B）做的事是
    /// 给这个问题的代价**封顶**。
    func test_整表一次布局_窗口化前后() throws {
        let entries = try loadEntries()
        warmUp()

        let allTexts = entries.compactMap(\.text).filter { !$0.isEmpty }
        let windowedTexts = CrewChatWindow
            .window(entries, limit: CrewChatWindow.pageSize)
            .compactMap(\.text).filter { !$0.isEmpty }

        let fullBefore = timelineLayoutCost(allTexts, selectable: true, probe: .fullHeight)
        let fullAfterB = timelineLayoutCost(windowedTexts, selectable: true, probe: .fullHeight)
        let fullAfterBD = timelineLayoutCost(windowedTexts, selectable: false, probe: .fullHeight)

        let vpBefore = timelineLayoutCost(allTexts, selectable: true, probe: .viewport)
        let vpAfterB = timelineLayoutCost(windowedTexts, selectable: true, probe: .viewport)

        print("""

        ╔══ [#443] 整表一次布局（一个真 LazyVStack，不是把单行成本加起来）
        ║
        ║ ① 有人问「整张表多高」——现场那条 hang 栈就在这儿（逐行 sizeThatFits）
        ║    修之前 全部 \(allTexts.count) 条              \(String(format: "%7.1f ms", fullBefore))
        ║    只上 B 窗口 \(windowedTexts.count) 条              \(String(format: "%7.1f ms", fullAfterB))
        ║    B + D  窗口 \(windowedTexts.count) 条 · 按需选中     \(String(format: "%7.1f ms", fullAfterBD))
        ║
        ║ ② 没人问总高度、只布一屏——LazyVStack 的惰性生效，与总条数无关
        ║    全部 \(allTexts.count) 条                     \(String(format: "%7.1f ms", vpBefore))
        ║    窗口 \(windowedTexts.count) 条                     \(String(format: "%7.1f ms", vpAfterB))
        ╚══

        """)

        XCTAssertLessThan(fullAfterB, fullBefore,
                          "窗口化必须让「问总高度」这一下真的变便宜（B）")
        XCTAssertLessThanOrEqual(fullAfterBD, fullAfterB,
                                 "按需 textSelection 不该更贵（D 在 B 之上的增量）")
    }

    /// **把「离屏量不到 selection 成本」这句话从推理变成实测。**
    ///
    /// 前任（和我）都不能起 GUI，所以一直只能说「`SelectionOverlay` 的真 NSTextField
    /// 没有窗口就不创建，我们量到的是下限」。这条测试直接去数视图树里的 NSTextField
    /// 个数 —— 数出来多少，收尾报告里就写多少，不许含糊。
    func test_探针_离屏到底有没有创建SelectionOverlay的NSTextField() throws {
        let texts = Array(try loadEntries().compactMap(\.text).filter { !$0.isEmpty }.prefix(30))
        warmUp()

        let on = makeTimelineHost(texts, selectable: true, probe: .fullHeight)
        on.layoutSubtreeIfNeeded()
        _ = on.fittingSize
        let onCount = countTextFields(in: on)

        let off = makeTimelineHost(texts, selectable: false, probe: .fullHeight)
        off.layoutSubtreeIfNeeded()
        _ = off.fittingSize
        let offCount = countTextFields(in: off)

        print("""

        ╔══ [#443] 离屏视图树里的 NSTextField（= SelectionOverlay 的真实成本载体）
        ║ textSelection 常开 \(texts.count) 条：\(onCount) 个
        ║ 按需（都不挂）      \(texts.count) 条：\(offCount) 个
        ║ 建出来了 ⇒ 上一版「没有窗口就不创建、所以量不到」是**错的**，D 的收益是实测的。
        ║ 仍量不到的是重复那一维：真窗口里每次视图图更新都要把全部 overlay 再跑一遍。
        ╚══

        """)

        // 不断言个数 —— 它是**观测**，不是契约。断言只钉一件事：按需模式绝不会
        // 比常开更多。真变多了说明 D 接反了。
        XCTAssertLessThanOrEqual(offCount, onCount,
                                 "按需 textSelection 不该创建出更多 NSTextField")
    }

    /// 成本必须与「这个 crew 聊过多少条」脱钩 —— tech-debt #1621 说的那条
    /// 「随消息数线性增长」到此为止。
    ///
    /// 这里量的是**毫秒**不是条数：条数相等只是实现细节，真正要证的是「聊得越久
    /// 越卡」这件事被掐断了。本 crew（PendingCrew）已经 338 条，这条最要紧。
    func test_渲染成本与历史长度脱钩() throws {
        let entries = try loadEntries()
        warmUp()

        // 同一批真实消息重复 5 倍 ≈ 360 条，模拟「聊了很久的 crew」。
        let long = Array(repeating: entries, count: 5).flatMap { $0 }
        let longTexts = long.compactMap(\.text).filter { !$0.isEmpty }
        let shortTexts = entries.compactMap(\.text).filter { !$0.isEmpty }

        let beforeShort = timelineLayoutCost(shortTexts, selectable: true, probe: .fullHeight)
        let beforeLong = timelineLayoutCost(longTexts, selectable: true, probe: .fullHeight)

        let afterShort = timelineLayoutCost(
            CrewChatWindow.window(shortTexts, limit: CrewChatWindow.pageSize),
            selectable: false, probe: .fullHeight)
        let afterLong = timelineLayoutCost(
            CrewChatWindow.window(longTexts, limit: CrewChatWindow.pageSize),
            selectable: false, probe: .fullHeight)

        print("""

        ╔══ [#443] 「聊得越久越卡」是否被掐断（问总高度这一下）
        ║              \(shortTexts.count) 条        \(longTexts.count) 条
        ║ 修之前   \(String(format: "%7.1f ms", beforeShort))   \(String(format: "%7.1f ms", beforeLong))   ← 线性增长
        ║ B + D    \(String(format: "%7.1f ms", afterShort))   \(String(format: "%7.1f ms", afterLong))   ← 与长度无关
        ╚══

        """)

        XCTAssertEqual(
            CrewChatWindow.window(long, limit: CrewChatWindow.pageSize).count,
            CrewChatWindow.pageSize,
            "\(long.count) 条的 crew 和 \(entries.count) 条的 crew 必须渲染同样多行")
        XCTAssertLessThan(afterLong, beforeLong / 2,
                          "长 crew 的打开成本必须被窗口按住，不许跟着历史长")
        // 脱钩的定义：长 crew 与短 crew 的成本差不多（放宽到 2 倍容噪声）。
        XCTAssertLessThan(afterLong, max(afterShort, 1) * 2,
                          "窗口化之后，长 crew 不该比短 crew 更贵")
    }

    /// Todo #56 ①：渲染窗口封顶后改用 eager VStack，必须把真实高度先量出来，同时仍守住
    /// 一次重排 100ms 的既有预算。否则只是用卡顿换掉空白，不算修好。
    func test_窗口内12条eager布局仍在预算内() throws {
        let texts = try loadEntries().compactMap(\.text).filter { !$0.isEmpty }
        warmUp()
        let windowed = CrewChatWindow.window(texts, limit: CrewChatWindow.pageSize)
        // SwiftUI 首个离屏 host 偶尔会和系统 AppIntents/linkd 注册撞在一起；那是进程
        // 一次性噪声，不是列表布局。连续量三次取最小值，仍然每次都建新 host，守的是
        // 可复现的无争用成本，而不是碰巧被系统服务暂停了多久。
        let samples = (0 ..< 3).map { _ in
            timelineLayoutCost(windowed, selectable: false, probe: .fullHeight, eager: true)
        }
        let cost = try XCTUnwrap(samples.min())
        print(String(format: "[#56] %d 条 eager VStack 全高布局：%.1f ms（样本 %@）",
                     CrewChatWindow.pageSize, cost,
                     samples.map { String(format: "%.1f", $0) }.joined(separator: ", ")))
        XCTAssertLessThan(cost, budgetMs,
                          "eager measure 必须留在既有 100ms 预算内，不能用卡顿换空白")
    }
}
#endif
