import CoreGraphics

/// 群聊「跟随底部 / 未读」的判定（人类 Todo #28 → #45 → #47，第三次修同一件事）——
/// 纯逻辑，不碰视图。
///
/// ## 三条要的行为（Todo #47 定稿）
///
/// 1. 点进任一 crew → 停在最底部，最新一条完整可见。
/// 2. 新消息到达、当前**在底部** → 显示并继续贴底。
/// 3. 新消息到达、当前**不在底部** → 位置不动，右下角出一个箭头 + 未读数字；
///    点它才落底并清零。
///
/// ## 前两次为什么没守住（2026-08-11 查清，Todo #47）
///
/// #28（`55efea6a`）加的三处滚底一处都没赶上首屏 —— 那次的诊断在 #45 里写过。
/// #45（`a5cd588c`，11:52 合 main，14:24 打进 0.1.8，人类 16:52 装、16:53 重开）
/// **确实进了人类在跑的那个构建**，然后他仍然报「还没解决好」。所以不是「被后来的
/// 改动撞掉」——#443 的渲染窗口 18:44 才合，根本不在那个构建里。是 **#45 的修法
/// 自己没生效**，病根就在下面这一条：
///
/// > #45 引入的跟随开关 `Pin`，被**我们自己的程序化滚动**关掉了。
///
/// 当时的 `BottomPinTracker` 写的是 `onScrollPhaseChange { _, phase, _ in
/// guard phase == .idle ... }` —— **把 oldPhase 丢了**。于是「用户滑上去停住」
/// （interacting/decelerating → idle）和「我们自己 `scrollTo` 的那一下收尾」
/// （animating → idle）走的是同一条路。而 `LazyVStack` 在程序化滚动收尾的那一刻
/// 往往还在用估算行高，内容高度随后才被修正；那一瞬读到的几何就是「没到底」——
/// 跟随被**自己刚要修的那件事**关掉，此后每一记 `landAtBottom` 全成空转，
/// 包括 #45 专门为「第二波数据改行高」补的那一记。修法把自己的保险丝烧了。
///
/// 所以这一版的核心不变式是：**程序化滚动的收尾只许把跟随打开，永远不许关掉**
/// （`settled(atBottom:byUser:)`）。关掉只能是用户自己滑出来的。
///
/// ## 第四次：为什么还会「有时底、有时顶」（2026-08-12 查清，Todo #54）
///
/// #47 之后跟随开关不再被自己关掉了，可人类仍报「有时落在最新、有时落在最早」。
/// 病根**不在跟随开关，在那两记落底跑得太早**：
///
/// > 首屏的两记落底（条目数 0→N、行高输入变了）在本地路径上落在**同一个 runloop
/// > 回合**里，此时 `LazyVStack` 一行真实高度都还没量过。
///
/// `PendingCrewBackend` 是 `@MainActor` 协议，`LocalPendingCrewBackend` 的
/// `listCrewWhiteboard` / `listCrewMembers` 方法体里**一次 `await` 都没有**（纯内存
/// 读 store）。于是 `refresh()` 里 `entries = …` 与 `members = … / captainBotId = …`
/// 两次 `@State` 写入之间从不让出主线程，SwiftUI 把它们合并成**同一次** body 更新，
/// 两个 `onChange` 背靠背触发 —— 两记 `scrollTo(tail, anchor: .bottom)` 解目标偏移时
/// 手里只有懒容器的**估算行高**。估算远小于真实（气泡有头像列、名字行、多行正文、
/// 图片），解出来的偏移量很小，内容按估算算不满一屏时**就是 0** —— 那正是「停在最早
/// 那条」。之后真实行高被量出来、内容撑高，而这条路上再没有任何一记确定性落底补正。
///
/// 「有时又是对的」= 这一趟碰巧多了一记**晚到**的落底：这个 crew 正好有 session 在跑
/// （`CrewTypingIndicatorRow` 冒出来时会回调一次落底）、第二波数据真的晚了一拍、或者
/// 别的什么让 body 多更新了一轮。那一记发生在量完之后，落点就对。所以人类看到的是
/// 「有时底、有时顶」这种像随机的表现 —— 它其实是「落底跑在测量前 vs 跑在测量后」。
///
/// 修法是补上**第三个执行者**：`shouldLandOnContentGrowth` —— 「内容真实高度长高了」
/// 这个离散事件本身。它天然发生在测量之后，两条路径由此归一。
///
/// ## 为什么不能用定时器兜底
///
/// 「滚动锚点 + 程序化 scrollTo + 永不结束的动画」三者凑齐会把布局打成自激、进程被
/// AppKit 打死（2026-07-26 事故，见 `TypingDotsLayerView` 顶部与 `LayoutLoopRegressionTests`）。
/// 所以这里要的是「内容真正稳定后确定性地落一次」，**不是**「多滚几次总有一次对」。
/// 触发点只有离散事件：条目数变了、行高输入变了、内容真实高度长高了、用户点了箭头、
/// 自己发了一条。
enum CrewChatBottomFollow {

    /// 判定「已在底部」的容差（点）。一行气泡（头像 + 名字行 + 一行正文）远高于它，
    /// 所以不会把「差一整行」误判成到底；又足够吸收滚动惯性停下时的零点几点残差。
    static let bottomSlack: CGFloat = 40

    /// 落底滚到的锚点 id。
    ///
    /// 它是一条挂在 `LazyVStack` **最末尾**、在所有消息行之后的 1pt 透明哨兵 ——
    /// 落点因此恒等于「整条时间线的末端」，而不是「渲染窗口的边界」或「最后一条
    /// 消息的顶边」。滚到它、锚 `.bottom`，最新那条才完整可见（Todo #47 行为 1）。
    ///
    /// 常量放在这里是为了让测试能钉住「视图里用的就是这一个」——历史上这屏出过
    /// 「滚到某条 id」在懒容器里拿旧下标跑行闭包而 trap 的事（2026-08-07），
    /// 哨兵是那次换来的做法，别改回滚到某条消息 id。
    static let bottomAnchorID = "tail"

    /// 滚动视图是否停在底部（含容差）。
    ///
    /// 取 UIScrollView/NSScrollView 的通用几何：偏移量的取值范围是
    /// `-insetTop ... contentHeight + insetBottom - containerHeight`。顶部 inset 已经体现在
    /// 最小偏移的负值里，**不能再加进最大偏移**；加了会让真实到底仍差一个 top inset，
    /// 正是 Todo #89「已经滑到底，未读按钮仍不消失」的根因。
    /// 内容比容器短时上界会小于下界，此时恒为「在底部」——短内容本来就整屏可见。
    static func isAtBottom(
        contentOffsetY: CGFloat,
        containerHeight: CGFloat,
        contentHeight: CGFloat,
        insetTop: CGFloat = 0,
        insetBottom: CGFloat = 0,
        slack: CGFloat = bottomSlack
    ) -> Bool {
        let maxOffsetY = contentHeight + insetBottom - containerHeight
        return contentOffsetY >= maxOffsetY - slack
    }

    /// 影响**行高**的那几样异步输入揉成一个可比较的令牌。它一变 = 每行的外观可能变了
    /// （`CrewChatAdapter.adapt` 的 `isMine` / `groupSender` 都吃这几样），滚动偏移
    /// 需要重新落底。
    ///
    /// 只取 id 与数量，不含显示名之类不改高度的字段 —— 令牌变一次就滚一次，别让无关
    /// 字段的抖动带出多余滚动。
    static func layoutToken(
        memberIds: [String],
        captainBotId: String?,
        localUserId: String?
    ) -> String {
        "\(memberIds.count)|\(memberIds.joined(separator: ","))|\(captainBotId ?? "-")|\(localUserId ?? "-")"
    }

    // MARK: - 这次「停稳」是谁造成的

    /// SwiftUI `ScrollPhase` 的中立镜像。`ScrollPhase` 只在 macOS 15 / iOS 18 起有、
    /// 且拖着 SwiftUI 依赖，测不了；视图侧把它映射成这个枚举，判定留在这里可测。
    enum ScrollPhaseKind: Equatable {
        case idle
        case tracking
        case interacting
        case decelerating
        case animating
    }

    /// 回到 idle 的这一次「停稳」，是不是**用户自己滑**出来的。
    ///
    /// `animating → idle` 是我们自己 `scrollTo` 的收尾，**不算**用户意图 ——
    /// 认成用户意图正是 Todo #45 修法失效的病根（见文件头）。
    static func settleIsUserDriven(previous: ScrollPhaseKind) -> Bool {
        switch previous {
        case .tracking, .interacting, .decelerating: return true
        case .animating, .idle: return false
        }
    }

    /// 这个相位是不是「用户的手还在滚动上」（含松手后的惯性）。
    ///
    /// 与 `settleIsUserDriven` 同一套分类，但问的是**当下**而不是「上一段是谁造成的」：
    /// 它用来在 `shouldLandOnContentGrowth` 里挡住「人正往上翻、懒容器把更早那几行真实
    /// 量出来、内容一长高就把他拽回底部」——那会让人根本翻不动。停稳之后跟不跟随由
    /// `settled` 定夺，不归这里管。
    static func isUserActive(_ phase: ScrollPhaseKind) -> Bool {
        switch phase {
        case .tracking, .interacting, .decelerating: return true
        case .animating, .idle: return false
        }
    }

    /// 「用户的手还在滚动上」这个瞬时标志的持有者 —— **引用型，故意不进 SwiftUI 依赖图**。
    ///
    /// 为什么不塞进 `Pin`（那是个 `@State` 值）：相位一变就写 `@State` 会让 body 失效，
    /// 而 body 失效意味着 `LazyVStack` 全量重新测量整条消息列表（#443 那两份真实 hang
    /// 报告的热点栈就是这个），在**滚动手势刚开始**那一下做这件事最伤。这个标志没有任何
    /// 视图外观依赖它，只被 `shouldLandOnContentGrowth` 在回调里读一次 —— 所以放引用型
    /// 盒子里改（与 `CrewBubbleSelectionOwner` 同一套做法：`@State` 只用来拿一个跨 body
    /// 稳定的实例，从头到尾没人给那个 `@State` 赋值）。
    final class ScrollPhaseBox {
        private(set) var isUserScrolling = false

        init() {}

        /// 滚动相位变了。只记「手在不在上面」，不碰跟随/未读 —— 那仍归 `Pin.settled`。
        func phaseChanged(to phase: ScrollPhaseKind) {
            isUserScrolling = CrewChatBottomFollow.isUserActive(phase)
        }
    }

    // MARK: - 第三个执行者：内容真实高度长出来了

    /// 「长高了」的判定阈值（点）。亚像素级的抖动不算长高。
    static let growthEpsilon: CGFloat = 0.5

    /// 内容高度变了，要不要补一记落底（Todo #54）。
    ///
    /// 首屏那两记落底跑在 `LazyVStack` 量出真实行高**之前**（原因见文件头），落点因此
    /// 偏上甚至就是顶。真实行高展开、内容高度长高本身是个**离散事件**，且天然发生在
    /// 测量之后 —— 拿它当第三个执行者，首屏无论数据早到晚到都确定性落一次底。
    ///
    /// 三条守则，都是为了不把 2026-07-26 / 2026-08-10 那两次布局自激种回来：
    ///
    /// 1. **只认长高**，回缩一律不动。落底会让懒容器 realize 更多行，也可能把离屏行的
    ///    测量丢回估算而让总高**回缩** —— 只认长高，我们自己的动作就无法经由「缩」这条
    ///    边回头触发自己，环缺一条边。
    /// 2. **用户的手在上面时不动**（`isUserScrolling`）—— 见 `isUserActive`。
    /// 3. **不跟随就不动**（`isFollowing`）—— 用户滑上去看历史时新消息只记未读（行为 3）。
    ///
    /// 收敛性：渲染窗口内行数有限（`CrewChatWindow.pageSize`），量完就不再变；而且落底
    /// 时已经在底部的话再滚一次是空操作，不产生新的高度变化。**不是**「多滚几次总有
    /// 一次对」——是「每长高一次就把偏移量补正一次，高度一稳就自己停」。
    static func shouldLandOnContentGrowth(
        oldHeight: CGFloat,
        newHeight: CGFloat,
        isFollowing: Bool,
        isUserScrolling: Bool
    ) -> Bool {
        guard isFollowing, !isUserScrolling else { return false }
        return newHeight > oldHeight + growthEpsilon
    }

    // MARK: - 跟随 + 未读

    /// 「跟不跟着底部走」+「攒了多少条没看的」。
    ///
    /// 开屏默认跟随、未读 0：还没人碰过，就该停在最新一条（行为 1）。
    /// 用户自己滑上去看历史 → 松开跟随，此后新消息只记数不移动（行为 3）；
    /// 滑回底部 / 点箭头 / 自己发一条 → 重新挂上并清零。
    struct Pin: Equatable {
        /// 是否跟随底部。
        private(set) var isFollowing: Bool
        /// 松开跟随之后攒下的新消息条数。跟随时恒为 0。
        private(set) var unread: Int

        init(isFollowing: Bool = true, unread: Int = 0) {
            self.isFollowing = isFollowing
            self.unread = unread
        }

        /// 一次滚动停稳了。
        ///
        /// - Parameter atBottom: 停下来的位置在不在底部（`isAtBottom`）。
        /// - Parameter byUser: 这次停稳是不是用户自己滑出来的
        ///   （`settleIsUserDriven`）。
        ///
        /// **不变式**：`byUser == false`（我们自己的程序化滚动收尾）只许把跟随
        /// 打开，永远不许关掉。这条就是 Todo #47 要守的那根弦 —— 少了它，
        /// `LazyVStack` 还在修正估算行高的那一瞬会把跟随关掉，此后所有落底空转。
        mutating func settled(atBottom: Bool, byUser: Bool) {
            if atBottom {
                reachedBottom()
                return
            }
            guard byUser else { return }
            isFollowing = false
        }

        /// 真实滚动位置跨进「已到底」区间 —— 立即恢复跟随并清未读。
        ///
        /// 这与 `settled(atBottom:byUser:)` 分开，是因为滚动相位并不是位置事实：SwiftUI
        /// 可能在手势结束前后少派一次 phase 回调，或程序化滚动的相位路径不同；Todo #56
        /// 现场就是已经到底、按钮却仍留着。视图侧把几何投影成 Bool，只在 false→true
        /// 跨阈值时调用，因此这里也保持幂等，已经干净就不再写状态。
        mutating func reachedBottom() {
            guard !isFollowing || unread != 0 else { return }
            isFollowing = true
            unread = 0
        }

        /// 用户的滚动已经把视口带离底部 —— **立即**松开跟随，不等手势停稳。
        ///
        /// 成熟 IM 的锚定边界看的是位置事实，不是「滚动结束」事件：新消息可能恰好在
        /// tracking / decelerating 中途到达。如果等 idle 才关，`received` 仍会认为人在底部，
        /// 当场把视口拽回去，表现就是 Todo #89 的「看历史时来消息会跳一下」。
        mutating func leftBottomByUser() {
            guard isFollowing else { return }
            isFollowing = false
        }

        /// 来了 `added` 条新消息。返回 **true = 该落底**（行为 2）；
        /// 返回 false 时未读已经加上去了，位置一动不动（行为 3）。
        @discardableResult
        mutating func received(_ added: Int) -> Bool {
            guard added > 0 else { return false }
            if isFollowing {
                unread = 0
                return true
            }
            unread += added
            return false
        }

        /// 自己经 composer 发了一条 —— 无条件贴底、清未读。
        ///
        /// IM 通行做法（iMessage / Slack / 微信都是）：自己说话就把视线带回最新，
        /// 不该让人发完还得手动滑下去，更不该给自己发的话记一条未读。
        mutating func didSendOwnMessage() {
            isFollowing = true
            unread = 0
        }

        /// 点了右下角那个箭头：落底 + 清零 + 重新挂上跟随。
        mutating func jumpToBottom() {
            isFollowing = true
            unread = 0
        }
    }
}
