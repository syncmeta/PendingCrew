#if os(macOS)
import XCTest

/// `StartupPromptDelivery` 这台状态机本身（P5a）—— 时间由测试喂，所以每一条上限
/// 都能当场量到，不用真等 90 秒。
///
/// 与另外两组的分工：`StartupPromptDeliveryTests` 在真 PTY 上验「规矩」，
/// `ClaudeInputBoxFixtureTests` 拿真字节验「尺子」，这一组验的是**账**：
/// 重投几次、什么时候认输、认输之后说了什么、以及**每种坏消息只报一次**。
final class StartupPromptDeliveryPlanTests: XCTestCase {

    private let t0 = Date(timeIntervalSince1970: 1_700_000_000)
    private var timing: StartupPromptDelivery.Timing {
        var t = StartupPromptDelivery.Timing()
        t.readinessDeadline = 10
        t.verifyWindow = 2
        t.submitGap = 0.5
        t.maxAttempts = 3
        t.dialogStable = 3
        return t
    }

    private func obs(_ row: String?, bodyVisible: Bool = false,
                     dialog: Bool = false, at offset: TimeInterval)
        -> StartupPromptDelivery.Observation {
        .init(inputRow: row, bodyVisible: bodyVisible, dialogPresent: dialog,
              now: t0.addingTimeInterval(offset))
    }

    // MARK: - 就绪之前

    /// 输入框没画出来就是没画出来 —— 不许因为「已经等了一会儿」就下注。
    /// 这正是旧代码的病根：`Task.sleep(100ms)` 之后闭着眼写。
    func test_没有输入行时一个字节都不投() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        for offset in stride(from: 0.0, through: 9.0, by: 1.0) {
            XCTAssertEqual(d.step(obs(nil, at: offset)), .idle, "第 \(offset)s")
        }
    }

    /// 等到期限还没见到输入框 → **报出来**，而且只报一次（白板不刷屏）。
    /// 报完不放弃：真就绪了照投不误。
    func test_等不到输入框会报一次而不是静默也不是放弃() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        XCTAssertEqual(d.step(obs(nil, at: 9)), .idle)

        guard case let .notReady(detail) = d.step(obs(nil, at: 10)) else {
            return XCTFail("等过了期限必须报出来 —— 最怕的就是静默")
        }
        XCTAssertTrue(detail.contains("开场任务"), detail)
        XCTAssertEqual(d.step(obs(nil, at: 20)), .idle, "同一条坏消息不许反复刷白板")
        XCTAssertFalse(d.isFinished, "报了不等于放弃")

        XCTAssertEqual(d.step(obs("", at: 30)), .writeBody(attempt: 1),
                       "输入框最终画出来了就该照投")
    }

    // MARK: - 正常路径

    func test_就绪后写正文校验落地再单发回车() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        XCTAssertEqual(d.step(obs("", at: 0.5)), .writeBody(attempt: 1))
        // 回显还没到 —— 别急着重投。
        XCTAssertEqual(d.step(obs("", at: 1.0)), .idle)
        // 输入行从空变成有东西 = 正文真的进去了。
        XCTAssertEqual(d.step(obs("[Pasted text #1 +212 lines]", at: 1.5)), .submit(attempt: 1))
        // 回车还没被处理。
        XCTAssertEqual(d.step(obs("[Pasted text #1 +212 lines]", at: 2.0)), .idle)
        // 输入行被清空 = 提交了。
        XCTAssertEqual(d.step(obs("", at: 2.5)), .delivered)
        XCTAssertTrue(d.isFinished)
        XCTAssertEqual(d.step(obs("", at: 3)), .idle, "收工之后不再动手")
    }

    /// **刚写完那一瞬的画面不作数。** 真 claude 会在空输入框里自己摆一句灰色示例
    /// 提示（拿真字节实测到的，见 `ClaudeInputBoxFixtureTests`）——那一下变化跟我们
    /// 这笔写入毫无关系，却长得一模一样。认了它就会在正文其实还没进去的时候把回车
    /// 发出去，然后把「输入行又变了」当成「提交成功」——**又是一次静默的没送到**。
    /// 同一条底线顺带满足 claude 的粘贴判定（正文与回车必须分开到达）。
    func test_刚写完那一瞬的画面变化不算落地() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        XCTAssertEqual(d.step(obs("", at: 0)), .writeBody(attempt: 1))
        // TUI 自己把示例提示摆了进去 —— 不是我们那一笔。
        XCTAssertEqual(d.step(obs("Try \"how do I log an error?\"", at: 0.2)), .idle)
        XCTAssertEqual(d.step(obs("Try \"how do I log an error?\"", at: 0.49)), .idle)
        // 过了底线之后画面上的东西才作数。
        XCTAssertEqual(d.step(obs("[Pasted text #1 +212 lines]", at: 0.5)),
                       .submit(attempt: 1))
    }

    /// 「落地」不是逐字匹配 brief —— claude 对大段输入会折成占位。逐字匹配会在
    /// **正常路径**上判失败、反复重投并误报 health，比原来的静默还糟。
    func test_落地判据是输入行从空变成非空而不是逐字匹配() {
        XCTAssertFalse(StartupPromptDelivery.landed(row: "", baseline: ""))
        XCTAssertFalse(StartupPromptDelivery.landed(row: "Try \"fix the bug\"",
                                                    baseline: "Try \"fix the bug\""),
                       "占位提示原样没变 = 什么都没进去")
        XCTAssertTrue(StartupPromptDelivery.landed(row: "[Pasted text #1 +212 lines]",
                                                   baseline: ""))
        XCTAssertTrue(StartupPromptDelivery.landed(row: "你是修一条 P5a 的硬前置 bug",
                                                   baseline: "Try \"fix the bug\""))
    }

    // MARK: - 投不进去

    /// 正文反复没进输入行 → 重投**有上限** → 到顶翻 health。
    /// 重投第 2 次起要先清输入行（调用方按 `attempt > 1` 发 Ctrl-U）——
    /// 上一笔万一其实落了一半，两笔会叠成一段谁也看不懂的东西。
    func test_正文反复不落地会重投到上限然后报出来() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        XCTAssertEqual(d.step(obs("", at: 0)), .writeBody(attempt: 1))
        XCTAssertEqual(d.step(obs("", at: 1.9)), .idle, "窗口没到就不重投")
        XCTAssertEqual(d.step(obs("", at: 2.0)), .writeBody(attempt: 2))
        XCTAssertEqual(d.step(obs("", at: 4.0)), .writeBody(attempt: 3))

        guard case let .undelivered(detail) = d.step(obs("", at: 6.0)) else {
            return XCTFail("投到上限还没进去，必须报出来而不是静默")
        }
        XCTAssertTrue(detail.contains("3 次"), detail)
        XCTAssertTrue(d.isFinished, "认输之后别再往一个吞字节的输入框里写")
    }

    /// 正文进去了但回车反复没被接受（多半有个模态挡在前面）—— 同样有上限、
    /// 同样要报，不能让 brief 就那么躺在输入框里没提交。
    func test_回车反复不被接受也会报出来() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        XCTAssertEqual(d.step(obs("", at: 0)), .writeBody(attempt: 1))
        XCTAssertEqual(d.step(obs("brief…", at: 0.5)), .submit(attempt: 1))
        XCTAssertEqual(d.step(obs("brief…", at: 2.5)), .submit(attempt: 2))
        XCTAssertEqual(d.step(obs("brief…", at: 4.5)), .submit(attempt: 3))

        guard case let .unsubmitted(detail) = d.step(obs("brief…", at: 6.5)) else {
            return XCTFail("回车到上限还没被接受，必须报出来")
        }
        XCTAssertTrue(detail.contains("没提交"), detail)
        XCTAssertTrue(d.isFinished)
    }

    /// 画面重绘到一半、输入行暂时不在 —— 那是重绘，不是失败，别把窗口白白烧掉。
    func test_重绘期间输入行暂时消失不算一次失败() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        XCTAssertEqual(d.step(obs("", at: 0)), .writeBody(attempt: 1))
        XCTAssertEqual(d.step(obs(nil, at: 3.0)), .idle, "输入行不在就等，别当成没投进去")
        XCTAssertEqual(d.step(obs("", at: 3.1)), .writeBody(attempt: 2))
    }

    func test_超长正文把提示符顶出屏幕后仍按可见正文提交() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        XCTAssertEqual(d.step(obs("", at: 0)), .writeBody(attempt: 1))
        XCTAssertEqual(d.step(obs(nil, bodyVisible: false, at: 0.5)), .idle,
                       "没有正文证据时仍按重绘处理")
        XCTAssertEqual(d.step(obs(nil, bodyVisible: true, at: 0.6)), .submit(attempt: 1))
    }

    // MARK: - 首屏是需要人回答的对话框

    /// 对话框在场时**一个字节都不投**：那一笔回车会被对话框吃掉、选中「No, exit」，
    /// session 秒退且零输出（机长实测过的第二种翻车形状）。只报不答，现场留给
    /// `inspect_session`。
    func test_对话框在场时不投递只报一次() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        // 半成品画面会短暂长得像菜单，所以要稳定够久才认。
        XCTAssertEqual(d.step(obs(nil, dialog: true, at: 0)), .idle)
        XCTAssertEqual(d.step(obs(nil, dialog: true, at: 2.9)), .idle)

        guard case let .blockedByDialog(detail) = d.step(obs(nil, dialog: true, at: 3.0)) else {
            return XCTFail("卡在等人回答的对话框上，必须报出来")
        }
        XCTAssertTrue(detail.contains("需要人回答"), detail)
        XCTAssertEqual(d.step(obs(nil, dialog: true, at: 5)), .idle, "同一条不许反复刷白板")
        XCTAssertFalse(d.isFinished, "答完之后还要把开场任务补投出去")
    }

    /// 卡在等人身上的那段时间**不该算进「没等到输入框」的账** —— 否则一个正在
    /// 等人拍板的 session 会再挨一条「等不到输入框」，两条说的是同一件事。
    func test_被对话框挡住期间不计就绪超时() {
        var d = StartupPromptDelivery(timing: timing, startedAt: t0)
        for offset in stride(from: 0.0, through: 30.0, by: 1.0) {
            let step = d.step(obs(nil, dialog: true, at: offset))
            if case .notReady = step { XCTFail("等人期间不该再报「等不到输入框」（第 \(offset)s）") }
        }
        // 有人代答了 → 输入框出现 → 自己把开场任务补投出去。
        XCTAssertEqual(d.step(obs("", at: 31)), .writeBody(attempt: 1))
    }
}
#endif
