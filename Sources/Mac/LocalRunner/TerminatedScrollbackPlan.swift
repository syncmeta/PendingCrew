#if os(macOS)
import Foundation

/// 「已终止的 session 该留多少行回滚」的纯判定（2026-08-18：开久了卡第二条）。
///
/// ## 病根
///
/// 停掉的 run **故意**留在列表里（跑完了还能翻回去看它干了什么），可它连着的
/// `AgentTerminalSession` 一直攥着 SwiftTerm 的整个回滚缓冲不放，只有用户手动
/// `remove(_:)` 才释放。开一天下来起几十个 session，内存单向上涨 → 换页 → 整机发卡。
///
/// 占用怎么算：`CharData` stride 24 B，一行占 `cols × 24 B`，缓冲里有多少**实际行**
/// 就占多少（SwiftTerm 1.18 的 `Buffer.resize` 只遍历 `lines.count`，空槽位按需创建）。
/// 也就是 `min(输出行数, scrollback) × cols × 24 B` —— **跑得越久越贵，顶到 10000 行封顶**。
/// 本机实测（210 列、回滚顶满）：**一个已终止 session 52.9 MB**。而 claude 的 TUI 持续
/// 重绘，几小时就能把 10000 行顶满（当初 500 行「跑一小会儿就顶满」就是这么来的）。
///
/// ⚠️ `TerminalMirrorView.scrollbackLines` 那段注释里「不是按需增长、窗口第一次改宽
/// 就一次性吃满」说的是 **SwiftTerm 1.13**（那时 `resize` 遍历 `lines.maxLength`，
/// 下标 getter 会给每个空槽位 `makeEmpty`）。**1.18 已经改成只遍历 `lines.count`**，
/// 这条不再成立 —— 短命 session 本来就不占多少，别照着老心智模型去调参数。
///
/// ## 治法
///
/// 进程一终止就把回滚上限收下来（`Terminal.changeScrollback` → `Buffer.changeHistorySize`
/// → `CircularList.maxLength` 的 didSet 会重建数组、把多余的 `BufferLine` 释放掉）。
/// 终端视图**照旧留着** —— 颜色、选择、复制、`inspect_session` 读画面全不变，
/// 只是往上能翻的历史短了一截。
///
/// ## 回应 `TerminalMirrorView.scrollbackLines` 那段注释（为什么敢往下调）
///
/// 那段注释给 10000 行写了两条理由，逐条看它们对**已终止**的 session 还成不成立：
///
/// 1. 「500 行 agent 跑一小会儿就顶满 → 往上滑只能滑一小段」——
///    这条只对**在跑**的 session 成立：新行不断把老行挤出去。进程都没了就不会再有
///    新行，此刻留下的就是它最后的样子，稳定不掉。
/// 2. 「`Buffer.resize` 在列数变化时按 2 列 reflow 会把历史截顶」——
///    这条的根治手段是 `TerminalMirrorView.setFrameSize` 那道零尺寸闸
///    （`isRealLayout`，**没动**）。剩下的只是「人**真的**把栏拖得很窄」时折行后
///    可能溢出上限、顶部被裁 —— 所以这里保留 `reflowHeadroom` 倍余量，
///    并且下限不低于 `floor` 行。
///
/// 本机实测（3 个终端各喂 11000 行、210 列）：终止并收窄后 physical footprint
/// **52.9 MB/session → 22.8 MB/session（一次放掉 57%）**；按行数算是 10040 → 2040 行
/// （少 80%），差额是 malloc 没把全部空闲页还给系统 —— 在长跑的 app 里那部分会被后续
/// 分配复用，不会继续往上顶。实际行数不到 2000 的 session 本来就不占多少，`retainedLines`
/// 会原样留着它的历史，不做无谓的裁剪。
enum TerminatedScrollbackPlan {
    /// 已终止 session 保留的回滚行数上限。2000 行 ≈ 50 屏（40 行/屏），
    /// 够回看它最后在干什么；再往上留只是为已经死掉的 session 继续占内存。
    static let cap = 2_000
    /// 下限 —— 再短的 session 也留这么多，免得「翻回去看」只剩当前一屏。
    static let floor = 400
    /// 窄列 reflow 余量：把栏拖窄会把每行折成多行，留一倍余量再夹。
    static let reflowHeadroom = 2.0
    /// SwiftTerm 把 `scrollThumbsize` 夹在 0.01 下限（`max(rows/lines.count, 0.01)`），
    /// 到了下限就说明「行数多到从这个值反推不准了」—— 这时不猜，直接按 `cap` 保守留。
    static let saturatedThumb = 0.0101

    /// 该保留多少行。
    ///
    /// - Parameters:
    ///   - rows: 终端可视行数（`Terminal.rows`）。
    ///   - thumbSize: `TerminalView.scrollThumbsize` —— 公开接口，语义是
    ///     `max(rows / 缓冲总行数, 0.01)`；alternate buffer 时为 0。
    ///   - canScroll: `TerminalView.canScroll` —— false = 缓冲行数还没超过一屏。
    ///
    /// 反推缓冲总行数 = `rows / thumbSize`。反推不可信（thumb 触底 / alternate buffer /
    /// 参数不合法）时**不猜**，按 `cap` 保守保留：宁可少省点内存，也不许把还在的历史裁掉。
    static func retainedLines(rows: Int, thumbSize: Double, canScroll: Bool) -> Int {
        guard rows > 0 else { return cap }
        // 还没滚出过一屏 —— 缓冲里就这么点东西，留下限即可。
        guard canScroll else { return floor }
        guard thumbSize > saturatedThumb, thumbSize <= 1 else { return cap }
        let usedLines = Double(rows) / thumbSize
        let wanted = Int((usedLines * reflowHeadroom).rounded(.up))
        return min(cap, max(floor, wanted))
    }
}
#endif
