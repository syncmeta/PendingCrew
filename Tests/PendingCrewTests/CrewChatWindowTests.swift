import XCTest

/// 渲染窗口的纯逻辑（#443 第三道闸）。**「不减功能」这条靠这里钉住** ——
/// 窗口只决定「一次往视图树里塞多少行」，往上翻必须真能翻到最早那一条。
final class CrewChatWindowTests: XCTestCase {

    private func msgs(_ n: Int) -> [Int] { Array(0 ..< n) }

    func test_只渲染最近一页且保持时间顺序() {
        let all = msgs(70)
        let w = CrewChatWindow.window(all, limit: CrewChatWindow.pageSize)
        XCTAssertEqual(w.count, CrewChatWindow.pageSize)
        XCTAssertEqual(w.first, all.count - CrewChatWindow.pageSize,
                       "取的是最近的那一段（末尾），不是开头")
        XCTAssertEqual(w.last, 69, "最新一条必须在窗口里 —— 打开就该看到最新")
        XCTAssertEqual(w, Array(w).sorted(), "顺序仍是旧→新")
    }

    func test_历史比一页短时全给不留占位() {
        let all = msgs(7)
        XCTAssertEqual(CrewChatWindow.window(all, limit: CrewChatWindow.pageSize), all)
        XCTAssertFalse(CrewChatWindow.hasMore(total: 7, limit: CrewChatWindow.pageSize))
        XCTAssertEqual(CrewChatWindow.remaining(total: 7, limit: CrewChatWindow.pageSize), 0)
    }

    func test_只有首屏eager_手点加载历史后恢复lazy() {
        XCTAssertTrue(CrewChatWindow.usesEagerInitialLayout(limit: CrewChatWindow.pageSize))
        XCTAssertFalse(CrewChatWindow.usesEagerInitialLayout(limit: CrewChatWindow.pageSize * 2))
    }

    func test_一页一页能翻到最早一条_不减功能() {
        let all = msgs(70)
        var limit = CrewChatWindow.pageSize
        var guardCount = 0
        while CrewChatWindow.hasMore(total: all.count, limit: limit) {
            limit = CrewChatWindow.expanded(limit, total: all.count)
            guardCount += 1
            XCTAssertLessThan(guardCount, 100, "翻页必须收敛，不许原地打转")
        }
        let w = CrewChatWindow.window(all, limit: limit)
        XCTAssertEqual(w, all, "翻到底必须是完整历史 —— 一条都不许丢")
        XCTAssertFalse(CrewChatWindow.hasMore(total: all.count, limit: limit),
                       "到顶后占位必须消失")
    }

    func test_占位上的数字是还没渲染的条数() {
        XCTAssertEqual(CrewChatWindow.remaining(total: 70, limit: 30), 40)
        XCTAssertEqual(CrewChatWindow.remaining(total: 70, limit: 65), 5)
        XCTAssertEqual(CrewChatWindow.remaining(total: 70, limit: 70), 0)
    }

    func test_越界的上限被夹回合法区间() {
        XCTAssertEqual(CrewChatWindow.clampedLimit(-5, total: 10), 0)
        XCTAssertEqual(CrewChatWindow.clampedLimit(999, total: 10), 10)
        XCTAssertEqual(CrewChatWindow.clampedLimit(30, total: 0), 0)
        XCTAssertEqual(CrewChatWindow.window(msgs(10), limit: 999), msgs(10))
        XCTAssertEqual(CrewChatWindow.expanded(999, total: 10), 10)
    }

    // MARK: - 新消息进来时，已经翻出来的不许消失

    func test_没翻过页时上限恒定_窗口跟着新消息往前滑() {
        // 默认状态：来 20 条新的，上限仍是一页 —— 封顶不许被聊天一路长回整表。
        XCTAssertEqual(
            CrewChatWindow.afterInsert(limit: CrewChatWindow.pageSize, added: 20),
            CrewChatWindow.pageSize)
    }

    func test_翻过页之后新消息把上限顶高_已露出的留在原地() throws {
        let expanded = CrewChatWindow.expanded(CrewChatWindow.pageSize, total: 200) // 60
        let after = CrewChatWindow.afterInsert(limit: expanded, added: 3)
        XCTAssertEqual(after, expanded + 3)

        // 真正要证的：翻出来的那条**还在**窗口里。
        let before = CrewChatWindow.window(msgs(200), limit: expanded)
        let oldest = try XCTUnwrap(before.first)
        // 追加 3 条新的（旧→新，所以接在末尾）。
        let grown = msgs(200) + [200, 201, 202]
        let now = CrewChatWindow.window(grown, limit: after)
        XCTAssertTrue(now.contains(oldest),
                      "刚翻出来的最老那条不许因为来了新消息就从上面消失")
        XCTAssertEqual(now.last, 202, "最新一条仍然在")
    }

    func test_没有新消息时上限不动() {
        XCTAssertEqual(CrewChatWindow.afterInsert(limit: 60, added: 0), 60)
        // 撤回/切 crew 让条目变少 → 负数，不许把上限改小（window 自己会夹）。
        XCTAssertEqual(CrewChatWindow.afterInsert(limit: 60, added: -5), 60)
    }

    func test_成本与历史长度脱钩() {
        // 同一个上限，crew 聊得再长，窗口取出来的行数不变 —— tech-debt #1621 那条
        // 「重排成本随消息数线性增长」就是被这一条掐掉的。
        for total in [30, 70, 338, 5_000] {
            XCTAssertEqual(
                CrewChatWindow.window(msgs(total), limit: CrewChatWindow.pageSize).count,
                min(total, CrewChatWindow.pageSize),
                "total=\(total)")
        }
    }

    // MARK: - Todo #60：加载更早要保持位置，不许甩到新那一页的开头

    func test_展开前的锚点是当前窗口最顶那条() throws {
        let all = msgs(70)
        let anchor = try XCTUnwrap(CrewChatWindow.anchorOnExpand(
            CrewChatWindow.window(all, limit: CrewChatWindow.pageSize),
            limit: CrewChatWindow.pageSize,
            isFollowing: false))
        XCTAssertEqual(anchor, 70 - CrewChatWindow.pageSize,
                       "锚的是人眼前那条 —— 展开前窗口的第一条")
    }

    func test_展开后锚点仍在窗口里_而且上面正好多了一页() throws {
        let all = msgs(70)
        let limit = CrewChatWindow.pageSize
        let before = CrewChatWindow.window(all, limit: limit)
        let anchor = try XCTUnwrap(
            CrewChatWindow.anchorOnExpand(before, limit: limit, isFollowing: false))

        let after = CrewChatWindow.window(all, limit: CrewChatWindow.expanded(limit, total: all.count))
        let idx = try XCTUnwrap(after.firstIndex(of: anchor))
        XCTAssertEqual(idx, CrewChatWindow.insertedAbove(total: all.count, limit: limit),
                       "锚点上面新插进来的正好是这一页 —— 把它钉回顶部，眼前的内容就不动")
        XCTAssertEqual(Array(after.suffix(from: idx)), before,
                       "锚点往下那一段一字不变 —— 展开只在上面加东西")
    }

    func test_最后一页不足一整页时锚点照样对齐() throws {
        // 70 条、已经放出 65 条：只剩 5 条可放，位移就是 5 不是 pageSize。
        let all = msgs(70)
        let limit = 65
        XCTAssertEqual(CrewChatWindow.insertedAbove(total: 70, limit: limit), 5)
        let before = CrewChatWindow.window(all, limit: limit)
        let anchor = try XCTUnwrap(
            CrewChatWindow.anchorOnExpand(before, limit: limit, isFollowing: false))
        let after = CrewChatWindow.window(all, limit: CrewChatWindow.expanded(limit, total: 70))
        XCTAssertEqual(after.firstIndex(of: anchor), 5)
        XCTAssertEqual(after, all, "这一下翻到底，占位随之消失")
    }

    func test_还跟着底部时不锚_别把人从底部拽到顶部() {
        // 历史短到「加载更早」和最新一条同屏时会走到这里：那时尺寸变化锚的是底部，
        // 视口本来就纹丝不动，再补一记 scrollTo 就是平白把人拽走，还要跟 landAtBottom
        // 抢同一拍。
        XCTAssertNil(CrewChatWindow.anchorOnExpand(
            CrewChatWindow.window(msgs(20), limit: CrewChatWindow.pageSize),
            limit: CrewChatWindow.pageSize,
            isFollowing: true))
    }

    func test_没东西可锚时返回nil_不崩() {
        XCTAssertNil(CrewChatWindow.anchorOnExpand([Int](), limit: CrewChatWindow.pageSize, isFollowing: false))
        XCTAssertNil(CrewChatWindow.anchorOnExpand(msgs(5), limit: 0, isFollowing: false))
        XCTAssertEqual(CrewChatWindow.insertedAbove(total: 0, limit: CrewChatWindow.pageSize), 0)
        XCTAssertEqual(CrewChatWindow.insertedAbove(total: 70, limit: 70), 0,
                       "已经到底，再点也没得插 —— 占位这时已经不在了")
    }

    func test_连点多次每一次的锚点都还在新窗口里() throws {
        // 第一次点击（12→24）与之后的点击是两种不同现场（容器身份翻面 vs 纯 lazy），
        // 但「锚哪条、锚点在新窗口的第几位」这条纯逻辑对两者一模一样。
        let all = msgs(70)
        var limit = CrewChatWindow.pageSize
        for step in 0 ..< 4 {
            let before = CrewChatWindow.window(all, limit: limit)
            let anchor = try XCTUnwrap(
                CrewChatWindow.anchorOnExpand(before, limit: limit, isFollowing: false),
                "第 \(step + 1) 次点击应当有锚点")
            let shift = CrewChatWindow.insertedAbove(total: all.count, limit: limit)
            limit = CrewChatWindow.expanded(limit, total: all.count)
            let after = CrewChatWindow.window(all, limit: limit)
            XCTAssertEqual(after.firstIndex(of: anchor), shift, "第 \(step + 1) 次点击")
        }
    }
}

#if os(macOS)
import AppKit
import SwiftUI

/// 「加载更早」那一下的**位置**探针（Todo #60）。
///
/// 上面那组钉的是纯逻辑（该锚哪一条）；这里钉另一半：**那一下到底有没有把位置按住**。
/// 离屏窗口里把那一下真的走一遍 —— 同一对 `defaultScrollAnchor`、同一个
/// `if VStack / else LazyVStack` 容器身份翻面、同一次「先改上限、再排一次主线程 hop」。
///
/// 量的是 `contentOffset.y` 与 `contentSize.height - contentOffset.y`（视口顶到内容底
/// 的距离）。选后者而不是「锚点那一行在视口里的 y」，是因为懒容器里没露脸的行根本不
/// 上报几何，量行会时有时无；而展开只在**上面**加东西、锚点以下的内容一字不变，所以
/// 「视口顶 → 内容底」的距离不变 ⇔ 眼前的内容没动。
///
/// 每个用例都配一次**不补偿的对照**：对照必须跳、补偿过的必须不跳 —— 否则「绿」只
/// 说明这个探针根本没测到东西。
///
/// ## 2026-08-26 返工：首尾一致 ≠ 中间没经过别的地方
///
/// 上面那三条全绿，人类装上更新后仍然退回来了，原话是「感觉像是又从上面滑下来的感觉，
/// **位置倒是一样**」。「位置一样」正是那三条钉住的东西 —— 它们量的是**起点和终点**，
/// 从来没量过**中间经过哪里**。所以本文件下半部分是路径探针，判据是**提交边界**
/// （见 `assertLandsInOneCommit`）。
///
/// 这不是一根新柴火：窗口从不 `orderFront`，跑完就撒手；没有定时器、没有反复重排。
@available(macOS 15.0, *)
final class CrewChatExpandAnchorProbeTests: XCTestCase {

    /// 驱动这一下的外部把手。
    private final class Rig: ObservableObject {
        @Published var limit = CrewChatWindow.pageSize
        @Published var ids: [String] = []
        /// 这一拍尺寸变化锚哪边（`.bottom` = 「未走的路」那一档）。
        @Published var expanding = false
        /// 容器身份：`.flipping` 是现状（`limit <= pageSize` 判据，12→24 那一下翻面）。
        @Published var container = Container.flipping
        /// C 档：连 `.sizeChanges` 那个锚都不挂 —— 「没有机制」的基准线。
        @Published var attachSizeChangesAnchor = true

        var scroll: ((String) -> Void)?
        /// 视口顶 → 内容底 的距离。
        var distanceFromBottom: CGFloat = .nan
        var offsetY: CGFloat = .nan
        var contentHeight: CGFloat = .nan

        // MARK: `.scrollPosition(id:)` 那一档

        /// 每次 run 之内是常量，所以条件修饰符不会改视图身份。
        var usesScrollPosition = false
        /// **故意不是 `@Published`。** `.scrollPosition(id:)` 会往绑定里回写「现在顶部
        /// 是哪一条」；写进被观察的属性就是**每次回写重算一次 body**，正是
        /// 2026-08-17「开久了卡」配方的另一半（第 3 条治的是 body 里做全量 IO）。
        /// 这里只当输出通道用：我们从不主动写它，所以它不需要被观察。
        var pinnedID: String?
        var pinnedWrites = 0
        /// 坏接法：把回写落进一个**被观察**的属性（`@State` / `@Published` 的等价物）。
        /// 它存在的唯一理由，是让下面那把尺子先在一个已知的坏例子上红一次（第 5 条）。
        var publishPinned = false
        @Published var pinnedPublished: String?

        // MARK: 计数器

        /// `Harness.body` 被求值了几次。探针里破例在 body 里记一笔 —— 要量的就是它自己。
        var bodyEvals = 0
        /// 滚动几何回调响了几次 —— 用来自检「SwiftUI 到底有没有察觉这次滚动」。
        var geoCallbacks = 0

        // MARK: 路径采样

        /// 每条采样的时间戳来源（路径采样时由驱动侧装上：距离点击多少毫秒）。
        var recording = false
        var stamp: (() -> Int)?
        var turn: Int { stamp?() ?? 0 }
        /// 几何回调序列（去掉连续重复）—— 「经过了哪些位置」。
        var trace: [Sample] = []
        /// **提交边界**序列 —— 「每一帧画的是哪个位置」。见 `commit` 的注释。
        var commits: [Sample] = []
        /// runloop 进入等待的次数。CoreAnimation 就是在这个点提交事务的，所以
        /// **一次 beforeWaiting ≈ 一帧**。毫秒在离屏窗口里不受刷新率约束（runloop
        /// 空转得比 60Hz 快得多），提交次数不受这个影响。
        var commit = 0

        struct Sample {
            var turn: Int
            var commit: Int
            var offset: CGFloat
            var content: CGFloat
            /// 视口顶 → 内容底。这一格不变 ⇔ 眼前的内容没动。
            var distance: CGFloat { content - offset }
        }

        private var sample: Sample? {
            guard !offsetY.isNaN, !contentHeight.isNaN else { return nil }
            return Sample(turn: turn, commit: commit, offset: offsetY, content: contentHeight)
        }

        /// `.scrollPosition(id:)` 的绑定 setter 走的就是这条路 —— 单测直接调它，
        /// 好把「回写一次多少钱」和「SwiftUI 回写几次」这两件事分开量。
        func writePinned(_ id: String?) {
            pinnedWrites += 1
            if publishPinned { pinnedPublished = id } else { pinnedID = id }
        }

        func record() {
            guard recording, let s = sample else { return }
            if let last = trace.last, last.offset == s.offset, last.content == s.content { return }
            trace.append(s)
        }

        /// 每次 runloop 进入等待时记一条 —— **不去重**，否则「这个位置停了几帧」就没了。
        func recordCommit() {
            guard recording, let s = sample else { return }
            commits.append(s)
        }
    }

    /// 「点一下加载更早」这一拍怎么保位置。
    private enum Fix {
        /// 什么都不做（修之前的行为）—— 对照组。
        case none
        /// 现状：`.top` 锚 + 一次主线程 hop 里 `scrollTo(anchor: .top)`。
        case scrollToHop
        /// 「未走的路」：这一拍把 `.sizeChanges` 的锚翻成 `.bottom`，不排 hop。
        case bottomAnchor
        /// 同上，但**提前一拍**把锚翻好，再改上限。用来分辨「这条路不行」和
        /// 「锚改得太晚、和尺寸变化撞在同一次更新里」。
        case bottomAnchorPreArmed
        /// 锚从一开始就是 `.bottom`。不是候选方案，是尺子的标定档。
        case bottomAnchorStatic
        /// 另一套原语：`.scrollPosition(id:anchor:)` 把「哪一条在顶部」做成绑定，
        /// 由 SwiftUI 在内容变化时自己维持它。**不排 hop、不 scrollTo。**
        case scrollPositionPin
    }

    /// 容器身份的三种取法 —— 「翻面」本身就是被怀疑的对象，所以做成可切换的。
    private enum Container { case flipping, alwaysLazy, alwaysEager }

    /// 内容往哪边长。`.above` 是「加载更早」；`.below` 是「来新消息」——
    /// 后者只用来标定那把锚。
    private enum Growth { case above, below }

    /// `onScrollGeometryChange` 要的 Equatable 载荷。分开存 offset 与 contentSize，
    /// 不预先相减 —— 「内容在上面长了多少」和「视口被拽了多少」是两件事。
    private struct Geo: Equatable {
        var offset: CGFloat
        var content: CGFloat
    }

    /// `.sizeChanges` 那个锚挂不挂。`on` 每次 run 内是常量，不会改视图身份。
    @available(macOS 15.0, *)
    private struct SizeChangesAnchor: ViewModifier {
        let on: Bool
        let anchor: UnitPoint
        @ViewBuilder func body(content: Content) -> some View {
            if on { content.defaultScrollAnchor(anchor, for: .sizeChanges) } else { content }
        }
    }

    /// `.scrollTargetLayout()` 只在 scrollPosition 那一档挂上（它是 `scrollPosition(id:)`
    /// 的前提：没有它，SwiftUI 不知道该拿哪一层的子视图当目标）。
    private struct TargetLayout: ViewModifier {
        let on: Bool
        @ViewBuilder func body(content: Content) -> some View {
            if on { content.scrollTargetLayout() } else { content }
        }
    }

    @available(macOS 15.0, *)
    private struct Harness: View {
        @ObservedObject var rig: Rig

        /// 行高故意不等（真群聊也不等），但对同一个 id 恒定 —— 否则两次量的是两张表。
        private func height(_ id: String) -> CGFloat {
            30 + CGFloat((Int(id.dropFirst()) ?? 0) % 4) * 18
        }

        /// 现状的判据只在 `.flipping` 下用；另两种是为了把「翻面」单独关掉再量。
        private var eager: Bool {
            switch rig.container {
            case .flipping: return CrewChatWindow.usesEagerInitialLayout(limit: rig.limit)
            case .alwaysLazy: return false
            case .alwaysEager: return true
            }
        }

        private var pinned: Binding<String?> {
            Binding(get: { rig.publishPinned ? rig.pinnedPublished : rig.pinnedID },
                    set: { rig.writePinned($0) })
        }

        var body: some View {
            rig.bodyEvals += 1
            return content
        }

        @ViewBuilder private var content: some View {
            if rig.usesScrollPosition {
                scroll.scrollPosition(id: pinned, anchor: .top)
            } else {
                scroll
            }
        }

        private var scroll: some View {
            ScrollViewReader { proxy in
                ScrollView {
                    Group {
                        if eager {
                            VStack(alignment: .leading, spacing: 0) { rows }
                                .modifier(TargetLayout(on: rig.usesScrollPosition))
                        } else {
                            LazyVStack(alignment: .leading, spacing: 0) { rows }
                                .modifier(TargetLayout(on: rig.usesScrollPosition))
                        }
                    }
                    .padding(.vertical, 10)
                }
                // 与 CrewChatView.ChatScrollAnchor 一字不差（isFollowing = false，
                // 也就是「人自己滑上去看历史」那个现场）。
                .defaultScrollAnchor(.bottom, for: .initialOffset)
                .modifier(SizeChangesAnchor(on: rig.attachSizeChangesAnchor,
                                            anchor: rig.expanding ? .bottom : .top))
                .onScrollGeometryChange(for: Geo.self) { geo in
                    Geo(offset: geo.contentOffset.y, content: geo.contentSize.height)
                } action: { _, v in
                    rig.offsetY = v.offset
                    rig.contentHeight = v.content
                    rig.distanceFromBottom = v.content - v.offset
                    rig.geoCallbacks += 1
                    rig.record()
                }
                .onAppear { rig.scroll = { id in proxy.scrollTo(id, anchor: .top) } }
            }
            .frame(width: 420, height: 500)
        }

        @ViewBuilder private var rows: some View {
            if CrewChatWindow.hasMore(total: rig.ids.count, limit: rig.limit) {
                // 顶部那条「加载更早」占位（高度取真实那条的量级）。
                Color.gray.opacity(0.2).frame(height: 30).padding(.vertical, 8)
            }
            ForEach(CrewChatWindow.window(rig.ids, limit: rig.limit), id: \.self) { id in
                Color.blue.opacity(0.15).frame(height: height(id)).id(id)
            }
        }
    }

    private func ids(_ n: Int) -> [String] { (0 ..< n).map { "m\($0)" } }

    /// 一个**不显示**的窗口：SwiftUI 的 ScrollView 要挂在窗口上才会真的滚
    /// （只挂离屏 `NSHostingView` 时 `scrollTo` 的落点时有时无，实测过）。
    /// 从不 `orderFront` / `makeKey`，所以屏幕上什么都不会冒出来。
    private func hostInWindow(_ view: some View) -> NSWindow {
        let host = NSHostingView(rootView: AnyView(view))
        host.frame = CGRect(x: 0, y: 0, width: 420, height: 500)
        let win = NSWindow(
            contentRect: host.frame, styleMask: [.borderless],
            backing: .buffered, defer: false)
        win.isReleasedWhenClosed = false
        win.contentView = host
        win.layoutIfNeeded()
        return win
    }

    /// 转主线程直到**滚动几何连续 25 拍不再变**（约 0.25s 静止），最多等 4s。
    ///
    /// 不写成「固定转 0.4 秒」：那样的等待时长与机器负载耦合，整套 1400+ 用例一起跑时
    /// 这一拍可能没转够，用例就成了看运气的红。等「不再动」是它自己的收敛条件。
    private func settle(_ win: NSWindow, timeout: TimeInterval = 4.0) {
        let rig = self.currentRig
        var last = CGPoint(x: CGFloat.nan, y: CGFloat.nan)
        var stable = 0
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.01))
            win.layoutIfNeeded()
            win.contentView?.displayIfNeeded()
            let now = CGPoint(x: rig?.offsetY ?? CGFloat.nan,
                              y: rig?.distanceFromBottom ?? CGFloat.nan)
            if now.x.isNaN || now.y.isNaN {
                stable = 0
            } else if now == last {
                stable += 1
                if stable >= 25 { return }
            } else {
                stable = 0
            }
            last = now
        }
    }

    /// `settle` 要读的那个 rig（每个驱动函数开头挂上）。
    private var currentRig: Rig?

    // MARK: - 首尾探针（原有）

    private struct Measure {
        var beforeDistance: CGFloat, afterDistance: CGFloat
        var beforeOffset: CGFloat, afterOffset: CGFloat
        var shift: CGFloat { abs(afterDistance - beforeDistance) }
    }

    /// 跑一次「点加载更早」。
    /// - compensate: 走不走那一记 scrollTo（false = 修之前的行为，对照组）。
    /// - insertDuring: 点下去的同一拍有没有来一条新消息（末尾追加，走 afterInsert）。
    private func run(total: Int, startLimit: Int, compensate: Bool,
                     insertDuring: Bool = false, label: String) -> Measure {
        let rig = Rig()
        rig.ids = ids(total)
        rig.limit = startLimit
        let anchorID = CrewChatWindow.anchorOnExpand(
            CrewChatWindow.window(rig.ids, limit: startLimit),
            limit: startLimit, isFollowing: false)!
        currentRig = rig
        let win = hostInWindow(Harness(rig: rig))
        settle(win)
        // 人自己滑到顶（看得见「加载更早」那条）。
        rig.scroll?(anchorID)
        settle(win)
        let m0 = (rig.distanceFromBottom, rig.offsetY)

        // 点一下：先改上限，再（视 compensate）排一次主线程 hop 把锚点钉回顶部。
        rig.limit = CrewChatWindow.expanded(rig.limit, total: rig.ids.count)
        if compensate { DispatchQueue.main.async { rig.scroll?(anchorID) } }
        if insertDuring {
            // 同一拍来一条新消息，走 CrewChatView.onChange(count) 里那两句。
            rig.ids.append("new")
            rig.limit = CrewChatWindow.afterInsert(limit: rig.limit, added: 1)
        }
        settle(win)
        let m = Measure(beforeDistance: m0.0, afterDistance: rig.distanceFromBottom,
                        beforeOffset: m0.1, afterOffset: rig.offsetY)
        win.contentView = nil
        print("[#60 probe] \(label): 视口顶→内容底 \(m.beforeDistance) → \(m.afterDistance)"
              + "（位移 \(m.shift)）；contentOffset \(m.beforeOffset) → \(m.afterOffset)")
        return m
    }

    /// 12 → 24：`usesEagerInitialLayout` 判据翻面，`if VStack / else LazyVStack` 换容器、
    /// 整棵内容树重建 —— 任何靠 anchor 保位置的做法都救不了这一下。
    func test_第一次点击_容器身份翻面那一下也按得住() {
        let ctrl = run(total: 70, startLimit: CrewChatWindow.pageSize,
                       compensate: false, label: "第一次点击·对照（不补偿）")
        XCTAssertLessThan(ctrl.beforeOffset, 120,
                          "起点必须是「人滑到顶」，实际 contentOffset=\(ctrl.beforeOffset)")
        XCTAssertGreaterThan(ctrl.shift, 200,
                             "对照组必须跳 —— 不跳说明探针没测到东西")

        let fixed = run(total: 70, startLimit: CrewChatWindow.pageSize,
                        compensate: true, label: "第一次点击·补偿后")
        XCTAssertLessThan(fixed.shift, 80, "补偿过的必须基本不动")
    }

    /// 24 → 36：一直在 `LazyVStack` 里，没有身份翻面，但上面那些行是懒的。
    func test_第二次点击_纯LazyVStack里也按得住() {
        let start = CrewChatWindow.expanded(CrewChatWindow.pageSize, total: 70)   // 24
        let ctrl = run(total: 70, startLimit: start,
                       compensate: false, label: "第二次点击·对照（不补偿）")
        XCTAssertLessThan(ctrl.beforeOffset, 120,
                          "起点必须是「人滑到顶」，实际 contentOffset=\(ctrl.beforeOffset)")
        XCTAssertGreaterThan(ctrl.shift, 200, "对照组必须跳")

        let fixed = run(total: 70, startLimit: start, compensate: true,
                        label: "第二次点击·补偿后")
        XCTAssertLessThan(fixed.shift, 80, "补偿过的必须基本不动")
    }

    /// 展开的同一拍正好来一条新消息 —— 两件事必须同时成立：锚点还按得住，且新消息
    /// （在**下面**长）不许把视口顶上去（Todo #47 行为 3）。
    func test_展开的同一拍来新消息_锚点仍按得住且视口不被新消息顶走() {
        let quiet = run(total: 70, startLimit: CrewChatWindow.pageSize, compensate: true,
                        label: "展开·没来新消息（基线）")
        XCTAssertLessThan(quiet.shift, 80, "基线这一趟本身要先站得住")

        let busy = run(total: 70, startLimit: CrewChatWindow.pageSize, compensate: true,
                       insertDuring: true, label: "展开的同一拍来新消息")
        XCTAssertLessThan(
            abs(busy.afterOffset - quiet.afterOffset), 20,
            "视口不许被下面新长出来的那条顶走：有新消息 \(busy.afterOffset) vs 没有 \(quiet.afterOffset)")
    }

    // MARK: - 路径（Todo #60 返工：首尾一致 ≠ 中间没经过别的地方）

    /// 一次**不加干预**的路径采样。
    ///
    /// 上面那三条量的是「点之前」和「静止之后」两个点。人类验收没过，抱怨的是
    /// 「感觉像是又从上面滑下来的感觉，位置倒是一样」—— **位置一样**正是那三条钉住的
    /// 东西，所以它们全绿并不能说明这条修好了。要回答的是另一个问题：
    /// **中间经过了哪些位置、每个位置画了几帧。**
    ///
    /// 所以这里**什么都不强制**：不 `layoutIfNeeded`、不 `displayIfNeeded`、不逐帧驱动，
    /// 只让 runloop 自己转。强制 layout 会改变提交时机，量出来的就不是这条路本身了。
    private func tracePath(total: Int, startLimit: Int, fix: Fix,
                           container: Container = .flipping,
                           growth: Growth = .above, startAtTop: Bool = true,
                           attachAnchor: Bool = true,
                           label: String, settleFor: TimeInterval = 1.0) -> [Rig.Sample] {
        let rig = Rig()
        rig.ids = ids(total)
        rig.limit = startLimit
        rig.container = container
        rig.usesScrollPosition = (fix == .scrollPositionPin)
        rig.attachSizeChangesAnchor = attachAnchor
        if fix == .bottomAnchorStatic { rig.expanding = true }
        let anchorID = CrewChatWindow.anchorOnExpand(
            CrewChatWindow.window(rig.ids, limit: startLimit),
            limit: startLimit, isFollowing: false)!
        currentRig = rig
        let win = hostInWindow(Harness(rig: rig))
        settle(win)
        if startAtTop {
            rig.scroll?(anchorID)  // 人自己滑到顶
            settle(win)
        }                          // 否则就停在 `.initialOffset` 放下的地方：底部

        // 备好那两档的前置状态，**并且在开始录之前把这一拍走完**。
        //
        // 这里踩过一次坑，留着当例证：第一版把预热放在开始录**之后**，于是「第一帧」量到
        // 的是预热那一拍 —— 那一拍什么尺寸都没变，当然偏 0pt。数字漂亮得可疑，而它量的
        // 根本不是展开那一下。**尺子被自己的驱动步骤污染了**，跟毫秒那次是同一类错。
        if fix == .bottomAnchorPreArmed {
            rig.expanding = true
            settle(win)
        }
        if fix == .scrollPositionPin {
            rig.pinnedID = anchorID
            settle(win)
        }

        // 从这里开始只观察，不干预。turn 存的是「距离点击多少毫秒」。
        let t0 = CFAbsoluteTimeGetCurrent()
        rig.stamp = { Int(((CFAbsoluteTimeGetCurrent() - t0) * 1000).rounded()) }

        // 数提交边界：CoreAnimation 在 runloop 进入等待时提交事务，所以这里每响一次
        // ≈ 屏幕上换一次内容。装在 `.beforeWaiting` 上，只读不写，不改变任何时机。
        let observer = CFRunLoopObserverCreateWithHandler(
            kCFAllocatorDefault, CFRunLoopActivity.beforeWaiting.rawValue, true, 0
        ) { [weak rig] _, _ in
            guard let rig, rig.recording else { return }
            rig.commit += 1
            rig.recordCommit()
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .defaultMode)
        defer { CFRunLoopRemoveObserver(CFRunLoopGetMain(), observer, .defaultMode) }

        rig.recording = true
        rig.record()               // 起点

        if growth == .below {
            // 标定那一路：内容往**下**长（来了一页新消息）。
            rig.limit += CrewChatWindow.pageSize
            rig.ids += (0 ..< CrewChatWindow.pageSize).map { "n\($0)" }
        } else {
            // 点下去那一拍。各档的区别全在这几行里：
            switch fix {
            case .none:
                rig.limit = CrewChatWindow.expanded(rig.limit, total: rig.ids.count)
            case .scrollToHop:
                // 现状：先改上限，再排一次主线程 hop 把锚点钉回顶部。
                rig.limit = CrewChatWindow.expanded(rig.limit, total: rig.ids.count)
                DispatchQueue.main.async { rig.scroll?(anchorID) }
            case .bottomAnchor:
                // 「未走的路」：**同一次 body 更新里**把尺寸变化的锚翻成底部，再改上限。
                rig.expanding = true
                rig.limit = CrewChatWindow.expanded(rig.limit, total: rig.ids.count)
            case .bottomAnchorPreArmed, .bottomAnchorStatic, .scrollPositionPin:
                // 锚 / 绑定已经在上面备好了，这里只改上限 —— 一次 body 更新，没有 hop。
                rig.limit = CrewChatWindow.expanded(rig.limit, total: rig.ids.count)
            }
        }

        let deadline = Date().addingTimeInterval(settleFor)
        while Date() < deadline {
            RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.005))
        }
        rig.recording = false
        rig.expanding = false
        win.contentView = nil

        let path = rig.trace
        if growth == .below {
            print(String(format: "[#60 锚标定] %@：视口顶→内容底 %.0f → %.0f（位移 %+.0f）",
                         label, path.first!.distance, path.last!.distance,
                         path.last!.distance - path.first!.distance))
            return path
        }
        print("[#60 path] \(label) —— 经过 \(path.count) 个不同几何：")
        for s in path {
            print(String(format: "  +%4dms 第%3d 次提交  offset=%8.2f  content=%8.2f  视口顶→内容底=%8.2f",
                         s.turn, s.commit, s.offset, s.content, s.distance))
        }
        print("  —— 提交边界（一次 ≈ 一帧）：")
        for s in rig.commits.prefix(4) {
            print(String(format: "     第%3d 次提交 (+%4dms)  视口顶→内容底=%8.2f",
                         s.commit, s.turn, s.distance))
        }
        return path
    }

    /// 提交边界序列（跑完 `tracePath` 后可读）。
    private var lastCommits: [Rig.Sample] { currentRig?.commits ?? [] }

    /// **这一条才是人类抱怨的那件事。** 上面几条钉终点，这条钉路径。
    ///
    /// 判据：**起点之后的第一次提交，画的就必须已经是终点位置。** 换句话说，
    /// 「新的一页插进来」和「视口跟上去」必须落在**同一帧**里。
    ///
    /// 为什么不拿毫秒当判据 —— 这把尺子换过一次，换的理由要留下来：第一版判据写的是
    /// 「一个中间几何停留超过一帧（≈17ms）才算看得见」，拿现状喂进去**它是绿的**
    /// （中间位置只停 4ms / 2ms）。可现状恰恰是人类退回来的那一版。根因是**离屏窗口
    /// 的 runloop 空转比 60Hz 快得多，毫秒数根本不受刷新率约束** —— 那把尺子从原理上
    /// 就看不见「画了几帧」。能数的只有 CoreAnimation 的提交次数。
    /// （CONTRIBUTING 第 5 条：一把在已知坏例子上红不出来的尺子，它的绿是无内容的。）
    private func assertLandsInOneCommit(_ path: [Rig.Sample], commits: [Rig.Sample], label: String) {
        guard let start = path.first, let end = path.last else {
            return XCTFail("\(label)：一条采样都没有，探针没接上")
        }
        XCTAssertLessThan(abs(end.distance - start.distance), 80,
                          "\(label)：终点本身要先站得住")
        guard let firstCommit = commits.first else {
            return XCTFail("\(label)：一次提交都没数到，提交观察器没接上")
        }
        XCTAssertLessThan(
            abs(firstCommit.distance - start.distance), 80,
            "\(label)：起点之后的**第一次提交**画的不是终点位置 —— "
            + String(format: "偏 %.0fpt（起点 %.0f → 第一帧 %.0f → 终点 %.0f）。"
                     + "人眼看到的第一帧就是没修之前那一帧。",
                     firstCommit.distance - start.distance,
                     start.distance, firstCommit.distance, end.distance))
    }

    func test_第一次点击_插入与跟上必须落在同一帧() {
        let path = tracePath(total: 70, startLimit: CrewChatWindow.pageSize,
                             fix: .scrollPositionPin, container: .alwaysLazy,
                             label: "第一次点击·路径")
        assertLandsInOneCommit(path, commits: lastCommits, label: "第一次点击")
    }

    func test_第二次点击_插入与跟上必须落在同一帧() {
        let start = CrewChatWindow.expanded(CrewChatWindow.pageSize, total: 70)   // 24
        let path = tracePath(total: 70, startLimit: start,
                             fix: .scrollPositionPin, container: .alwaysLazy,
                             label: "第二次点击·路径")
        assertLandsInOneCommit(path, commits: lastCommits, label: "第二次点击")
    }

    /// **把改动退回旧做法，这把尺子必须重新红。**（第 5 条，也是机长点名要明说的一步。）
    ///
    /// 退回去的那一档就是人类退货的那一版：`scrollTo` + 一次主线程 hop。已知坏值 1374 ——
    /// 起点 694，第一次提交画在 1374，偏 680pt，和「什么都不做」那一趟的第一次提交同值。
    func test_尺子自检_退回旧做法必须红() {
        let path = tracePath(total: 70, startLimit: CrewChatWindow.pageSize,
                             fix: .scrollToHop, label: "退回旧做法（scrollTo + hop）")
        let start = path.first!.distance
        let firstCommit = lastCommits.first!.distance
        XCTAssertGreaterThan(
            abs(firstCommit - start), 200,
            "旧做法没被这把尺子抓住（起点 \(start) → 第一帧 \(firstCommit)）—— "
            + "抓不住已知的坏例子，它给新做法的绿就不构成证据")
    }

    /// 对照：什么都不做那一趟的**终点**必须就是错的 —— 否则这个探针根本没测到东西。
    func test_路径探针本身有效_什么都不做那趟必须停在错的位置上() {
        let path = tracePath(total: 70, startLimit: CrewChatWindow.pageSize,
                             fix: .none, label: "什么都不做（对照）")
        XCTAssertGreaterThan(abs(path.last!.distance - path.first!.distance), 200,
                             "对照组的终点必须就是错的")
    }

    // MARK: - 那把锚的标定与「未走的路」的死因

    /// **标定（第 5 条）：先证明这把尺子分得出 `.bottom` 和 `.top`，再信它的「无效」。**
    ///
    /// 换一个**已知锚必须起作用**的方向来喂它：视口贴着底部、内容往**下**长。
    /// 那个方向上 `.top`（视口不动）和 `.bottom`（跟着底部走）必须给出不同的数。
    func test_锚标定_贴底时必须分得出bottom和top() {
        let top = tracePath(total: 40, startLimit: 24, fix: .none, container: .alwaysLazy,
                            growth: .below, startAtTop: false, label: "贴底·内容往下长·锚 .top")
        let bottom = tracePath(total: 40, startLimit: 24, fix: .bottomAnchorStatic,
                               container: .alwaysLazy, growth: .below, startAtTop: false,
                               label: "贴底·内容往下长·锚 .bottom")
        let topShift = top.last!.distance - top.first!.distance
        let bottomShift = bottom.last!.distance - bottom.first!.distance
        XCTAssertGreaterThan(
            abs(topShift - bottomShift), 50,
            "这把尺子分不出 .top 和 .bottom（.top 位移 \(topShift)，.bottom 位移 \(bottomShift)）"
            + " —— 分不出的话，「两个锚值给出同一个数」就什么都不能证明")
    }

    /// **「未走的路」的死因，钉成一条用例。**
    ///
    /// tech-debt 里写着「内容在上面长、锚底部 = 视口一像素不动，原生机制」。**实测不成立。**
    /// `.bottom` 的语义是「把视口钉在内容底部」，不是「保持与底部的相对距离」——
    /// 它只在视口已经贴底时做事。而「加载更早」的现场按定义就是人已经滑上去了
    /// （`anchorOnExpand` 在跟随时返回 nil），正是 `.bottom` 不响的那个现场。
    ///
    /// 上面那条标定用例保证了这条不是「尺子没量到锚」。
    func test_未走的路_人滑上去之后bottom锚与top锚读数相同() {
        let withBottom = tracePath(total: 70, startLimit: CrewChatWindow.pageSize,
                                   fix: .bottomAnchorStatic, container: .alwaysLazy,
                                   label: "加载更早·锚 .bottom")
        let withTop = tracePath(total: 70, startLimit: CrewChatWindow.pageSize,
                                fix: .none, container: .alwaysLazy,
                                label: "加载更早·锚 .top（只差锚值一个字）")
        let a = withBottom.last!.distance - withBottom.first!.distance
        let b = withTop.last!.distance - withTop.first!.distance
        XCTAssertLessThan(
            abs(a - b), 20,
            "锚 .bottom 在这个现场居然有作用了（.bottom 位移 \(a)，.top 位移 \(b)）—— "
            + "那 tech-debt 那条的死因判断要重看")
        XCTAssertGreaterThan(abs(a), 200, "两档都得是错的位置，否则这条用例没意义")
    }

    // MARK: - `.scrollPosition(id:)` 的代价

    /// **回写一次要付多少钱。**
    ///
    /// 先说清楚这条**没能**量到什么，别让它的绿说过头：**「人拿手拖的时候 SwiftUI 会
    /// 回写几次」，在离屏窗口里量不出来。** 两种驱动都试过，都不成立 ——
    ///
    /// - 分多拍直接改 clip view 的原点：SwiftUI 的几何回调响了 **89 次**（它确实察觉了
    ///   这次滚动），但 `.scrollPosition` 的绑定**一次都没回写**；
    /// - 造 `scrollWheel` 事件直接调那个 scroll view 的 `scrollWheel(with:)`：几何回调
    ///   **0 次** —— 事件没有窗口上下文，SwiftUI 整个没理它。
    ///
    /// 而 GUI 自动化是禁的（会以 PendingCrew 的名义弹系统权限框），所以「真拖一次」这
    /// 条路在自动化测试里走不通。拿上面任何一个 0 去证「真滚动不吵」，都是用一个**从来
    /// 没发生过被测那件事**的用例给出绿 —— CONTRIBUTING 第 5 条实例二那个形状。
    ///
    /// **所以这条改问一个能答、而且足够的问题：回写一次，要付几次 body 求值？**
    /// 频率量不到没关系 —— **单价是 0 的话，频率乘上去还是 0。** 而单价是接法决定的，
    /// 不是 SwiftUI 决定的：绑定落进被观察的属性（`@State` / `@Published`）就是每次
    /// 回写重算一次 body（2026-08-17「开久了卡」配方的另一半）；落进一个不被观察的
    /// 盒子就是 0 次。坏接法在场就是为了它**必须**红。
    func test_scrollPosition的回写不许把body打成每帧重算() {
        func cost(publishPinned: Bool, _ label: String) -> Int {
            let rig = Rig()
            rig.ids = ids(70); rig.limit = 36; rig.container = .alwaysLazy
            rig.usesScrollPosition = true; rig.publishPinned = publishPinned
            currentRig = rig
            let win = hostInWindow(Harness(rig: rig))
            settle(win)
            rig.bodyEvals = 0
            // 直接走 `.scrollPosition(id:)` 那个绑定的 setter —— 也就是 SwiftUI 回写时
            // 走的同一条路（`Harness.pinned` 的 set 就是调它），只是由我们来定次数。
            for i in 0 ..< 30 {
                rig.writePinned("m\(i)")
                RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.004))
                win.layoutIfNeeded()
            }
            let evals = rig.bodyEvals
            win.contentView = nil
            print("[#60 回写单价] \(label)：回写 30 次 → body 求值 \(evals) 次")
            return evals
        }
        let bad = cost(publishPinned: true, "坏接法：绑定落进被观察属性（@State 等价物）")
        let good = cost(publishPinned: false, "本次接法：绑定落进不被观察的盒子")

        // ① 先证明这把尺子看得见「被打成每帧重算」。
        XCTAssertGreaterThanOrEqual(
            bad, 25,
            "坏接法没把 body 打爆（只数到 \(bad) 次）—— 这把尺子数不到 body 求值，"
            + "那它给出的任何「0 次」都不构成证据")
        // ② 再拿本次接法去绿。
        XCTAssertEqual(good, 0, "本次接法的回写把 body 打到了重算（\(good) 次）")
    }

    // MARK: - 比选表（只打印，不断言：把选型从论证变成一张表）

    func test_修法比选_各档第一帧画在哪() {
        var rows: [String] = []
        func measure(_ fix: Fix, _ container: Container, _ name: String) {
            let path = tracePath(total: 70, startLimit: CrewChatWindow.pageSize,
                                 fix: fix, container: container, label: name)
            let start = path.first!.distance
            let first = lastCommits.first?.distance ?? .nan
            let end = path.last!.distance
            rows.append(String(format: "  %@\n     起点 %.0f → 第一帧 %.0f（偏 %+.0f）→ 终点 %.0f（偏 %+.0f）",
                               name, start, first, first - start, end, end - start))
        }
        measure(.none, .flipping, "①什么都不做·翻面（对照）")
        measure(.scrollToHop, .flipping, "②现状：scrollTo+hop·翻面")
        measure(.bottomAnchor, .flipping, "③锚 .bottom·翻面还在")
        measure(.bottomAnchor, .alwaysLazy, "④锚 .bottom·翻面已消灭")
        measure(.none, .alwaysLazy, "⑤锚 .top·翻面已消灭（与④只差锚值）")
        measure(.scrollPositionPin, .alwaysLazy, "⑥scrollPosition(id:anchor:.top)·翻面已消灭")
        measure(.scrollPositionPin, .flipping, "⑦scrollPosition(id:anchor:.top)·翻面还在")
        print("[#60 比选]\n" + rows.joined(separator: "\n"))
    }
}
#endif
