import Foundation
import XCTest

/// 「驾驶舱打开和关闭都要很久」（人类 Todo #96）的回归。
///
/// ## 诊断结论（2026-09-01，读代码所得，不是量出来的）
///
/// 打开慢和关闭慢**是同一个病**，所以两个方向一样慢：
///
/// 1. 开关位 `showingCockpit` 挂在 `CrewSessionRunner`（2600+ 行）上当 `@Published`。
///    SwiftUI 的 `ObservableObject` 没有属性粒度 —— 翻这一个 bool 会给**每一个观察
///    这个 runner 的视图**发 `objectWillChange`：群聊 `CrewChatView`、终端与
///    transcript `CrewSessionWindowView`、侧栏每个 crew 一行 `CrewSidebarCrewRow`
///    （本机 42 个）……开一次发一次、关一次再发一次。
/// 2. `.animation(_:value:)` 挂在**包住整棵 `NavigationSplitView` 的那个 ZStack** 上。
///    它的作用域是整个子树，于是第 1 条那次全树失效被裹进一个 0.16s 的动画事务里
///    **逐帧重算**，而不是一次算完。
/// 3. 打开时还多发一次：`CockpitView.onAppear` 给 `cockpitSegmentRequest` 赋 nil，
///    而 `@Published` 不判等值。那个属性**全仓库没有任何地方读**。
/// 4. 打开路径在 MainActor 上同步走 `CockpitPlanStore.list` —— 里面是阻塞式
///    `flock(LOCK_EX)` + JSON 解码。
/// 5. `planChanges` 把整个白板目录的变更**不加过滤**并进来：别的 crew 发一条群消息，
///    驾驶舱就在主线程重读一遍自己的计划文件。
///
/// ## 这里量到什么、量不到什么（别含糊）
///
/// **量得到**：上面五条的**结构**是否还在 —— 谁被挂在谁身上、动画包住了谁、
/// 读账走哪条路、门控接没接上。这几条全是「只要形状对了，那次广播就不会发生」的
/// 硬结构，源码文本扫得出来，不依赖运行期、不依赖机器负载。
///
/// **量不到**：毫秒。本仓库已有的计时断言正在飘（见 `docs/tech-debt.md` 里
/// `CrewChatOpenCostTests` 那一族与 `LayoutLoopRegressionTests:245`），在功能全量里
/// 做计时测量本身就不稳；再加一条只会多一条飘的。**所以这里一个毫秒预算都不设，
/// 也不编造「快了几倍」** —— 本文件绿只证明「那几次全树广播的结构性来源没了」，
/// 不证明人的手感。手感要人自己开一次 app 看。
final class CockpitOpenCloseCostTests: XCTestCase {

    // MARK: - ① 开关位不许再挂在最热的那个对象上

    func testCockpitFlagIsNotPublishedOnTheHotSessionRunner() throws {
        let runner = Self.codeOnly(try Self.text(of: "CrewSessionRunner.swift"))
        XCTAssertFalse(
            runner.contains("showingCockpit"),
            """
            `showingCockpit` 还挂在 CrewSessionRunner 上 —— 群聊、终端、侧栏 42 行\
            都在观察这个对象，翻一下这个 bool 就把它们全部作废重算，开和关各一次。\
            它该待在自己的 CockpitPresentation 里。
            """)
    }

    func testNoViewReachesTheCockpitFlagThroughTheRunner() throws {
        let sources = try Self.sourceFiles()
        XCTAssertGreaterThan(sources.count, 50, "源码扫描没扫到东西，测试本身失效了")
        let offenders = sources
            .filter { Self.codeOnly($0.1).contains("showingCockpit") }
            .map { $0.0.lastPathComponent }
        XCTAssertTrue(
            offenders.isEmpty,
            "还有文件在用 showingCockpit（\(offenders.joined(separator: "、"))）—— 开关位没真拆干净")
    }

    /// 三栏那一层**保管**开关位，但不许**订阅**它。
    ///
    /// 这条单独立，是因为它是整修里唯一一处「改一个字就静默失效、编译器一句话不说」的
    /// 地方：`@State` 换成 `@StateObject` 看起来更"正确"（持有一个 ObservableObject 嘛），
    /// 实际上会让 `MacThreePaneView` 重新订阅这个开关位 —— 于是整棵三栏又回到失效范围里，
    /// #96 白修。
    func testThreePaneHoldsTheCockpitFlagWithoutSubscribingToIt() throws {
        let text = Self.codeOnly(try Self.text(of: "MacRootView.swift"))
        let threePane = try Self.topLevelBlock(named: "struct MacThreePaneView", in: text)
        XCTAssertTrue(
            threePane.contains("@State private var cockpit"),
            "MacThreePaneView 没有用 @State 保管 CockpitPresentation")
        for subscribing in ["@StateObject private var cockpit",
                            "@ObservedObject private var cockpit",
                            "@EnvironmentObject private var cockpit"] {
            XCTAssertFalse(
                threePane.contains(subscribing),
                """
                MacThreePaneView 用 \(subscribing) **订阅**了驾驶舱开关位 —— \
                那等于把整棵 NavigationSplitView 重新拉回失效范围，#96 修了个寂寞。\
                这一层只该「保管」它（@State 只存引用、不订阅），观察归 CockpitLayer。
                """)
        }
    }

    // MARK: - ③ 那个没人读的属性，连同打开时那次多余的二次广播

    func testTheUnreadCockpitSegmentRequestIsGone() throws {
        let sources = try Self.sourceFiles()
        let offenders = sources
            .filter { Self.codeOnly($0.1).contains("cockpitSegmentRequest") }
            .map { $0.0.lastPathComponent }
        XCTAssertTrue(
            offenders.isEmpty,
            """
            `cockpitSegmentRequest` 还在（\(offenders.joined(separator: "、"))）。\
            全仓库没有任何地方**读**它，它今天唯一的运行时效果就是 CockpitView.onAppear \
            给它赋一次 nil —— @Published 不判等值，于是打开驾驶舱要多付一整轮全树广播。
            """)
    }

    // MARK: - ② 动画只许包驾驶舱自己，不许挂在三栏祖先上

    func testCockpitAnimationDoesNotWrapTheThreePaneTree() throws {
        let text = Self.codeOnly(try Self.text(of: "MacRootView.swift"))
        let threePane = try Self.topLevelBlock(named: "struct MacThreePaneView", in: text)

        XCTAssertTrue(
            threePane.contains("NavigationSplitView("),
            "MacThreePaneView 里找不到 NavigationSplitView —— 这条测试的前提没了，先修测试")
        XCTAssertFalse(
            threePane.contains(".animation("),
            """
            `.animation(_:value:)` 还挂在 MacThreePaneView 里 —— 它的作用域是整个子树，\
            包括整棵 NavigationSplitView（侧栏 + 群聊 + 终端）。开关驾驶舱那一下的失效\
            会被裹进 0.16s 的动画事务里逐帧重算。动画该挂在只装驾驶舱的那一层上。
            """)
    }

    func testCockpitHasItsOwnAnimatedLayer() throws {
        let text = Self.codeOnly(try Self.text(of: "MacRootView.swift"))
        let layer = try Self.topLevelBlock(named: "struct CockpitLayer", in: text)
        XCTAssertTrue(
            layer.contains(".animation("),
            "CockpitLayer 里没有动画 —— 淡入淡出被整个删掉了，不是挪过来了")
        XCTAssertFalse(
            layer.contains("NavigationSplitView("),
            "CockpitLayer 里出现了 NavigationSplitView —— 那就等于动画又包住三栏了")
    }

    // MARK: - ④ 读账不许在主线程上等一把阻塞式跨进程锁

    func testCockpitPlanReadIsNotDoneOnTheMainActor() throws {
        let view = Self.codeOnly(try Self.text(of: "CockpitTasksView.swift"))
        let mind = try Self.topLevelBlock(named: "struct CockpitAgentMindView", in: view)
        XCTAssertFalse(
            mind.contains("CockpitPlanStore.shared.list("),
            """
            驾驶舱正文还在 `.task`（MainActor）里同步调 `CockpitPlanStore.shared.list` —— \
            那条路里是阻塞式 `flock(LOCK_EX)` + JSON 解码，任一 helper 正在写这个 crew 的 \
            .plan.lock，主线程就停在那儿等。该走 `listOffMain`。
            """)
        XCTAssertTrue(
            mind.contains("listOffMain("),
            "驾驶舱正文没有改走后台读（listOffMain）")
    }

    // MARK: - ⑤ 目录事件必须按本 crew 的 .plan.json 指纹门控

    func testPlanChangesUsesTheFingerprintGate() throws {
        let store = Self.codeOnly(try Self.text(of: "CockpitPlanStore.swift"))
        XCTAssertTrue(
            store.contains("FileChangeGateBox("),
            """
            `planChanges` 没有接指纹门控 —— 目录监听是**整个白板目录**的（本机 42 个 crew、\
            2000+ 个文件），不加过滤就意味着别人发一条群消息，驾驶舱就重读一遍自己的账。\
            白板那条流早就用 FileChangeGateBox 治过同一个病（PendingCrewBackend.whiteboardChanges），\
            照着接上，别再写第三套。
            """)
    }

    /// 跨进程目录事件的**行为**断言 —— 不是扫文本，是真订阅、真发事件、真数。
    ///
    /// 手动往 `directoryChanged` 打 tick（不靠 FSEvents），所以不看机器负载、没有计时。
    /// 计划文件一个字节没动 ⇒ 一次都不该 yield。
    func testUnrelatedDirectoryEventsDoNotWakeThePlanStream() async throws {
        let dir = try Self.temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = CockpitPlanStore(directory: dir)
        let crewId = "crew-unrelated"

        let yields = Counter()
        let stream = store.planChanges(crewId: crewId)
        let task = Task {
            for await _ in stream { yields.bump() }
        }
        defer { task.cancel() }

        // 让 AsyncStream 的两条 Combine 订阅真正建起来，再开始打 tick。
        try await Task.sleep(nanoseconds: 300_000_000)
        for _ in 0..<5 {
            LocalWhiteboardStore.shared.directoryChanged.send(())
            try await Task.sleep(nanoseconds: 80_000_000)
        }
        try await Task.sleep(nanoseconds: 200_000_000)

        XCTAssertEqual(
            yields.value, 0,
            """
            这个 crew 的 .plan.json 一个字节没动，却因为白板目录里别处有动静被唤醒了 \
            \(yields.value) 次 —— 每一次都是主线程上的一趟 flock + 整份 JSON 解码。
            """)
    }

    /// 反面：本 crew 的计划文件真变了，必须醒。门控不能把该刷的刷没了。
    func testOwnPlanFileChangeStillWakesThePlanStream() async throws {
        let dir = try Self.temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = CockpitPlanStore(directory: dir)
        let crewId = "crew-own"

        let yields = Counter()
        let stream = store.planChanges(crewId: crewId)
        let task = Task {
            for await _ in stream { yields.bump() }
        }
        defer { task.cancel() }
        try await Task.sleep(nanoseconds: 300_000_000)

        // 绕开本进程 `changes`（那条无条件推），直接改盘 —— 走的就是跨进程那条路。
        let file = dir.appendingPathComponent("\(crewId).plan.json")
        try Data("[]".utf8).write(to: file)

        var seen = 0
        for _ in 0..<20 {
            LocalWhiteboardStore.shared.directoryChanged.send(())
            try await Task.sleep(nanoseconds: 100_000_000)
            seen = yields.value
            if seen > 0 { break }
        }
        XCTAssertGreaterThan(
            seen, 0,
            "本 crew 的 .plan.json 变了却没唤醒 —— 门控把该刷的刷没了，驾驶舱会显示陈旧的计划")
    }

    // MARK: - 后台读回来的结果：迟到的要丢，别把上一个 crew 的账挂在新 crew 头上

    func testStaleLoadIsDroppedAfterCrewSwitch() {
        var feed = CockpitPlanFeed()
        XCTAssertTrue(feed.apply(rows: [Self.plan(1, "甲的活")], requested: "A", current: "A"))
        XCTAssertEqual(feed.plans(for: "A").count, 1)

        // 切到 B：A 那次读**迟到**了才回来 —— 不许采纳。
        XCTAssertFalse(
            feed.apply(rows: [Self.plan(9, "甲的旧活")], requested: "A", current: "B"),
            "切到 B 之后 A 的读还回来了，却被采纳了 —— 人会在 B 的标题下看到 A 的计划")
        XCTAssertEqual(feed.plans(for: "B"), [], "B 的账还没回来，此刻就该是空的")
    }

    func testNeverShowsAnotherCrewsPlans() {
        var feed = CockpitPlanFeed()
        _ = feed.apply(rows: [Self.plan(1, "甲的活")], requested: "A", current: "A")
        XCTAssertEqual(
            feed.plans(for: "B"), [],
            """
            切到 B 时把 A 的计划显示出来了。后台读把「读回来」变成异步之后，\
            这一格必须按 crew 认领，否则切群那一瞬人看到的是别人的作战板。
            """)
    }

    func testListOffMainReturnsTheSameRowsAsTheBlockingRead() async throws {
        let dir = try Self.temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = CockpitPlanStore(directory: dir)
        let crewId = "crew-parity"
        XCTAssertNotNil(store.add(crewId: crewId, title: "一"))
        XCTAssertNotNil(store.add(crewId: crewId, title: "二"))

        let blocking = store.list(crewId: crewId)
        let offMain = await store.listOffMain(crewId: crewId)
        XCTAssertEqual(blocking.map(\.number), offMain.map(\.number),
                       "后台读拿到的账跟阻塞读不一样 —— 换路子换出了内容差异")
        XCTAssertEqual(offMain.count, 2)
    }

    // MARK: - 入口按钮不许再经过 runner

    func testCockpitButtonWritesThroughThePresentationHandle() throws {
        let center = Self.codeOnly(try Self.text(of: "CrewCenterView.swift"))
        XCTAssertTrue(
            center.contains("cockpitPresentation"),
            """
            中栏 toolbar 那颗「驾驶舱」按钮没有改走 `@Environment(\\.cockpitPresentation)`。\
            走 `@EnvironmentObject` 会让中栏**订阅**这个开关位，等于把刚拆掉的广播\
            又接回来一半。
            """)
        XCTAssertFalse(
            center.contains("EnvironmentObject private var cockpit"),
            "驾驶舱开关位被中栏用 @EnvironmentObject 订阅了 —— 那就还是会因为开关而重算中栏")
    }

    // MARK: - 小工具

    /// 把注释剥掉，只留代码。
    ///
    /// **这道工序不是洁癖，是这把尺子能不能用的前提**：本文件全是「某个符号还在不在」
    /// 的断言，而搬走一个符号时**正该在原地留一句注释说明它去哪了、为什么别搬回来**。
    /// 不剥注释的话，那句注释本身会让断言红 —— 于是尺子在逼着人删掉最该留下的那段话。
    /// （同一个坑的另一面：文档里的反面例子别长成被检测的形状。）
    private static func codeOnly(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    private final class Counter: @unchecked Sendable {
        private let lock = NSLock()
        private var count = 0
        func bump() { lock.lock(); count += 1; lock.unlock() }
        var value: Int { lock.lock(); defer { lock.unlock() }; return count }
    }

    private static func plan(_ number: Int, _ title: String) -> CockpitPlanItem {
        CockpitPlanItem(
            id: UUID().uuidString, number: number, title: title,
            status: CockpitPlanStatus.notStarted.rawValue,
            createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z")
    }

    private static func temporaryDirectory() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cockpit-96-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    /// 取一个顶层类型的源码块（从 `struct X` 到下一个顶层声明为止）。
    /// 「这一段里有没有出现某个东西」比「谁在文件里排前面」结实得多。
    private static func topLevelBlock(named header: String, in text: String) throws -> String {
        guard let start = text.range(of: header) else {
            throw XCTSkip("源码里找不到 \(header) —— 这条测试的锚点没了，先修测试")
        }
        let rest = text[start.upperBound...]
        let terminators = ["\nstruct ", "\nprivate struct ", "\nfinal class ",
                           "\nprivate final class ", "\nenum ", "\nprivate enum ",
                           "\nextension ", "\n#endif"]
        let end = terminators
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
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else {
            throw XCTSkip("读不到源码目录 \(root.path)（不在开发机上跑）")
        }
        return walker.compactMap { any in
            guard let url = any as? URL, url.pathExtension == "swift",
                  let text = try? String(contentsOf: url, encoding: .utf8)
            else { return nil }
            return (url, text)
        }
    }
}
