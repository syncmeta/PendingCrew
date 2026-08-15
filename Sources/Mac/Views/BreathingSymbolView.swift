#if os(macOS)
import SwiftUI
import AppKit

/// 一个会「呼吸」的 SF Symbol —— **动画跑在 CoreAnimation 里，不进 SwiftUI 的动画图**。
///
/// 用在 Todo 状态圆圈上（`CrewTodoStatusCircle`）：「进行中」缓慢明暗+缩放脉动，
/// 其余状态静止。
///
/// ## 为什么必须 CoreAnimation（2026-08-10 20:49 那次闪退的根因）
///
/// 这是 2026-07-26 那次的**同一个病根换了个地方复发**。原来这里写的是
/// `.opacity(…).scaleEffect(…).animation(.easeInOut.repeatForever(autoreverses:), value:)`，
/// 由 `.task(id:)` 点火。三样凑齐就成环：
///
/// - `repeatForever` 在 SwiftUI 的动画事务里**永不结束**，每帧都让视图图重解析；
/// - 它长在 Todo 面板的 `LazyVStack` 里，懒容器每次重解析都要重算可见窗口、
///   把子图摘下插回（崩溃栈里的 `LazyLayoutCacheItem` 字典 filter →
///   `AGSubgraphRef.didReinsert`）；
/// - 重插的子图带 appearance effect（`.task` 与 `.onAppear` 同源），
///   `AppearanceEffect.didReinsert` → `graphDidChange` → `NSHostingView.requestUpdate`
///   → `setNeedsUpdateConstraints`，把窗口重新标脏。
///
/// 于是一个显示周期永远结束不了：AppKit 的统一日志里能看到
/// `NSDisplayCycleFlush restarting…` 一圈圈重来、`Marking window … (limit: 275, count: 276…)`
/// 一路涨，涨过 275 就抛 NSException，`+[NSApplication _crashOnException:]` 打死进程。
/// 全程不需要任何人操作 —— 只要有一条 Todo 处于「进行中」。
///
/// ⚠️ 这条环比 2026-07-26 那条**慢得多**（约 9.4Hz，每圈 ~106ms，而那条是上万 Hz），
/// 所以旧回归用例「安静 2 秒内 layout() < 50 次」根本看不见它。判据见
/// `LayoutLoopRegressionTests`：安静窗口里健康是 **0**，别把阈值放宽。
///
/// ## 这里怎么做到「动画不进布局」
///
/// 布局与基线仍由 SwiftUI 的 `Image` 出（`HStack(alignment: .firstTextBaseline)` 靠它
/// 对齐文字），呼吸态时把它 `opacity(0)` 藏起来当量尺；真正在动的那张图画在
/// `overlay` 里的 `CALayer` 上 —— overlay 不参与布局，图层的 `opacity` /
/// `transform.scale` 也不回流到 SwiftUI 的几何。于是 SwiftUI 侧自始至终是静态的。
struct BreathingSymbol: View {
    let symbol: String
    let pointSize: CGFloat
    let tint: Color
    /// 是否呼吸。false 时连图层都不建，就是一个普通 `Image`。
    let breathing: Bool

    /// `accessibilityReduceMotion` 打开时不加动画，退化成静态符号 ——
    /// 信号（有填充的实心圈）不丢，动效才是可选的那部分。
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var animates: Bool { breathing && !reduceMotion }

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: pointSize, weight: .regular))
            .foregroundStyle(tint)
            // 呼吸态：SwiftUI 这张只当量尺 + 基线，画面交给下面的图层。
            .opacity(animates ? 0 : 1)
            .overlay {
                if animates {
                    BreathingSymbolLayerView(symbol: symbol, pointSize: pointSize, color: tint)
                }
            }
    }
}

/// 单个 `CALayer`（内容是着色后的符号图）+ 两条 `CABasicAnimation`
/// （opacity 1→0.35、scale 1→0.88，autoreverse + repeatForever）。
/// 时长 1.1s 与原 SwiftUI 版一致，看上去没有任何变化。
private final class BreathingSymbolHostView: NSView {
    private let glyph = CALayer()
    private var symbol: String
    private var pointSize: CGFloat
    private var color: NSColor

    init(symbol: String, pointSize: CGFloat, color: NSColor) {
        self.symbol = symbol
        self.pointSize = pointSize
        self.color = color
        super.init(frame: .zero)
        wantsLayer = true
        glyph.contentsGravity = .resizeAspect
        layer?.addSublayer(glyph)
        addPulse()
        redrawGlyph()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func update(symbol: String, pointSize: CGFloat, color: NSColor) {
        guard symbol != self.symbol || pointSize != self.pointSize || color != self.color else { return }
        self.symbol = symbol
        self.pointSize = pointSize
        self.color = color
        redrawGlyph()
    }

    /// 图层跟着视图的 bounds 走。`layout()` 里改 sublayer 的 frame 不碰约束，
    /// 不会把窗口标脏 —— 这正是整条修复的要点。
    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)   // 别让 frame 变化本身也动画
        glyph.frame = bounds
        CATransaction.commit()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        glyph.contentsScale = window?.backingScaleFactor ?? 2
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        redrawGlyph()
    }

    private func addPulse() {
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 1.0
        fade.toValue = 0.35
        let shrink = CABasicAnimation(keyPath: "transform.scale")
        shrink.fromValue = 1.0
        shrink.toValue = 0.88
        for a in [fade, shrink] {
            a.duration = 1.1
            a.autoreverses = true
            a.repeatCount = .infinity
            a.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        }
        glyph.add(fade, forKey: "breathe.opacity")
        glyph.add(shrink, forKey: "breathe.scale")
    }

    /// SF Symbol 是模板图，`CALayer.contents` 不做着色 —— 自己按当前外观着一次色。
    private func redrawGlyph() {
        effectiveAppearance.performAsCurrentDrawingAppearance { [self] in
            let config = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .regular)
            guard let base = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
                .withSymbolConfiguration(config) else {
                glyph.contents = nil
                return
            }
            let rect = NSRect(origin: .zero, size: base.size)
            let tinted = NSImage(size: base.size)
            tinted.lockFocus()
            base.draw(in: rect)
            color.set()
            rect.fill(using: .sourceAtop)
            tinted.unlockFocus()
            glyph.contents = tinted.cgImage(forProposedRect: nil, context: nil, hints: nil)
        }
    }
}

private struct BreathingSymbolLayerView: NSViewRepresentable {
    var symbol: String
    var pointSize: CGFloat
    var color: Color

    func makeNSView(context: Context) -> BreathingSymbolHostView {
        BreathingSymbolHostView(symbol: symbol, pointSize: pointSize, color: NSColor(color))
    }

    func updateNSView(_ nsView: BreathingSymbolHostView, context: Context) {
        nsView.update(symbol: symbol, pointSize: pointSize, color: NSColor(color))
    }

    /// 恒等于外面给的提案 —— overlay 本就不参与布局，这里再钉一次「几何是静态的」。
    @MainActor
    func sizeThatFits(
        _ proposal: ProposedViewSize, nsView: BreathingSymbolHostView, context: Context
    ) -> CGSize? {
        CGSize(width: proposal.width ?? pointSize, height: proposal.height ?? pointSize)
    }
}
#endif
