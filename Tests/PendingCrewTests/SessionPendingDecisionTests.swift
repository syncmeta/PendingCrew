import XCTest
// SessionPendingDecision 直接编进 PendingCrewTests target，无需 import。

/// 「session 卡在待决策上」的检测 + 通知选靶单测（Todo #6）。
///
/// 病根：claude 在 PTY 里弹的菜单（命令审批 / 计划确认 / 信任文件夹 / 选登录方式）
/// 不经过我们的 PreToolUse 钩子（那个只 gate computer-use），所以群里一个字都没有，
/// 没人盯右栏这个 session 就一直干等 —— 事实上死掉。
///
/// 这里钉住两件事：
/// 1. **认得出**菜单、且**认不错**（agent 正文里的编号列表绝不能误判成菜单）。
/// 2. **@ 对人**（机长先拍，机长自己卡住直接找人，等太久再升级）。
final class SessionPendingDecisionTests: XCTestCase {

    // MARK: - Fixtures（形状取自 claude code 终端实际渲染）

    /// 命令审批：带 box 边框、`❯` 光标、连续编号。
    private let permissionMenu = """
        ╭──────────────────────────────────────────────╮
        │ Bash command                                 │
        │                                              │
        │   rm -rf build                               │
        │   Remove the build directory                 │
        │                                              │
        │ Do you want to proceed?                      │
        │ ❯ 1. Yes                                     │
        │   2. Yes, and don't ask again for rm commands │
        │   3. No, and tell Claude what to do (esc)    │
        ╰──────────────────────────────────────────────╯
        """

    /// 计划模式确认（无 box，裸行）。
    private let planMenu = """
        Would you like to proceed?
        ❯ 1. Yes, and auto-accept edits
          2. Yes, and manually approve edits
          3. No, keep planning
        """

    /// 信任文件夹（首次在新目录起 claude）。
    private let trustMenu = """
        Do you trust the files in this folder?
        ❯ 1. Yes, proceed
          2. No, exit
        """

    // MARK: - 认得出

    func testParsesPermissionMenu() {
        let d = TerminalMenuParser.parse(permissionMenu)
        XCTAssertEqual(d?.prompt, "Do you want to proceed?")
        XCTAssertEqual(d?.options, ["Yes",
                                    "Yes, and don't ask again for rm commands",
                                    "No, and tell Claude what to do (esc)"])
    }

    func testParsesPlanMenu() {
        XCTAssertEqual(TerminalMenuParser.parse(planMenu)?.prompt, "Would you like to proceed?")
        XCTAssertEqual(TerminalMenuParser.parse(planMenu)?.options.count, 3)
    }

    func testParsesTrustFolderMenu() {
        let d = TerminalMenuParser.parse(trustMenu)
        XCTAssertEqual(d?.prompt, "Do you trust the files in this folder?")
        XCTAssertEqual(d?.options, ["Yes, proceed", "No, exit"])
    }

    /// 光标可能停在第二项（人已经按过下箭头）——照样认得。
    func testMarkerOnSecondOptionStillParses() {
        let menu = """
            Do you want to proceed?
              1. Yes
            ❯ 2. No
            """
        XCTAssertEqual(TerminalMenuParser.parse(menu)?.options, ["Yes", "No"])
    }

    // MARK: - 认不错（误报比漏报更伤：群里刷噪音，人就不再看通知了）

    /// **最要紧的一条**：agent 正文里的编号列表满地都是（计划、清单、总结），
    /// 没有 `❯` 选择光标就绝不是交互菜单。这条挂了整个特性就是噪音制造机。
    func testProseNumberedListIsNotAMenu() {
        let prose = """
            我打算这么做：
            1. 先查现状
            2. 再写单测
            3. 最后接线
            """
        XCTAssertNil(TerminalMenuParser.parse(prose))
    }

    /// 菜单已经答完、后面又滚了新输出 —— 不再是「在等人选」。
    func testAnsweredMenuFollowedByOutputIsNotPending() {
        let after = permissionMenu + """

            ⏺ Bash(rm -rf build)
              ⎿  (No content)
            ⏺ 删好了，继续下一步。
            正在读取 src/main.swift …
            """
        XCTAssertNil(TerminalMenuParser.parse(after))
    }

    /// rate-limit 菜单归 `RateLimitMenuScanner` 自动应答（选「Stop and wait」），
    /// 不该再走「找人拍板」这条路 —— 否则每次撞额度都白喊一次人。
    func testRateLimitMenuIsExcluded() {
        let menu = """
            What do you want to do?
            ❯ 1. Stop and wait for limit to reset
              2. Upgrade your plan
            """
        XCTAssertNil(TerminalMenuParser.parse(menu))
    }

    func testSingleOptionIsNotAMenu() {
        XCTAssertNil(TerminalMenuParser.parse("Continue?\n❯ 1. Yes\n"))
    }

    /// 编号必须从 1 连续 —— 断号的多半是正文里凑巧排在一起的引用。
    func testNonConsecutiveNumbersRejected() {
        XCTAssertNil(TerminalMenuParser.parse("见下：\n❯ 1. 甲\n  3. 丙\n"))
    }

    // MARK: - Tracker：什么时候才算「真的在等」

    private func tracker(stable: TimeInterval = 3) -> PendingDecisionTracker {
        PendingDecisionTracker(stable: stable)
    }

    private func feed(_ t: PendingDecisionTracker, _ s: String) {
        let bytes = Array(s.utf8)
        t.feed(bytes[...])
    }

    /// 菜单刚画出来不立刻报 —— 要在画面上稳住够久才算真在等人。
    func testMenuMustBeStableBeforeFiring() {
        let t = tracker()
        let t0 = Date()
        feed(t, permissionMenu)
        XCTAssertNil(t.poll(now: t0))
        XCTAssertNil(t.poll(now: t0.addingTimeInterval(2)))
        guard case let .appeared(d)? = t.poll(now: t0.addingTimeInterval(3.1)) else {
            return XCTFail("稳定超过窗口后应报出待决策")
        }
        XCTAssertEqual(d.prompt, "Do you want to proceed?")
    }

    /// TUI 会把同一个菜单反复重绘。重绘**不**该重置稳定计时，
    /// 否则一个每秒重画的菜单永远等不到「静下来」，就永远报不出来。
    func testRedrawOfSameMenuDoesNotResetTheClock() {
        let t = tracker()
        let t0 = Date()
        feed(t, permissionMenu)
        XCTAssertNil(t.poll(now: t0))
        feed(t, "\n" + permissionMenu)          // 原样重绘一遍
        XCTAssertNil(t.poll(now: t0.addingTimeInterval(1)))
        guard case .appeared? = t.poll(now: t0.addingTimeInterval(3.1)) else {
            return XCTFail("重绘同一菜单不应把计时清零")
        }
    }

    func testFiresOnlyOncePerMenu() {
        let t = tracker()
        let t0 = Date()
        feed(t, permissionMenu)
        _ = t.poll(now: t0)
        XCTAssertNotNil(t.poll(now: t0.addingTimeInterval(4)))
        XCTAssertNil(t.poll(now: t0.addingTimeInterval(5)), "同一个菜单不重复刷群")
        XCTAssertNil(t.poll(now: t0.addingTimeInterval(60)))
    }

    /// 人答完了 → 必须报 `.cleared`。**这是 #545 的教训**：状态进得去出不来
    /// 比没有状态更糟（机长按谎报的状态派活）。
    func testClearsWhenMenuGoesAway() {
        let t = tracker()
        let t0 = Date()
        feed(t, permissionMenu)
        _ = t.poll(now: t0)
        XCTAssertNotNil(t.poll(now: t0.addingTimeInterval(4)))
        feed(t, "\n⏺ Bash(rm -rf build)\n  ⎿  (No content)\n⏺ 删好了，继续。\n继续读文件…\n")
        XCTAssertEqual(t.poll(now: t0.addingTimeInterval(5)), .cleared)
        XCTAssertNil(t.poll(now: t0.addingTimeInterval(6)), "清过一次就别再重复清")
    }

    /// 清掉之后同一个菜单再弹出来，要能再报一次（不能被「每个指纹一次」永久哑掉）。
    func testSameMenuFiresAgainAfterBeingCleared() {
        let t = tracker()
        var now = Date()
        feed(t, permissionMenu)
        _ = t.poll(now: now); now += 4
        XCTAssertNotNil(t.poll(now: now))
        feed(t, "\n⏺ 干完了，继续下一步，正在读文件…\n再读一个文件…\n还在读…\n")
        now += 1
        XCTAssertEqual(t.poll(now: now), .cleared)
        feed(t, "\n" + permissionMenu)
        now += 1
        XCTAssertNil(t.poll(now: now))
        now += 4
        guard case .appeared? = t.poll(now: now) else {
            return XCTFail("清除后再次弹出的同款菜单应能再报")
        }
    }

    /// 正在流式吐字时不报 —— 输出里没有菜单形状，压根不该进入候选。
    func testStreamingOutputNeverFires() {
        let t = tracker()
        var now = Date()
        for i in 0..<10 {
            feed(t, "⏺ 正在处理第 \(i) 个文件…\n")
            now += 1
            XCTAssertNil(t.poll(now: now))
        }
    }

    // MARK: - 通知选靶：@ 对的人

    private func post(_ stage: SessionDecisionNotice.Stage, isCaptain: Bool) -> SessionDecisionNotice.Post {
        SessionDecisionNotice.post(
            stage: stage, sessionName: "小明", sessionId: "worker-abc",
            isCaptain: isCaptain, question: "Do you want to proceed?",
            options: ["Yes", "No"], waitedMinutes: stage == .escalate ? 7 : 0)
    }

    /// worker 卡住 → 先找机长（机长手上有 inspect_session/nudge_session，能直接代答）。
    /// 这一步**不** @人：能机长拍的就别惊动人，否则通知很快就被无视。
    func testWorkerFirstNoticeGoesToCaptainOnly() {
        XCTAssertEqual(post(.first, isCaptain: false).mentionKinds, ["captain"])
    }

    /// 机长自己卡住 → 直接找人。绝不 @ 自己：@机长会触发「目标缺席拉起」，
    /// 卡着的机长起不来又发一条，就此成环（#541 同款坑）。
    func testCaptainStuckAsksHumanDirectly() {
        XCTAssertEqual(post(.first, isCaptain: true).mentionKinds, ["human"])
    }

    /// 机长拍不了 / 没动静 → 升级找人。
    func testEscalationGoesToHuman() {
        XCTAssertEqual(post(.escalate, isCaptain: false).mentionKinds, ["human"])
        XCTAssertEqual(post(.escalate, isCaptain: true).mentionKinds, ["human"])
    }

    /// 通知得说清「在等什么」——问题原文 + 选项都要在正文里，
    /// 否则人还得点进右栏看终端，等于没通知。
    func testNoticeTextCarriesQuestionAndOptions() {
        let p = post(.first, isCaptain: false)
        XCTAssertTrue(p.text.contains("Do you want to proceed?"), p.text)
        XCTAssertTrue(p.text.contains("1. Yes"), p.text)
        XCTAssertTrue(p.text.contains("2. No"), p.text)
        XCTAssertTrue(p.text.contains("小明"), p.text)
    }

    func testEscalationTextSaysHowLongItHasWaited() {
        XCTAssertTrue(post(.escalate, isCaptain: false).text.contains("7"))
    }

    func testShouldEscalateOnlyAfterThreshold() {
        let t0 = Date()
        XCTAssertFalse(SessionDecisionNotice.shouldEscalate(
            raisedAt: t0, now: t0.addingTimeInterval(60), after: 300))
        XCTAssertTrue(SessionDecisionNotice.shouldEscalate(
            raisedAt: t0, now: t0.addingTimeInterval(301), after: 300))
    }

    // MARK: - 状态如实：卡在菜单上的 session 不许显示成「空闲」

    /// 这条是整个特性在点名里的样子。菜单挂着时 session 不吐输出 → isWorking 为假 →
    /// 旧推导会报「🟡 空闲」，机长照常派活、活石沉大海。
    func testAwaitingDecisionOutranksIdle() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false, awaitingDecision: true),
            CrewSessionStateDerivation.awaitingDecision)
    }

    /// 答完就必须回正（#545：进得去出不来比没有状态更糟）。
    func testStateReturnsToIdleWhenDecisionCleared() {
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false, awaitingDecision: false),
            "idle")
    }

    /// 坏消息优先：真挂了/撞限额比「在等人选」更要紧，别被待决策盖住。
    func testHealthProblemsOutrankAwaitingDecision() {
        let limited = CrewSessionHealth(kind: .rateLimited, detail: "限额")
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: limited, isWorking: false, awaitingDecision: true),
            "rateLimited")
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: false, health: nil, isWorking: false, awaitingDecision: true),
            "exited")
    }

    /// codex 侧复用同一份选靶逻辑（它没有终端菜单，但同样「在等一个我们给不出的回答」）。
    func testCodexElicitationNoticeUsesSameTargeting() {
        let p = SessionDecisionNotice.post(
            stage: .first, sessionName: "codex-1", sessionId: "s1", isCaptain: false,
            question: "工具要求填一个表单", options: [], waitedMinutes: 0)
        XCTAssertEqual(p.mentionKinds, ["captain"])
        XCTAssertTrue(p.text.contains("工具要求填一个表单"))
    }
}
