#if os(macOS)
import SwiftUI

/// Todo 状态圆圈（Todo #11）——逻辑照抄提醒事项/Todo App 左侧圆圈：
/// 待办 = 空心圆；进行中 = 有填充且缓慢呼吸；已完成 = 有填充、不呼吸。
///
/// 外观全部由 `TodoListPresentation.statusIcon`（纯逻辑、有单测）决定，
/// 这里只负责画 + 跑动画。概览面板与详细窗口共用。
struct CrewTodoStatusCircle: View {
    let status: String
    var size: CGFloat = 13

    private var icon: TodoListPresentation.StatusIcon {
        TodoListPresentation.statusIcon(status)
    }

    private var tint: Color {
        switch status {
        case "in_progress": return Theme.Palette.accent
        case "completed": return Theme.Palette.success
        // 人类原话：「在等我回复的应该是黄色」。琥珀是本仓库既有的那一档提醒色
        // （群聊/侧栏共用），不为这一处新造颜色。
        case LocalTodoItem.blockedOnHumanStatus: return Theme.Palette.amber
        // 「已叫停」：静音色。它不是坏事、也不是成就，不该抢眼。
        case LocalTodoItem.droppedStatus: return Theme.Palette.inkMuted
        default: return Theme.Palette.inkMuted
        }
    }

    /// 呼吸交给 CoreAnimation（`BreathingSymbol`）——这里**不许**出现 SwiftUI 的
    /// `.repeatForever`：它长在 Todo 面板的 `LazyVStack` 里，会把显示周期顶成
    /// 死循环、被 AppKit 抛异常打死进程（2026-08-10 20:49 闪退，完整验尸见
    /// `BreathingSymbolView.swift` 顶部）。状态翻转时相位自然重置 —— 不呼吸的
    /// 状态压根不建图层，从「进行中」翻到「完成」停在满状态，不留残影。
    var body: some View {
        BreathingSymbol(symbol: icon.symbol, pointSize: size, tint: tint,
                        breathing: icon.isBreathing)
            .accessibilityLabel(TodoListPresentation.statusAccessibilityLabel(status))
            .help(TodoListPresentation.statusAccessibilityLabel(status))
    }
}
#endif
