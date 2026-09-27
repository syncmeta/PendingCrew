import Foundation
#if os(macOS)
import AppKit
import SwiftUI
#endif

/// 群聊时间线的**渲染窗口**（#443 第三道闸）。
///
/// ## 为什么要有它
///
/// 前两道闸（`FileChangeGate` 少给 tick、`CrewChatRefreshGate` 内容没变不碰 @State）
/// 治的都是「不该重排的时候别重排」。**该重排的那一下本身有多贵，一直没人管** ——
/// 2026-08-11 从人类机器上那两份真实 hang 报告里读到的主线程栈：
///
/// - 0.1.8（build 3584）卡 7.03s：12/12 采样全在
///   `LazyStack.measureEstimates → LazyHVStack.lengthAndSpacing → 逐行 sizeThatFits`，
///   即**整条列表被全量重新测量**。
/// - 0.1.7 卡 68.68s：4/11 采样在 `SelectionOverlay.updateNSView →
///   -[NSTextField setAttributedStringValue:]` —— `.textSelection(.enabled)` 让
///   **每段可选中文字背后挂一个真 NSTextField**，全量重设。
///
/// 两条热点有个共同前提：**列表里有多少行，这一下就付多少行的钱**。所以真正的修法
/// 不是把单行做便宜一点，是**别把全部行都放进视图树**。
///
/// ## 这个窗口做什么
///
/// 只把**最近 `pageSize` 条**交给 `ForEach`；更早的用顶部一条「加载更早的消息」占位，
/// 接近顶部自动续载，按钮仍可手动兜底。这样打开一个 crew 的成本与「这个 crew 历史有多长」**脱钩** ——
/// 从 O(消息数) 变成 O(pageSize)。
///
/// **不减功能**：更早的消息一条都没丢，往上翻就能加载到最早（`hasMore` 为 false 时
/// 占位消失）。渲染窗口只影响「一次往视图树里塞多少」，不影响数据。
///
/// 自动续载仅认用户滚动，且每次手势至多一页；内容增长自己造成的几何变化不再触发
/// 第二次续载，避免滚动位置 ↔ 内容增长的布局自激。
enum CrewChatWindow {

    /// 首屏 30 条（Agent Todo #163）。首屏仍 eager measure 以免估算高度
    /// 造成空白落点；历史 fixture 曾量出 30 条约 130–143ms，需保留性能回归风险。
    static let pageSize = 30
    /// 每次续页也是 30 条；离屏首帧锚探针覆盖第一次与第二次续页。
    static let historyBatchSize = 30

    static let autoLoadThreshold: CGFloat = 180

    static func isNearTop(offsetY: CGFloat, insetTop: CGFloat) -> Bool {
        offsetY + insetTop <= autoLoadThreshold
    }

    static func shouldAutoLoad(nearTop: Bool, towardTop: Bool) -> Bool {
        nearTop && towardTop
    }

    /// 引用型门闩，不写进 SwiftUI 的依赖图；每次真实用户滚动手势至多续载一页。
    final class AutoLoadGate {
        private var consumedGesture: Int?

        func shouldLoad(nearTop: Bool, isUserScrolling: Bool, isFollowing: Bool,
                        gesture: Int, total: Int, limit: Int) -> Bool {
            guard nearTop, isUserScrolling, !isFollowing, gesture > 0,
                  hasMore(total: total, limit: limit), consumedGesture != gesture else {
                return false
            }
            consumedGesture = gesture
            return true
        }
    }

    /// macOS 14 没有 SwiftUI 滚动相位；只从 AppKit 的真实滚轮事件产生手势令牌。
    /// 用事件时间生成令牌，传感器随 eager/lazy 容器重建时也不会复用旧令牌。
    /// 连续滚轮事件与惯性事件共用一号，停止 0.3 秒后再次滚动才开下一页。
    struct WheelBurst {
        private var lastWheelAt: TimeInterval?
        private(set) var generation = 0

        mutating func recordWheel(at time: TimeInterval, beginsGesture: Bool = false,
                                  isPhased: Bool = false) -> Int {
            if beginsGesture || generation == 0 ||
                (!isPhased && (lastWheelAt.map({ time - $0 > 0.3 || time < $0 }) ?? true)) {
                generation = Int((time * 1_000).rounded(.down)) + 1
            }
            lastWheelAt = time
            return generation
        }
    }

    /// 只有首屏用 eager stack 把真实行高一次量完；续载后仍用 lazy，
    /// 否则一路翻到几百条会把窗口化省下来的成本重新吃光。
    static func usesEagerInitialLayout(limit: Int) -> Bool {
        limit <= pageSize
    }

    /// 本次该渲染的条数上限。`nil` / 越界都夹回合法区间。
    static func clampedLimit(_ limit: Int, total: Int) -> Int {
        if total <= 0 { return 0 }
        return min(max(limit, 0), total)
    }

    /// 取「最近 `limit` 条」。顺序不变（仍是旧 → 新），只是砍掉前面更早的那一段。
    static func window<T>(_ all: [T], limit: Int) -> [T] {
        let n = clampedLimit(limit, total: all.count)
        guard n < all.count else { return all }
        return Array(all.suffix(n))
    }

    /// 上面还有没有更早的没渲染。
    static func hasMore(total: Int, limit: Int) -> Bool {
        clampedLimit(limit, total: total) < total
    }

    /// 「加载更早」按一下之后的新上限。到顶就是 total（占位随之消失）。
    static func expanded(_ limit: Int, total: Int, pageSize: Int = CrewChatWindow.historyBatchSize) -> Int {
        clampedLimit(clampedLimit(limit, total: total) + pageSize, total: total)
    }

    /// 「加载更早」按下去的那一刻，该把哪一条钉回视口顶部（Todo #60）。
    ///
    /// 返回**展开前**窗口最顶那条。人能点到那个按钮说明视口已经在内容顶端附近、那一条
    /// 就在他眼前；展开后把它钉回顶部，新放出来的一页落在它上面，眼前的内容基本不动
    /// （误差只有按钮那一行、以及那条原本因为「窗口第一条」而带的时间分隔展开后可能
    /// 消失）。不补这一记的话视口会停在**新那一页的开头** —— 因为 `ChatScrollAnchor`
    /// 在不跟随时把尺寸变化锚在**内容顶端**（Todo #47 行为 3 要的是「新消息在下面长、
    /// 视口不许动」），而这里内容是在**上面**长，同一个锚点方向正好相反。
    ///
    /// `isFollowing` 还挂着时返回 nil：那时尺寸变化锚的是底部，视口本来就纹丝不动
    /// （历史短到按钮和底部同屏才会发生），再补一记 `scrollTo` 只会把人从底部拽到顶部，
    /// 还要和 `landAtBottom` / `BottomOnContentGrowth` 抢同一拍。
    static func anchorOnExpand<T>(_ all: [T], limit: Int, isFollowing: Bool) -> T? {
        guard !isFollowing else { return nil }
        return window(all, limit: limit).first
    }

    /// 自动续载时人在按钮下面几行也可能触发；优先锚真实可见行，失效时回退窗口首行。
    static func anchorOnAutoExpand<T: Equatable>(_ windowed: [T], visibleTop: T?,
                                                  isFollowing: Bool) -> T? {
        guard !isFollowing else { return nil }
        if let visibleTop, windowed.contains(visibleTop) { return visibleTop }
        return windowed.first
    }

    /// 展开之后，锚点那条上面新插进来了几条 —— 也就是「不补偿的话视口会被推走多远」
    /// （按行数算）。为 0 时说明这一下什么都没多出来，锚不锚都一样。
    static func insertedAbove(total: Int, limit: Int, pageSize: Int = CrewChatWindow.historyBatchSize) -> Int {
        expanded(limit, total: total, pageSize: pageSize) - clampedLimit(limit, total: total)
    }

    /// 还没渲染的条数 —— 占位上写「上面还有 N 条」，让人知道翻上去有东西。
    static func remaining(total: Int, limit: Int) -> Int {
        max(0, total - clampedLimit(limit, total: total))
    }

    /// 来了 `added` 条新消息之后的上限。
    ///
    /// 窗口取的是**最近 limit 条**，所以来一条新的，最老的那条就被挤出窗口。
    /// 对默认状态（还没翻过页）那正是想要的：成本恒定封顶。
    ///
    /// 但**用户已经点开过「加载更早」**的话，挤出去的是他刚刚特意翻出来的内容 ——
    /// 正在读的东西从上面消失，这是 bug 不是优化。所以只要翻过页，就把新增的条数
    /// 补进上限，让已经露出来的那一段**留在原地**。
    ///
    /// 没翻过页时不补，上限恒为 `pageSize` —— 否则挂着不动的窗口会随着聊天一路
    /// 长回「整表」，封顶就白做了。
    static func afterInsert(limit: Int, added: Int, pageSize: Int = CrewChatWindow.pageSize) -> Int {
        guard added > 0, limit > pageSize else { return limit }
        return limit + added
    }

    /// 同上，但问的是**对的那个问题**（人类 Todo #144）。
    ///
    /// ## 上面那个守卫问错了对象
    ///
    /// `limit > pageSize` 问的是「**他翻过页没有**」。要挡的事却是
    /// 「**他正在看上面的内容，别把他正看的东西抽走**」—— 而看上面的内容
    /// **不需要先翻页**：默认窗口就有 `pageSize`(\(pageSize)) 条，气泡又高，
    /// 在窗口内往上滚几屏是最常见的读历史方式，此时 `limit == pageSize`，
    /// 上面那个守卫直接放行。
    ///
    /// 于是新消息一到：窗口仍取「最近 limit 条」→ **最老那条被挤出窗口** →
    /// 视口上方的内容少了一行的高度。而不跟随时滚动锚点是**内容顶端**
    /// （`ChatScrollAnchor`：`defaultScrollAnchor(.top, for: .sizeChanges)`），
    /// 顶端一缩，他正在读的那段就**整体上移一行** —— 这就是「消息位置会乱跳」。
    ///
    /// **注意方向**：不跟随时锚在顶端，本来是为了「新消息在**下面**长、视口不动」。
    /// 那一半是对的。坏在这里的增长不是发生在下面，是**上面被剪掉了**，
    /// 同一个锚点对这两件事的效果正好相反。
    ///
    /// ## 判据
    ///
    /// **用户已经滑走（`!isFollowing`）时，窗口里最顶那条在新消息到达前后必须是同一条。**
    /// 做法就是把新增条数补进上限 —— 窗口取的是后缀，上限跟着涨，顶端那条就不动。
    ///
    /// ## 成本还封得住吗
    ///
    /// 封得住，但靠的是**另一头**：他滑回底部（重新跟随）时把 `limit` 归位到一页。
    /// 所以窗口最多长「这一次往上看」的那段时间里来的消息数，不会一路长回整表。
    /// 而且补出来的那几行全在视口**下方**、且此时窗口已超过一页（`usesEagerInitialLayout`
    /// 为 false），是懒渲染的，不进视图树、不付测量的钱。
    /// **归位那一记必须由调用方在「重新跟随」时做** —— 这个纯函数只管别把人正看的
    /// 东西抽走。
    static func afterInsert(limit: Int, added: Int, isFollowing: Bool,
                            pageSize: Int = CrewChatWindow.pageSize) -> Int {
        guard added > 0 else { return limit }
        // 跟随中：窗口滑走是想要的（成本恒定封顶，而且他看的就是最新那几条）。
        guard !isFollowing else {
            return afterInsert(limit: limit, added: added, pageSize: pageSize)
        }
        return limit + added
    }
}

#if os(macOS)
/// macOS 14 没有 `onScrollGeometryChange` 和 `onScrollPhaseChange`；新系统也用此传感器
/// 识别近顶时的滚动方向，避免「已在阈值内但向下滚」被 Bool 几何回调误判。
/// 只监听落在本 ScrollView 内的真实滚轮事件；内容插入引起的 bounds 通知没有新手势号，
/// 因此不会把自己的插入当成下一次续页。放在 ScrollView 的内容背景里获取 enclosingScrollView。
struct LegacyTopApproachSensor: NSViewRepresentable {
    let scopeID: String
    let onUserScroll: (Int, Bool, Bool, Bool) -> Void

    func makeNSView(context: Context) -> WheelView {
        let view = WheelView()
        view.scopeID = scopeID
        view.onUserScroll = onUserScroll
        return view
    }

    func updateNSView(_ nsView: WheelView, context: Context) {
        nsView.onUserScroll = onUserScroll
        nsView.scopeID = scopeID
        nsView.connect()
    }

    @MainActor final class WheelView: NSView {
        var onUserScroll: ((Int, Bool, Bool, Bool) -> Void)?
        var scopeID = "" {
            didSet {
                if oldValue != scopeID {
                    disconnect()
                    burst = CrewChatWindow.WheelBurst()
                    connect()
                }
            }
        }
        var isConnected: Bool { trackedScroll != nil && wheelMonitor != nil }
        var attachedScrollView: NSScrollView? { trackedScroll }
        private weak var trackedScroll: NSScrollView?
        private var wheelMonitor: Any?
        private var boundsObserver: NSObjectProtocol?
        private var burst = CrewChatWindow.WheelBurst()
        private var pendingGesture: Int?
        private var pendingTowardTop = false
        private var pendingOriginY: CGFloat = 0
        private var pendingAt: TimeInterval = 0

        override func viewDidMoveToSuperview() {
            super.viewDidMoveToSuperview()
            connect()
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            connect()
        }

        func connect() {
            guard let scroll = enclosingScrollView, window != nil else {
                disconnect()
                return
            }
            guard trackedScroll !== scroll else { return }
            disconnect()
            trackedScroll = scroll
            scroll.contentView.postsBoundsChangedNotifications = true
            boundsObserver = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: scroll.contentView,
                queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.boundsChanged() }
                }
            wheelMonitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
                MainActor.assumeIsolated { self?.wheel(event) }
                return event
            }
        }

        private func disconnect() {
            if let boundsObserver { NotificationCenter.default.removeObserver(boundsObserver) }
            if let wheelMonitor { NSEvent.removeMonitor(wheelMonitor) }
            boundsObserver = nil
            wheelMonitor = nil
            trackedScroll = nil
            pendingGesture = nil
        }

        private func nearTop(_ scroll: NSScrollView) -> Bool {
            guard let document = scroll.documentView else { return false }
            let clip = scroll.contentView.bounds
            let distance = document.isFlipped ? clip.minY : document.bounds.height - clip.maxY
            return distance <= CrewChatWindow.autoLoadThreshold
        }

        private func atBottom(_ scroll: NSScrollView) -> Bool {
            guard let document = scroll.documentView else { return true }
            let clip = scroll.contentView.bounds
            let distance = document.isFlipped ? document.bounds.height - clip.maxY : clip.minY
            return distance <= CrewChatBottomFollow.bottomSlack
        }

        private func movedInWheelDirection(_ scroll: NSScrollView) -> Bool {
            guard let document = scroll.documentView else { return false }
            let change = scroll.contentView.bounds.minY - pendingOriginY
            let towardTop = document.isFlipped ? change < -0.5 : change > 0.5
            return pendingTowardTop ? towardTop : (document.isFlipped ? change > 0.5 : change < -0.5)
        }

        func acceptsWheel(window eventWindow: NSWindow?, locationInWindow: NSPoint) -> Bool {
            guard let scroll = trackedScroll, eventWindow === scroll.window else { return false }
            return scroll.bounds.contains(scroll.convert(locationInWindow, from: nil))
        }

        private func wheel(_ event: NSEvent) {
            guard let scroll = trackedScroll,
                  acceptsWheel(window: event.window, locationInWindow: event.locationInWindow),
                  !event.phase.contains(.ended) else { return }
            receiveWheelSample(at: event.timestamp, scrollingDeltaY: event.scrollingDeltaY,
                               nearTop: nearTop(scroll), atBottom: atBottom(scroll),
                               beginsGesture: event.phase.contains(.began),
                               isPhased: !event.phase.isEmpty,
                               isMomentum: !event.momentumPhase.isEmpty)
        }

        /// 由本地 NSEvent 监听器调用；测试可直接注入滚轮样本，不向系统派发事件。
        func receiveWheelSample(at time: TimeInterval, scrollingDeltaY: CGFloat,
                                nearTop: Bool, atBottom: Bool,
                                beginsGesture: Bool = false, isPhased: Bool = false,
                                isMomentum: Bool = false) {
            guard let scroll = trackedScroll, scrollingDeltaY != 0 else { return }
            let gesture = isMomentum ? burst.generation : burst.recordWheel(
                at: time, beginsGesture: beginsGesture, isPhased: isPhased)
            guard gesture > 0 else { return }
            pendingGesture = gesture
            pendingTowardTop = scrollingDeltaY > 0
            pendingOriginY = scroll.contentView.bounds.minY
            pendingAt = ProcessInfo.processInfo.systemUptime
            onUserScroll?(gesture, nearTop, atBottom, pendingTowardTop)
        }

        private func boundsChanged() {
            guard let scroll = trackedScroll, let gesture = pendingGesture,
                  ProcessInfo.processInfo.systemUptime - pendingAt < 1.0,
                  movedInWheelDirection(scroll) else { return }
            pendingOriginY = scroll.contentView.bounds.minY
            onUserScroll?(gesture, nearTop(scroll), atBottom(scroll), pendingTowardTop)
        }

        deinit {
            if let boundsObserver { NotificationCenter.default.removeObserver(boundsObserver) }
            if let wheelMonitor { NSEvent.removeMonitor(wheelMonitor) }
        }
    }
}
#endif
