import XCTest

/// 「来了新消息」这一拍的**真实调用路径**（人类 Todo #144）。
///
/// ## 跟 `CrewChatWindowTests` 那批的分工
///
/// 那批量的是 `CrewChatWindow.afterInsert` **自己算得对不对**。
/// 这批量的是**视图那一拍真的这么算了** —— `CrewChatView` 的
/// `.onChange(of: timelineEntries.count)` 现在整段调 `CrewChatNewMessages.apply`，
/// 所以这里调的就是它调的那一个。
///
/// **为什么非要分开**：#144 的病根不是 `afterInsert` 算错，是**调用点没传
/// `isFollowing`**。那种错误下，`afterInsert` 的一整排用例全绿，而人翻历史照样跳。
/// 本仓两天内第三次「建好了没接上」，所以这一层必须自己有尺子。
final class CrewChatNewMessagesTests: XCTestCase {

    private func msgs(_ n: Int) -> [Int] { Array(0..<n) }

    private func pin(following: Bool, unread: Int = 0) -> CrewChatBottomFollow.Pin {
        CrewChatBottomFollow.Pin(isFollowing: following, unread: unread)
    }

    // MARK: - 判据：滑走看历史时，窗口最顶那条不许变

    /// 他**没点过「加载更早」**（`limit == pageSize`，最常见的状态），
    /// 只是往上滚了几屏。新消息到达前后，窗口里最顶那条必须是同一条。
    func test_滑走时新消息不改变窗口最顶那条() throws {
        let all = msgs(200)
        let limit = CrewChatWindow.pageSize
        let topBefore = try XCTUnwrap(CrewChatWindow.window(all, limit: limit).first)

        let out = CrewChatNewMessages.apply(
            added: 1, renderLimit: limit, pin: pin(following: false))

        let after = CrewChatWindow.window(all + [200], limit: out.renderLimit)
        XCTAssertEqual(after.first, topBefore,
                       "最顶那条被剪掉了 —— 不跟随时锚在内容顶端，顶端一缩，"
                       + "他正在读的那段就整体上移，这就是「位置乱跳」")
        XCTAssertEqual(after.last, 200, "新消息仍在窗口里（只是在视口下方）")
        XCTAssertFalse(out.shouldLandAtBottom, "滑走了就不许把他拽回底部")
        XCTAssertEqual(out.pin.unread, 1, "只记未读")
    }

    /// 连着来十条也一样 —— 顶端一条都不许少。
    func test_连来十条最顶那条仍然不变() throws {
        let all = msgs(200)
        var limit = CrewChatWindow.pageSize
        var p = pin(following: false)
        let topBefore = try XCTUnwrap(CrewChatWindow.window(all, limit: limit).first)

        var grown = all
        for i in 0..<10 {
            let out = CrewChatNewMessages.apply(added: 1, renderLimit: limit, pin: p)
            limit = out.renderLimit
            p = out.pin
            grown.append(200 + i)
            XCTAssertEqual(CrewChatWindow.window(grown, limit: limit).first, topBefore,
                           "第 \(i + 1) 条之后顶端变了")
        }
        XCTAssertEqual(p.unread, 10)
    }

    /// 跟随中照旧：窗口往前滑（成本封顶那一半不许丢），并且要落底。
    func test_跟随时窗口照旧往前滑并落底() {
        let out = CrewChatNewMessages.apply(
            added: 3, renderLimit: CrewChatWindow.pageSize, pin: pin(following: true))
        XCTAssertEqual(out.renderLimit, CrewChatWindow.pageSize, "封顶被弄丢了")
        XCTAssertTrue(out.shouldLandAtBottom)
        XCTAssertEqual(out.pin.unread, 0)
    }

    func test_没有新消息时什么都不动() {
        let out = CrewChatNewMessages.apply(
            added: 0, renderLimit: 60, pin: pin(following: false, unread: 2))
        XCTAssertEqual(out.renderLimit, 60)
        XCTAssertFalse(out.shouldLandAtBottom)
        XCTAssertEqual(out.pin.unread, 2, "撤回/切筛选让条数变少时不许乱动未读")
    }

    // MARK: - 回到底部时回吐这一次涨出来的那段

    /// 回吐的是**这一次往上看期间**涨出来的那段，
    /// **不是**他滑走之前特意翻出来的那几页。
    func test_回到底部只回吐这一次涨出来的那段() {
        // 他先点过「加载更早」到 36，然后滑走，期间来了 5 条 → 41。
        let released = CrewChatNewMessages.followChanged(
            isFollowing: false, renderLimit: 36, limitBeforeExcursion: nil)
        XCTAssertEqual(released.limitBeforeExcursion, 36)

        var limit = released.renderLimit
        var p = pin(following: false)
        for _ in 0..<5 {
            let out = CrewChatNewMessages.apply(added: 1, renderLimit: limit, pin: p)
            limit = out.renderLimit
            p = out.pin
        }
        XCTAssertEqual(limit, 41)

        let resumed = CrewChatNewMessages.followChanged(
            isFollowing: true, renderLimit: limit,
            limitBeforeExcursion: released.limitBeforeExcursion)
        XCTAssertEqual(resumed.renderLimit, 36,
                       "回吐过头了 —— 他滑走之前翻出来的那几页不该被收回去")
        XCTAssertNil(resumed.limitBeforeExcursion, "跟随中不该留着记号")
    }

    /// **跟随开关会在一次「往上看」里抖好几次**（手势相位和几何投影两条路都写它）。
    /// 记号一旦记下就不许被后来的抖动抬高，否则回吐的基准会一路往上跑。
    func test_松开跟随抖动不许抬高记号() {
        var limit = 12
        var mark: Int? = nil
        for _ in 0..<3 {
            let r = CrewChatNewMessages.followChanged(
                isFollowing: false, renderLimit: limit, limitBeforeExcursion: mark)
            limit = r.renderLimit
            mark = r.limitBeforeExcursion
            // 中间来了一条新消息，窗口涨一格
            limit = CrewChatNewMessages.apply(
                added: 1, renderLimit: limit, pin: pin(following: false)).renderLimit
        }
        XCTAssertEqual(mark, 12, "记号被抬高了 —— 回吐就回不到原处")
        XCTAssertEqual(
            CrewChatNewMessages.followChanged(
                isFollowing: true, renderLimit: limit, limitBeforeExcursion: mark).renderLimit,
            12)
    }

    /// 一开始就在底部（从没滑走过）→ 没有记号，回吐是空操作。
    func test_从没滑走过时回到底部不动窗口() {
        let r = CrewChatNewMessages.followChanged(
            isFollowing: true, renderLimit: 36, limitBeforeExcursion: nil)
        XCTAssertEqual(r.renderLimit, 36)
        XCTAssertNil(r.limitBeforeExcursion)
    }

    /// 滑走期间他又点了「加载更早」（窗口比记号还深）→ 回吐不许把那一页收掉。
    func test_滑走期间又翻了页则回吐取较小者() {
        let resumed = CrewChatNewMessages.followChanged(
            isFollowing: true, renderLimit: 24, limitBeforeExcursion: 36)
        XCTAssertEqual(resumed.renderLimit, 24,
                       "min 取的是较小者 —— 窗口比记号浅时别反过来把它撑大")
    }

    // MARK: - 「它被谁调了」
    //
    // 接线闸不在这儿 —— 仓库里**本来就有**一整节「源码级闸」
    // （`CrewChatBottomFollowTests` 的 `testNewMessagesGoThroughThePinInsteadOfAlwaysScrolling`
    // 等），带着跳注释行的 `containsCode` 帮手。#144 的接线钉在那里，
    // 跟 Todo #45 / #47 / #56 那几条闸放在一起。
    //
    // 我第一版在这里又写了一个自己读文件的版本 —— 那是「没找仓库里正确的孪生」，
    // 而且它用裸 `contains`，会被注释里提到的同一串字骗过去。
}
