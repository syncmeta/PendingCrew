import Foundation
import CoreGraphics

/// 自绘终端滚动条（`TerminalScrollbarOverlay`）的几何换算 —— 从 View body 里摘出来的纯函数，
/// 单测直接钉住「条显示的可滚范围」和「真实历史」对得上。
///
/// 输入的 `thumbSize` / `position` 都取自 SwiftTerm 的公开滚动接口
/// （`scrollThumbsize` = 可见行数 ÷ 总行数，`scrollPosition` = yDisp ÷ 可回滚行数），
/// 语义都是 0…1。所以「条的范围」天然等于「真实 scrollback 行数」——这里要保证的是
/// **换算本身不引入偏差**：knob 顶到轨道顶 = 历史最老一行，knob 触底 = 最新一行，
/// 拖到某个位置读回来的 position 与画上去的 knob 位置互为逆运算。
struct TerminalScrollbarGeometry: Equatable {
    /// 轨道可视高度（overlay 的 geo.size.height）。
    let trackExtent: CGFloat
    /// knob 距轨道两端的留白。
    let inset: CGFloat
    /// knob 最短长度 —— 历史很长时 thumbSize 会小到看不见，兜一个可抓的下限。
    let minKnobExtent: CGFloat

    init(trackExtent: CGFloat, inset: CGFloat, minKnobExtent: CGFloat) {
        self.trackExtent = trackExtent
        self.inset = inset
        self.minKnobExtent = minKnobExtent
    }

    /// 去掉两端留白后 knob 可占的长度。
    var usableExtent: CGFloat { max(0, trackExtent - inset * 2) }

    func knobExtent(thumbSize: Double) -> CGFloat {
        let proportional = CGFloat(clamp01(thumbSize)) * usableExtent
        return min(usableExtent, max(minKnobExtent, proportional))
    }

    /// knob 能走的距离：0 时（内容不足一屏 / knob 顶满轨道）拖动无意义。
    func travel(thumbSize: Double) -> CGFloat {
        max(0, usableExtent - knobExtent(thumbSize: thumbSize))
    }

    /// 画 knob 用：position 0 → 贴轨道顶（最老的历史），1 → 贴轨道底（最新输出）。
    func knobOffset(position: Double, thumbSize: Double) -> CGFloat {
        inset + travel(thumbSize: thumbSize) * CGFloat(clamp01(position))
    }

    /// 拖动/点击轨道用：把光标位置换回 0…1 的 position（knob 中心对齐光标）。
    /// 与 `knobOffset` 互为逆运算，结果已夹到 0…1，不把越界值传给 SwiftTerm。
    func position(forPointerAt pointer: CGFloat, thumbSize: Double) -> Double {
        let travel = travel(thumbSize: thumbSize)
        guard travel > 0 else { return 0 }
        let centered = pointer - inset - knobExtent(thumbSize: thumbSize) / 2
        return clamp01(Double(centered / travel))
    }

    private func clamp01(_ value: Double) -> Double { min(1, max(0, value)) }
}
