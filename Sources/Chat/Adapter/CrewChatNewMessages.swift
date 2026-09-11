import Foundation

/// 「来了 N 条新消息」这一拍，窗口和跟随该怎么变（人类 Todo #144）。
///
/// ## 为什么要单独有这个类型
///
/// 这套状态转移原本整段长在 `CrewChatView` 的
/// `.onChange(of: timelineEntries.count)` 闭包里，而**那个文件不进 test bundle**。
/// 于是 `CrewChatWindow.afterInsert` 有一整排用例、`Pin.received` 也有，
/// **真实调用路径却一条尺子都没有** —— 谁把 `isFollowing:` 从调用点漏掉，
/// 两边的用例照样全绿，而人翻历史时照样跳。
///
/// **这是本仓两天内第三次「建好了没接上」。** 所以这一笔不是「改一行调用」，
/// 是把那一拍的判定搬到有尺子的地方。
///
/// ## 判据
///
/// **用户已经滑走（`!isFollowing`）时，窗口里最顶那条在新消息到达前后必须是同一条。**
enum CrewChatNewMessages {

    struct Outcome: Equatable {
        /// 这一拍之后的渲染窗口上限。
        var renderLimit: Int
        /// 这一拍之后的跟随/未读状态。
        var pin: CrewChatBottomFollow.Pin
        /// 要不要补一记落底（行为 2：还在跟随就跟着走）。
        var shouldLandAtBottom: Bool
    }

    /// 新消息到达。
    ///
    /// 两件事在同一拍里定：窗口上限怎么走（别把人正看的东西剪掉），
    /// 以及跟不跟着落底（`Pin.received` 说了算）。
    static func apply(added: Int,
                      renderLimit: Int,
                      pin: CrewChatBottomFollow.Pin) -> Outcome {
        // 先按**进这一拍时**的跟随状态定窗口 —— 这个问题是「能不能剪掉最顶那条」。
        let limit = CrewChatWindow.afterInsert(
            limit: renderLimit, added: added, isFollowing: pin.isFollowing)
        // 再问「要不要滚到底」。`received` 只动 unread，不动 isFollowing，
        // 但把顺序写成先读后改，读的人不必去核这一点。
        var pin = pin
        let land = pin.received(added)
        return Outcome(renderLimit: limit, pin: pin, shouldLandAtBottom: land)
    }

    /// 跟随开关翻了。
    ///
    /// - 松开跟随（人往上看）→ 记下此刻的窗口深度。
    /// - 重新跟随（人回到底部）→ **只回吐这一次「往上看」期间涨出来的那一段**，
    ///   回到松开时的深度。
    ///
    /// 为什么不是「一律归位到一页」：他可能在滑走之前就点过「加载更早」，
    /// 那段是他特意翻出来的，**回到底部不该把它收回去**。
    /// 为什么要回吐：不回吐的话，一次长时间的「往上看」会让窗口随着聊天一路长大，
    /// #443 那道成本封顶就白做了。
    ///
    /// - Returns: 新的窗口上限，以及新的「松开时深度」记号（跟随中恒为 nil）。
    static func followChanged(isFollowing: Bool,
                              renderLimit: Int,
                              limitBeforeExcursion: Int?)
        -> (renderLimit: Int, limitBeforeExcursion: Int?) {
        guard isFollowing else {
            // 已经记过就别覆盖：一次「往上看」里跟随开关可能抖动好几次
            // （手势相位 + 几何投影两条路都会写它），覆盖会把基准一路抬高。
            return (renderLimit, limitBeforeExcursion ?? renderLimit)
        }
        guard let before = limitBeforeExcursion else { return (renderLimit, nil) }
        return (min(renderLimit, before), nil)
    }
}
