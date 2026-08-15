import SwiftUI
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// 一颗会「呼吸」的实心圆点 —— **动画跑在 CoreAnimation 里，不进 SwiftUI 的动画图**。
///
/// 用在 session 头像的 `attention` 红点上（`SessionStatusDot.breathes`）：需要人出手时
/// 缓慢明暗脉动，其余状态是静止的纯色圆。
///
/// 为什么必须 CoreAnimation（2026-07-26 那次 Mac 转彩虹圈闪退的根因，见
/// `TypingDotsLayerView` 的完整验尸）：SwiftUI 的 `.opacity(...).animation(.repeatForever,)`
/// 在动画事务里**永不结束**；同一个 hosting view 里只要还有个「滚动位置被管理」的
/// ScrollView（群聊那个），每帧动画都会让滚动几何重解析 → 子图重插 → `graphDidChange`
/// → `setNeedsUpdateConstraints` → 窗口的 "Update Constraints in Window" 计数只增不减
/// → 超过视图数时 AppKit 抛异常打死进程。全程不需要任何人操作。
/// 头像点比打字点更危险：它挂在**每一个** session 头像上，气泡列表里可能同时有好几个。
///
/// 所以这里照 `TypingDotsLayerView` 抄：图层自己按 CoreAnimation 的时钟插值，SwiftUI
/// 那边的几何自始至终静态（固定 `size`，`sizeThatFits` 恒定），锚点解析一次即收敛。
///
/// `accessibilityReduceMotion` 打开时**完全不加动画**，退化成静态实心点（红还是红，
/// 只是不动）—— 信号不丢，动效才是可选的那部分。
struct BreathingDot: View {
    var size: CGFloat
    var color: Color

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        // reduce motion → 干脆用普通 Circle：连 representable 都不建，零动画开销。
        if reduceMotion {
            Circle().fill(color).frame(width: size, height: size)
        } else {
            BreathingDotLayerView(size: size, color: color)
                .frame(width: size, height: size)
        }
    }
}

#if os(macOS)
private typealias PlatformViewBase = NSView
private typealias PlatformColor = NSColor
#else
private typealias PlatformViewBase = UIView
private typealias PlatformColor = UIColor
#endif

/// 单个 `CALayer` + 一条 `CABasicAnimation`（autoreverse + repeatForever）。
/// 时长选 1.1s（一来一回 2.2s ≈ 一次平稳呼吸）；透明度 0.35↔1.0 —— 谷底仍看得见，
/// 是「在呼吸」不是「在闪烁」。
private final class BreathingDotHostView: PlatformViewBase {
    private let dotSize: CGFloat
    var color: PlatformColor { didSet { applyColor() } }
    private let dot = CALayer()

    init(size: CGFloat, color: PlatformColor) {
        self.dotSize = size
        self.color = color
        super.init(frame: CGRect(x: 0, y: 0, width: size, height: size))
        #if os(macOS)
        wantsLayer = true
        #else
        backgroundColor = .clear
        isUserInteractionEnabled = false
        #endif
        dot.frame = CGRect(x: 0, y: 0, width: size, height: size)
        dot.cornerRadius = size / 2
        let pulse = CABasicAnimation(keyPath: "opacity")
        pulse.fromValue = 1.0
        pulse.toValue = 0.35
        pulse.duration = 1.1
        pulse.autoreverses = true
        pulse.repeatCount = .infinity
        pulse.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        dot.opacity = 1.0
        dot.add(pulse, forKey: "breathe")
        // `NSView.layer` 是 Optional（要先 wantsLayer），`UIView.layer` 不是 —— 别用
        // `layer?.` 一把梭，那在 iOS 侧是编译错（2026-08-08 就是这么被 iOS 端打红的）。
        #if os(macOS)
        layer?.addSublayer(dot)
        #else
        layer.addSublayer(dot)
        #endif
        applyColor()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    #if os(macOS)
    override var intrinsicContentSize: NSSize { CGSize(width: dotSize, height: dotSize) }

    /// 深浅色切换：动态色要按当前外观重解析成 CGColor，图层才跟着变。
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyColor()
    }

    private func applyColor() {
        effectiveAppearance.performAsCurrentDrawingAppearance { [self] in
            dot.backgroundColor = color.cgColor
        }
    }
    #else
    override var intrinsicContentSize: CGSize { CGSize(width: dotSize, height: dotSize) }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        applyColor()
    }

    private func applyColor() {
        dot.backgroundColor = color.resolvedColor(with: traitCollection).cgColor
    }
    #endif
}

#if os(macOS)
private struct BreathingDotLayerView: NSViewRepresentable {
    var size: CGFloat
    var color: Color

    func makeNSView(context: Context) -> BreathingDotHostView {
        BreathingDotHostView(size: size, color: NSColor(color))
    }

    func updateNSView(_ nsView: BreathingDotHostView, context: Context) {
        nsView.color = NSColor(color)
    }

    @MainActor
    func sizeThatFits(
        _ proposal: ProposedViewSize, nsView: BreathingDotHostView, context: Context
    ) -> CGSize? {
        CGSize(width: size, height: size)
    }
}
#else
private struct BreathingDotLayerView: UIViewRepresentable {
    var size: CGFloat
    var color: Color

    func makeUIView(context: Context) -> BreathingDotHostView {
        BreathingDotHostView(size: size, color: UIColor(color))
    }

    func updateUIView(_ uiView: BreathingDotHostView, context: Context) {
        uiView.color = UIColor(color)
    }

    @MainActor
    func sizeThatFits(
        _ proposal: ProposedViewSize, uiView: BreathingDotHostView, context: Context
    ) -> CGSize? {
        CGSize(width: size, height: size)
    }
}
#endif
