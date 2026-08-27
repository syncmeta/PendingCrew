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
        ("UpdateSettingsSection(", "UpdateSettingsSection.swift",
         "设置里没有「更新」区，检查更新点不到（Sparkle 接入失效）"),
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
        XCTAssertFalse(panel.contains(".strikethrough("),
                       "已完成的 Todo 不该划删除线（人类明确要求，只变灰）")

        let cockpit = try Self.text(of: "CockpitTasksView.swift")
        XCTAssertTrue(cockpit.contains("TodoListPresentation.newestFirst"),
                      "驾驶舱任务段的 Todo 没走同一套排序")
        XCTAssertTrue(cockpit.contains("CrewTodoStatusCircle("),
                      "驾驶舱任务段的 Todo 没用同一套状态圆圈")
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
        // 药丸要贴着群名（人类：「群聊页面的右上方（群名的右侧）」）。群名走
        // navigationTitle、在标题栏前端，所以这一组里越靠前越贴着它 —— 钉死它是第一个。
        if let pill = center.range(of: "Toggle(isOn: $onlyMentions)"),
           let firstOther = center.range(of: "showingDetail = true") {
            XCTAssertTrue(pill.lowerBound < firstOther.lowerBound,
                          "「仅@你」药丸不是这组 toolbar 的第一个，没贴着群名（Todo #69）")
        } else {
            XCTFail("找不到药丸或第一个图标钮，测试本身失效了")
        }

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

    // MARK: - 源码扫描

    /// 按文件名取源码原文（找不到 → 失败，不静默放过）。
    private static func text(of fileName: String) throws -> String {
        guard let hit = try sourceFiles().first(where: { $0.0.lastPathComponent == fileName })
        else { throw XCTSkip("找不到源码文件 \(fileName)") }
        return hit.1
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
