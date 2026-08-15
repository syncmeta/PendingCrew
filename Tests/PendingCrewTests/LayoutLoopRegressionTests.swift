#if os(macOS)
import XCTest
import SwiftUI
import AppKit

/// 布局自激回归（2026-07-26 17:24 与 2026-08-10 20:49 两次闪退）。
///
/// 病理判据不是「动画期间布局跑得多」——那正常且会停；而是**数据不再变、也没人碰，
/// 布局却永远停不下来**（用户当时的原话：没做任何操作，直接转彩虹圈）。
/// 所以这里量「安静窗口」：先喂几条数据变化，停手，静置 3 秒，再数
/// `NSHostingView.layout()` 跑了几次。
///
/// ## 阈值为什么这么紧（2026-08-10 补）
///
/// 健康是 **0**，不是「几十」。第二次闪退那条环只有 ~9.4Hz（每圈 ~106ms，因为那屏
/// 重解析一次就要这么久），3 秒安静窗口里也才 ~28 次 —— 原来「< 50」的判据看着它
/// 从眼皮底下走过去了。所以阈值收到 10，**别再放宽**：任何一次非零都值得来人看一眼。
///
/// 病根一样、终点一样，只是驱动不同：SwiftUI 的 `.repeatForever` 在动画事务里永不
/// 结束 → 每帧重解析视图图 → 懒容器/受管滚动视图把子图摘下插回 → 重插的子图带
/// appearance effect（`.task`/`.onAppear`）→ `graphDidChange` → `requestUpdate` →
/// `setNeedsUpdateConstraints`，窗口又脏。一个显示周期结束不了，AppKit 数到 275 次
/// 重标就抛 NSException 打死进程（统一日志里能看到
/// `Marking window … (limit: 275, count: 276…)` 与 `NSDisplayCycleFlush restarting…`）。
///
/// 钉死四条（两条反面守前提，两条正面守线上件）：
/// 1. **反面**：SwiftUI `.repeatForever` ＋ 滚动锚点 ＋ 程序化 `scrollTo` 会自激。
/// 2. **反面**：SwiftUI `.repeatForever` 长在 `LazyVStack` 行里（Todo 面板那屏）会自激。
///    （两条前提要是哪天被 SwiftUI 修了，会转红，提醒我们下面那些防护可能已不需要。）
/// 3. **正面**：`TypingDotsLayerView` / `BreathingDot`（CoreAnimation 驱动）不自激。
/// 4. **正面**：`BreathingSymbol`（Todo 圆圈现在用的这版）不自激。
///
/// 另有一道**源码级**闸拦「第四处又随手写 `.repeatForever`」——
/// 见 `NoSwiftUIRepeatForeverTests`。行为测试只能守住已知那几屏，源码闸守的是没写出来的那处。
///
/// ## 为什么这里要建真窗口，又为什么它必须离开屏幕
///
/// 自激只在 AppKit 真实的显示周期里复现 —— 窗口得真进窗口服务器，AppKit 才会持续驱动
/// `layout()`；光拿 `NSHostingView` 自己 `layoutIfNeeded()` 是测不出来的。所以窗口不能省。
///
/// 但「被系统渲染」和「出现在用户眼前」是两件事，要拆开：窗口建在任何屏幕之外
/// （原点 -20000），styleMask 用 `.borderless`。`orderFrontRegardless()` 保留 —— 它只排层序、
/// 不抢焦点，窗口在屏幕外就无所谓，而少了它窗口进不了显示周期、自激也就测不出来。
///
/// **别把它改回屏幕内的 `.titled` 窗口**：那样每次跑测试都会往用户桌面上闪一个真窗口。
/// 2026-08-08 用户就是这么发现的 —— 专门跑来问「那个写着消息 #0…#149 的框是什么」。
@MainActor
final class LayoutLoopRegressionTests: XCTestCase {

    private final class CountingHostingView<V: View>: NSHostingView<V> {
        var layouts = 0
        var counting = false
        override func layout() { if counting { layouts += 1 }; super.layout() }
    }

    private final class Feed: ObservableObject {
        @Published var rows: [Int] = Array(0..<150)
    }

    /// 群聊那屏的骨架：长列表 + 底部锚点 + 新消息到达时程序化滚到底 + 一个常驻动画行。
    private struct ChatShape<Indicator: View>: View {
        @ObservedObject var feed: Feed
        let indicator: () -> Indicator

        var body: some View {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(feed.rows, id: \.self) { i in
                            Text("消息 #\(i)").padding(.vertical, 6).id(i)
                        }
                        indicator()
                        Color.clear.frame(height: 1).id("tail")
                    }
                }
                .defaultScrollAnchor(.bottom)
                .onChange(of: feed.rows.count) { _, _ in
                    withAnimation(.linear(duration: 0.05)) { proxy.scrollTo("tail", anchor: .bottom) }
                }
            }
        }
    }

    /// 病根形态的复刻：SwiftUI `.repeatForever`。
    private struct SwiftUIRepeatForeverDots: View {
        @State private var animating = false
        var body: some View {
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { i in
                    Circle().frame(width: 7, height: 7)
                        .opacity(animating ? 1 : 0.25)
                        .animation(.easeInOut(duration: 0.55).repeatForever(autoreverses: true)
                            .delay(Double(i) * 0.18), value: animating)
                }
            }
            .padding(12)
            .onAppear { animating = true }
        }
    }

    private func quietLayouts<I: View>(@ViewBuilder _ indicator: @escaping () -> I) -> Int {
        let feed = Feed()
        let host = CountingHostingView(rootView: ChatShape(feed: feed, indicator: indicator))
        // 离屏但仍进窗口服务器：原点远在任何显示器之外，`.borderless` 免掉标题栏，
        // 于是 AppKit 照常驱动显示周期，用户屏幕上却什么都不会出现（见类型顶部说明）。
        // `.borderless` 还顺带绕开了 AppKit 对 `.titled` 窗口的「把 frame 拉回屏幕内」约束。
        let window = NSWindow(contentRect: NSRect(x: -20_000, y: -20_000, width: 520, height: 700),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFrontRegardless()
        window.layoutIfNeeded()
        for i in 0..<3 {
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
            feed.rows.append(200 + i)
        }
        RunLoop.current.run(until: Date().addingTimeInterval(1.5))   // 让动画/滚动自然收尾
        host.counting = true
        RunLoop.current.run(until: Date().addingTimeInterval(3.0))   // 安静窗口（见类型顶部）
        let layouts = host.layouts
        host.counting = false
        // 两条测试共用这个函数，别给下一条留残窗：orderOut 只是摘下来，close() 才让窗口
        // 服务器立刻放手（isReleasedWhenClosed = false，所以 close 之后对象仍由 ARC 管）。
        window.orderOut(nil)
        window.contentView = nil
        window.close()
        print("[quietLayouts] 安静窗口内 layout() 次数 = \(layouts)")
        return layouts
    }

    /// 前提：这个组合确实有毒（红了说明 SwiftUI 侧变了，回来重估下面那条防护还要不要）。
    func testSwiftUIRepeatForeverInAnchoredScrollViewSelfExcites() {
        let n = quietLayouts { SwiftUIRepeatForeverDots() }
        XCTAssertGreaterThan(
            n, 1000,
            "SwiftUI repeatForever ＋ 滚动锚点 ＋ scrollTo 这个组合本该自激（实测几万次）。"
            + "只测到 \(n) 次 —— 要么 SwiftUI 修了这个坑，要么这条测试的判据失效了，两种都要人来看一眼。")
    }

    /// 第二条真防线：session 头像上那颗「需要人出手」的呼吸红点（2026-08-08 两点合一）。
    /// 它比「正在输入」更危险 —— 挂在**每一个** session 头像上，气泡列表里可能同时好几颗。
    func testBreathingDotDoesNotSelfExcite() {
        let n = quietLayouts { BreathingDot(size: 8, color: .red).padding(12) }
        XCTAssertLessThan(
            n, 10,
            "呼吸红点在安静期把布局跑了 \(n) 次 —— 布局自激回来了，"
            + "会重演 2026-07-26 17:24 那次闪退。别把呼吸改回 SwiftUI 的 .repeatForever，"
            + "见 BreathingDotView.swift 顶部说明。")
    }

    /// 真防线：线上用的这版不自激。
    func testTypingDotsLayerViewDoesNotSelfExcite() {
        let n = quietLayouts { TypingDotsLayerView(color: .secondaryLabelColor) }
        XCTAssertLessThan(
            n, 10,
            "「正在输入」圆点在安静期把布局跑了 \(n) 次 —— 布局自激回来了，"
            + "会重演 2026-07-26 17:24 那次闪退（窗口约束计数顶爆 → AppKit 抛异常打死进程）。"
            + "别把动画改回 SwiftUI 的 .repeatForever，见 TypingDotsLayerView 顶部说明。")
    }

    // MARK: - 第二条路径：Todo 面板那屏（2026-08-10 20:49 闪退）

    private final class Todos: ObservableObject {
        @Published var rows: [Int] = Array(0..<30)
    }

    /// Todo 面板那屏的骨架：滚动列 + `LazyVStack` + 行首状态圆圈。
    /// 注意这屏**没有** `ScrollViewReader`/`scrollTo`/滚动锚点 —— 光是「懒容器 ＋
    /// 行内一个永不结束的 SwiftUI 动画」就够自激了，比第一条路径的门槛还低。
    private struct TodoPanelShape<Circle: View>: View {
        @ObservedObject var todos: Todos
        /// 第几行「进行中」（会呼吸）。-1 = 全都不呼吸。
        let breathingRow: Int
        let circle: (Bool) -> Circle

        var body: some View {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(todos.rows, id: \.self) { i in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            circle(i == breathingRow)
                            Text("#\(i)").font(.caption.weight(.semibold))
                            Text(String(repeating: "待办条目文本。", count: (i % 4) + 1))
                                .font(.footnote)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// `CrewTodoStatusCircle` 出事那版的原样复刻（只去掉 Theme 依赖）。
    /// 留着是为了守住「这个写法确实有毒」这个前提 —— 别照抄进 Sources。
    private struct TodoCircleAsCrashed: View {
        let breathes: Bool
        @State private var breathing = false
        var body: some View {
            Image(systemName: breathes ? "largecircle.fill.circle" : "circle")
                .font(.system(size: 13, weight: .regular))
                .foregroundStyle(Color.accentColor)
                .opacity(breathes && breathing ? 0.35 : 1)
                .scaleEffect(breathes && breathing ? 0.88 : 1)
                .animation(breathes
                           ? .easeInOut(duration: 1.1).repeatForever(autoreverses: true)
                           : .default,
                           value: breathing)
                .task(id: breathes) { breathing = breathes }
        }
    }

    private func quietTodoLayouts<C: View>(
        breathingRow: Int, @ViewBuilder circle: @escaping (Bool) -> C
    ) -> Int {
        let todos = Todos()
        let host = CountingHostingView(
            rootView: TodoPanelShape(todos: todos, breathingRow: breathingRow, circle: circle))
        // 离屏窗口，理由同 `quietLayouts`（见类型顶部）——**别改回屏幕内的 `.titled`**。
        let window = NSWindow(contentRect: NSRect(x: -20_000, y: -20_000, width: 380, height: 700),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFrontRegardless()
        window.layoutIfNeeded()
        for i in 0..<3 {
            RunLoop.current.run(until: Date().addingTimeInterval(0.3))
            todos.rows.append(100 + i)
        }
        RunLoop.current.run(until: Date().addingTimeInterval(1.5))
        host.counting = true
        RunLoop.current.run(until: Date().addingTimeInterval(3.0))
        let layouts = host.layouts
        host.counting = false
        window.orderOut(nil)
        window.contentView = nil
        window.close()
        print("[quietTodoLayouts] 安静窗口内 layout() 次数 = \(layouts)")
        return layouts
    }

    /// 前提：出事那版确实有毒（实测 3 秒里约 4 万次）。
    func testSwiftUIRepeatForeverInLazyListSelfExcites() {
        let idle = quietTodoLayouts(breathingRow: -1) { TodoCircleAsCrashed(breathes: $0) }
        XCTAssertLessThan(
            idle, 10,
            "没有任何一行呼吸时本该是静的，却跑了 \(idle) 次 —— 说明这屏骨架自己就有问题，"
            + "下面那条对照就不成立了，先查骨架。")
        let n = quietTodoLayouts(breathingRow: 3) { TodoCircleAsCrashed(breathes: $0) }
        XCTAssertGreaterThan(
            n, 1000,
            "SwiftUI repeatForever 长在 LazyVStack 行里本该自激（实测几万次），只测到 \(n) 次。"
            + "要么 SwiftUI 修了这个坑，要么这条测试的判据失效了，两种都要人来看一眼。")
    }

    /// 真防线：Todo 状态圆圈现在用的这版（`BreathingSymbol`，CoreAnimation 驱动）不自激。
    func testBreathingSymbolDoesNotSelfExcite() {
        let n = quietTodoLayouts(breathingRow: 3) {
            BreathingSymbol(symbol: $0 ? "largecircle.fill.circle" : "circle",
                            pointSize: 13, tint: .accentColor, breathing: $0)
        }
        XCTAssertLessThan(
            n, 10,
            "Todo 状态圆圈在安静期把布局跑了 \(n) 次 —— 第二条布局自激回来了，"
            + "会重演 2026-08-10 20:49 那次闪退。别把呼吸改回 SwiftUI 的 .repeatForever"
            + "（连 `.scaleEffect` 一起，那还多一层几何抖动），见 BreathingSymbolView.swift 顶部说明。")
    }
}
#endif
