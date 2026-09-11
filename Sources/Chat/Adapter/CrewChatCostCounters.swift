import Foundation

/// 群聊热路径上「这一帧又重建了多少次贵东西」的计数器 —— 人类 Todo #140 那三处浪费的**尺子**。
///
/// ## 为什么是计数，不是计时
///
/// 本单的现场读数（`docs/internal/2026-09-11-typing-lag-profile.md`）是在整机换页、
/// load 5.9 的机器上采的；毫秒在那种机器上浮动一倍都算正常，两趟采样主线程忙的比例
/// 本身就是 77% 和 45%。而「构造了几次 `ISO8601DateFormatter`」「花名册判定索引重建了
/// 几次」「整条时间线真筛了几遍」是**离散的**，跟机器忙不忙无关 —— 同一份代码在任何
/// 机器上都是同一个数。所以成本断言钉在这些数上，毫秒只印出来当观测。
///
/// ## 这把尺子量不到什么（别把它的绿读成「不卡了」）
///
/// - **它只盯下面 `Kind` 里那三处。** 本文件之外还有几十处 `ISO8601DateFormatter()`
///   （`Sources/Stores/*` 写时间戳那一批），**一处都不计数** —— 它们不在渲染热路径上，
///   本单没碰。所以 `iso8601FormatterBuild == 2` 的意思是「群聊**解析**路只建了两个」，
///   **不是**「全 app 只建了两个」。
/// - 它不量布局、不量 SwiftUI 失效次数、不量 WindowServer 那 44%~52%。
/// - 三个数全是 0 增长也只证明「不再重复构造」，**不证明界面变快了** —— 那个要装新版
///   再采一趟 `sample` 才算。
///
/// ## 为什么留在生产代码里而不是 `#if DEBUG`
///
/// 自增只发生在**构造贵东西**那一刻（每帧个位数），不在任何热循环里；而 `#if DEBUG`
/// 会让测试量的和发布跑的不是同一条路 —— 这一单要钉的恰恰是「这条路上发生了几次」。
enum CrewChatCostCounters {

    enum Kind: String, CaseIterable {
        /// `CrewTimestamp` 那两个进程级 ISO8601 格式器的构造。
        case iso8601FormatterBuild
        /// `CrewMentionFilter.Roster.MatchIndex`（小写人类名集合 + 最长匹配顺序）的构造。
        case mentionMatcherBuild
        /// `CrewTimelineFilter.resolve` 真的跑了一遍筛选（缓存命中不算一次）。
        case timelineFilterRun
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var table: [Kind: Int] = [:]

    static func note(_ kind: Kind) {
        lock.lock()
        table[kind, default: 0] += 1
        lock.unlock()
    }

    static func total(_ kind: Kind) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return table[kind] ?? 0
    }

    /// **默认一律量 delta，别量绝对值。** 这些计数器是进程级的，同一趟测试里别的用例
    /// 也在往上加 —— 拿绝对值断言等于顺带断言了用例执行顺序，那种红跟真回归长得一样。
    ///
    /// （唯一的例外是 `iso8601FormatterBuild`：它的上界由「两个 `static let`」这个
    /// 结构本身封死，整个进程里最多就是 2，所以那一处**故意**断言绝对值 —— 同时也是
    /// 「这个计数器还活着」的证明：它要是 0，说明尺子根本没接上。）
    static func delta(_ kind: Kind, during body: () throws -> Void) rethrows -> Int {
        let before = total(kind)
        try body()
        return total(kind) - before
    }
}
