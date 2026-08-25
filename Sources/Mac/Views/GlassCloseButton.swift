#if os(macOS)
import SwiftUI

/// 子窗口/浮层的**关闭件**（Todo #22）—— 玻璃白圆形叉，全 app 只此一处定义。
///
/// 人类原话：「驾驶舱的关闭按钮 请使用玻璃白按钮样式 这个按钮和各个子窗口统一」。
/// 所以它不是某一处的私有样式，而是所有「浮在内容上、要关掉」的地方共用的那颗按钮：
/// 驾驶舱临时窗口（`CockpitView` 头部）等。**新增浮层
/// 一律用它**，别再复制一遍圆形+叉的样式代码。
///
/// 为什么是 `.regularMaterial` 而不是实色：浮层压在群聊之上，关闭件得同时读得出
/// （不被底下内容吃掉）又不喧宾夺主。材质会采样背后内容做半透明模糊 —— 深浅两种外观
/// 下都自动成立，不用为深色模式再配一套色（部署目标 macOS 14，没有 Liquid Glass API，
/// 材质 + 一圈 hairline 是这一代能拿到的「玻璃白」）。
struct GlassCloseButton: View {
    let action: () -> Void
    /// tooltip 文案；默认「关闭」，调用方可补快捷键提示（如「关闭驾驶舱（Esc）」）。
    var help: String = "关闭"
    /// 圆形直径。默认 22 —— 与红绿灯（20）同一量级，指哪打哪又不抢戏。
    var diameter: CGFloat = 22

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(.system(size: diameter * 0.42, weight: .bold))
                .foregroundStyle(Theme.Palette.ink)
                .frame(width: diameter, height: diameter)
                .background(.regularMaterial, in: Circle())
                .overlay(Circle().strokeBorder(Theme.Palette.hairline, lineWidth: 1))
                // 悬停时微微提亮 —— 光标落上去要有「这是个按钮」的回应。
                .overlay(Circle().fill(Theme.Palette.ink.opacity(hovering ? 0.08 : 0)))
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(help)
        .accessibilityLabel(help)
    }
}
#endif
