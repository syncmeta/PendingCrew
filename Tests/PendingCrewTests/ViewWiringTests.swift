import XCTest

/// **接线断言** —— 防「零件造好了没装到车上」。
///
/// 病根（2026-07-26）：Todo 面板返工（Todo #4/#5/#11）新增了排序纯逻辑、状态圆圈、
/// 详细窗口三样零件，单测全绿、也合了 main，但**没有任何一个现有视图去用它们** ——
/// 用户装上新构建看到的还是老界面。单测只测零件本身，测不出「没装车」。
///
/// 这里补的就是那一刀：对每个「只要没人调用、功能就等于不存在」的符号，断言它在
/// 定义文件**之外**至少还有一处出现。扫的是仓库源码文本（路径由 `#filePath` 推出），
/// 不依赖运行期，也不需要把视图跑起来。
///
/// 加新零件时的规矩：如果它是「用户能看见的东西的入口」，在下面 `wirings` 里加一行。
final class ViewWiringTests: XCTestCase {

    /// (符号, 定义它的文件名, 人话说明它没接线会怎样)
    private static let wirings: [(symbol: String, definedIn: String, impact: String)] = [
        ("TodoListPresentation.newestFirst", "TodoListPresentation.swift",
         "Todo 列表不会从新到旧排，新建的条目不在最上面"),
        // 恢复 session 那条链（Todo #58）：印记 → 判定 → 弹窗 → 真恢复。
        // 判定层有 18 条用例，**接线一条都没有** —— 而这条链最容易断的正是接线：
        // 判定照样全绿，而人重启之后什么都不会被问、也什么都不会恢复。
        ("SessionRestoreOffer.decide", "SessionRestoreOffer.swift",
         "重启后不再判「上次是怎么结束的」，那枚退出印记写了没人读，恢复永远不触发"),
        ("SessionRestoreOffer.afterBackendReplaced", "SessionRestoreOffer.swift",
         "「刚更新过」那一路不再问，换完后端 session 静默全丢"),
        ("restoreSessions(", "CrewSessionRunner.swift",
         "人点了「恢复」也没有任何东西被拉起来 —— 弹窗成了一个空按钮"),
        ("CrewTodoStatusCircle(", "CrewTodoStatusCircle.swift",
         "Todo 行还是旧的方块状态标签，没有提醒事项那种圆圈/呼吸"),
        ("CrewTodoDetailWindowPresenter.shared", "CrewTodoDetailWindow.swift",
         "Todo 详细窗口没有任何入口，永远打不开"),
        ("CrewTodoFollowUp.perform", "CrewTodoFollowUp.swift",
         "Todo 追问/重开不发群也不唤醒机长，人的追问石沉大海"),
        ("GlassCloseButton(", "GlassCloseButton.swift",
         "玻璃白关闭件没人用，各浮层的关闭按钮还是各画各的（Todo #22 失效）"),
        ("CrewTodoPanel(", "CrewTodoPanel.swift",
         "右栏根本不显示 Todo 面板"),
        ("QuotaRingLayout.footnote", "QuotaRingLayout.swift",
         "额度行不显示重置时刻（Todo #14 的悬停效果失效）"),
        ("QuotaRingsFooter(", "QuotaRingsFooter.swift",
         "侧栏底部看不到额度环"),
        ("CrewMemberOrdering.sorted", "CrewMemberOrdering.swift",
         "成员列表不按创建时间倒序（Todo #15 失效）"),
        ("UncaughtExceptionLog.install", "UncaughtExceptionLog.swift",
         "未捕获异常不留痕，下次闪退又只剩一份没有异常名的 .ips"),
        ("CrewMentionFilter.onlyHumanMentions", "CrewMentionFilter.swift",
         "群聊时间线没人筛，「只看 @ 我的消息」判定造好了但列表照旧全显（Todo #61 失效）"),
        ("showOnlyHumanMentions:", "CrewChatView.swift",
         "没有任何地方把筛选开关喂给群聊，toolbar 上那个钮点了不动（Todo #61 失效）"),
        ("CrewMentionPickerLayout.maxHeight", "CrewMentionPickerLayout.swift",
         "@ 候选浮层的限高算好了却没人扣上去，列表照旧顶穿窗口（Todo #69 失效）"),
        ("ChiefResortRequest.decide", "ChiefResortRequest.swift",
         "刷新按钮的判定造好了没人调 —— 按下去要么什么都不发，要么绕开冷却窗连发（Todo #145 失效）"),
        // 找的是**调用点的形状**（`crewStore.requestChiefResort()`），不是方法名 ——
        // 光搜 `requestChiefResort` 会被 `CrewChiefListView` 顶上那段注释里的
        // `CrewStore.requestChiefResort` 满足，**接线拆掉照样绿**（拆线跑一趟当场抓到，
        // 与同文件 `acknowledgeBells` 那条是同一个坑）。
        ("crewStore.requestChiefResort()", "CrewStore.swift",
         "侧栏「总机长」视图上那个刷新按钮没接到任何动作，按了不动（Todo #145 失效）"),
        ("AgentCLIVersionView(", "AgentCLIVersionView.swift",
         "设置里看不到 claude / codex 版本，检测/升级/回滚/doctor 全没有入口（Todo #131 挪过去之后就只剩设置这一个调用点）"),
        ("TerminalBellTrace.summary(", "TerminalBellTrace.swift",
         "响铃变成了纯静音：BEL 不再发声，但也没人显示是哪个 session 响的（Todo #110 只剩一半）"),
    ]

    func testEveryUserFacingPieceIsActuallyWiredUp() throws {
        let sources = try Self.sourceFiles()
        XCTAssertGreaterThan(sources.count, 50, "源码扫描没扫到东西，测试本身失效了")

        for wiring in Self.wirings {
            let callSites = sources.filter { url, text in
                // **先把注释剥掉再找。** 不剥的话，一句 doc comment 里提到这个名字
                // 就足以让这条永远绿 —— 零件从车上拆下来了，尺子说还装着。
                //
                // 两趟变异量出来的（2026-09-12，拆掉 `newestFirst` 在详细窗口里那个
                // 唯一真调用点，只留 `CrewTodoPanel` 顶上那句注释）：
                //   不剥注释 → **绿**（这个洞是真的）
                //   剥了注释 → 红
                //
                // 本仓另外四份源码级扫描（`TodoBlockedOnHumanTests` /
                // `DecisionKindHasNoProducerTests` / `TodoDroppedAndAttentionTests` /
                // `CockpitOpenCloseCostTests`）早就各自带着 `codeOnly` 了，
                // 其中一份的注释还写着它是变异自证抓到的 ——
                // **这一份是漏掉的那个，不是新发明。**
                url.lastPathComponent != wiring.definedIn
                    && Self.codeOnly(text).contains(wiring.symbol)
            }
            XCTAssertFalse(
                callSites.isEmpty,
                """
                「\(wiring.symbol)」在 \(wiring.definedIn) 之外没有任何调用点 —— \
                零件造好了没装到车上：\(wiring.impact)。
                """)
        }
    }

    /// **响铃的痕迹要真的接到人看得见的那一行上**（人类 Todo #110）。
    ///
    /// 上面那条 wirings 只保证「`summary` 有人调」。这条钉住另外两个接头 —— 少任何
    /// 一个，声音是没了，但「哪个 session 响过」也跟着没了，那就只是换了个方式吞掉它：
    /// - `onBell` 没接：run 不知道响过，切换条那行不会重绘（提示要等下一次别的变更才蹭出来）；
    /// - `acknowledgeBells` 没接：铃铛点进去也不消，长亮的角标会被人训练成看不见。
    func testSessionRowIsWiredToTheBellTrace() throws {
        let runner = try Self.text(of: "CrewSessionRunner.swift")
        XCTAssertTrue(runner.contains("terminalView?.onBell"),
                      "响铃事件没接到 run 上：切换条不会因为它重绘")
        // 找的是**调用点**（`?.acknowledgeBells()`）不是方法名 —— 光搜
        // `acknowledgeBells()` 会撞上同一个文件里的 `func acknowledgeBells()` 定义，
        // 那把尺子在接线被拆掉时照样绿。（这条是拆线跑一趟当场抓到的。）
        XCTAssertTrue(runner.contains("?.acknowledgeBells()"),
                      "没人在选中 session 时清掉响铃提示：铃铛会长亮")

        // **两处都要挂**：切换条那一行只在终端模式看得到，而右栏平时停在成员列表 ——
        // 只挂一处的话，人大部分时间根本看不见铃铛，等于响铃被静静吞了。
        let window = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertEqual(
            window.components(separatedBy: "SessionBellHintView(run: run)").count - 1, 2,
            "响铃提示没有同时挂在切换条那一行和成员列表那一行上")
    }

    /// 上面那条只保证「有人用」；这条钉死**用户实际看的那个面板**在用。
    /// 2026-07-26 的漏接正是这种：详细窗口自己用了新零件，而右栏那块面板没有，
    /// 于是「有调用点」成立、用户却什么变化都看不到。
    func testTodoOverviewPanelUsesTheNewPresentation() throws {
        let panel = try Self.text(of: "CrewTodoPanel.swift")
        XCTAssertTrue(panel.contains("TodoListPresentation.newestFirst"),
                      "右栏 Todo 概览面板没按从新到旧排")
        XCTAssertTrue(panel.contains("CrewTodoStatusCircle("),
                      "右栏 Todo 概览面板还在用旧的状态标签，没换成状态圆圈")
        XCTAssertTrue(panel.contains("CrewTodoDetailWindowPresenter.shared"),
                      "右栏 Todo 概览面板没有开详细窗口的入口")
        XCTAssertTrue(panel.contains(".lineLimit(layout.bodyLineLimit)"),
                      "右栏 Todo 概览正文没有接三行截断契约")
        XCTAssertTrue(panel.contains("TodoListPresentation.overviewResponse(for: item)"),
                      "右栏 Todo 概览没有接精简的末条回应")
        XCTAssertTrue(panel.contains("UnevenRoundedRectangle("),
                      "右栏 Todo 概览卡片没有接左上方角、其余圆角的形状")
        XCTAssertFalse(panel.contains(".strikethrough("),
                       "已完成的 Todo 不该划删除线（人类明确要求，只变灰）")

    }

    /// Todo #92：概览卡片与群聊对方气泡必须共用同一组主题 token，不能再另写
    /// `surfaceMuted` 或颜色字面量，免得主题调整后两处悄悄漂开。
    func testTodoOverviewCardsReuseIncomingChatBubbleSurfaceAndHairline() throws {
        let panel = try Self.text(of: "CrewTodoPanel.swift")
        let bubble = try Self.text(of: "BubbleView.swift")
        XCTAssertTrue(bubble.contains(".fill(Theme.Palette.surface)"),
                      "群聊对方气泡的样式真值已变化，请同步更新 Todo 契约")
        XCTAssertTrue(panel.contains(".fill(CrewTodoPagePalette.card)"),
                      "Todo 概览卡片没有使用页面共用的白卡片颜色")
        XCTAssertTrue(panel.contains("static let card = Theme.Palette.surface"),
                      "Todo 白卡片没有复用群聊对方气泡的 surface token")
        let hairline = ".strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)"
        XCTAssertTrue(bubble.contains(hairline),
                      "群聊对方气泡的描边真值已变化，请同步更新 Todo 契约")
        XCTAssertTrue(panel.contains(hairline),
                      "Todo 概览卡片没有复用群聊气泡的描边")
        XCTAssertFalse(panel.contains("Theme.Palette.surfaceMuted.opacity(0.5)"),
                       "Todo 概览仍在使用旧的灰色填充")
    }

    /// Todo #95：两本账共用的概览行和详细行都必须显示同一份创建/更新时间文案。
    func testTodoRowsShowSharedCreationAndUpdateMetadata() throws {
        let panel = try Self.text(of: "CrewTodoPanel.swift")
        let detail = try Self.text(of: "CrewTodoDetailWindow.swift")
        let wiring = "TodoListPresentation.metadataText(for: item)"

        XCTAssertTrue(panel.contains(wiring), "Todo 概览卡片没有显示创建/更新时间")
        XCTAssertTrue(detail.contains(wiring), "Todo 详细行没有复用同一份创建/更新时间口径")
    }

    /// Todo #81：驾驶舱只展示 Agent 自己写下的计划与想法，不能再因仓库没有
    /// `docs/roadmap.md` 而空白，也不能把 Todo / task 账混进来冒充 Agent 的判断。
    func testCockpitOnlyShowsAgentPlansAndThoughts() throws {
        let root = try Self.text(of: "CockpitView.swift")
        XCTAssertTrue(root.contains("CockpitAgentMindView(crewId:"),
                      "驾驶舱没有接到 Agent 计划与想法视图")
        XCTAssertFalse(root.contains("CockpitRoadmapSegment("),
                       "旧仓库 roadmap 仍占据驾驶舱")
        XCTAssertFalse(root.contains("CockpitLoader.load("),
                       "驾驶舱仍依赖 crew 工作目录里的手工账本")

        let mind = try Self.text(of: "CockpitTasksView.swift")
        XCTAssertTrue(mind.contains("CockpitPlanStore.shared.list"),
                      "Agent 作战板没有成为驾驶舱的数据源")
        XCTAssertTrue(mind.contains("plan.updates.reversed()"),
                      "点开计划看不到 Agent 的判断与更新")
        XCTAssertFalse(mind.contains("LocalTodoStore.shared"),
                       "人类 Todo 仍被混进驾驶舱")
        XCTAssertFalse(mind.contains("data.taskItems"),
                       "coding-agent task 账仍被混进驾驶舱")
    }

    /// Todo #61：筛选钮必须**在中栏 toolbar 上**、且开关状态真的被喂进时间线。
    /// 「有调用点」不够 —— 判定函数被某个测试或别处引用一下也算有调用点，
    /// 但人在窗口里点不到就等于没有。
    func testMentionFilterIsReachableFromTheChatToolbar() throws {
        let center = try Self.text(of: "CrewCenterView.swift")
        XCTAssertTrue(center.contains("showOnlyHumanMentions:"),
                      "中栏没把筛选开关喂给 CrewChatView")
        XCTAssertTrue(center.contains("ToolbarItem"),
                      "中栏 toolbar 没了，筛选钮无处可挂")
        XCTAssertTrue(center.contains("Toggle(isOn: $onlyMentions)"),
                      "toolbar 上没有能翻这个开关的钮，人点不到")
        // Todo #69：人类指定的四个字，一个都不许润色。这条测试就是那四个字的守卫 ——
        // 谁哪天觉得「只看@我」更顺口就改，这里当场红。
        XCTAssertTrue(center.contains("Text(\"仅@你\")"),
                      "筛选钮不是人类指定的文字药丸「仅@你」（Todo #69）")
        XCTAssertFalse(center.contains("systemImage: \"at.circle\""),
                      "筛选钮还是那个没有文字的图标 —— 人看不出它是干什么的（Todo #69）")
        // 位置被人类改过两次，两次都写下来 —— 只留最新的一条，下一个人会看不出
        // 「最右」当初是被谁、为什么推翻的：
        // - Todo #79：钉在群聊栏**最右**上角（`.primaryAction`）。
        // - Todo #128（推翻上一条，人类原话「仅@你改成放在三个按钮的左侧」）：
        //   挪到 crew 详情 / 驾驶舱 / 刷新这三个按钮**左边**，即 toolbar 最前。
        //   所以它不能再用 `.primaryAction` —— 那个 placement 本身就是「推到最右」。
        // 声明顺序那条判据在 `MentionsFilterDefaultOnTests`，这里只钉「不许再钉右边」。
        XCTAssertFalse(center.contains("ToolbarItem(placement: .primaryAction)"),
                      "「仅@你」又被钉回最右了（Todo #128 要它在那三个按钮左侧）")
        XCTAssertTrue(center.contains(".tint(Theme.Palette.accent)"),
                      "「仅@你」点亮态没有复用发送键的主题绿色（Todo #79）")

        let sidebar = try Self.text(of: "CrewSidebarView.swift")
        XCTAssertTrue(sidebar.contains(".pickerStyle(.segmented)"))
        XCTAssertTrue(sidebar.contains(".tint(Theme.Palette.accent)"),
                      "层级/时间流仍继承系统蓝色，没有改成主题绿色（Todo #79）")

        let chat = try Self.text(of: "CrewChatView.swift")
        // 人类 Todo #140 ③ 之后筛选判定住在 `CrewTimelineFilter`（那个计算属性一帧被读
        // 八次、每次重筛 2618 条），所以「走没走筛选」要在那个文件里钉。
        let filter = try Self.text(of: "CrewTimelineFilter.swift")
        // **切到 `resolve` 的函数体里去看，而且要带括号。** 整文件找
        // `CrewMentionFilter.onlyHumanMentions` 会撞上同一文件注释里那句「口径见
        // `CrewMentionFilter.onlyHumanMentions`」—— 第一版就是这样，于是「把筛选整条
        // 拿掉」那刀被读成了绿。变异自证那一趟抓到的：**为了讲清楚而写下的注释，
        // 长成了被检测的形状。**
        let resolveBody: String = {
            guard let decl = filter.range(of: "static func resolve(") else { return "" }
            let tail = filter[decl.upperBound...]
            guard let end = tail.range(of: "\n    }") else { return String(tail) }
            return String(tail[..<end.lowerBound])
        }()
        XCTAssertFalse(resolveBody.isEmpty, "CrewTimelineFilter.resolve 不见了")
        XCTAssertTrue(resolveBody.contains("CrewMentionFilter.onlyHumanMentions("),
                      "群聊时间线判定没走筛选")
        // 仍然必须筛在 timelineEntries 这个源头 —— 渲染窗口那一整套（renderLimit /
        // hasMore /「上面还有 N 条」/ anchorOnExpand）全读它，筛在下游会出现
        // 「显示还有 300 条、点开什么都没有」。判定搬走了，**这条判据一个字没松**：
        // timelineEntries 仍要先取到筛过的那一份，windowedEntries 才在它下游开窗。
        let source = chat.range(of: "private var timelineEntries")
        let filtered = chat.range(of: "timelineFilterCache.entries(for:")
        let windowed = chat.range(of: "private var windowedEntries")
        XCTAssertNotNil(source); XCTAssertNotNil(filtered); XCTAssertNotNil(windowed)
        if let source, let filtered, let windowed {
            XCTAssertTrue(source.lowerBound < filtered.lowerBound
                          && filtered.lowerBound < windowed.lowerBound,
                          "筛选没落在 timelineEntries 里 —— 渲染窗口会按未筛选的条数算")
        }
        XCTAssertTrue(chat.contains("CrewChatWindow.window(timelineEntries"),
                      "渲染窗口不再开在 timelineEntries 下游")
    }

    /// 人类消息只能由白板观察器按 message id 投递。composer / Todo 再直投一次会用
    /// 随机 source key 绕过去重，表现成一条群消息唤醒两轮。
    func testHumanWhiteboardWakeHasOneDeliverySource() throws {
        for file in [
            "CrewChatView.swift",
            "CrewLocalTodoLanding.swift",
            "CrewHumanTodoRespond.swift",
            "CrewTodoFollowUp.swift",
        ] {
            let source = try Self.text(of: file)
            XCTAssertFalse(source.contains("CrewLocalMentionDelivery.injectAndWake"),
                           "\(file) 又绕过白板 message id 直投，人类消息会重复唤醒")
        }
        let roster = try Self.text(of: "CrewSessionWindowView.swift")
        guard let subscribe = roster.range(of: "private func subscribeRoster() async"),
              let refresh = roster.range(of: "private func refreshRoster() async") else {
            return XCTFail("找不到 roster 白板订阅边界")
        }
        let body = String(roster[subscribe.lowerBound..<refresh.lowerBound])
        XCTAssertFalse(body.contains("sessionRunner.startCaptain"),
                       "右栏观察器仍会与白板唯一 waker 抢拉 captain，赢家可能不带原消息")

        let runner = try Self.text(of: "CrewSessionRunner.swift")
        let outbound = try Self.text(of: "CrewDeferredWakeQueue.swift")
        XCTAssertTrue(outbound.contains("admission.performSend("),
                      "direct backend submit must execute inside durable admission")
        XCTAssertTrue(runner.contains("CrewSessionLaunchAdmission.perform("),
                      "Runner.start must call the testable production launch gate")
        XCTAssertFalse(runner.contains("run.backend.send(input)"),
                       "agent nudge text must not bypass durable admission")
        XCTAssertTrue(outbound.contains("await backend.submitWake(text)"),
                      "runner 仍把无回执 send 当作 wake 已投递")
        XCTAssertFalse(runner.contains("run.send(ready.text)"),
                       "瞬时 idle 后仍直接 fire-and-forget，拒绝会被误消费")
        XCTAssertTrue(runner.contains("deferredWakes.resolve(delivery, as: result)"))
        let wakeEntry = try XCTUnwrap(runner.range(of: "private func attemptWakeDelivery("))
        let wakeTail = runner[wakeEntry.lowerBound...]
        XCTAssertTrue(wakeTail.contains("CrewWakeOutbound.submit("),
                      "Runner must use the tested owner/viewer outbound route")
        XCTAssertTrue(runner.contains("scheduleDeferredWakeRetry(for: run)"),
                      "拒绝后仍要等第二条消息/新 idle 边沿，不能自行补投")
        let codex = try Self.text(of: "CodexAppServerBackend.swift")
        XCTAssertTrue(codex.contains("func submitWake(_ text: String) async"),
                      "Codex wake 没有以 turn/start RPC 受理为边界")
        XCTAssertTrue(codex.contains("wb?.commit()"),
                      "Codex 仍可能在 turn/start 受理前推进白板消费游标")
        let remote = try Self.text(of: "RemoteSessionBackend.swift")
        let endpoints = try Self.text(of: "SessionProtocolEndpoints.swift")
        XCTAssertTrue(endpoints.contains("wakeAdmission.performSend("),
                      "daemon submitWake must use the same durable admission")
        XCTAssertFalse(endpoints.contains("trustedPreAdmittedWake"),
                       "protocol servers must never trust an unproven pre-admission claim")
        XCTAssertFalse(remote.contains("trustedPreAdmittedWake"),
                       "the in-process bridge must use its own authoritative admission")
        XCTAssertFalse(endpoints.contains("priority:in-process"),
                       "unknown daemon roster must not silently bypass admission")
        let host = try Self.text(of: "SessionHost.swift")
        XCTAssertFalse(host.contains("crewMessageWakes.take()"),
                       "cross-crew report must not wake once by ID and again by direct host send")
        let waker = try Self.text(of: "CrewLocalMentionWaker.swift")
        XCTAssertTrue(waker.contains("CrewWakeScanProgress(cursor: cursors[crewId])"))
        XCTAssertTrue(waker.contains("guard progress.process("),
                      "scan must not consume its cursor when durable debt registration fails")
        let daemon = try Self.text(of: "SessionDaemonMain.swift")
        let launchGate = try XCTUnwrap(runner.range(of: "CrewSessionLaunchAdmission.perform("))
        let backendStart = try XCTUnwrap(runner.range(of: "let cliLease = config.kind.isAgent"))
        XCTAssertLessThan(launchGate.lowerBound, backendStart.lowerBound,
                          "Runner.start must admit before constructing or launching a backend")
        XCTAssertTrue(daemon.contains("case SessionOrchestrationOp.admissionRecovery:"))
        XCTAssertTrue(daemon.contains("runner.requestAdmissionRecovery(scope: scope, crewId: crewId)"),
                      "viewer recovery must be applied by the owning daemon, not claimed locally")
        XCTAssertTrue(daemon.contains("runner.submitExplicitText(text, id: id"),
                      "legacy daemon sendText must not bypass admission")
        XCTAssertTrue(endpoints.contains("submitExplicitInput(sessionId: sessionId"),
                      "protocol .input must not bypass admission")
        XCTAssertTrue(endpoints.contains("admitLiveTerminalReturn(sessionId: sessionId"),
                      "terminal Return must be admitted as live control input")
        XCTAssertFalse(endpoints.contains("输入行仍在终端"),
                       "a mixed text and Enter batch can already have changed the terminal")
        XCTAssertFalse(endpoints.contains("rawBytes: value.bytes"),
                       "terminal control bytes must never enter the durable text queue")
        let admission = try Self.text(of: "AutomaticWakeAdmission.swift")
        XCTAssertFalse(admission.contains("var rawBytes: [UInt8]?"),
                       "pending human text must not persist terminal control bytes")
        XCTAssertTrue(runner.contains("if item.requiresManualReview { continue }"),
                      "legacy raw-control markers must never be replayed into a new menu")
        let window = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertTrue(window.contains("sessionRunner.submitExplicitText(text, id: id, to: run)"),
                      "composer must wait for backend acceptance before clearing input")
        XCTAssertTrue(window.contains("恢复缺席机长的自动唤醒"),
                      "stopped captain must have a session-scoped recovery action")
        XCTAssertTrue(remote.contains("func submitWake(_ text: String) async"),
                      "远端 backend 没有把 wake 受理结果暴露给 runner")
        XCTAssertTrue(endpoints.contains("op: \"submitWake\""),
                      "协议端点没有把 wake 受理回执转发到统一字节流")
    }

    /// Todo #21：详细窗口得真有「改 / 删 / 追问」三件，且都在窗口里做完。
    ///
    /// Todo #62 起改 / 删走 `LocalTodoStore.shared(ledger)` —— 详细窗口现在有两个
    /// 药丸、看的是哪本账由 `ledger` 说了算。**必须带上 `(ledger)`**：写死
    /// `.shared` 就是对着 `.agent` 那本改人类那本的条目（#N 在两本账里指两件事）。
    func testTodoDetailWindowHasEditDeleteFollowUp() throws {
        let detail = try Self.text(of: "CrewTodoDetailWindow.swift")
        XCTAssertTrue(detail.contains("LocalTodoStore.shared(ledger).edit("),
                      "Todo 详细窗口改不了条目正文（或没跟着药丸走那本账）")
        XCTAssertTrue(detail.contains("LocalTodoStore.shared(ledger).delete("),
                      "Todo 详细窗口删不掉条目（或没跟着药丸走那本账）")
        XCTAssertTrue(detail.contains("CrewTodoFollowUp.perform"),
                      "Todo 详细窗口的追问没接发群+唤醒机长那条编排")
        // 「在详细的列表里面回复」= 就地输入，不弹新窗/新 sheet。
        XCTAssertTrue(detail.contains("TextField("),
                      "追问/改正文没有行内输入框，人被迫跳出去填")
        XCTAssertFalse(detail.contains(".sheet("),
                       "输入不该弹 sheet —— 人类要求在详细列表里就地做完")
    }

    /// Todo #21：详细窗口不能再「开出来就是最小的」。
    func testTodoDetailWindowOpensAtAUsableSize() throws {
        let detail = try Self.text(of: "CrewTodoDetailWindow.swift")
        XCTAssertTrue(detail.contains("sizingOptions = []"),
                      "没关掉 NSHostingController 的自动定尺，窗口会被 SwiftUI 理想尺寸压回最小")
        XCTAssertTrue(detail.contains("setContentSize("),
                      "没显式给初始内容尺寸")
        XCTAssertTrue(detail.contains("setFrameAutosaveName("),
                      "人拉过的窗口尺寸不会被记住，下次开又得重拉")
    }

    /// **Todo #138 ①：viewer 侧放掉链路只许有一个出口，而且必须走 `client.close()`。**
    ///
    /// 这条只能靠读源码：`Sources/Mac/Services` 不在 test bundle 里（project.yml 里
    /// 那一段注释写了为什么），所以那三条自发关闭路径没法在单测里真跑一遍。
    /// **这是这条断言的边界，写在这儿免得有人以为行为被覆盖了** —— 行为那一半在
    /// `RemoteSessionBackendTests.testSelfInitiatedCloseTearsDownEveryBackendState`，
    /// 它测的是出口本身；这一条只保证三条路都从那个出口走。
    ///
    /// 判据选「`client = nil` 只许出现一次」而不是「每处都调过 close」：后者要靠
    /// 读上下文，改法一变就成假的；前者是结构性的 —— 只有一个地方能放掉它，
    /// 那个地方对了就都对了。
    func testViewerLinkTeardownHasExactlyOneSiteAndItGoesThroughTheClient() throws {
        let text = try Self.text(of: "ViewerSessionClient.swift")
        // 只数**代码**里的：注释里解释这条规矩时也会写出 `client = nil` 这几个字，
        // 数进去的话这把尺子会被自己要防的那段说明骗到。
        let code = text.split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")
        let drops = code.components(separatedBy: "client = nil").count - 1
        XCTAssertEqual(drops, 1,
                       "放掉 client 的地方不止一处 —— 断链的状态清理迟早会漏掉其中一条")
        XCTAssertTrue(code.contains("client?.close()"),
                      "那个唯一出口没走 client.close()，句柄和能力表不会跟着断")
        XCTAssertTrue(code.contains("teardownLink()"),
                      "三条自发关闭路径没有汇合到同一个出口")
    }

    /// #121 第二批：不是只造 TLS 零件。daemon 入口、viewer 选择与设置按钮必须首尾接通；
    /// 远程分支失败时不能经过本机 daemon 的接管/拉起路径。
    func testSecureRemoteBackendIsWiredIntoProductionCallersWithoutLocalFallback() throws {
        let main = Self.codeOnly(try Self.text(of: "SessionDaemonMain.swift"))
        let host = Self.codeOnly(try Self.text(of: "SessionDaemonHost.swift"))
        let viewer = Self.codeOnly(try Self.text(of: "ViewerSessionClient.swift"))
        let runner = Self.codeOnly(try Self.text(of: "CrewSessionRunner.swift"))
        let sessionHost = Self.codeOnly(try Self.text(of: "SessionHost.swift"))
        let settings = Self.codeOnly(try Self.text(of: "CrewSettingsView.swift"))

        XCTAssertTrue(main.contains("SessionDaemonSecureListenerConfiguration.fromEnvironment"),
                      "--daemon 生产入口没有显式读取安全监听配置")
        XCTAssertTrue(main.contains("SessionDaemonHost(secureListener:"),
                      "安全监听配置没有传进真实 daemon host")
        XCTAssertTrue(host.contains("SecureTCPListener("), "daemon host 没有真正创建 TLS listener")
        XCTAssertTrue(host.contains("server.accept(link: link)"),
                      "TLS accept 没汇入现有 SessionProtocolServer")

        XCTAssertTrue(viewer.contains("BackendRegistry.connectRemote("),
                      "viewer 远程分支没有调用安全 registry connector")
        XCTAssertTrue(viewer.contains("SessionProtocolClient("),
                      "viewer 没用现有协议客户端握手并取 roster")
        guard let remoteStart = viewer.range(of: "private func connectRemoteBackend()"),
              let remoteEnd = viewer.range(of: "private func", range: remoteStart.upperBound..<viewer.endIndex)
        else { return XCTFail("找不到独立的远程连接分支") }
        let remoteBody = String(viewer[remoteStart.lowerBound..<remoteEnd.lowerBound])
        XCTAssertFalse(remoteBody.contains("applyFallback("),
                       "远程失败走进了本机接管路径，会把本机 session 冒充远端")
        XCTAssertFalse(remoteBody.contains("UnixSocketTransport.connect"),
                       "远程失败路径仍可能退到本机 socket")
        XCTAssertTrue(remoteBody.contains("runner.viewerBackendWillChange()"),
                      "远程 connector 启动前没有清掉旧后端的 viewer roster/link")
        XCTAssertTrue(runner.contains("func viewerBackendWillChange()"),
                      "runner 没有把跨后端切换与普通断线区分开")

        XCTAssertTrue(sessionHost.contains("func connectViewer(to ref: BackendRef)"),
                      "SessionHost 没有可由后端选择触发的重连入口")
        XCTAssertTrue(settings.contains("sessionHost.connectViewer(to: ref)"),
                      "设置里的后端选择没有接到 viewer")

        let sharedLink = try Self.projectText(of: "Sources/Shared/SessionMessageLink.swift")
        XCTAssertFalse(sharedLink.contains("#if os(macOS)"),
                       "下一批 iOS 要复用的链路接口仍被 macOS 编译条件锁死")
        XCTAssertTrue(sharedLink.contains("protocol SessionMessageLink"))
        XCTAssertTrue(sharedLink.contains("protocol SessionMessageLinkConnecting"))
    }

    func testManualPairingAndPersistentListenerAreWiredIntoProductionSettingsAndDaemon() throws {
        let main = Self.codeOnly(try Self.text(of: "SessionDaemonMain.swift"))
        let settings = Self.codeOnly(try Self.text(of: "CrewSettingsView.swift"))
        let shared = try Self.projectText(of: "Sources/Shared/PairingExchange.swift")

        XCTAssertTrue(main.contains("fromPersistentSettings("),
                      "daemon 仍只靠人工环境变量启用安全监听")
        XCTAssertTrue(settings.contains("ManualPairingCoordinator.production("),
                      "设置页没有接到生产配对存储")
        XCTAssertTrue(settings.contains("createInvitation("), "设置页不能生成手动邀请")
        XCTAssertTrue(settings.contains("importText("), "设置页不能导入邀请或回应")
        XCTAssertTrue(settings.contains("sessionHost.restartLocalBackend()"),
                      "配对完成后没有复用已有安全重启入口")
        XCTAssertTrue(settings.contains("需要重启"), "设置页没告诉人监听配置何时生效")

        XCTAssertFalse(shared.contains("#if os(macOS)"),
                       "下一批 iOS 要复用的配对数据格式仍被锁在 macOS")
        XCTAssertTrue(shared.contains("struct ManualPairingInvitationEnvelope"))
        XCTAssertTrue(shared.contains("struct ManualPairingResponseEnvelope"))
        XCTAssertTrue(shared.contains("enum ManualPairingTextCodec"))
    }

    /// #121 第四批：行为测试证明了真实 TLS/RPC；这条只锁生产 caller，防止绿零件
    /// 没有被 iOS 根状态、配对入口和 daemon 唯一账本接上。
    func testIOSRemoteCrewDataPlaneIsWiredIntoProductionCallers() throws {
        let appModel = Self.codeOnly(try Self.text(of: "AppModel.swift"))
        let pairing = Self.codeOnly(try Self.text(of: "IOSRemotePairingView.swift"))
        let shell = Self.codeOnly(try Self.text(of: "IPadShell.swift"))
        let list = Self.codeOnly(try Self.text(of: "CrewListView.swift"))
        let store = Self.codeOnly(try Self.text(of: "CrewStore.swift"))
        let daemon = Self.codeOnly(try Self.text(of: "SessionDaemonMain.swift"))
        let server = try Self.projectText(of: "Sources/Mac/LocalRunner/SessionProtocolEndpoints.swift")

        XCTAssertTrue(appModel.contains("RemoteBackendConfiguration.production()"))
        XCTAssertTrue(appModel.contains("RemotePendingCrewBackend(configuration:"))
        XCTAssertFalse(appModel.contains("return nil\n        #endif"),
                       "iOS AppModel.backend 又退回恒 nil 空壳")
        XCTAssertTrue(pairing.contains("ManualPairingCoordinator.production()"))
        XCTAssertTrue(pairing.contains("appModel.reloadRemoteBackend()"))
        XCTAssertTrue(pairing.contains("crewStore.refreshList()"),
                      "首次配对后没有主动刷新，启动时跑过的 .task 不会再来")
        XCTAssertTrue(shell.contains("IOSRemotePairingView()"))
        XCTAssertTrue(list.contains("Button(\"重试\")"))
        XCTAssertTrue(store.contains("error = nil"), "列表重试仍会显示上一轮错误")
        XCTAssertTrue(daemon.contains("host.server.crewBackend = model.backend"),
                      "daemon 没把唯一 LocalBackend 账本接到 crew RPC")
        XCTAssertTrue(server.contains("backend.postCrewMessage("),
                      "远端 post 没有直达 daemon backend")

        for path in [
            "Sources/Shared/SessionProtocol.swift",
            "Sources/Shared/SecureTCPTransport.swift",
            "Sources/Shared/ManualPairingCoordinator.swift",
            "Sources/Shared/CrewRPC.swift",
        ] {
            XCTAssertFalse(try Self.projectText(of: path).contains("#if os(macOS)"),
                           "\(path) 仍被 macOS-only 条件锁住")
        }
    }

    /// #121 第五批的 production caller：保留 iOS 远端会话和终端导航，
    /// 退役的本地手动审批不再是连接门槛。
    func testIOSRemoteSessionTerminalIsWiredIntoProductionCallers() throws {
        let shell = Self.codeOnly(try Self.text(of: "IPadShell.swift"))
        let chat = Self.codeOnly(try Self.text(of: "CrewChatView.swift"))
        let roster = Self.codeOnly(try Self.text(of: "CrewRosterBar.swift"))
        let detail = Self.codeOnly(try Self.text(of: "IOSRemoteSessionView.swift"))

        XCTAssertTrue(roster.contains("onOpenSession(sessionID)"),
                      "iOS roster 的 session 成员仍不可点")
        XCTAssertTrue(chat.contains("onOpenSession: onOpenRemoteSession"),
                      "CrewChat 没把远端 session 导航交给 roster")
        XCTAssertTrue(shell.contains("IOSRemoteSessionView(crewID:"),
                      "iOS shell 没有 session 详情导航")
        XCTAssertTrue(detail.contains("backend.openSession(sessionID:"),
                      "详情页没有接共享连接上的 attach")
        XCTAssertTrue(detail.contains("remoteSessionBackend?.closeSession(sessionID:"),
                      "详情退出时没有 detach/release 远端 session")
        XCTAssertTrue(detail.contains("Button(\"重新连接\""),
                      "显式断线态没有恢复入口")
    }

    /// Todo #22：关闭按钮只此一处定义 —— 别的浮层不许再手糊圆形叉。
    func testCloseButtonStyleIsDefinedOnlyOnce() throws {
        for file in ["CockpitView.swift"] {
            let text = try Self.text(of: file)
            XCTAssertTrue(text.contains("GlassCloseButton("),
                          "\(file) 的关闭按钮没用共用的玻璃白件")
            XCTAssertFalse(text.contains("Theme.Palette.danger, in: Circle())"),
                           "\(file) 还留着自己那颗红圆叉，样式又分了两处")
        }
    }

    /// Todo #4/#5：不能只造 protocol/store 零件。人必须能从正在看的 Codex
    /// Todo #82/#83/#90/#151：窄右栏保留独立的模型与 effort 菜单；Codex
    /// 技术流折叠态只说具体程序/档名，完整命令与路径放进可展开详情。
    func testSessionHeaderAndCodexActivityUseHumanFacingPresentation() throws {
        let view = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertTrue(view.contains("private var modelMenu"), "模型没有独立手动菜单")
        XCTAssertTrue(view.contains("private var effortMenu"), "effort 没有独立手动菜单")
        XCTAssertTrue(view.contains("跟随 Codex 默认"),
                      "模型菜单没有恢复到 Codex 当前默认的明确入口")
        XCTAssertTrue(view.contains("当前在途推理保持原模型"),
                      "Codex 菜单必须区分已选模型的即时提交与在途推理仍用原模型的边界")

        let runner = try Self.text(of: "CrewSessionRunner.swift")
        XCTAssertTrue(runner.contains("clearModelOverride("),
                      "选择 Codex 默认后没有清掉持久模型覆盖，重启还会回到旧模型")
        XCTAssertTrue(runner.contains("codexDefaultModelSelection"),
                      "UI 的默认选项没有传到真正的运行时切换编排")
        XCTAssertTrue(runner.contains("resolveNativeDefaultModel()"),
                      "运行中“跟随 Codex 默认”必须让 app-server 按 session cwd 原生解析")
        XCTAssertFalse(runner.contains("requestedModel = SessionLaunchOptions.codexDefaultModel("),
                       "运行中切换不能用顶层 config 或 model/list 自己固定一个 slug")

        let codex = try Self.text(of: "CodexTranscriptView.swift")
        let presentation = try Self.text(of: "CodexThreadItem.swift")
        XCTAssertTrue(presentation.contains("已读取档案"))
        XCTAssertTrue(presentation.contains("已执行指令"))
        XCTAssertTrue(presentation.contains("已修改档案"))
        XCTAssertTrue(codex.contains("DisclosureGroup"), "活动行不能点击展开详情")
        XCTAssertTrue(presentation.contains("完整指令"), "展开态没有完整命令")
        XCTAssertTrue(presentation.contains("涉及档案"), "展开态没有完整文件路径")
        XCTAssertTrue(codex.contains("presentation.headline"), "折叠态没有具体活动摘要")
        XCTAssertFalse(codex.contains("Text(command).font(Theme.Fonts.monoSmall)"),
                       "Codex 活动流仍在折叠态直接铺 shell 原文")
    }

    /// Todo #151：这是源码级结构/命令回归，测试 target 不编译 SwiftUI 详情页。
    /// 把关键区段单独截出，避免 Claude/终端路径或注释碰巧含同名控件而假绿。
    func testCodexSessionChromeKeepsControlsUnderNameAndCommandsReachable() throws {
        let view = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        func section(_ start: String, _ end: String) throws -> String {
            let a = try XCTUnwrap(view.range(of: start), "缺少 \(start)")
            let b = try XCTUnwrap(view.range(of: end, range: a.upperBound..<view.endIndex), "缺少 \(end)")
            return String(view[a.upperBound..<b.lowerBound])
        }

        let terminal = try section("private var terminalContent:", "private var memberListMode:")
        XCTAssertTrue(terminal.contains("if sessionRunner.current?.kind == .codex"))
        XCTAssertTrue(terminal.contains("codexComposer"), "Codex 未接独立 composer")

        let composer = try section("private var codexComposer:", "private var canStartSession:")
        for symbol in ["CodexSessionComposer(", "onSend:"] {
            XCTAssertTrue(composer.contains(symbol), "Codex composer 缺少 \(symbol)")
        }
        XCTAssertTrue(terminal.contains("sessionRunner.applyProfileChange("))
        XCTAssertTrue(view.contains("sessionRunner.submitExplicitText(text, id: id, to: run)"),
                      "人工发送必须经 admission 并等受理回执")
        let chrome = try section("private struct CodexSessionComposer:", "private struct CodexControlPillLabel:")
        for symbol in ["ComposerTextField(", "onHardwareReturn:", "CodexWorkspaceFooter(", "GitInspector.repoRoot(", "GitInspector.currentBranch(", "GitInspector.isLinkedWorktree(", ".accessibilityLabel(\"发送到 Codex\")"] {
            XCTAssertTrue(chrome.contains(symbol),
                          "Codex 输入区缺少可发现的操作或状态：\(symbol)")
        }
        let runContent = try section("private struct SessionRunContentView:", "private struct CodexSessionComposer:")
        XCTAssertTrue(runContent.contains("if run.kind == .codex"), "Codex 没有独立的简洁顶栏")
        XCTAssertTrue(runContent.contains("SessionProfileControl(run: run, onSwitch: onSwitchProfile)"), "会话配置未移到名称下")
        XCTAssertTrue(runContent.contains("run.stop()"), "停止命令消失")
        XCTAssertTrue(runContent.contains("CodexTranscriptView(transcript:"), "结构化 transcript 消失")
        XCTAssertTrue(runContent.contains("codexUsageRow"), "上下文和额度信息消失")
        XCTAssertTrue(view.contains(".accessibilityLabel(\"停止这个 Codex session\")"))
        XCTAssertTrue(view.contains(".accessibilityLabel(\"选择模型\")"))
        XCTAssertTrue(view.contains(".accessibilityLabel(\"选择推理强度\")"))
    }

    /// Todo #188: CLI recovery must be visible before a run exists and while
    /// the Codex transcript is open. Both surfaces consume the same typed state;
    /// the view must not construct install commands from an error string.
    func testCodexCLIRecoveryUsesTypedStateInBothSessionSurfaces() throws {
        let session = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let transcript = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CodexTranscriptView.swift"))
        XCTAssertTrue(session.contains("CodexCLIProvisioningNotice("),
                      "启动前的成员列表必须给出可操作的 CLI 恢复提示")
        XCTAssertTrue(transcript.contains("CodexCLIProvisioningNotice("),
                      "运行中的 Codex transcript 必须保留同一恢复提示")
        XCTAssertTrue(transcript.contains("CodexCLIProvisioningState"),
                      "统一提示必须消费核心的类型化状态，不能从报错字符串猜安装动作")
        XCTAssertTrue(transcript.contains("center.prepareCodexInstall()"),
                      "首次点击只能向核心准备固定安装动作")
        XCTAssertEqual(transcript.components(separatedBy: "center.installCodex(").count - 1, 1,
                       "安装动作只应在二次确认按钮中出现一次")
        XCTAssertTrue(transcript.contains("请由你自行完成原生 Codex 登录"),
                      "安装成功只证明 CLI，不能冒充已认证或自动启动 session")
    }

    /// #188 review: only a typed Codex-missing launch failure may put the install
    /// action on that error message; dismissing the settings dialog must reset it.
    func testCodexCLIRecoveryActionIsScopedToTypedFailureAndDismissalResetsIt() throws {
        let session = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let settings = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/AgentCLIVersionView.swift"))
        let runner = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Services/CrewSessionRunner.swift"))
        XCTAssertFalse(session.contains("selectedKind == .codex || sessionRunner.lastStartError != nil"),
                       "Claude 启动报错不能顺带露出 Codex 安装提示")
        XCTAssertTrue(session.contains("action: sessionRunner.lastStartErrorAction")
                      && runner.contains("lastStartErrorAction = StartErrorAction.forFailure(error)"),
                      "错误行动必须来自启动失败的类型化标记")
        XCTAssertTrue(session.contains("CrewSessionErrorMessage("),
                      "安装行动必须附在对应的报错消息，而不是仅有独立提示卡")
        XCTAssertTrue(session.contains("actions.contains(.prepareOfficialCodexCLIInstall)")
                      && session.contains("CodexCLIInstallActionButton(center: cliVersions)"),
                      "报错行的按钮必须受类型化行动门控")
        XCTAssertTrue(settings.contains(".onChange(of: confirmingCodexInstall)"),
                      "Esc 或外侧关闭确认框时必须复位待确认动作")
    }

    /// Preparing the fixed action changes `.missing` to `.awaitingSecondConfirmation`.
    /// The dialog's state owner must survive that switch in both entry points.
    func testCodexInstallConfirmationOwnerSurvivesMissingToAwaitingTransition() throws {
        let transcript = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CodexTranscriptView.swift"))
        let session = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let notice = try Self.requiredSection(transcript, "struct CodexCLIProvisioningNotice: View {",
                                              "@ViewBuilder\n    private func notice")
        XCTAssertTrue(notice.contains("CodexCLIInstallActionButton(center: center)"),
                      "notice 的确认框宿主不能只活在 missing 分支中")
        let error = try Self.requiredSection(session, "private struct CrewSessionErrorMessage: View {",
                                             "private struct SessionBellHintView: View {")
        guard let waiting = error.range(of: "case .awaitingSecondConfirmation"),
              let button = error.range(of: "CodexCLIInstallActionButton(center: cliVersions)") else {
            return XCTFail("报错行缺少等待确认状态或安装动作")
        }
        XCTAssertGreaterThan(button.lowerBound, waiting.lowerBound,
                             "报错行确认框宿主不能只活在 missing 分支中")
    }

    /// #188: one allowlisted action model must drive a system launch error and
    /// a Codex transcript row. Message text is payload only, never an action key.
    func testBuiltinMessageActionsAreWiredToErrorAndTranscriptRows() throws {
        let session = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let transcript = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CodexTranscriptView.swift"))
        let runner = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Services/CrewSessionRunner.swift"))
        XCTAssertTrue(runner.contains("enum CrewMessageAction"),
                      "缺少封闭的类型化消息动作目录")
        XCTAssertTrue(transcript.contains("CrewMessageActionCatalog.forTranscript"),
                      "会话正文没有接消息动作")
        XCTAssertTrue(session.contains("CrewMessageActionCatalog.forSystemError"),
                      "系统启动错误没有接同一套动作目录")
        XCTAssertTrue(session.contains("CrewMessageActionDispatcher.dispatch"),
                      "系统错误行没有经白名单分发")
        XCTAssertTrue(transcript.contains("CrewMessageActionDispatcher.dispatch"),
                      "会话动作没有经白名单分发")
        XCTAssertTrue(transcript.contains("if actions.isEmpty {"),
                      "无动作的富文本、未知或命令行不得被空 contextMenu 覆盖")
        XCTAssertTrue(transcript.contains("let actions = CrewMessageActionCatalog.forTranscript(item)"),
                      "菜单可用性必须由类型化目录决定")
    }

    /// #166: source contract for Codex's narrow session chrome. The test target
    /// does not compile the SwiftUI view, so this guards wiring and labels;
    /// the macOS app build separately checks SwiftUI type correctness.
    func testCodexSessionChromeUsesCompactControlsAndChatBubblePalette() throws {
        let view = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let transcript = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CodexTranscriptView.swift"))
        func section(_ source: String, _ start: String, _ end: String) throws -> String {
            let a = try XCTUnwrap(source.range(of: start), "缺少 \(start)")
            let b = try XCTUnwrap(source.range(of: end, range: a.upperBound..<source.endIndex), "缺少 \(end)")
            return String(source[a.upperBound..<b.lowerBound])
        }
        let header = try section(view, "private var codexHeader:", "private var header:")
        let usage = try section(view, "private var codexUsageRow:", "private func statusBadge(")
        let composer = try section(view, "private struct CodexSessionComposer:", "private struct CodexWorkspaceFooter:")
        let profile = try section(view, "private struct SessionProfileControl:", "#endif")
        let agent = try section(transcript, "private func agentRow(", "private func userRow(")
        let user = try section(transcript, "private func userRow(", "enum Dot")

        XCTAssertTrue(view.contains("CodexControlPillLabel(title: name)"))
        XCTAssertTrue(view.contains("CodexControlPillLabel(title: run.effort ?? \"默认\")"))
        XCTAssertFalse(profile.contains("icon: \"cpu\""))
        XCTAssertFalse(profile.contains("icon: \"brain.head.profile\""))
        XCTAssertTrue(profile.contains("run.fastMode.map { $0 ? \"bolt.fill\" : \"bolt\" } ?? \"questionmark\""))
        XCTAssertTrue(profile.contains("run.fastMode == nil)"), "未知态不能显示为关闭并接受点击")
        XCTAssertTrue(profile.contains(".accessibilityLabel(\"Codex 快速模式\")"))
        XCTAssertTrue(profile.contains("SessionLaunchOptions.modelPickerOptions(for: run.kind, catalog: catalog.file)"),
                      "模型须来自已验新鲜度的运行时目录")
        XCTAssertTrue(profile.contains("SessionLaunchOptions.codexDefaultModelSelection"),
                      "跟随 Codex 默认仍须清除持久覆盖")
        XCTAssertTrue(composer.contains("placeholder: \"\""))
        XCTAssertFalse(composer.contains("向 Codex 发送消息"))

        XCTAssertTrue(agent.contains(".fill(Theme.Palette.surface)"))
        XCTAssertTrue(user.contains(".fill(Theme.Palette.userBubble)"))
        XCTAssertTrue(header.contains("Circle().trim(from: 0, to:"), "顶栏缺少上下文占用圆环")
        XCTAssertTrue(header.contains("to: usage.contextFraction"), "圆环须反映当前上下文占用")
        XCTAssertTrue(header.contains("showingCodexUsage = true"), "统计入口必须保留")
        XCTAssertFalse(header.contains("Button(action: onCompact)"), "Codex 压缩由原生流程管理")
        XCTAssertFalse(header.contains("arrow.down.right.and.arrow.up.left"), "移除疑似退出全屏的压缩图标")
        XCTAssertTrue(header.contains("SessionProfileControl(run: run, onSwitch: onSwitchProfile)"),
                      "模型、effort、快速控件应在 session 名下")
        XCTAssertTrue(header.contains(".frame(width: 18, height: 18)"), "上下文圆环应缩小")
        XCTAssertTrue(header.contains("Circle().fill(.red)"), "停止按钮应恢复红色")
        XCTAssertTrue(usage.contains("usage.contextTokens.formatted()"))
        XCTAssertTrue(usage.contains("usage.contextWindow.formatted()"))
        XCTAssertFalse(composer.contains("SessionProfileControl(run: run"), "输入框内不应重复放配置控件")
        XCTAssertFalse(profile.contains(".toggleStyle(.button)"), "快速图标不应有按钮底色")
        XCTAssertTrue(profile.contains("let current = run.fastMode else { return }"), "未知状态不能发反向请求")
        XCTAssertTrue(profile.contains("onSwitch(nil, nil, !current) {"), "快速请求须从已确认状态取反")
        XCTAssertTrue(profile.contains("fastRequestInFlight = false"), "须等切换回调完成才解锁")
        XCTAssertTrue(profile.contains("|| fastRequestInFlight || run.fastMode == nil)"),
                      "连点和未知状态须禁用快速入口")
        XCTAssertFalse(header.contains("Menu {"), "压缩折叠菜单应移除")
        XCTAssertFalse(header.contains("查看上下文与额度"), "重复统计入口应移除")
        XCTAssertFalse(usage.contains("Button(\"压缩上下文\""), "统计弹窗里不应重复压缩入口")
    }

    func testSessionVersionBadgeRemovedAndModelPillsUseWhiteSurface() throws {
        let view = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let row = try Self.requiredSection(view, "private func sessionRowContent(", "private func latestStep(")
        XCTAssertTrue(row.contains("if run.kind != .codex,"), "Codex 行应隐藏版本标签")
        XCTAssertTrue(row.contains("HelperBuildBadgeLabel"), "其他 runner 原有版本状态应保留")
        let pill = try Self.requiredSection(view, "private struct CodexControlPillLabel:", "private func profileLabel(")
        XCTAssertTrue(pill.contains(".background(Theme.Palette.surface, in: Capsule())"))
        XCTAssertTrue(pill.contains(".overlay(Capsule().strokeBorder(Theme.Palette.hairline"))
        let readonlyPill = try Self.requiredSection(view, "private struct SessionProfilePillLabel:", "private struct HelperBuildBadgeLabel:")
        XCTAssertTrue(readonlyPill.contains(".fill(Theme.Palette.surface)"))
        XCTAssertTrue(readonlyPill.contains(".strokeBorder(Theme.Palette.hairline"))
    }

    func testUnreadableQuotaViewsAreHiddenWithoutClearingErrors() throws {
        let view = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let footer = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/QuotaRingsFooter.swift"))
        let usage = try Self.requiredSection(view, "private var codexUsageRow:", "private func statusBadge(")
        XCTAssertTrue(usage.contains("QuotaRingLayout.shouldDisplay(quota.codex, failure: quota.codexError)"))
        XCTAssertFalse(usage.contains("账号额度：读不到"))
        XCTAssertTrue(footer.contains("QuotaRingLayout.shouldDisplay(quota.claude, failure: quota.claudeError)"))
        XCTAssertTrue(footer.contains("QuotaRingLayout.shouldDisplay(quota.codex, failure: quota.codexError)"))
        XCTAssertFalse(footer.contains("staleBadge: claudeWarning"))
        XCTAssertFalse(footer.contains("staleBadge: codexWarning"))
    }

    /// #166 follow-up: Codex's running-session menus must consume only the
    /// fresh app-server picker snapshot. This source contract checks the View
    /// wiring; catalog eligibility and per-model efforts need executable core
    /// tests, and visible menu layout still needs human review.
    func testCodexSessionProfileRequiresFreshPickerAndHonestEmptyState() throws {
        let view = Self.codeOnly(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let start = try XCTUnwrap(view.range(of: "private struct SessionProfileControl:"))
        let end = try XCTUnwrap(view.range(of: "#endif", range: start.upperBound..<view.endIndex))
        let profile = String(view[start.lowerBound..<end.lowerBound])

        XCTAssertTrue(profile.contains("PickerOptions"), "Codex 菜单未接入目录资格判定")
        XCTAssertFalse(profile.contains("SessionLaunchOptions.codexDefaultModel(catalog:"),
                       "运行中 session 的默认模型不能用全局目录猜 slug")
        XCTAssertTrue(profile.contains("if run.kind == .codex { Text(name) }"),
                      "实际运行 slug 不能冒充用户显式选择的模型")
        XCTAssertTrue(profile.contains("if run.kind == .codex || model != run.model"),
                      "跟随默认解析到相同 slug 时仍须允许显式固定该模型")
        XCTAssertTrue(profile.contains("if run.kind == .codex || effort != run.effort"),
                      "Codex 当前 effort 与显式覆盖也不能只凭显示值混同")
        XCTAssertTrue(profile.contains("availableModels.isEmpty"), "无合格目录时未显示空态")
        XCTAssertTrue(profile.contains("暂无可选模型"), "无目录时缺简短空态")
        XCTAssertTrue(profile.contains("重试"), "无目录时缺探测重试线索")
        XCTAssertTrue(profile.contains("Text(modelPickerOptions.unavailableReason ??"),
                      "模型不可用的具体原因必须在菜单正文可见，不能只藏在 help")
        XCTAssertTrue(profile.contains("Text(effortUnavailableReason ??"),
                      "effort 不可用的具体原因必须在菜单正文可见，不能只藏在 help")
        XCTAssertTrue(profile.contains(".accessibilityLabel(modelPickerOptions.unavailableReason ??"),
                      "模型不可用原因的完整文案须可供辅助使用读取")
        XCTAssertTrue(profile.contains(".accessibilityLabel(effortUnavailableReason ??"),
                      "effort 不可用原因的完整文案须可供辅助使用读取")
        XCTAssertTrue(profile.contains("SessionLaunchOptions.effortPickerOptions("),
                      "effort 候选未走逐模型 PickerOptions")
        XCTAssertTrue(profile.contains("for: .codex, model: run.model, catalog: catalog.file"),
                      "换模型后须按新目标模型重算 effort 候选")
        XCTAssertTrue(profile.contains("SessionLaunchOptions.codexDefaultModelSelection"),
                      "跟随 Codex 默认入口必须始终独立可选")
    }

    /// Todo #80：退出后的成员行必须恢复那一个持久 session，不能把点击退化成
    /// 「新 session」入口；runner 类型也必须以持久账本为准，不能靠可改的显示名猜。
    func testExitedMemberRowResumesItsPersistedSession() throws {
        let view = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertTrue(view.contains("persistedMember:"),
                      "成员行没有携带持久 session 记录，退出后无法区分该恢复哪一个")
        XCTAssertTrue(view.contains("openPersistedSession(member)"),
                      "点击退出成员没有接到恢复原 session 的动作")
        XCTAssertTrue(view.contains("sessionRunner.restartMember("),
                      "恢复动作没有复用原 sessionId / agent conversation id")
        XCTAssertTrue(view.contains("$0.sessionId == member.sessionId"),
                      "恢复后没有精确选择并打开被点击的那个 session")

        let runner = try Self.text(of: "CrewSessionRunner.swift")
        let start = try XCTUnwrap(runner.range(of: "    func restartMember("))
        let end = try XCTUnwrap(runner.range(of: "    /// worker 启动共用体", range: start.upperBound..<runner.endIndex))
        let restart = String(runner[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(restart.contains("let kind = try LocalCodingAgentKind.restartingMember("))
        XCTAssertTrue(restart.contains("recordedKind: recorded?.kind, displayName: member.displayName"),
                      "恢复 runner 必须接到持久化记录决策")
        XCTAssertTrue(restart.contains("onIncident: { recordReadFailure = $0.summary }"))
        XCTAssertTrue(restart.contains("recordReadFailure: recordReadFailure)"))
        XCTAssertTrue(restart.contains("brief: brief, kind: kind, workdir: workdir"),
                      "实际启动必须使用恢复决策的 runner")
        XCTAssertFalse(restart.contains("captainDefault("))
        XCTAssertFalse(restart.contains("inferred(fromDisplayName:"))
    }

    /// Todo #88：系统帮助菜单必须落到公开文档站，不能依赖未配置的 Help Book。
    func testHelpMenuOpensPendingCrewDocumentation() throws {
        let app = try Self.text(of: "PendingCrewApp.swift")
        XCTAssertTrue(app.contains("CommandGroup(replacing: .help)"),
                      "帮助菜单没有被 PendingCrew 的公开文档入口接管")
        XCTAssertTrue(app.contains("https://docs.pendingname.com/pendingcrew/"),
                      "帮助菜单没有指向人类指定的 PendingCrew 文档地址")
        XCTAssertTrue(app.contains("NSWorkspace.shared.open(PendingCrewLinks.helpDocumentation)"),
                      "帮助菜单只定义了地址但没有真正打开它")
    }

    /// Todo #93：README 图片只能使用可移植的仓库相对路径或公网 URL；相对路径
    /// 必须真实存在且大小写完全一致，避免开发机绝对路径在 GitHub 上静默变成 404。
    func testReadmeImagesUsePortableExistingRepositoryPaths() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let expression = try NSRegularExpression(
            pattern: #"(?:!\[[^\]]*\]\(([^)]+)\)|<img\b[^>]*\bsrc="([^"]+)")"#,
            options: [.caseInsensitive]
        )

        for readmeName in ["README.md", "README_EN.md"] {
            let readme = try Self.projectText(of: readmeName)
            let matches = expression.matches(
                in: readme, range: NSRange(readme.startIndex..., in: readme))
            let references = matches.compactMap { match -> String? in
                for group in 1...2 where match.range(at: group).location != NSNotFound {
                    guard let range = Range(match.range(at: group), in: readme) else { continue }
                    return String(readme[range])
                }
                return nil
            }
            XCTAssertFalse(references.isEmpty, "\(readmeName) 没扫到任何图片，校验本身失效")

            for reference in references {
                if reference.hasPrefix("https://") || reference.hasPrefix("http://") {
                    continue
                }
                XCTAssertFalse(
                    reference.hasPrefix("/") || reference.hasPrefix("file://"),
                    "\(readmeName) 使用了不可移植的本机绝对图片路径：\(reference)")

                let decoded = reference.removingPercentEncoding ?? reference
                let candidate = root.appendingPathComponent(decoded).standardizedFileURL
                XCTAssertTrue(
                    candidate.path.hasPrefix(root.path + "/"),
                    "\(readmeName) 图片路径逃出了仓库：\(reference)")
                XCTAssertTrue(
                    FileManager.default.fileExists(atPath: candidate.path),
                    "\(readmeName) 图片不存在：\(reference)")
                XCTAssertTrue(
                    Self.repositoryPathExistsWithExactCase(decoded, under: root),
                    "\(readmeName) 图片路径大小写与仓库不一致：\(reference)")
            }
        }
    }

    /// Todo #43：系统通知不再冒充普通 session 的随机 emoji 头像；旧白板与新写入
    /// 都由 resolver 认成 PendingCrew，并在气泡位使用 App 品牌图标。
    func testPendingCrewSystemIdentityUsesAppIcon() throws {
        let resolver = try Self.text(of: "CrewSenderResolver.swift")
        let sender = try Self.text(of: "GroupBubbleSender.swift")
        let avatar = try Self.text(of: "CrewAvatarBadges.swift")
        XCTAssertTrue(resolver.contains("PendingCrewSystemMessage.isSystem"),
                      "历史 system 行没有进入统一 PendingCrew 身份判定")
        XCTAssertTrue(sender.contains("isPendingCrewApp"),
                      "气泡 sender 没携带 PendingCrew App 身份")
        XCTAssertTrue(avatar.contains("if sender.isPendingCrewApp"),
                      "头像渲染没有为 PendingCrew App 分流")
        XCTAssertTrue(avatar.contains("Image(\"BrandMark\")"),
                      "PendingCrew 系统通知仍用随机 emoji，不是 App 品牌图标")
    }

    /// PendingCrew 系统通知仍占用群聊统一的 30pt 头像槽，只把 App 图标本体缩小
    /// 一个小档位（4pt → 26pt）。这样普通人类/session 头像与消息行布局均不变。
    func testPendingCrewSystemAvatarIsSmallerInsideStandardMessageSlot() throws {
        let bubble = try Self.text(of: "BubbleView.swift")
        let avatar = try Self.text(of: "CrewAvatarBadges.swift")

        XCTAssertTrue(bubble.contains("CrewAvatarBadges(sender: g, size: 30)"),
                      "群聊头像槽不再是统一的 30pt，系统头像调整不应改消息行布局")
        XCTAssertTrue(
            avatar.contains("private static let pendingCrewAppImageReduction: CGFloat = 4"),
            "PendingCrew App 图标没有固定缩小一个 4pt 小档位（30pt → 26pt）")
        XCTAssertTrue(
            avatar.contains("sender.isPendingCrewApp ? size - Self.pendingCrewAppImageReduction : size"),
            "缩小尺寸没有只接到 PendingCrew App 身份分支")
        XCTAssertTrue(avatar.contains(".frame(width: baseImageSize, height: baseImageSize)"),
                      "PendingCrew App 图标没有使用缩小后的本体尺寸")
        XCTAssertTrue(avatar.contains(".frame(width: size, height: size)"),
                      "头像组件外层槽位被缩小，消息行对齐会随之改变")
    }

    /// Todo #43：只有进程自己退出才发这句；文案必须走统一语义函数，不能由各个
    /// lifecycle 分支自行拼出不同口径。
    func testSessionSelfEndNoticeUsesOneLiteralTemplate() throws {
        let store = try Self.text(of: "LocalWhiteboardStore.swift")
        let runner = try Self.text(of: "CrewSessionRunner.swift")
        XCTAssertTrue(store.contains("Session「\\(sessionName)」自己结束了。它最后一句话：\\(closing)"),
                      "session 自己结束文案不是人类指定的统一模板")
        XCTAssertTrue(runner.contains("PendingCrewSystemMessage.sessionEnded"),
                      "session lifecycle 没有调用统一结束语义")
        XCTAssertTrue(runner.contains("reason != .userStopped"),
                      "人/机长主动停止也会被误报成 session 自己结束")
    }

    /// Todo #87：订阅档位只能自动检测；设置页不留人工覆盖、更新入口移到 App 菜单
    /// 「关于 PendingCrew」下面，外观区不再堆说明文字。
    func testSettingsAndMenusMatchTodo87() throws {
        let settings = try Self.text(of: "CrewSettingsView.swift")
        let app = try Self.text(of: "PendingCrewApp.swift")
        let quota = try Self.text(of: "AgentQuota.swift")
        let center = try Self.text(of: "QuotaCenter.swift")
        let launch = try Self.text(of: "LocalSessionLaunch.swift")
        let worldModel = try Self.text(of: "LocalSessionWorldModel.swift")
        let project = try Self.projectText(of: "project.yml")

        XCTAssertFalse(settings.contains("「跟随系统」随设备的浅色/深色自动切换"),
                       "外观区域说明仍在")
        XCTAssertFalse(settings.contains("AgentSubscriptionPlanPreference"),
                       "设置页仍能人工覆盖订阅档位")
        XCTAssertFalse(settings.contains("UpdateSettingsSection("),
                       "更新入口仍在设置页")
        XCTAssertTrue(app.contains("CommandGroup(after: .appInfo)"),
                      "检查更新没有放到「关于 PendingCrew」下方")
        XCTAssertTrue(app.contains("Button(\"检查更新…\")"),
                      "App 菜单缺少中文检查更新入口")
        XCTAssertTrue(project.contains("developmentLanguage: zh-Hans"),
                      "macOS 自动生成菜单仍以英文作为开发语言")

        for (name, source) in [
            ("AgentQuota.swift", quota),
            ("QuotaCenter.swift", center),
            ("LocalSessionLaunch.swift", launch),
            ("LocalSessionWorldModel.swift", worldModel),
        ] {
            XCTAssertFalse(source.contains("subscriptionPlanOverride"),
                           "\(name) 仍保留人工覆盖字段/注入")
            XCTAssertFalse(source.contains("AgentSubscriptionPlanPreference"),
                           "\(name) 仍保留人工覆盖持久化入口")
            XCTAssertFalse(source.contains("手动设置"),
                           "\(name) 仍可能向 session 注入手动档位")
        }
    }

    /// 人类 Todo #131：左下角那块只留额度环 —— 账号头像那行的今日 token 用量去掉，
    /// claude / codex 的 CLI 版本从页脚**挪进**设置页（挪，不是复制）。
    ///
    /// 三条分别对着三个会翻车的方向：
    /// ① 版本还留在左下角（挪成了复制）；
    /// ② 设置页压根没接上（挪丢了）；
    /// ③ 账号头像那行的用量还在（该去的没去）。
    func testAgentCLIVersionMovedFromFooterToSettingsTodo131() throws {
        let footer = try Self.text(of: "QuotaRingsFooter.swift")
        let sidebar = try Self.text(of: "CrewSidebarView.swift")
        let settings = try Self.text(of: "CrewSettingsView.swift")

        // ① 左下角不再出现 CLI 版本 —— 页脚和侧栏都不许再引用它。
        for (name, source) in [("QuotaRingsFooter.swift", footer),
                               ("CrewSidebarView.swift", sidebar)] {
            XCTAssertFalse(source.contains("AgentCLIVersionView("),
                           "\(name) 仍在左下角画 CLI 版本（Todo #131 要求挪走，不是复制）")
            XCTAssertFalse(source.contains("AgentCLIVersionCenter"),
                           "\(name) 仍持有 CLI 版本检测中心，左下角还会跑版本检测")
        }

        // ② 设置里出现了，而且四个能力（升级 / 回滚 / doctor / 重新检测）跟着一起到。
        XCTAssertTrue(settings.contains("AgentCLIVersionView("),
                      "设置页没有 CLI 版本入口 —— 版本被挪丢了，不是挪走了")
        XCTAssertTrue(settings.contains("AgentCLIVersionCenter.shared"),
                      "设置页没接上版本检测中心，打开设置不会去检测")
        // ⚠️ 认的是**能力**，不是那四个按钮的全名。原来这里钉的是逐字的按钮文案，
        // 2026-09-12 把 popover 摊平成设置里的一块（Todo #11）时顺手改短了两个标签
        // （「运行健康检查（doctor）」→「健康检查（doctor）」、「重新检测版本」→
        // 「重新检测」），这条当场红 —— 它红得对（有东西变了），但红的原因是措辞，
        // 而它要防的是**能力丢失**。措辞会一直改，能力不该没了，所以判据下沉到
        // 认得出那件事的最短片段。
        let version = try Self.text(of: "AgentCLIVersionView.swift")
        for capability in ["检查更新并升级", "回滚到本机保留版本", "doctor", "重新检测"] {
            XCTAssertTrue(version.contains(capability),
                          "挪位置时把「\(capability)」这项能力一起删了 —— 人类要的是换个地方显示")
        }
        // 这一块保持直接可见；Todo #152 移除了 CLI 手选目录。
        XCTAssertFalse(version.contains(".popover("),
                       "编码工具那块又变回「点一下才出来」了 —— 人类原话：不希望点击之后再出一个框")
        XCTAssertFalse(version.contains("LocalCodingAgentExecutable.setOverrideDirectory"),
                       "CLI 路径由 PATH 首命中决定，设置不应提供旧的手选目录")

        // ④ 「后端」那一页真的接上了模型层（人类 Todo #11 后半 / #121）。
        //
        // **这条不是锦上添花**：`BackendRegistry` 建好之后，全仓引用它的文件数是 **0** ——
        // 一个 192 行、注释写得很完整的模型层，界面上一个字都看不到。
        // 「建好了没接上」在这个仓库里是常客，而它最安静：编译过、测试绿、没人报错。
        XCTAssertTrue(settings.contains("BackendRegistry.load"),
                      "设置里没有「后端」那一页，或者它没去读登记表 —— 模型层又成了孤儿")
        // 人类要的是「**管理**、连接后端」，不是「看一眼后端」。新增现在必须通过
        // 完整配对事务落 trust + registry；删除必须持锁重读，不能用打开设置时的旧快照
        // 覆盖刚配对出来的后端。`removePersisted` 内部仍复用 `removing` 的内置拒绝理由。
        XCTAssertTrue(settings.contains("ManualPairingCoordinator.production"),
                      "新增远程后端没有走完整配对事务")
        XCTAssertTrue(settings.contains("BackendRegistry.removePersisted"),
                      "「移除」没有走持锁重读的模型路径，可能覆盖并发配对新增")
        XCTAssertTrue(settings.contains("BackendRegistry.connectivity"),
                      "「能不能连」没问模型层 —— 界面自己判的话，"
                      + "「远程绝不静默降级成本机」那条保证就绕过去了")
        // 实况与重启入口（计划 #15）：判定在模型层（`BackendLiveStatusTests`），
        // 这里钉**接上了**。界面自己去探本机再填到行上，「外部条目不许拿本机读数冒充」
        // 那条就绕过去了；自己拼按钮文案，「viewer 里不许叫停用」那条也绕过去了。
        XCTAssertTrue(settings.contains("BackendRegistry.liveStatus"),
                      "后端页没显示实况，或者没问模型层")
        XCTAssertTrue(settings.contains("BackendRegistry.restartAction"),
                      "重启按钮没问模型层 —— 文案和能不能按是界面自己定的")
        XCTAssertTrue(settings.contains("sessionHost.restartLocalBackend"),
                      "重启没走 SessionHost —— 换代公告 / 问接回就漏了")
        let app = Self.codeOnly(try Self.text(of: "PendingCrewApp.swift"))
        if let settingsScene = app.range(of: "Settings {") {
            XCTAssertTrue(app[settingsScene.upperBound...].prefix(400)
                            .contains(".environmentObject(sessionHost)"),
                          "设置窗没注入 sessionHost —— 打开「后端」页一读环境对象就崩")
        } else {
            XCTFail("找不到 Settings 场景")
        }
        let host = Self.codeOnly(try Self.text(of: "SessionHost.swift"))
        if let restart = host.range(of: "func restartLocalBackend()") {
            let body = String(host[restart.upperBound...].prefix(3000))
            if let announce = body.range(of: "appendSessionMessage"),
               let stop = body.range(of: "DaemonStopper(") {
                XCTAssertLessThan(announce.lowerBound, stop.lowerBound,
                                  "重启是先停再说 —— 人先看到 session 全断、几秒后才看到解释")
            } else {
                XCTFail("restartLocalBackend 里找不到公告或停旧")
            }
            XCTAssertTrue(body.contains("SessionRestoreOffer.afterBackendReplaced"),
                          "设置里换代之后没问要不要接回")
        } else {
            XCTFail("找不到 SessionHost.restartLocalBackend")
        }

        // ③ 账号头像那行的今日 token 用量去掉（额度环是另一回事，必须还在）。
        XCTAssertFalse(sidebar.contains("AgentUsageLine"),
                       "账号头像那行还挂着今日 token 用量")
        XCTAssertFalse(sidebar.contains("LocalAgentUsageMonitor"),
                       "侧栏仍在读今日 token 用量")
        XCTAssertTrue(sidebar.contains("QuotaRingsFooter(quota: quota)"),
                      "订阅额度环被一起删掉了 —— 人类去掉的是账号头像那行的用量，不是额度环")
    }

    /// A live GUI must carry the interrupted daemon's registry through reconnect.
    /// The replacement daemon removes that registry before its first hello.
    func testDaemonReconnectReachesTheGUIRestoreOfferOnce() throws {
        let viewer = Self.codeOnly(try Self.text(of: "ViewerSessionClient.swift"))
        let host = Self.codeOnly(try Self.text(of: "SessionHost.swift"))
        guard let closed = viewer.range(of: "private func linkClosed()"),
              let reconnected = viewer.range(of: "private func finishConnected()"),
              let deliver = viewer.range(of: "private func deliverInterruptedDaemonOffer(to"),
              let install = host.range(of: "private func installViewer(selection:") else {
            return XCTFail("viewer 断线 / 重连 / GUI 接线入口缺失")
        }
        let closeBody = String(viewer[closed.upperBound...].prefix(2200))
        let reconnectBody = String(viewer[reconnected.upperBound...].prefix(900))
        let deliverBody = String(viewer[deliver.upperBound...].prefix(2400))
        let installBody = String(host[install.upperBound...].prefix(1600))
        XCTAssertTrue(closeBody.contains("reconnectOffer.capture(hello: lastHello"))
        XCTAssertTrue(closeBody.contains("registry.daemonPid == lastHello.pid"))
        XCTAssertTrue(reconnectBody.contains("deliverInterruptedDaemonOffer(to: lastHello)"))
        XCTAssertTrue(deliverBody.contains("reconnectOffer.complete(with: hello)"))
        XCTAssertTrue(deliverBody.contains("onDaemonReconnected?"))
        XCTAssertTrue(viewer.contains("store.readRun(pid: interruptedDaemon.hello.pid)"),
                      "新 daemon 覆盖共享印记后仍须读旧进程的最终状态")
        XCTAssertTrue(viewer.contains("marker.belongs(to: interruptedDaemon.hello)"),
                      "旧 PID 被复用时不能把另一轮印记当成旧进程的退出状态")
        XCTAssertTrue(deliverBody.contains("current.exit == .diedWhileDraining"),
                      "正常排空期间新 daemon 抢先握手时不能误弹")
        XCTAssertTrue(installBody.contains("viewer.onDaemonReconnected ="))
        XCTAssertTrue(installBody.contains("self.restoreOffer = offer"),
                      "恢复提示必须交给 GUI 的 SessionHost")
        XCTAssertTrue(installBody.contains("offer.shouldAsk, !self.restoreOffer.shouldAsk"),
                      "已有启动或换代提示时不能叠第二个")
        XCTAssertTrue(installBody.contains("suppressedDaemonReconnectPID == previousPID"),
                      "设置里主动重启不能再由重连事件弹第二次")
        XCTAssertTrue(host.contains("suppressedDaemonReconnectPID = previousDaemonPID"),
                      "主动重启前必须记住要忽略的旧 daemon")
    }

    /// 恢复弹窗 / 前端更新带后台换代 / app 退出印记，必须跑在**界面进程**里。
    ///
    /// 它们原来写在 `SessionHost.start`。默认模式（viewer）下界面从不调 `start`，
    /// 调它的是**后台进程** —— 于是弹窗永远不弹、换代永远不换，后台还冒名写 app 印记。
    /// 三笔判定层都有测试，接线只有构建证明；这条补的就是那一截。
    /// 见 `docs/internal/2026-09-13-startup-duties-wiring-fix.md`。
    func testInterfaceStartupDutiesRunInTheInterfaceProcessNotTheDaemon() throws {
        let host = Self.codeOnly(try Self.text(of: "SessionHost.swift"))
        let daemon = Self.codeOnly(try Self.text(of: "SessionDaemonMain.swift"))

        /// 从签名切到下一个同缩进的方法声明。
        func body(_ signature: String) throws -> String {
            guard let head = host.range(of: signature) else {
                throw XCTSkip("SessionHost.swift 里找不到 \(signature) —— 改名了就同步改这条测试")
            }
            let rest = host[head.upperBound...]
            let next = ["\n    func ", "\n    private func ", "\n    @discardableResult"]
                .compactMap { rest.range(of: $0)?.lowerBound }.min() ?? rest.endIndex
            return String(rest[..<next])
        }
        let start = try body("func start(model: AppModel, crewStore: CrewStore)")
        let begin = try body("func begin(model: AppModel, crewStore: CrewStore)")
        let duties = try body("private func runInterfaceStartupDutiesOnce()")

        // ① `start` 里不许再有它们 —— 后台进程也调 `start`。
        for needle in ["role: .app", "BackendUpdateCoordinator.runIfNeeded",
                       "SessionRestoreOffer.decide", "willTerminateNotification"] {
            XCTAssertFalse(start.contains(needle),
                           "`SessionHost.start` 里又出现了 \(needle) —— 那会跑在后台进程里，界面一次都不跑")
            XCTAssertTrue(duties.contains(needle), "界面启动职责里缺了 \(needle)")
            XCTAssertFalse(daemon.contains(needle) && needle == "role: .app",
                           "后台进程在写 app 印记")
        }

        // ② `begin` 必须调它，而且在按角色分岔**之前** —— viewer 那一支也得跑。
        guard let call = begin.range(of: "runInterfaceStartupDutiesOnce()"),
              let fork = begin.range(of: "switch ProcessRole.requested") else {
            return XCTFail("`begin` 没调界面启动职责 —— 默认模式下弹窗又不弹了")
        }
        XCTAssertLessThan(call.lowerBound, fork.lowerBound,
                          "界面启动职责排在角色分岔之后 —— 某一支会漏掉它")

        // ③ 只跑一次：`begin` 挂在 `.task` 上会重跑。
        XCTAssertTrue(duties.contains("guard !interfaceDutiesRan"),
                      "界面启动职责没有只跑一次的门 —— 视图重挂会把本轮印记读成上一轮")

        // ④ ⌘Q 时只有真编排者才停 run。viewer 里的 run 是后台的镜像，
        //    停它等于关界面就停掉后台全部 session。
        guard let guardAt = duties.range(of: "ProcessRole.effective == .orchestrator"),
              let stopAt = duties.range(of: "run.stop()") else {
            return XCTFail("⌘Q 观察者里找不到「只有编排者才停 run」那道判断")
        }
        XCTAssertLessThan(guardAt.lowerBound, stopAt.lowerBound,
                          "⌘Q 时 viewer 也会停 run —— 关界面就停掉后台的 session")
    }

    /// 恢复弹窗里点「接回来」：viewer 必须整笔转交后台，后台必须真的接住。
    ///
    /// 判定在 `SessionRestoreRoute`（有测试）；这条钉的是**接线**：`restoreSessions`
    /// 先问路由再决定在本进程拉，后台 `handle` 真的处理了这个操作 —— 少了后一半，
    /// 请求会落进 `default:` 只写一行日志，而界面回执说「已交给后台」。
    func testRestoreIsRoutedAndTheDaemonHandlesIt() throws {
        let runner = Self.codeOnly(try Self.text(of: "CrewSessionRunner.swift"))
        let daemon = Self.codeOnly(try Self.text(of: "SessionDaemonMain.swift"))

        guard let head = runner.range(of: "func restoreSessions(") else {
            return XCTFail("找不到 CrewSessionRunner.restoreSessions")
        }
        let rest = runner[head.upperBound...]
        guard let route = rest.range(of: "SessionRestoreRoute.decide("),
              let local = rest.range(of: "restoreHere(") else {
            return XCTFail("`restoreSessions` 没先问路由 —— viewer 里会自己拉 agent，与后台双头")
        }
        XCTAssertLessThan(route.lowerBound, local.lowerBound,
                          "在本进程接回排在路由判定之前 —— viewer 里会先拉起来再问")
        XCTAssertTrue(rest.contains("SessionOrchestrationOp.restoreSessions"),
                      "转交那一支没发编排请求")

        guard let handled = daemon.range(of: "case SessionOrchestrationOp.restoreSessions:") else {
            return XCTFail("后台没处理 restoreSessions —— 请求会落进 default: 被静默丢掉")
        }
        let branch = daemon[handled.upperBound...].prefix(900)
        XCTAssertTrue(branch.contains("runner.restoreSessions("),
                      "后台接到了请求却没去接回")
    }

    /// Todo #56 ④⑤：纯终端既要真接进 session UI，也必须从 crew agent 编排面隔离。
    func testPlainTerminalIsWiredIntoSessionUIWithoutAgentOrchestration() throws {
        let view = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertTrue(view.contains("Image(systemName: \"apple.terminal\")"),
                      "新建 session 页没有统一的终端图标")
        XCTAssertTrue(view.contains("private var sessionKindControls"),
                      "Claude Code / Codex / 终端没有共用纵向药丸选择器")
        XCTAssertTrue(view.contains("VStack(spacing: 8)"),
                      "session 类型没有从上到下纵向排列")
        XCTAssertTrue(view.contains("sessionKindPill(.terminal, title: \"终端\")"),
                      "新建 session 页没有纯终端选项")
        XCTAssertTrue(view.contains("Capsule().fill("),
                      "session 类型的每一行没有画成完整药丸")
        XCTAssertFalse(view.contains(".pickerStyle(.segmented)"),
                       "session 类型仍是横向分段选择，不是从上到下一行一个药丸")
        XCTAssertTrue(view.contains("case .terminal:\n                break"),
                      "纯终端启动分支没有与世界观/MCP 注入明确断开")
        XCTAssertTrue(view.contains("$0.crewId == crewStore.selectedDetail?.crew.id && $0.kind.isAgent"),
                      "右栏成员富列表仍可能把纯终端画成 crew 成员")

        let runner = try Self.text(of: "CrewSessionRunner.swift")
        XCTAssertTrue(runner.contains("for run in runs where run.kind.isAgent"),
                      "list_sessions 快照仍可能登记纯终端")
        XCTAssertTrue(runner.contains("if role != .captain, config.kind.isAgent"),
                      "纯终端仍可能登记进成员花名册/@ 候选")
        XCTAssertTrue(runner.contains("guard run.status == .running, run.kind.isAgent else { return }"),
                      "crew 唤醒仍可能向纯终端注入文本")

        let launch = try Self.text(of: "LocalSessionLaunch.swift")
        XCTAssertTrue(launch.contains("guard runnerKind.isAgent else { return nil }"),
                      "世界观渲染入口没有拒绝纯终端")
    }

    /// Todo #70：「设为机长」必须真的走 captain 启动语义，而且新建页明确新开
    /// conversation；只画一枚勾选框、最后仍以 worker role 启动不算完成。
    func testNewSessionCanStartAsAFreshCaptain() throws {
        let view = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertTrue(view.contains("Toggle(\"设为机长\", isOn: $startsAsCaptain)"),
                      "新建 session 页面没有「设为机长」勾选项")
        XCTAssertTrue(view.contains("if startsAsCaptain {"),
                      "勾选状态没有接到发送/启动分支")
        XCTAssertTrue(view.contains("sessionRunner.startFreshCaptain("),
                      "勾选后没有走新机长的交接编排入口")
        XCTAssertTrue(view.contains("kind: selectedKind"),
                      "机长启动没有使用人在页面上选的 session 类型")
        XCTAssertTrue(view.contains("启动后会停止当前机长，由这个新 session 接任。"),
                      "已有运行中机长时，页面没有向人说明会发生交接")

        let runner = try Self.text(of: "CrewSessionRunner.swift")
        XCTAssertTrue(runner.contains("func startFreshCaptain("),
                      "runner 没有新建机长的单一编排入口")
        XCTAssertTrue(runner.contains("CaptainHandoffTransaction.perform("),
                      "新机长没有走可回滚的统一交接事务")
        XCTAssertTrue(runner.contains("CaptainHandoffAuthorization.validateLiveRequester("),
                      "直系子机长救援没有在 live runner 复核父 crew 当前机长")
        XCTAssertTrue(runner.contains("request.sourceCrewId"))
        XCTAssertTrue(runner.contains("request.targetCrewId"))
        XCTAssertTrue(runner.contains("setCaptainAgentKindReportingFailure"),
                      "新机长类型没有以可报告失败的方式落盘")
        // Todo #101：GUI 的两个交接入口在 viewer 里必须**整笔**转交后台。
        // 只转发「起新」那一步、把「确认在跑」留在只有镜像的本地，正是那个 bug 本身。
        XCTAssertTrue(runner.contains("private func forwardCaptainHandoffToOwner("),
                      "runner 没有把整笔交接转交持有者的单一出口")
        XCTAssertEqual(
            runner.components(separatedBy: "if forwardCaptainHandoffToOwner(").count - 1, 2,
            "GUI 的两个交接入口（现有成员 / 新建机长）必须都先问归属")
        XCTAssertTrue(runner.contains("CaptainHandoffOwnership.claim(isViewer: isViewer)"),
                      "归属判定没有走可单测的那一处，又散成了就地的 if isViewer")
        XCTAssertTrue(runner.contains("ownership: CaptainHandoffOwnership,"),
                      "归属票没有落在 executeCaptainHandoff 的参数表里 —— 只写注释拦不住下一个人")
        XCTAssertTrue(runner.contains("throw RunnerError.captainHandoffNotOwner"),
                      "viewer 里跑交接没有硬失败，第三条路仍然能悄悄长出来")
        XCTAssertTrue(runner.contains("func performForwardedCaptainHandoff("),
                      "daemon 侧没有承接转交过来的交接")
        // 交接期间挡下的普通 @唤醒必须留下来补投；`return false` 是它的终点，
        // 两个调用方都不看这个 Bool。
        XCTAssertTrue(runner.contains("captainHandoffHeldWakes.hold(crewId: crewId, text: wakeText)"),
                      "交接门禁又在静默丢唤醒了")
        XCTAssertTrue(runner.contains("defer { releaseCaptainHandoffHeldWakes(crewId: crewId) }"),
                      "被挡下的唤醒没有在交接收尾时补投/留痕")
        let daemon = try Self.text(of: "SessionDaemonMain.swift")
        XCTAssertTrue(daemon.contains("SessionOrchestrationOp.captainHandoff"),
                      "daemon 没有接线机长交接的编排请求，转发过去会被 default 分支丢掉")

        XCTAssertTrue(runner.contains("resumePreviousConversation: false"),
                      "新建机长错误地续接了旧机长 conversation")
        XCTAssertTrue(runner.contains("resumePreviousConversation: Bool = true"),
                      "普通机长重启的历史续跑默认语义被破坏")
        XCTAssertTrue(runner.contains("if resumeCaptainId == nil && resumePreviousConversation"),
                      "新建机长没有真正绕开历史 conversation 查询")
        XCTAssertTrue(runner.contains("run.onCaptainUnavailable ="),
                      "普通机长启动失败/认证失效仍只报错，没有接到自动救援入口")
        XCTAssertTrue(runner.contains("recoverUnavailableClaudeCaptain("),
                      "Claude 机长不可用时没有实际执行 Codex 接任")
        XCTAssertTrue(runner.contains("model: \"gpt-5.6-sol\", effort: \"high\""),
                      "自动接任没有钉住获授权的 Codex Sol/high 配置")
    }

    /// Todo #71：纯逻辑说「黄色呼吸」还不够，侧栏实际那颗 crew 点必须真的用上
    /// CoreAnimation 版 BreathingDot，不能只留一颗静态 Circle。
    func testCrewTodoYellowIndicatorIsWiredToBreathingDot() throws {
        let row = try Self.text(of: "CrewSidebarCrewRow.swift")
        XCTAssertTrue(row.contains("if color.breathes"),
                      "crew 状态点没有读取黄色呼吸语义")
        XCTAssertTrue(row.contains("BreathingDot(size: 10, color: fill(color))"),
                      "黄色 Todo 指示仍是静态点，没有接 CoreAnimation 呼吸点")
        XCTAssertTrue(row.contains(".accessibilityLabel(accessibilityLabel(color))"),
                      "状态点没有把本 crew / 下属 crew 的区别接到辅助功能文案")
    }

    /// Todo #78：删除的是跨机 Workspace 同步整层，不只是藏掉侧栏入口。
    /// 工作目录迁移与 session git worktree 属于本地执行基础，仍由各自测试覆盖。
    func testWorkspaceSyncLayerIsAbsent() throws {
        let sources = try Self.sourceFiles()
        let removedFiles: Set<String> = [
            "WorkspaceSyncView.swift", "WorkspaceSetupSheet.swift", "WorkspaceSyncStore.swift",
            "SyncEngine.swift", "WorkspaceRepoService.swift", "WorkspaceRepoLayout.swift",
            "WorkspaceManifest.swift", "MachineRegistration.swift", "ProjectSyncService.swift",
            "WorkspaceGit.swift", "SyncReceipt.swift",
        ]
        XCTAssertTrue(
            sources.allSatisfy { !removedFiles.contains($0.0.lastPathComponent) },
            "Workspace 同步实现文件又被编回产品；#78 要求整层删除")

        let sidebar = try Self.text(of: "CrewSidebarView.swift")
        XCTAssertFalse(sidebar.contains("Workspace 同步"),
                       "侧栏仍暴露已删除的 Workspace 同步入口")
        XCTAssertFalse(sidebar.contains("showingWorkspaceSync"),
                       "侧栏仍保留 Workspace 同步 sheet 状态/接线")
    }

    /// #64: the dynamic pipe tests end at SessionBackend (the standalone test
    /// target excludes CrewSessionRun). This is explicitly a source wiring guard.
    func testFirstTurnFailureHealthReachesRunAndMemberStatusDot() throws {
        func code(_ text: String) -> String {
            text.components(separatedBy: "\n").map {
                String($0.components(separatedBy: "//")[0])
            }.joined(separator: "\n")
        }
        let runner = code(try Self.projectText(of: "Sources/Mac/Services/CrewSessionRunner.swift"))
        let observerStart = try XCTUnwrap(runner.range(of: "private func observeBackendHealth()"))
        XCTAssertTrue(runner[..<observerStart.lowerBound].contains("observeBackendHealth()"))
        let observerEnd = try XCTUnwrap(runner.range(
            of: "private func observeLaunchParameterProblems()", range: observerStart.upperBound..<runner.endIndex))
        let observer = String(runner[observerStart.upperBound..<observerEnd.lowerBound])
        XCTAssertTrue(observer.contains("for await h in self.backend.healthPublisher.values"))
        XCTAssertTrue(observer.contains("self.health = h"), "Backend error must reach the member's run")
        let view = code(try Self.projectText(of: "Sources/Mac/Views/CrewSessionWindowView.swift"))
        let dotStart = try XCTUnwrap(view.range(of: "@ViewBuilder private var statusDot:"))
        let dot = view[dotStart.upperBound...]
        XCTAssertTrue(dot.contains("CrewSessionStateDerivation.state("))
        XCTAssertTrue(dot.contains("health: run.health"))
        XCTAssertTrue(dot.contains("SessionStatusDotDerivation.dot(state: state)"))
    }

    // MARK: - 源码扫描

    /// 按文件名取源码原文（找不到 → 失败，不静默放过）。
    /// **汇报线上那条派生规则，有没有真的被接到投递路上**（人类 Todo #141 / #137）。
    ///
    /// `ReportingParentTests` 测的是规则本身对不对（`reportingParentIds` 在什么时候
    /// 派生出总机组）。但**那组测试证明不了规则被用上了** —— 实测过：把下面这两处
    /// 各自改回 `parentIds`，全量 2553 条**一条都不红**。`CrewStore` 不在 test
    /// target 里，`LocalSessionLaunch` 在、却没有测试走到那条路。
    ///
    /// 所以这里用的是本文件既有那套办法（扫源码文本），理由跟本文件开头那段一样：
    /// 「零件造好了没装到车上」测不出来。两处都**正反各断一次** ——
    /// 只断「有 reportingParentIds」的话，有人把那一行改回 `parentIds` 又在别处留下
    /// 一个 `reportingParentIds` 的提及，这把尺子照样绿。
    func testTheReportingParentRuleIsWiredIntoDelivery() throws {
        let store = try Self.text(of: "CrewStore.swift")
        XCTAssertTrue(
            store.contains("targets = store.reportingParentIds(of: cmd.crewId)"),
            """
            to_parent 投递没走派生的父：顶层机组往上汇报会回「你已是根」，            总机组永远收不到任何汇报（Todo #141 白做）。
            """)
        XCTAssertFalse(
            store.contains("targets = store.parentIds(of: cmd.crewId)"),
            "to_parent 投递被改回了「存下来的边」—— 那是上一版的行为")

        let launch = try Self.text(of: "LocalSessionLaunch.swift")
        XCTAssertTrue(
            launch.contains("LocalCrewStore.shared.reportingParentIds(of: detail.crew.id)"),
            """
            注入给 agent 的「上级是谁」没走派生的父：提示词会说「你是根、没有上级」，            而它 report_to_parent 照样送到总机组 —— 提示词对 agent 撒谎。
            """)
        XCTAssertFalse(
            launch.contains("LocalCrewStore.shared.parentIds(of: detail.crew.id)"),
            "注入那处被改回了「存下来的边」—— 提示词会跟投递说两样话")
    }

    /// 反过来钉住**不该被派生污染的那一侧**：机长交接的授权判定必须走
    /// `parentIds`（存下来的边）。
    ///
    /// 这条比上面那条更要紧。`resolveTargetCrewId` 判的是「目标是不是我的直系子」——
    /// 换成派生的父之后，**总机组的机长凭空成为所有顶层机组的父**，
    /// 等于把机长交接权放开到全机。它不会报错，只会悄悄多给权限。
    func testCaptainHandoffAuthorizationStillUsesStoredEdges() throws {
        let store = try Self.text(of: "CrewStore.swift")
        XCTAssertTrue(
            store.contains("LocalCrewStore.shared.parentIds(of: $0) } ?? []"),
            "机长交接授权那处不再读「存下来的边」了 —— 越权风险，见本测试注释")
        let runner = try Self.text(of: "CrewSessionRunner.swift")
        XCTAssertTrue(
            runner.contains("targetParentIds: LocalCrewStore.shared.parentIds(of: request.targetCrewId)"),
            "daemon 侧机长交接授权那处不再读「存下来的边」了 —— 越权风险")
        XCTAssertFalse(
            runner.contains("reportingParentIds"),
            """
            CrewSessionRunner 里出现了 reportingParentIds —— 这个文件里唯一用到父边的            地方是交接授权，它必须走存下来的边。
            """)
    }

    /// **总机组那一行跟普通机组用同一个行视图**（人类 2026-09-12：「总机组和普通机组
    /// 的样式要一样 包括sidebar里的 颜色条可以不使用」）。
    ///
    /// 这条挡的是一种特定的退化：有人为了给总机组加点什么，又写回一个自定义行。
    /// 那正是这一单之前的状态 —— 上一版刻意做成不一样，人类当面推翻了。
    /// 共用行视图是本仓既有的做法（见 `CrewSidebarCrewRow` 开头那段「为什么必须共用」），
    /// 自定义行会让两者再次漂开，而且只有目视才看得出来。
    func testTheChiefLayerRowUsesTheSharedCrewRow() throws {
        let chiefView = try Self.text(of: "CrewChiefListView.swift")
        XCTAssertTrue(chiefView.contains("CrewSidebarCrewRow("),
                      "总机组那一行没用共用行视图 —— 它会跟普通行漂开")
        XCTAssertTrue(chiefView.contains("showsColorBar: false"),
                      "总机组那一行还画着谱系色条 —— 人类点名说可以不要，而它没有父边，那道条对它没有含义")
        XCTAssertTrue(chiefView.contains("showsContextMenu: false"),
                      """
                      总机组那一行挂着右键菜单 —— 那两项对它都会坏：建子会被 refuseBuiltin                       抛；「藏起来」藏完进不了「已隐藏的群」那份列表（它喂的是不含 builtin                       的 crewStore.crews），没有第二个入口取回来。
                      """)

        // 那个自定义行必须真的没了 —— 只断「用了共用行」的话，两个行视图并存、
        // 而视图里实际画的是旧那个，这把尺子照样绿。
        let sources = try Self.sourceFiles()
        XCTAssertGreaterThan(sources.count, 50, "源码扫描没扫到东西，测试本身失效了")
        let strays = sources.filter { url, text in
            url.lastPathComponent != "CrewChiefListView.swift"   // 那里只有一句注释提到它
                && text.contains("CrewChiefLayerEntryRow")
        }
        XCTAssertTrue(strays.isEmpty,
                      "那个自成一格的自定义行又回来了：\(strays.map(\.0.lastPathComponent))")
    }

    /// 反向：**普通行的色条和右键菜单不许被顺手关掉。** 上面那条只说总机组要关，
    /// 这条守住「别顺手改别的行的样式」那条纪律 —— 两个开关的默认值必须仍是 true。
    func testOrdinaryRowsKeepTheirColorBarAndMenu() throws {
        let row = try Self.text(of: "CrewSidebarCrewRow.swift")
        XCTAssertTrue(row.contains("var showsColorBar: Bool = true"),
                      "色条开关的默认值不是 true —— 所有普通行的色条被顺手关掉了")
        XCTAssertTrue(row.contains("var showsContextMenu: Bool = true"),
                      "右键菜单开关的默认值不是 true —— 所有普通行的右键菜单被顺手关掉了")
    }

    /// **人类 Todo #145（2026-09-13）：刷新按钮那条路写不进去必须说实话，事后查得到。**
    ///
    /// 那天人类按了、总机组群聊里一条都没有，回执说「已请总机长重排」，而事后分不清
    /// 是没点着还是写盘失败被吞了。这里钉三个接头 —— 判定和回执文案那一半在
    /// `ChiefResortRequestTests`，但这三处都长在不进 test bundle 的文件里，只能读源码：
    /// ① `LocalBackend.postCrewMessage` 走会抛的写入口（原来那个 `try?` 就是吞错点）；
    /// ② 按钮回调**第一行**记「按下」—— 早于任何判定和写盘，它在不在是分辨「没点着」的唯一凭据；
    /// ③ `CrewStore` 那一步把拒绝 / 写进去 / 没写进去各记一行，回执走纯函数。
    func testChiefResortPathReportsWriteFailuresAndLogsEveryStep() throws {
        // ①
        let backend = Self.codeOnly(try Self.text(of: "PendingCrewBackend.swift"))
        guard let local = backend.range(of: "final class LocalBackend"),
              let post = backend.range(of: "func postCrewMessage(",
                                       range: local.upperBound..<backend.endIndex),
              let next = backend.range(of: "func whiteboardChanges(",
                                       range: post.upperBound..<backend.endIndex) else {
            return XCTFail("找不到 LocalBackend.postCrewMessage")
        }
        let postBody = String(backend[post.lowerBound..<next.lowerBound])
        XCTAssertTrue(postBody.contains("return try whiteboard.appendUserMessageReportingFailure("),
                      "LocalBackend 发人类消息没走会抛的写入口 —— 写不进去，回执照样说已发")
        XCTAssertFalse(postBody.contains("try?"),
                       "LocalBackend 发人类消息又把写盘错误 try? 掉了")
        XCTAssertFalse(postBody.contains("whiteboard.appendUserMessage("),
                       "LocalBackend 发人类消息又调回了不抛的 appendUserMessage")

        // ②
        let list = Self.codeOnly(try Self.text(of: "CrewChiefListView.swift"))
        guard let button = list.range(of: "private var resortButton"),
              let open = list.range(of: "Button {", range: button.upperBound..<list.endIndex),
              let task = list.range(of: "Task {", range: open.upperBound..<list.endIndex) else {
            return XCTFail("找不到刷新按钮的回调")
        }
        XCTAssertEqual(
            list[open.upperBound..<task.lowerBound].trimmingCharacters(in: .whitespacesAndNewlines),
            "ChiefResortRequest.logPressed()",
            "刷新按钮回调的第一行不是「按下」日志 —— 事后分不清没点着和没写进去")

        // ③
        let store = Self.codeOnly(try Self.text(of: "CrewStore.swift"))
        guard let start = store.range(of: "func requestChiefResort("),
              let end = store.range(of: "func postSystemNotice(",
                                    range: start.upperBound..<store.endIndex) else {
            return XCTFail("找不到 CrewStore.requestChiefResort")
        }
        let resort = String(store[start.lowerBound..<end.lowerBound])
        for needle in ["ChiefResortRequest.logRefused(", "ChiefResortRequest.logWritten(",
                       "ChiefResortRequest.logWriteFailed(", "ChiefResortRequest.receipt(for:"] {
            XCTAssertTrue(resort.contains(needle), "requestChiefResort 里少了 \(needle)")
        }
        XCTAssertFalse(resort.contains("\"已请总机长"),
                       "回执又在 CrewStore 里就地写死了「已请」—— 不经过纯函数，失败也会这么说")

        // 四行日志同一个 logger（同 subsystem + category，一条 log show 查得全）。
        let request = Self.codeOnly(try Self.text(of: "ChiefResortRequest.swift"))
        XCTAssertEqual(request.components(separatedBy: "Logger(").count - 1, 1,
                       "刷新按钮那条路的日志不止一个 logger，一条查询查不全")
    }

    /// #179：总机组视图与其它两种视图一样，行内必须展示真实末条消息。
    /// 摘要仍可存取，但不能再盖掉这行一手消息。
    func testChiefRowsShowActualLatestMessageInsteadOfSummary() throws {
        let list = Self.codeOnly(try Self.text(of: "CrewChiefListView.swift"))
        XCTAssertEqual(list.components(separatedBy: "statusLine: nil").count - 1, 2,
                       "总机组入口和普通机组行都应走共用行的最新消息预览")
        XCTAssertFalse(list.contains("CrewStatusLine.make("),
                       "总机长摘要或机长自报仍会遮住真实最新消息")
        let row = Self.codeOnly(try Self.text(of: "CrewSidebarCrewRow.swift"))
        XCTAssertTrue(row.contains("CrewSidebarCrewRow.preview(of: last)"),
                      "共用行没有接上末条消息预览")
    }

    private static func text(of fileName: String) throws -> String {
        guard let hit = try sourceFiles().first(where: { $0.0.lastPathComponent == fileName })
        else { throw XCTSkip("找不到源码文件 \(fileName)") }
        return hit.1
    }

    func testRetiredManualApprovalDoesNotReadLegacyLedgerOrShowCards() throws {
        let noLegacyApproval: [(String, [String])] = [
            ("CrewSessionWindowView.swift", ["SessionApprovalModeControl(", "SessionApprovalCardsView("]),
            ("MenuBarAttentionModel.swift", ["LocalApprovalStore.shared.pending"]),
            ("CrewSessionRunner.swift", ["CodexManualApprovalBridge.provider", "CodexApprovalModeStore.shared"]),
            ("SessionAwaitingReplyInputsCache.swift", ["LocalApprovalStore.shared"]),
            ("SessionUnreadStore.swift", ["approvals.pending("]),
            ("IOSRemoteSessionView.swift", ["approvalCard("]),
            ("SessionDaemonMain.swift", ["server.approvalStore ="]),
            ("SessionDaemonHost.swift", ["ApprovalRPC.capability"]),
            ("RemotePendingCrewBackend.swift", ["refreshApprovals(", "subscribeApprovals("]),
        ]
        for (file, forbidden) in noLegacyApproval {
            let source = try Self.codeOnly(Self.text(of: file))
            for symbol in forbidden {
                XCTAssertFalse(source.contains(symbol), "\(file) still uses retired manual approval: \(symbol)")
            }
        }
        XCTAssertFalse(try Self.codeOnly(Self.text(of: "CodexProtocol.swift"))
            .contains("case user"), "Codex must only expose its native auto reviewer")
        let sourceNames = Set(try Self.sourceFiles().map { $0.0.lastPathComponent })
        XCTAssertFalse(sourceNames.contains("LocalApprovalStore.swift"))
        XCTAssertFalse(sourceNames.contains("SessionApprovalCardsView.swift"))
        XCTAssertFalse(try Self.codeOnly(Self.text(of: "CrewRPC.swift"))
            .contains("enum ApprovalRPC"))
    }

    func testBackendConnectionStatusIsInSidebarAndProblemBubblesHaveRedWash() throws {
        let sidebar = try Self.codeOnly(Self.text(of: "CrewSidebarView.swift"))
        let notice = try Self.codeOnly(Self.text(of: "OrchestrationNoticeBar.swift"))
        let bubble = try Self.codeOnly(Self.text(of: "BubbleView.swift"))
        XCTAssertTrue(sidebar.contains("BackendSidebarConnectionStatus("))
        XCTAssertFalse(notice.contains("case let .connecting(detail):"))
        XCTAssertFalse(notice.contains("case let .refused(detail):"))
        XCTAssertTrue(bubble.contains("message.isProblem ? Theme.Palette.dangerBg"))
    }

    private static func projectText(of relativePath: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return try String(
            contentsOf: root.appendingPathComponent(relativePath), encoding: .utf8)
    }

    private static func repositoryPathExistsWithExactCase(
        _ relativePath: String,
        under root: URL
    ) -> Bool {
        var directory = root
        for component in relativePath.split(separator: "/").map(String.init) {
            guard let entries = try? FileManager.default.contentsOfDirectory(atPath: directory.path),
                  entries.contains(component)
            else { return false }
            directory.appendPathComponent(component)
        }
        return true
    }

    /// 仓库里 `apps/pendingcrew/Sources` 下的全部 .swift（路径由本文件位置推出）。
    /// 去掉行注释再扫。照抄本仓已有的同名助手，不发明第二种。
    private static func codeOnly(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    private static func requiredSection(_ source: String, _ start: String, _ end: String) throws -> String {
        let a = try XCTUnwrap(source.range(of: start), "缺少 \(start)")
        let b = try XCTUnwrap(source.range(of: end, range: a.upperBound..<source.endIndex), "缺少 \(end)")
        return String(source[a.upperBound..<b.lowerBound])
    }

    private static func sourceFiles() throws -> [(URL, String)] {
        let root = URL(fileURLWithPath: #filePath)      // .../Tests/PendingCrewTests/ViewWiringTests.swift
            .deletingLastPathComponent()                 // .../Tests/PendingCrewTests
            .deletingLastPathComponent()                 // .../Tests
            .deletingLastPathComponent()                 // .../apps/pendingcrew
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else {
            throw XCTSkip("读不到源码目录 \(root.path)（不在开发机上跑）")
        }
        return walker.compactMap { any in
            guard let url = any as? URL, url.pathExtension == "swift",
                  let text = try? String(contentsOf: url, encoding: .utf8)
            else { return nil }
            return (url, text)
        }
    }
}
