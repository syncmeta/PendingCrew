#if os(macOS)
import SwiftUI
import AppKit

/// 「正在输入」三个呼吸圆点 —— **动画跑在 CoreAnimation 里，不进 SwiftUI 的动画图**。
///
/// 为什么必须这样（2026-07-26 17:24 闪退的根因）：
/// 这三个点原来是 SwiftUI 的 `.opacity(...).animation(.repeatForever, value:)`。
/// `repeatForever` 在 SwiftUI 的动画事务里**永不结束**；只要同一个 `NSHostingView`
/// 里还有一个「滚动位置被管理」的 ScrollView（群聊那个：`.defaultScrollAnchor(.bottom)`
/// ＋ 新消息到达时 `proxy.scrollTo("tail")`），每一帧动画都会让滚动几何重新解析 →
/// lazy 容器重算 placement → 子图重插 → `graphDidChange` → `NSHostingView.setNeedsUpdate`
/// → `NSView.setNeedsUpdateConstraints` → 窗口的 "Update Constraints in Window" 计数 +1。
/// 这个计数**只增不减**，超过窗口里的视图数时 AppKit 抛 `NSGenericException`，
/// `+[NSApplication _crashOnException:]` 直接把进程打死。中间那一分半主线程一直在
/// 布局里打转，用户看到的就是彩虹圈 —— 全程不需要任何人操作。
///
/// 实测（`LayoutLoopRegressionTests`，量「数据不再变、无人操作」的安静窗口里
/// `NSHostingView.layout()` 跑几次）：
/// - 锚点 ＋ scrollTo ＋ SwiftUI repeatForever 三者凑齐 → 3 秒里 5~9 万次（自激）
/// - 三者缺任意一个 → 0
/// - 换成本文件这版（CoreAnimation 驱动）→ 0
///
/// 所以修的是「SwiftUI 图里有一个永不结束的动画事务」这条边：图层自己按
/// CoreAnimation 的时钟插值，SwiftUI 那边的几何自始至终是静态的，锚点解析一次即收敛。
/// **不是**「延迟一拍 / async 打断循环」那种掩盖式补丁 —— 环被拆掉了，不是被绕开。
struct TypingDotsLayerView: NSViewRepresentable {
    /// 圆点颜色（调用方给主题色）。深浅色切换时在 `viewDidChangeEffectiveAppearance` 重解析。
    var color: NSColor

    static let dotSize: CGFloat = 7
    static let spacing: CGFloat = 4
    /// 与原 SwiftUI 版 `HStack(spacing: 4) { 3 × Circle(7) }` 逐像素一致：29 × 7。
    /// 气泡内边距仍由 `CrewTypingIndicatorRow` 在外面给，别在这儿重复加。
    static var intrinsicSize: CGSize {
        CGSize(width: dotSize * 3 + spacing * 2, height: dotSize)
    }

    func makeNSView(context: Context) -> DotsView { DotsView(color: color) }

    func updateNSView(_ nsView: DotsView, context: Context) { nsView.color = color }

    @MainActor
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: DotsView, context: Context) -> CGSize? {
        Self.intrinsicSize
    }

    /// 三个 CALayer + `CABasicAnimation`（autoreverse + repeatForever + 逐个 delay），
    /// 与原 SwiftUI 版的时长/相位/透明度逐参数对齐，肉眼看不出差别。
    final class DotsView: NSView {
        var color: NSColor { didSet { applyColor() } }
        private var dots: [CALayer] = []

        init(color: NSColor) {
            self.color = color
            super.init(frame: NSRect(origin: .zero, size: TypingDotsLayerView.intrinsicSize))
            wantsLayer = true
            let d = TypingDotsLayerView.dotSize
            for i in 0..<3 {
                let dot = CALayer()
                dot.frame = CGRect(x: CGFloat(i) * (d + TypingDotsLayerView.spacing),
                                   y: 0, width: d, height: d)
                dot.cornerRadius = d / 2
                let a = CABasicAnimation(keyPath: "opacity")
                a.fromValue = 0.25
                a.toValue = 1.0
                a.duration = 0.55
                a.autoreverses = true
                a.repeatCount = .infinity
                a.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
                a.beginTime = CACurrentMediaTime() + Double(i) * 0.18
                dot.opacity = 0.25
                dot.add(a, forKey: "pulse")
                layer?.addSublayer(dot)
                dots.append(dot)
            }
            applyColor()
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError("not used") }

        override var intrinsicContentSize: NSSize { TypingDotsLayerView.intrinsicSize }

        /// 深浅色切换：动态 NSColor 要按当前外观重解析成 CGColor，图层才跟着变。
        override func viewDidChangeEffectiveAppearance() {
            super.viewDidChangeEffectiveAppearance()
            applyColor()
        }

        private func applyColor() {
            effectiveAppearance.performAsCurrentDrawingAppearance { [self] in
                let cg = color.cgColor
                for dot in dots { dot.backgroundColor = cg }
            }
        }
    }
}
#endif
