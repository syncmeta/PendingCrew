#if os(macOS)
import SwiftUI
import AppKit
import SwiftTerm

/// 把一个本地 PTY 的 LocalProcessTerminalView 宿主进 SwiftUI。
struct AgentTerminalView: NSViewRepresentable {
    let terminalView: ActivityTerminalView
    /// 当前外观（跟随系统/应用的浅深主题）。SwiftTerm 自己画一层不透明底，**不跟**
    /// macOS appearance —— 默认黑底白字，于是浅色主题下也是黑的。这里把它接上：
    /// 按 colorScheme 设 nativeBackground/Foreground（写进 terminal 的底色/默认前景，
    /// 也让 OSC 11 背景查询拿到正确值，TUI 能据此挑浅/深配色）。
    @Environment(\.colorScheme) private var colorScheme

    func makeNSView(context: Context) -> LocalProcessTerminalView {
        let view = terminalView
        // 紧凑等宽字体 —— SwiftTerm 默认 13pt 系统等宽，cell ~7.8pt，在 ~400pt 宽的
        // inspector 栏里只排得下 ~50 列；而 Claude/Codex 的全屏 TUI（边框盒、开场横幅）
        // 需要 ~58–80 列，列不够时盒子比实际网格宽 → 右边框换行落到空行 → 整屏被撑成
        // 竖向稀疏 + 残缺竖线（就是用户对比图里那种乱）。调到 11pt（cell ~6.6pt）多挤出
        // 一截列数，配合放宽 inspector 列宽（见 MacRootView），TUI 才排得正常。
        view.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        applyTheme(view, scheme: colorScheme)
        // 终端基于网格(默认 ~80 列)有个很宽的 intrinsicContentSize + 默认 750 的
        // 水平抗压缩优先级。宿主进窄的 inspector 栏时,NSSplitView 不肯把它压到
        // intrinsic 宽以下,会把整栏顶宽;栏比它窄就两边溢出被切(必须拖宽才正常)。
        // 调低水平抗压缩/抱紧优先级,让栏能自由把终端压到可用宽度——终端会按实际
        // 宽度重排列数,而不是顶死栏宽。
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        return view
    }

    func updateNSView(_ nsView: LocalProcessTerminalView, context: Context) {
        // 主题切换（系统/应用浅深）时重涂底色/前景 —— nativeBackgroundColor 在 set 时
        // 立刻把动态 NSColor 求值成固定终端色，不会自动跟 appearance 变，必须这里重设。
        applyTheme(nsView, scheme: colorScheme)
    }

    private func applyTheme(_ view: LocalProcessTerminalView, scheme: ColorScheme) {
        let dark = scheme == .dark
        // 与 Theme.Palette.canvas / .ink 对齐（浅 #FFFFFF/#1B1A14，深 #161512/#ECE9E0）。
        let bg = NSColor(srgb: dark ? 0x161512 : 0xFFFFFF)
        let fg = NSColor(srgb: dark ? 0xECE9E0 : 0x1B1A14)
        view.nativeBackgroundColor = bg
        view.nativeForegroundColor = fg
        view.caretColor = fg
    }
}

private extension NSColor {
    convenience init(srgb hex: UInt32) {
        self.init(
            srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
            green:   CGFloat((hex >>  8) & 0xFF) / 255,
            blue:    CGFloat( hex        & 0xFF) / 255,
            alpha:   1)
    }
}
#endif
