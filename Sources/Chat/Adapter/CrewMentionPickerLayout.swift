import CoreGraphics

/// `CrewMentionPicker` 的**高度判定**（Todo #69 第 1 条）—— 纯几何，不碰 SwiftUI，
/// 所以能被单测钉住。人类原话：「输入框打@之后 出来好长一条列表 太长太长了 不行
/// 要限定高度 然后在这个限高的窗口内滑动」。
///
/// ## 为什么不是一个写死的 pt 值
///
/// 这个列表有多长只取决于**成员数**：本机的 crew 动辄四十几个成员，四十几行乘一行
/// 34pt ≈ 1400pt —— 任何屏幕都顶穿。但「钉死 240pt」也不对：窗口被拖到很矮时
/// （composer 上方只剩 120pt），240pt 照样顶穿，只是换了个高度重犯同一个错。
///
/// 所以上限**跟着可用空间走**：宿主（`CrewChatView`）用 GeometryReader 量出
/// 「群聊那一栏的高 − composer 那一截的高」＝ composer 上方真正剩下的空间，按
/// `availableShare` 取一部分给这个浮层（它是临时浮层，不该把聊天记录整个盖住），
/// 这里再夹上下两道边。
///
/// ## 三道边，各自防一件具体的事
///
/// 1. **上界 = 内容自身高度**。候选只有 2 个时不许撑出一个空盒子 —— 少候选时它就是
///    今天这个大小，视觉一点不变。
/// 2. **下界 = `minVisibleRows` 行 + 半行**。窗口再矮也得看得见几行、并且**看得出
///    还能滚**；半行露头就是那个「还有」的信号（滚动条在 macOS 上默认是隐藏的，
///    只靠它等于没有提示）。
/// 3. **硬顶 = `availableHeight` 本身**。下界和可用空间打架时**可用空间赢** ——
///    否则「保底露 3 行」会在矮窗口里把浮层重新顶穿，正是本条要修的那个 bug。
///
/// 需要裁剪时高度**落在半行上**（`k + 0.5` 行），让最后一行被切一半 —— 这是刻意的，
/// 见第 2 条。
enum CrewMentionPickerLayout {

    // MARK: - 与 CrewMentionPicker 的实际渲染对齐的常量

    /// 一行候选的高度：头像 20pt + 上下各 7pt padding。
    static let rowHeight: CGFloat = 34
    /// 行间那条 `Divider()`。
    static let dividerHeight: CGFloat = 1
    /// 浮层自身的上下留白（`.padding(.vertical, 2)`）。
    static let chromeHeight: CGFloat = 4

    // MARK: - 政策

    /// 可用高度里分给这个浮层的比例。它是临时浮层，把聊天记录整个盖住会让人失去
    /// 上下文（正在回谁的话就看不见了），一半是「够翻」和「还看得见在聊什么」的折中。
    static let availableShare: CGFloat = 0.5
    /// 保底露出的整行数（外加半行露头，见类型注释第 2 条）。
    static let minVisibleRows: CGFloat = 3
    /// 宿主还没量出可用高度时（首帧、或没接测量的宿主）的兜底行数上限。
    /// **不是回到「无上限」** —— 量不到也不许顶穿。
    static let fallbackRows: CGFloat = 8

    // MARK: - 几何

    /// `rowCount` 行全部展开时的高度。
    static func contentHeight(rowCount: Int) -> CGFloat {
        guard rowCount > 0 else { return chromeHeight }
        return CGFloat(rowCount) * rowHeight
            + CGFloat(rowCount - 1) * dividerHeight
            + chromeHeight
    }

    /// 恰好露出 `rows` 行的高度。`rows` 可以带小数 —— `3.5` 就是「3 行整 + 半行露头」。
    static func height(showingRows rows: CGFloat) -> CGFloat {
        guard rows > 0 else { return chromeHeight }
        return rows * (rowHeight + dividerHeight) - dividerHeight + chromeHeight
    }

    /// 这次该给浮层多高。
    ///
    /// - Parameters:
    ///   - availableHeight: composer 上方剩下的高度（宿主量的）。`<= 0` = 没量到，
    ///     走 `fallbackRows` 兜底。
    ///   - rowCount: 当前候选条数。
    /// - Returns: 浮层的 `maxHeight`。**恒 ≤ 内容高度**，所以候选少时不会撑出空白。
    static func maxHeight(availableHeight: CGFloat, rowCount: Int) -> CGFloat {
        let content = contentHeight(rowCount: rowCount)
        guard rowCount > 0 else { return content }

        guard availableHeight > 0 else {
            return min(content, height(showingRows: fallbackRows))
        }

        // 够放就原样放 —— 少候选时视觉与改动前逐字相同。
        let share = availableHeight * availableShare
        if content <= share { return content }

        // 下界（保底几行 + 半行露头）与可用空间打架时，可用空间赢。
        let floorHeight = height(showingRows: minVisibleRows + 0.5)
        let capped = min(max(share, floorHeight), availableHeight)
        if content <= capped { return content }

        // 要裁了 —— 落在半行上，让最后一行被切一半，一眼看出「还能滚」。
        let fittingRows = (capped - chromeHeight + dividerHeight) / (rowHeight + dividerHeight)
        let wholeRows = max(1, (fittingRows - 0.5).rounded(.down))
        return min(height(showingRows: wholeRows + 0.5), capped)
    }
}
