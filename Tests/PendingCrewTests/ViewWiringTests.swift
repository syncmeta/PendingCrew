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
    ]

    func testEveryUserFacingPieceIsActuallyWiredUp() throws {
        let sources = try Self.sourceFiles()
        XCTAssertGreaterThan(sources.count, 50, "源码扫描没扫到东西，测试本身失效了")

        for wiring in Self.wirings {
            let callSites = sources.filter { url, text in
                url.lastPathComponent != wiring.definedIn && text.contains(wiring.symbol)
            }
            XCTAssertFalse(
                callSites.isEmpty,
                """
                「\(wiring.symbol)」在 \(wiring.definedIn) 之外没有任何调用点 —— \
                零件造好了没装到车上：\(wiring.impact)。
                """)
        }
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
        // Todo #79 覆盖旧位置：现在明确固定在群聊栏最右上角，并与发送键同色。
        XCTAssertTrue(center.contains("ToolbarItem(placement: .primaryAction)"),
                      "「仅@你」没有固定到群聊栏最右上角（Todo #79）")
        XCTAssertTrue(center.contains(".tint(Theme.Palette.accent)"),
                      "「仅@你」点亮态没有复用发送键的主题绿色（Todo #79）")

        let sidebar = try Self.text(of: "CrewSidebarView.swift")
        XCTAssertTrue(sidebar.contains(".pickerStyle(.segmented)"))
        XCTAssertTrue(sidebar.contains(".tint(Theme.Palette.accent)"),
                      "层级/时间流仍继承系统蓝色，没有改成主题绿色（Todo #79）")

        let chat = try Self.text(of: "CrewChatView.swift")
        XCTAssertTrue(chat.contains("CrewMentionFilter.onlyHumanMentions"),
                      "群聊时间线没走筛选判定")
        // 必须筛在 timelineEntries 这个源头 —— 渲染窗口那一整套（renderLimit /
        // hasMore /「上面还有 N 条」/ anchorOnExpand）全读它，筛在下游会出现
        // 「显示还有 300 条、点开什么都没有」。
        let source = chat.range(of: "private var timelineEntries")
        let filtered = chat.range(of: "CrewMentionFilter.onlyHumanMentions")
        let windowed = chat.range(of: "private var windowedEntries")
        XCTAssertNotNil(source); XCTAssertNotNil(filtered); XCTAssertNotNil(windowed)
        if let source, let filtered, let windowed {
            XCTAssertTrue(source.lowerBound < filtered.lowerBound
                          && filtered.lowerBound < windowed.lowerBound,
                          "筛选没落在 timelineEntries 里 —— 渲染窗口会按未筛选的条数算")
        }
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
        XCTAssertTrue(runner.contains("await run.backend.submitWake"),
                      "runner 仍把无回执 send 当作 wake 已投递")
        XCTAssertFalse(runner.contains("run.send(ready.text)"),
                       "瞬时 idle 后仍直接 fire-and-forget，拒绝会被误消费")
        XCTAssertTrue(runner.contains("deferredWakes.resolve(delivery, as: result)"))
        XCTAssertTrue(runner.contains("scheduleDeferredWakeRetry(for: run)"),
                      "拒绝后仍要等第二条消息/新 idle 边沿，不能自行补投")
        let codex = try Self.text(of: "CodexAppServerBackend.swift")
        XCTAssertTrue(codex.contains("func submitWake(_ text: String) async"),
                      "Codex wake 没有以 turn/start RPC 受理为边界")
        XCTAssertTrue(codex.contains("wb?.commit()"),
                      "Codex 仍可能在 turn/start 受理前推进白板消费游标")
        let remote = try Self.text(of: "RemoteSessionBackend.swift")
        XCTAssertTrue(remote.contains("op: \"submitWake\""),
                      "协议传输层没有转发 wake 受理回执")
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
    /// session 切模式，且同一详情页必须挂着按 sessionId 过滤的可操作卡。
    func testCodexApprovalModeAndCardsAreWiredIntoSessionDetail() throws {
        let view = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertTrue(view.contains("SessionApprovalModeControl(run: run"),
                      "Codex session 详情没有审批模式切换入口")
        XCTAssertTrue(view.contains("sessionRunner.applyCodexApprovalMode("),
                      "模式控件没有接 thread/settings/update + 持久化编排")
        XCTAssertTrue(view.contains("SessionApprovalCardsView(crewId: run.crewId, sessionId: run.sessionId)"),
                      "手动模式请求即使入账也没有挂到当前 session 的可操作卡")

        let backend = try Self.text(of: "CodexAppServerBackend.swift")
        XCTAssertTrue(backend.contains("approvalRequestDisposition(reviewer: approvalsReviewer)"),
                      "auto_review 没走禁止建卡/通知的门禁")
    }

    /// Todo #82/#83/#90：窄右栏不能再把四种控件挤成一排；Codex 技术流折叠态
    /// 只说具体程序/档名，完整命令与路径放进可展开详情。
    func testSessionHeaderAndCodexActivityUseHumanFacingPresentation() throws {
        let view = try Self.text(of: "CrewSessionWindowView.swift")
        XCTAssertTrue(view.contains("// 第一排：名字"))
        XCTAssertTrue(view.contains("// 第二排：模型与 effort"))
        XCTAssertTrue(view.contains("// 第三排：Codex 原生审批模式"))
        XCTAssertTrue(view.contains("private var modelMenu"), "模型没有独立手动菜单")
        XCTAssertTrue(view.contains("private var effortMenu"), "effort 没有独立手动菜单")

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
        XCTAssertTrue(runner.contains("LocalCodingAgentKind(rawValue: $0.kind)"),
                      "恢复 runner 仍靠显示名猜；改过标题的 Codex session 会被拉错类型")
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
        XCTAssertTrue(runner.contains("resumePreviousConversation: false"),
                      "新建机长错误地续接了旧机长 conversation")
        XCTAssertTrue(runner.contains("resumePreviousConversation: Bool = true"),
                      "普通机长重启的历史续跑默认语义被破坏")
        XCTAssertTrue(runner.contains("if resumeCaptainId == nil && resumePreviousConversation"),
                      "新建机长没有真正绕开历史 conversation 查询")
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

    // MARK: - 源码扫描

    /// 按文件名取源码原文（找不到 → 失败，不静默放过）。
    private static func text(of fileName: String) throws -> String {
        guard let hit = try sourceFiles().first(where: { $0.0.lastPathComponent == fileName })
        else { throw XCTSkip("找不到源码文件 \(fileName)") }
        return hit.1
    }

    private static func projectText(of relativePath: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return try String(
            contentsOf: root.appendingPathComponent(relativePath), encoding: .utf8)
    }

    /// 仓库里 `apps/pendingcrew/Sources` 下的全部 .swift（路径由本文件位置推出）。
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
