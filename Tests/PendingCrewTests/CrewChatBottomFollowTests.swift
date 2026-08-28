import XCTest
import Foundation
import CoreGraphics
// 待测源码直接编进 test bundle（见 project.yml），不 import app module。

/// 人类 Todo #28 → #45 → #47 → **#54**：点进 crew 群聊要停在最新一条；新消息在底部就
/// 跟着走，不在底部就只记未读、位置不动。
///
/// #54 补的是**时机**那一层：跟随开关在 #47 之后已经不会被自己关掉了，可两记落底双双
/// 跑在 `LazyVStack` 量出真实行高**之前**，落点按估算解出来偏上、算不满一屏时就是顶 ——
/// 人类看到的「有时最新、有时最早」是这个，不是随机。见文件末尾 Todo #54 那一段。
///
/// ## 这一版为什么要比上一版狠
///
/// #45 也留了测试，也全绿，然后**照样没修好** —— 因为它测的是 `isAtBottom` /
/// `layoutToken` 这些零件对不对，没有一条测「这些零件接起来会不会互相拆台」。真正
/// 出错的是接线：跟随开关被**我们自己的程序化滚动**关掉，此后所有落底空转
/// （见 `CrewChatBottomFollow` 文件头）。所以这里补的是**状态机的时序**和**接线的
/// 源码闸**，不是再加几条零件断言。
///
/// 仍然测不了的那一条写在末尾的注释里，别当成已覆盖。
final class CrewChatBottomFollowTests: XCTestCase {

    // MARK: - 在不在底部

    func testAtBottomWhenOffsetReachesTheEnd() {
        // 内容 2000，容器 600 → 到底的偏移是 1400。
        XCTAssertTrue(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: 1400, containerHeight: 600, contentHeight: 2000))
    }

    func testNotAtBottomWhenScrolledUpByMoreThanTheSlack() {
        // 往上翻了一屏 —— 用户在看历史，不该再被拽回底部。
        XCTAssertFalse(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: 800, containerHeight: 600, contentHeight: 2000))
    }

    /// 容差是为了吸收滚动停下时的零点几点残差，不是为了放过「差一整行」。
    func testSlackAbsorbsResidualButNotAWholeRow() {
        XCTAssertTrue(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: 1399.6, containerHeight: 600, contentHeight: 2000),
            "停稳时差零点几点仍算在底部，否则跟随会被自己的滚动残差关掉")
        XCTAssertFalse(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: 1400 - CrewChatBottomFollow.bottomSlack - 1,
            containerHeight: 600, contentHeight: 2000),
            "超出容差就是真的滑走了")
    }

    /// 内容比容器短：整屏都看得见，恒算「在底部」——否则空群/两三条消息的群一进去
    /// 就判成「用户滑走了」，后面的消息全都不跟随。
    func testShortContentIsAlwaysAtBottom() {
        XCTAssertTrue(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: 0, containerHeight: 600, contentHeight: 120))
        XCTAssertTrue(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: 0, containerHeight: 600, contentHeight: 0))
    }

    /// 底部 inset（macOS 的 composer 用 safeAreaInset 压在滚动视图上）算进可滚范围，
    /// 否则「真到底」永远差着一个 composer 的高度，判成没到底。
    func testBottomInsetCountsTowardTheScrollableRange() {
        let insetBottom: CGFloat = 90
        let maxOffset = 2000 + insetBottom - 600
        XCTAssertTrue(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: maxOffset, containerHeight: 600, contentHeight: 2000,
            insetBottom: insetBottom))
        XCTAssertFalse(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: maxOffset - 300, containerHeight: 600, contentHeight: 2000,
            insetBottom: insetBottom))
    }

    /// Todo #89：顶部 inset 只改变最小偏移（顶部是负值），不改变底部最大偏移。
    /// 把它重复加到最大偏移，会让人真实滑到底后仍永远差一个 top inset。
    func testTopInsetDoesNotMoveTheBottomThreshold() {
        let insetTop: CGFloat = 72
        let insetBottom: CGFloat = 90
        let trueBottom = 2000 + insetBottom - 600
        XCTAssertTrue(CrewChatBottomFollow.isAtBottom(
            contentOffsetY: trueBottom,
            containerHeight: 600,
            contentHeight: 2000,
            insetTop: insetTop,
            insetBottom: insetBottom),
            "真实底部不能因 top inset 被误判成还差一截")
    }

    // MARK: - 行高输入令牌（第二波异步数据）

    /// 这几样一到位，`CrewChatAdapter` 判「是不是我发的」就翻面，头像列与名字行跟着
    /// 出现/消失 —— 每行差一整行高度。令牌必须变，否则没人去补落底。
    func testLayoutTokenChangesWhenTheSecondWaveLands() {
        let before = CrewChatBottomFollow.layoutToken(
            memberIds: [], captainBotId: nil, localUserId: nil)
        let membersLanded = CrewChatBottomFollow.layoutToken(
            memberIds: ["m1", "m2"], captainBotId: nil, localUserId: nil)
        let captainLanded = CrewChatBottomFollow.layoutToken(
            memberIds: ["m1", "m2"], captainBotId: "bot-1", localUserId: nil)
        let identityLanded = CrewChatBottomFollow.layoutToken(
            memberIds: ["m1", "m2"], captainBotId: "bot-1", localUserId: "user-1")
        XCTAssertNotEqual(before, membersLanded, "成员名册到位")
        XCTAssertNotEqual(membersLanded, captainLanded, "机长 id 到位")
        XCTAssertNotEqual(captainLanded, identityLanded, "本机 user id 回填")
    }

    /// 同一批数据重复刷新（订阅每 tick 都 refresh 一次）不该让令牌抖动 —— 令牌一变就滚
    /// 一次，抖动 = 白滚。
    func testLayoutTokenIsStableAcrossIdenticalRefreshes() {
        let a = CrewChatBottomFollow.layoutToken(
            memberIds: ["m1", "m2"], captainBotId: "bot-1", localUserId: "user-1")
        let b = CrewChatBottomFollow.layoutToken(
            memberIds: ["m1", "m2"], captainBotId: "bot-1", localUserId: "user-1")
        XCTAssertEqual(a, b)
    }

    /// 成员次序变了（`CrewMemberOrdering` 重排）也算行高输入变 —— 谁排第几决定哪几行
    /// 连着同一个人、要不要重复名字行。这里只钉「不会把不同名册压成同一个令牌」。
    func testLayoutTokenDistinguishesDifferentRosters() {
        XCTAssertNotEqual(
            CrewChatBottomFollow.layoutToken(
                memberIds: ["m1", "m2"], captainBotId: nil, localUserId: nil),
            CrewChatBottomFollow.layoutToken(
                memberIds: ["m2", "m1"], captainBotId: nil, localUserId: nil))
    }

    // MARK: - 这次「停稳」是谁造成的（Todo #45 失效的病根）

    /// 用户自己滑：手指/滚轮在动，或者甩出去之后自己减速停下 —— 这才是「用户意图」。
    func testUserDrivenSettleComesFromUserPhases() {
        for phase in [CrewChatBottomFollow.ScrollPhaseKind.tracking, .interacting, .decelerating] {
            XCTAssertTrue(
                CrewChatBottomFollow.settleIsUserDriven(previous: phase),
                "\(phase) → idle 是用户自己滑完停下")
        }
    }

    /// **本次修复的核心断言**：程序化滚动（`.animating`）收尾不是用户意图。
    /// 认成用户意图 = Todo #45 的修法把自己的保险丝烧了。
    func testProgrammaticSettleIsNotUserDriven() {
        XCTAssertFalse(
            CrewChatBottomFollow.settleIsUserDriven(previous: .animating),
            "animating → idle 是我们自己 scrollTo 的收尾，不许当成用户滑走了")
        XCTAssertFalse(CrewChatBottomFollow.settleIsUserDriven(previous: .idle))
    }

    // MARK: - 跟随开关 + 未读（Todo #47 四条行为）

    /// 行为 1：开屏默认跟随、未读 0 —— 首屏与第二波数据两记落底都得放行。
    func testFollowsByDefaultSoTheFirstScreenLandsAtBottom() {
        let pin = CrewChatBottomFollow.Pin()
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0)
    }

    /// **回归闸（Todo #45 病根）**：我们自己滚了一下，收尾那一瞬 `LazyVStack` 还在
    /// 修正估算行高、几何读出来「没到底」—— 这**不许**关掉跟随。
    /// 关掉了，后面所有 `landAtBottom` 全成空转，人类看到的就是「一点没修」。
    func testProgrammaticSettleBelowBottomMustNotDropTheFollow() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: false)
        XCTAssertTrue(pin.isFollowing,
            "程序化滚动收尾在非底部只许放着不管 —— 关掉跟随就是 Todo #45 失效的那条路")
    }

    /// 用户往上翻历史 → 松开。这是「别修出新烦人事」那一条。
    func testUserScrollingUpStopsTheFollow() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: true)
        XCTAssertFalse(pin.isFollowing)
    }

    /// Todo #89：新消息可能在滚动手势中途到达，不能等 idle 才松开跟随。
    func testLeavingBottomByUserStopsFollowingBeforeTheGestureSettles() {
        var pin = CrewChatBottomFollow.Pin()
        pin.leftBottomByUser()
        XCTAssertFalse(pin.isFollowing)
        XCTAssertFalse(pin.received(1), "手势中途来的新消息只能记未读，不能把视口拽到底")
        XCTAssertEqual(pin.unread, 1)
    }

    /// 滑回底部 → 重新挂上并清未读（IM 惯例：回到底部 = 都看过了）。
    func testScrollingBackToBottomResumesTheFollowAndClearsUnread() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: true)
        pin.received(3)
        XCTAssertEqual(pin.unread, 3)
        pin.settled(atBottom: true, byUser: true)
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0)
    }

    /// Todo #56 ②：真正到达底部本身就要清未读，不能等下一次「滚动相位停稳」碰巧来。
    /// 视图的连续几何观察只在跨过 at-bottom 阈值时调用这一句，因此这里也要幂等。
    func testRealPositionReachingBottomClearsUnreadImmediately() {
        var pin = CrewChatBottomFollow.Pin(isFollowing: false, unread: 3)
        pin.reachedBottom()
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0)

        pin.reachedBottom()
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0, "已经到底时重复几何回调必须是空操作")
    }

    /// 行为 2：在底部 + 来新消息 → 该落底，未读不涨。
    func testNewMessageWhileAtBottomKeepsFollowingAndCountsNothing() {
        var pin = CrewChatBottomFollow.Pin()
        XCTAssertTrue(pin.received(1), "在底部就该跟着走")
        XCTAssertEqual(pin.unread, 0)
        XCTAssertTrue(pin.isFollowing)
    }

    /// 行为 3：不在底部 + 来新消息 → **不滚**（位置不动），未读 +1。
    func testNewMessageWhileScrolledUpCountsInsteadOfJumping() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: true)
        XCTAssertFalse(pin.received(1), "正在往回翻，不许自动跳到底部")
        XCTAssertEqual(pin.unread, 1)
        XCTAssertFalse(pin.isFollowing, "记了未读也不该顺手把跟随打开")
    }

    /// 连着来 → 累加（一次 refresh 带进来多条也按条数加）。
    func testUnreadAccumulatesAcrossArrivals() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: true)
        pin.received(1)
        pin.received(2)
        pin.received(4)
        XCTAssertEqual(pin.unread, 7)
    }

    /// 条目变少（撤回 / 订阅重放少给了）或没变：不加未读，也不该滚。
    func testNonPositiveDeltaNeitherScrollsNorCounts() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: true)
        XCTAssertFalse(pin.received(0))
        XCTAssertFalse(pin.received(-3))
        XCTAssertEqual(pin.unread, 0)

        var following = CrewChatBottomFollow.Pin()
        XCTAssertFalse(following.received(0), "没有新东西就别滚")
    }

    /// 行为 4：点箭头 → 落底 + 计数清零 + 重新挂上跟随。
    func testTappingTheArrowLandsAtBottomAndClearsTheCount() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: true)
        pin.received(9)
        pin.jumpToBottom()
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0)
        // 清零之后再来消息应当重新跟着走，而不是又开始记数。
        XCTAssertTrue(pin.received(1))
        XCTAssertEqual(pin.unread, 0)
    }

    /// 边界口径：**自己发的消息直接贴底、不计未读**，哪怕人刚才正在往回翻历史 ——
    /// 主动发言就是「我要回到现场」。
    func testSendingOwnMessageSnapsBackToBottomWithoutCounting() {
        var pin = CrewChatBottomFollow.Pin()
        pin.settled(atBottom: false, byUser: true)
        pin.received(2)
        pin.didSendOwnMessage()
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0, "自己发的话不该给自己记一条未读")
        XCTAssertTrue(pin.received(1), "紧接着白板回流的那条（就是自己刚发的）跟着走")
        XCTAssertEqual(pin.unread, 0)
    }

    /// 边界口径：切 crew = 全新的 `Pin`，未读从 0 算起、默认跟随。
    /// （视图侧靠 `.id(crewId)` 重建 + `subscribe()` 里显式归位，见下面的源码闸。）
    func testSwitchingCrewStartsFromACleanPin() {
        var previous = CrewChatBottomFollow.Pin()
        previous.settled(atBottom: false, byUser: true)
        previous.received(12)
        let fresh = CrewChatBottomFollow.Pin()
        XCTAssertTrue(fresh.isFollowing)
        XCTAssertEqual(fresh.unread, 0)
        XCTAssertNotEqual(previous, fresh, "别把上一个群的『用户滑走了 + 12 条未读』带过来")
    }

    // MARK: - 完整开屏时序（把上面几条接起来，钉住「零件对了、接线也不许拆台」）

    /// 走一遍真实开屏：空态 → 首屏 N 条 → 我们自己落底、收尾那瞬几何还没稳（读成
    /// 「没到底」）→ 第二波成员数据到位、行高剧变 → 还得再落一次底。
    ///
    /// **这条就是 Todo #45 漏掉的那条**：它的 `Pin` 在第三步就把跟随关了，第四步的
    /// 落底再也不会执行。
    func testOpeningSequenceSurvivesAMidMeasureSettle() {
        var pin = CrewChatBottomFollow.Pin()
        XCTAssertTrue(pin.received(70), "① 首屏 70 条到位 → 落底")
        // ② 我们自己 scrollTo 的收尾。此刻 LazyVStack 还在把估算行高换成实测行高，
        //    内容高度正在长，读出来是「没到底」。
        pin.settled(atBottom: false, byUser: false)
        XCTAssertTrue(pin.isFollowing, "②→③ 跟随必须还在，否则下一记落底空转")
        // ③ 第二波数据（成员/机长/本机身份）到位 → 行高翻面 → 视图侧再落一次底，
        //    这一记只有跟随还在才会执行。
        XCTAssertTrue(pin.isFollowing, "③ 行高输入变了那一记落底放行")
        // ④ 这次真的到底了。
        pin.settled(atBottom: true, byUser: false)
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0)
    }

    /// 再走一遍「人在读历史」的时序：落好底 → 用户上翻 → 连来 3 条 → 点箭头。
    func testReadingHistoryThenCatchingUp() {
        var pin = CrewChatBottomFollow.Pin()
        pin.received(70)
        pin.settled(atBottom: true, byUser: false)
        // 用户滚上去看历史，停住。
        pin.settled(atBottom: false, byUser: true)
        XCTAssertFalse(pin.isFollowing)
        // 来了 3 条：位置不动，只记数。
        XCTAssertFalse(pin.received(1))
        XCTAssertFalse(pin.received(2))
        XCTAssertEqual(pin.unread, 3)
        // 点箭头追上。
        pin.jumpToBottom()
        XCTAssertTrue(pin.isFollowing)
        XCTAssertEqual(pin.unread, 0)
    }

    // MARK: - 落点是「最后一条」，不是窗口边界、不是第一条

    /// 渲染窗口（#443）只放最近一页，但**最后一条永远是整表的最后一条** ——
    /// 落点落在窗口边界（= 一页之前那条）或第一条都是 bug。
    func testWindowAlwaysEndsAtTheNewestEntry() {
        let all = (1...200).map { "m\($0)" }
        let windowed = CrewChatWindow.window(all, limit: CrewChatWindow.pageSize)
        XCTAssertEqual(windowed.last, all.last, "窗口末端必须是最新那条")
        XCTAssertNotEqual(windowed.first, all.first, "不该从第一条开始渲染（那是全量）")
        XCTAssertEqual(windowed.count, CrewChatWindow.pageSize)
    }

    /// 落底哨兵挂在所有消息行**之后**，所以滚到它 = 滚到时间线末端，与窗口开了多深无关。
    func testBottomAnchorSitsAfterEveryMessageRow() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        guard let forEachLine = lines.firstIndex(where: {
                  Self.isCode($0) && $0.contains("ForEach(timelineRows)") }),
              let anchorLine = lines.firstIndex(where: {
                  Self.isCode($0) && $0.contains("id(CrewChatBottomFollow.bottomAnchorID)") })
        else {
            return XCTFail("找不到 `ForEach(timelineRows)` 或落底哨兵 —— "
                + "落点是不是时间线末端就没人守了（Todo #47 行为 1）。")
        }
        XCTAssertTrue(anchorLine > forEachLine,
            "落底哨兵必须排在消息行之后。挪到前面 = 滚到「窗口顶」，"
            + "人类看到的又是「点进去不落底」。")
        XCTAssertTrue(
            Self.containsCode("scrollTo(CrewChatBottomFollow.bottomAnchorID", in: text),
            "落底必须滚到那个哨兵常量 —— 滚到某条消息 id 会在懒容器里拿旧下标跑行闭包"
            + "（2026-08-07 那次 trap）。")
    }

    // MARK: - 源码级闸（判定对了，也得真接在那屏上）

    /// **本次修复的接线闸**：`onScrollPhaseChange` 必须把 **oldPhase** 喂给
    /// `settleIsUserDriven`。丢掉 oldPhase（写成 `{ _, phase, ... }`）就是 Todo #45
    /// 失效的那一行 —— 我们自己的滚动收尾会被记成「用户滑走了」，跟随被自己关掉。
    func testScrollPhaseTrackerUsesTheOldPhase() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertFalse(
            Self.containsCode("onScrollPhaseChange { _,", in: text),
            "`onScrollPhaseChange` 丢掉了 oldPhase —— 这正是 Todo #45 修法失效的那一行："
            + "程序化滚动的收尾会被当成用户滑走，跟随被自己关掉，此后所有落底空转。")
        XCTAssertTrue(
            Self.containsCode("settleIsUserDriven(", in: text),
            "「这次停稳是不是用户滑的」必须过 `settleIsUserDriven` —— "
            + "直接拿几何写跟随开关就回到了 Todo #45 的老路。")
    }

    /// 未读只走 `Pin.received`：视图里不许出现「不管在不在底部都滚一下」的老写法。
    func testNewMessagesGoThroughThePinInsteadOfAlwaysScrolling() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode("bottomPin.received(", in: text),
            "条目数变化必须过 `Pin.received` 判「跟着走还是记未读」（Todo #47 行为 2/3）。")
        XCTAssertTrue(
            Self.containsCode("bottomPin.jumpToBottom()", in: text),
            "未读箭头必须调 `jumpToBottom()`（落底 + 清零 + 重新跟随，行为 4）。")
        XCTAssertTrue(
            Self.containsCode("bottomPin.didSendOwnMessage()", in: text),
            "自己发消息要贴底且不计未读 —— 少了这一句，人发完自己的话还得手动滑下去。")
    }

    /// Todo #56 ② 的接线闸：未读按钮必须跟真实滚动位置走，而不是只在 phase→idle 时
    /// 猜一次。Bool 投影保证滚动过程中只有「跨过到底阈值」才写一次状态，不做逐帧重排。
    func testUnreadStateTracksTheRealBottomPosition() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode("BottomReachedTracker(pin: $bottomPin, phaseBox: scrollPhaseBox)", in: text),
            "群聊必须挂真实到底追踪器；否则滑到底后未读按钮会一直留着。")
        XCTAssertTrue(
            Self.containsCode("onScrollGeometryChange(for: Bool.self)", in: text),
            "到底判定应投影成 Bool，只在跨阈值时更新，不能逐帧写 @State。")
        XCTAssertTrue(
            Self.containsCode("pin.reachedBottom()", in: text),
            "真实位置到达底部时必须立即清未读并恢复跟随。")
        XCTAssertTrue(
            Self.containsCode("pin.leftBottomByUser()", in: text),
            "用户滚动一离开底部就要立即松开跟随，不能等手势结束后才处理。")
    }

    /// 人类明确要「只标数字」：不要「N 条新消息」这类文案，也不要别的修饰。
    func testUnreadBadgeShowsOnlyTheNumber() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode("Text(\"\\(bottomPin.unread)\")", in: text),
            "未读徽标要直接渲染数字本身。")
        for wording in ["条新消息", "条未读", "新消息\")"] {
            XCTAssertFalse(
                Self.containsCode(wording, in: text),
                "未读徽标出现了文案「\(wording)」—— 人类要的是只标数字。")
        }
    }

    /// 落底不许靠时序 hack。滚动锚点 + 程序化 `scrollTo` + 永不结束的动画三者凑齐会把
    /// 布局打成自激、进程被 AppKit 打死（2026-07-26 事故）。「延迟一拍再滚一次」看着能
    /// 修好，实际是往那个环里再加一根柴 —— 要的是内容稳定后确定性地落一次，不是多滚
    /// 几次总有一次对。
    func testChatViewLandsAtBottomWithoutTimingHacks() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        for hack in ["asyncAfter", "Task.sleep", "Timer.scheduledTimer", "Timer.publish"] {
            XCTAssertFalse(
                Self.containsCode(hack, in: text),
                "CrewChatView 里出现了 `\(hack)` —— 群聊落底不许用定时器/延迟兜底，"
                + "见 TypingDotsLayerView 顶部与 LayoutLoopRegressionTests。")
        }
    }

    /// 首屏那条路径靠「滚动视图跨空态一直活着」才成立：空态回到二选一分支的那一刻，
    /// 数据到位时滚动视图又是新建的，`.onChange(of: count)` 再次收不到 0→N，Todo #45 复发。
    func testTimelineKeepsTheScrollViewAliveAcrossTheEmptyState() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode(".overlay { if timelineEntries.isEmpty", in: text),
            "空态必须是盖在滚动视图上的一层，不能替换掉滚动视图 —— 否则首屏落底又只剩"
            + "原生锚点一个执行者（Todo #45 根因之一）。")
        XCTAssertTrue(
            Self.containsCode("onChange(of: bubbleLayoutToken)", in: text),
            "第二波异步数据（成员/机长/本机身份）到位后要再确定性地落一次底"
            + "（Todo #45 根因之二），这一记没了行高一变就停在上面某条。")
    }

    /// macOS 侧切 crew 必须重建视图（对齐 iPad）：复用实例会先渲染「新 crewId + 旧
    /// entries」一帧，滚底打在上一个 crew 的内容上。
    func testMacCenterViewRebuildsChatOnCrewSwitch() throws {
        let text = try Self.source("Mac/Views/CrewCenterView.swift")
        XCTAssertTrue(
            Self.containsCode(".id(detail.crew.id)", in: text),
            "CrewCenterView 建 CrewChatView 少了 `.id(crewId)` —— 切 crew 会复用旧实例"
            + "（Todo #45 的 macOS 错位；iPad 侧 IPadShell 一直是有 .id 的）。")
    }

    /// 切 crew 时跟随/未读要显式归位 —— `.id(crewId)` 是第一道，这句是「万一 .id 掉了」
    /// 的第二道。少了它，A 群翻历史留下的「用户滑走了」会让 B 群一进去就不落底。
    func testSubscribeResetsThePinOnCrewSwitch() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode("bottomPin = CrewChatBottomFollow.Pin()", in: text),
            "`subscribe()` 里要把 bottomPin 归位（Todo #47 的切 crew 口径：未读从 0 算起）。")
    }

    // MARK: - 第三个执行者：内容真实高度长出来了（Todo #54）
    //
    // #47 之后跟随开关不再被自己关掉了，人类仍报「有时落最新、有时落最早」。病根是那
    // 两记落底**跑在测量之前**：本地 backend 是 @MainActor 且方法体里一次不 await，
    // `entries` 与 `members` 两次 @State 写入被 SwiftUI 合并成同一次 body 更新，两个
    // onChange 背靠背触发，`scrollTo` 手里只有 LazyVStack 的估算行高 —— 估算算不满一屏
    // 时目标偏移就是 0，那就是「停在最早那条」。补第三记落底把两条路径归一。

    /// 跟随中、内容长高 → 补一记落底（首屏那条路径的正解）。
    func testContentGrowthLandsWhileFollowing() {
        XCTAssertTrue(CrewChatBottomFollow.shouldLandOnContentGrowth(
            oldHeight: 400, newHeight: 2600,
            isFollowing: true, isUserScrolling: false),
            "真实行高展开、内容长高时必须补一记落底 —— 少了它首屏那两记全落在测量之前")
    }

    /// **自激断边**：内容回缩一律不动。
    ///
    /// 落底会让懒容器 realize 更多行，也可能把离屏行的测量丢回估算而让总高回缩。
    /// 只认长高 → 我们自己的滚动无法经由「缩」这条边回头触发自己，环缺一条边。
    /// 这条测的是 2026-07-26 / 2026-08-10 两次布局自激不会被种回来。
    func testContentShrinkNeverLands() {
        XCTAssertFalse(CrewChatBottomFollow.shouldLandOnContentGrowth(
            oldHeight: 2600, newHeight: 2400,
            isFollowing: true, isUserScrolling: false),
            "内容回缩不许落底 —— 允许了，「落底→回缩→再落底」就凑成自激环")
    }

    /// 亚像素抖动不算长高，别为了零点几点反复滚。
    func testContentGrowthIgnoresSubPixelJitter() {
        XCTAssertFalse(CrewChatBottomFollow.shouldLandOnContentGrowth(
            oldHeight: 2600, newHeight: 2600 + CrewChatBottomFollow.growthEpsilon / 2,
            isFollowing: true, isUserScrolling: false))
        XCTAssertFalse(CrewChatBottomFollow.shouldLandOnContentGrowth(
            oldHeight: 2600, newHeight: 2600,
            isFollowing: true, isUserScrolling: false),
            "高度没变就什么都别做")
    }

    /// 人正往上翻的时候，懒容器把更早那几行真实量出来 → 内容长高。这时候落底 =
    /// 把人从手底下拽回底部，他会觉得「翻不动」。手在滚动上就不许动。
    func testContentGrowthDoesNotYankTheUserWhoIsScrolling() {
        XCTAssertFalse(CrewChatBottomFollow.shouldLandOnContentGrowth(
            oldHeight: 2000, newHeight: 4000,
            isFollowing: true, isUserScrolling: true),
            "用户的手还在滚动上时内容长高不许落底 —— 那会让他根本翻不动历史")
    }

    /// 已经松开跟随（在看历史）→ 内容长高也不动，新消息只记未读（行为 3）。
    func testContentGrowthStaysQuietWhenNotFollowing() {
        XCTAssertFalse(CrewChatBottomFollow.shouldLandOnContentGrowth(
            oldHeight: 2000, newHeight: 4000,
            isFollowing: false, isUserScrolling: false))
    }

    /// 相位盒子只记「手在不在上面」：拖 / 惯性算在，我们自己的程序化滚动（animating）
    /// 和停稳（idle）不算 —— 与 `settleIsUserDriven` 同一套分类，别让它俩漂开。
    func testScrollPhaseBoxTracksUserActivePhasesOnly() {
        let box = CrewChatBottomFollow.ScrollPhaseBox()
        XCTAssertFalse(box.isUserScrolling, "开屏默认没人在滚")

        for phase in [CrewChatBottomFollow.ScrollPhaseKind.tracking, .interacting, .decelerating] {
            box.phaseChanged(to: phase)
            XCTAssertTrue(box.isUserScrolling, "\(phase) 是用户的手还在上面")
            XCTAssertTrue(CrewChatBottomFollow.settleIsUserDriven(previous: phase),
                "`isUserActive` 与 `settleIsUserDriven` 的分类必须一致")
        }
        for phase in [CrewChatBottomFollow.ScrollPhaseKind.animating, .idle] {
            box.phaseChanged(to: phase)
            XCTAssertFalse(box.isUserScrolling,
                "\(phase) 不是用户在滚 —— animating 是我们自己的 scrollTo，"
                + "认成用户意图正是 Todo #45 修法失效的病根")
            XCTAssertFalse(CrewChatBottomFollow.settleIsUserDriven(previous: phase))
        }
    }

    /// **本次修复的接线闸**：首屏必须有第三个执行者接在滚动几何上。
    /// 少了它，两记落底双双跑在 LazyVStack 量出真实行高之前，Todo #54 复发。
    func testChatViewLandsAgainAfterRealRowHeightsExpand() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode("onScrollGeometryChange(", in: text),
            "群聊少了「内容真实高度长高」这个离散事件的落底（Todo #54）—— "
            + "只剩条目数与行高令牌两记，它俩在本地路径上都跑在测量之前。")
        XCTAssertTrue(
            Self.containsCode("shouldLandOnContentGrowth(", in: text),
            "「长高了要不要落底」的判定必须过 `shouldLandOnContentGrowth` —— "
            + "在视图里直接比高度会把「只认长高」这条自激断边写丢。")
        XCTAssertTrue(
            Self.containsCode("BottomOnContentGrowth(", in: text),
            "第三记落底要挂在滚动视图上（`BottomOnContentGrowth`），不是散在别处。")
    }

    /// Todo #56 ①：窗口已经把消息行封顶到 12 条，继续用 LazyVStack 只会把滚动目标交给
    /// 估算行高。初始 scrollTo 若按过大的估算落点、随后真实高度回缩，视口会停在内容外，
    /// 直到用户滚一下由系统钳回 —— 正是「空白，滑一下才出现」。有界窗口应一次量完。
    func testWindowedTimelineMeasuresRowsBeforeResolvingTheBottomTarget() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode("VStack(alignment: .leading, spacing: 0)", in: text),
            "渲染窗口只有一页，时间线应 eager measure 后再解底部目标。")
        XCTAssertTrue(
            Self.containsCode("usesEagerInitialLayout(limit: renderLimit)", in: text),
            "eager 只用于首屏；手点加载历史后仍需 lazy，不能把几百条全量 eager。")
    }

    /// 相位标志不许写进 `@State`：手势刚开始那一下让 body 失效 = `LazyVStack` 全量
    /// 重新测量整条列表（#443 那两份真实 hang 报告的热点栈），最伤的时机。
    func testScrollPhaseFlagStaysOutOfTheDependencyGraph() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            Self.containsCode("phaseBox.phaseChanged(", in: text),
            "相位要记进引用型 `ScrollPhaseBox`。")
        XCTAssertFalse(
            Self.containsCode("bottomPin.phaseChanged(", in: text),
            "相位标志被塞回 `@State` 的 Pin 了 —— 滚动手势一开始就让 body 失效，"
            + "整条消息列表被重新测一遍（#443 的热点）。放引用型盒子里。")
    }

    // MARK: - 明确没被自动守住的那一部分
    //
    // 「滚动偏移最后**真的**落在哪一像素」是 SwiftUI/AppKit 的事，进程里没有可读的
    // 断言点，起 GUI 又被硬约束禁掉。所以下面这几条只有人眼能验，挂 QA 批次 #443：
    //   - 点进去那一屏，最新一条是不是**完整**可见（不是「差半行」）；
    //   - 在底部时来新消息，跟随过程是不是平滑（有没有可见的跳一下）；
    //   - 未读徽标的位置/大小观感；
    //   - **反复切 crew（切走再切回、快切慢切、长 crew / 短 crew）看落点稳不稳**
    //     —— Todo #54 要的正是这一条：修的是「落底跑在测量前」，而「测量后落对了没」
    //     只有人眼能判（QA 尾巴挂 task #443）。
    // 上面这些测试守住的是**判定与接线**：谁能关掉跟随、什么时候记未读、落点锚在哪、
    // 那几条线有没有被拆掉。历史上翻车的正是这一层，不是像素。

    // MARK: - 源码扫描小工具

    /// `#filePath` → `apps/pendingcrew/Sources/<rel>`（测试 bundle 里没有源码，只能回推路径）。
    private static func source(_ relativePath: String) throws -> String {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()    // PendingCrewTests
            .deletingLastPathComponent()    // Tests
            .deletingLastPathComponent()    // apps/pendingcrew
            .appendingPathComponent("Sources")
            .appendingPathComponent(relativePath)
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// 整行注释跳过 —— 上面那些说明里就要提「asyncAfter」这些词，禁的是写出来的代码。
    private static func isCode(_ line: String) -> Bool {
        let t = line.trimmingCharacters(in: .whitespaces)
        return !t.hasPrefix("//") && !t.hasPrefix("*") && !t.hasPrefix("/*")
    }

    private static func containsCode(_ needle: String, in text: String) -> Bool {
        text.split(separator: "\n", omittingEmptySubsequences: false).contains { line in
            isCode(String(line)) && line.contains(needle)
        }
    }
}
