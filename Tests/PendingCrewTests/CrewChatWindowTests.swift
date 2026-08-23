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
/// 上面那组钉的是纯逻辑（该锚哪一条）；这里钉另一半：**那一记 scrollTo 到底有没有把
/// 位置按住**。离屏窗口里把那一下真的走一遍 —— 同一对 `defaultScrollAnchor`、同一个
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
/// 这不是一根新柴火：窗口从不 `orderFront`，跑完就撒手；没有定时器、没有反复重排。
@available(macOS 15.0, *)
final class CrewChatExpandAnchorProbeTests: XCTestCase {

    /// 驱动这一下的外部把手。
    private final class Rig: ObservableObject {
        @Published var limit = CrewChatWindow.pageSize
        @Published var ids: [String] = []
        var scroll: ((String) -> Void)?
        /// 视口顶 → 内容底 的距离。
        var distanceFromBottom: CGFloat = .nan
        var offsetY: CGFloat = .nan
    }

    @available(macOS 15.0, *)
    private struct Harness: View {
        @ObservedObject var rig: Rig

        /// 行高故意不等（真群聊也不等），但对同一个 id 恒定 —— 否则两次量的是两张表。
        private func height(_ id: String) -> CGFloat {
            30 + CGFloat((Int(id.dropFirst()) ?? 0) % 4) * 18
        }

        var body: some View {
            ScrollViewReader { proxy in
                ScrollView {
                    Group {
                        if CrewChatWindow.usesEagerInitialLayout(limit: rig.limit) {
                            VStack(alignment: .leading, spacing: 0) { rows }
                        } else {
                            LazyVStack(alignment: .leading, spacing: 0) { rows }
                        }
                    }
                    .padding(.vertical, 10)
                }
                // 与 CrewChatView.ChatScrollAnchor 一字不差（isFollowing = false，
                // 也就是「人自己滑上去看历史」那个现场）。
                .defaultScrollAnchor(.bottom, for: .initialOffset)
                .defaultScrollAnchor(.top, for: .sizeChanges)
                .onScrollGeometryChange(for: CGPoint.self) { geo in
                    CGPoint(x: geo.contentOffset.y,
                            y: geo.contentSize.height - geo.contentOffset.y)
                } action: { _, v in
                    rig.offsetY = v.x
                    rig.distanceFromBottom = v.y
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

    /// 转几拍主线程，让 SwiftUI 的更新 + 我们排的那一次 hop 都落地。
    private func settle(_ win: NSWindow, _ seconds: TimeInterval = 0.4) {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.01))
            win.layoutIfNeeded()
            win.contentView?.displayIfNeeded()
        }
    }

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
    ///
    /// 判据取「补偿后的 contentOffset 和没来新消息那一趟一不一样」：来新消息只往内容
    /// **末尾**加东西，所以「视口顶→内容底」的距离必然多出那一行；真正要证的是视口在
    /// 内容里的位置没被它推走。
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

}
#endif
