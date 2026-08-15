import Foundation

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
/// 点一下往前放一页。这样打开一个 crew 的成本与「这个 crew 历史有多长」**脱钩** ——
/// 从 O(消息数) 变成 O(pageSize)。
///
/// **不减功能**：更早的消息一条都没丢，往上翻就能加载到最早（`hasMore` 为 false 时
/// 占位消失）。渲染窗口只影响「一次往视图树里塞多少」，不影响数据。
///
/// ## 为什么不做「按滚动位置自动加载」
///
/// 这个文件所在的这条路上已经出过两次布局自激事故（2026-07-26 打字圆点、2026-08-10
/// Todo 呼吸圈）。「滚到顶自动加载」= 滚动位置驱动内容增长、内容增长又改滚动位置，
/// 正是自激的配方。所以这里只认**用户点一下**这个离散事件，和 `landAtBottom` 的两个
/// 触发点一样保守。
enum CrewChatWindow {

    /// 一页多少条。Todo #56 为消除 LazyVStack 估算高度导致的空白首屏，窗口改为 eager
    /// measure；拿当前真实 LED驱动板 fixture 多轮量：30 条 130–143ms、24 条
    /// 94–134ms、16 条在整组回归里 113ms。12 条给既有 100ms 预算留出抖动余量；
    /// 紧凑视口可能直接看见顶部「加载更早」，这是确定性首屏与翻页频率之间的取舍。
    static let pageSize = 12

    /// 只有首屏用 eager stack 把真实行高一次量完；人明确点过「加载更早」后仍用 lazy，
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
    static func expanded(_ limit: Int, total: Int, pageSize: Int = CrewChatWindow.pageSize) -> Int {
        clampedLimit(clampedLimit(limit, total: total) + pageSize, total: total)
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
}
