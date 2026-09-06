#if os(macOS)
import Foundation

/// 终端响铃（BEL, 0x07）的**痕迹账**（人类 Todo #110）。
///
/// ## 它替掉的是什么
///
/// SwiftTerm 的 `TerminalViewDelegate` 给 `bell(source:)` 留了默认实现，函数体
/// 只有一行 `NSSound.beep()`。`TerminalMirrorView` 一直没实现这条回调，于是
/// agent 每吐一个 BEL，macOS 就放一声系统提示音（就是「弹框弹出、点框外面」
/// 那声）—— 人听得见，却不知道是谁在响。
///
/// **但不能简单静音。**BEL 在 agent 手里的语义就是「我要叫人」（claude 一轮干完
/// 或需要输入时会敲一下），静音等于把这句话吞了，只剩「安静的失败」。所以这里换的
/// 是**输出通道**而不是把它关掉：不发声，改成记一条看得见、事后也查得到的痕迹。
///
/// ## 两层，故意分开
///
/// - `unseen` 驱动**当下的提示**：只在「你上次看过这个 session 之后又响过」时亮。
///   一个永远亮着的角标会训练人忽略它 —— claude 一天能敲几十下 BEL，长亮 = 没信息。
/// - `count` / `lastAt` 是**留下来的痕迹**：哪怕提示已经消了，「这个 session 响过
///   几次、最后一次什么时候」仍答得出来（`summary` 挂在 session 行上，悬停可见）。
///
/// 纯值类型、不碰 AppKit —— 判定全在这里，视图只负责画。
struct TerminalBellTrace: Equatable {
    /// 这个 session 一共响过几次。
    private(set) var count = 0
    /// 最后一次响铃时刻；从没响过时 nil。
    private(set) var lastAt: Date?
    /// 人上次看这个 session 时 `count` 是多少。
    private var acknowledged = 0

    /// 响了一次。
    mutating func record(at when: Date = Date()) {
        count += 1
        lastAt = when
    }

    /// 人看过这个 session 了 —— 提示消掉，但痕迹（count / lastAt）留着。
    mutating func acknowledge() {
        acknowledged = count
    }

    /// 上次看过之后又响了几次。
    var unseen: Int { count - acknowledged }

    /// 此刻该不该在 session 行上亮提示。
    var showsHint: Bool { unseen > 0 }

    /// 悬停可见的那句痕迹。**从没响过时返回 nil** —— 不占位、不长期挂一句
    /// 「一切正常」，那种尺子只会训练人不看它。
    /// - Parameter timeText: `lastAt` 的显示文本（格式化归视图，这里只拼句子）。
    static func summary(count: Int, timeText: String) -> String? {
        guard count > 0 else { return nil }
        return "响铃 \(count) 次 · 最近 \(timeText)"
    }
}
#endif
