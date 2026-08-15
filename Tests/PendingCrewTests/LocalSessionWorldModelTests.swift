import XCTest
// LocalSessionWorldModel + LocalPromptLoader + PromptTemplate 都编进 test bundle；
// prompt .md 作为 test bundle 资源（见 project.yml）。

final class LocalSessionWorldModelTests: XCTestCase {
    private var renderer: LocalSessionWorldModel {
        LocalSessionWorldModel(loader: LocalPromptLoader(bundle: Bundle(for: Self.self)))
    }

    private func sampleContext() -> LocalSessionWorldModel.Context {
        LocalSessionWorldModel.Context(
            sessionTaskBrief: "修登录崩溃",
            runnerKind: "claude_code",
            crewId: "local-abc",
            crewTitle: "登录修复组",
            workingDirectory: "/tmp/proj",
            humans: [.init(displayName: "阿德", role: "owner", userId: "u-1")],
            captainName: "机长甲",
            captainBotId: "local-bot-9",
            claudeSubscriptionPlan: "Max 5x（自动）",
            codexSubscriptionPlan: "Plus（自动）"
        )
    }

    func testRendersCoreFacts() throws {
        let out = try renderer.render(sampleContext())
        XCTAssertTrue(out.contains("修登录崩溃"), "task brief")
        XCTAssertTrue(out.contains("登录修复组"), "crew title")
        XCTAssertTrue(out.contains("claude_code"), "runner kind")
        XCTAssertTrue(out.contains("阿德"), "human roster")
        XCTAssertTrue(out.contains("机长甲"), "captain name")
        XCTAssertTrue(out.contains("local_host"), "local runtime")
        XCTAssertTrue(out.contains("auto mode"), "fixed auto-mode wording present")
        XCTAssertTrue(out.contains("Max 5x"), "Claude subscription tier reaches world model")
        XCTAssertTrue(out.contains("Plus"), "Codex subscription tier reaches world model")
        XCTAssertTrue(out.contains("禁止据此编造绝对额度"), "absolute quota must stay unknown")
    }

    func testGUIAutomationBanReachesRenderedPrompt() throws {
        // 界面自动化会以 PendingCrew 的名义弹系统权限框吓到人类 —— 这条禁令必须真的
        // 进到 agent 看到的渲染结果里，不能只躺在模板某个未渲染的角落。
        let out = try renderer.render(sampleContext())
        XCTAssertTrue(out.contains("AXUIElement"), "GUI automation ban must list the offending APIs")
        XCTAssertTrue(out.contains("screencapture"), "screencapture must be named")
        XCTAssertTrue(out.contains("等人点头再动手"), "must require human sign-off before running it")
    }

    func testNoTemplateSyntaxLeaks() throws {
        let out = try renderer.render(sampleContext())
        // 未提供的 lineage/shares/tiebreaker 槽必须被 strip，不泄漏给 agent。
        XCTAssertFalse(out.contains("{{"), "no {{ }} may leak")
    }

    func testEmptyRosterPlaceholder() {
        var ctx = sampleContext()
        ctx.humans = []
        XCTAssertEqual(renderer.buildVars(ctx)["humanRoster"], "(暂无)")
    }

    func testNoCaptainPlaceholder() {
        var ctx = sampleContext()
        ctx.captainName = nil
        ctx.captainBotId = nil
        XCTAssertEqual(renderer.buildVars(ctx)["captainBlock"], "本 crew 暂无指定 captain。")
    }

    func testBuildVarsFillsLineageOmitsShares() {
        let vars = renderer.buildVars(sampleContext())
        // #463 起 lineageBlock 本地真实填充（默认根 crew 表述）；
        // shares/tiebreaker 仍不提供（无责任分账）→ 交给 PromptTemplate strip。
        XCTAssertTrue((vars["lineageBlock"] ?? "").contains("根 crew"))
        XCTAssertNil(vars["sharesBlock"])
        XCTAssertNil(vars["tiebreakerBlock"])
        XCTAssertEqual(vars["runtimeLocation"], "local_host")
    }

    func testEmptyBriefPlaceholder() {
        var ctx = sampleContext()
        ctx.sessionTaskBrief = ""
        XCTAssertEqual(renderer.buildVars(ctx)["sessionTaskBrief"], "(无任务描述)")
    }
}
