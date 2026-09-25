import XCTest
// LocalPromptLoader + PromptTemplate 直接编进 test bundle；prompt .md 作为 test
// bundle 资源（见 project.yml），从 test bundle 加载验证 bundling + lookup 端到端。

final class LocalPromptLoaderTests: XCTestCase {
    private var loader: LocalPromptLoader {
        LocalPromptLoader(bundle: Bundle(for: Self.self))
    }

    func testLoadsWorldModelZhRaw() throws {
        let raw = try loader.rawTemplate(name: "session-world-model", locale: "zh")
        // bypass→auto mode 修复落地：含 "auto mode"，不含 "auto / bypass"。
        XCTAssertTrue(raw.contains("auto mode"), "should carry the fixed auto-mode wording")
        XCTAssertFalse(raw.contains("auto / bypass"), "old bypass wording must be gone")
        // 原始模板未填槽仍在。
        XCTAssertTrue(raw.contains("{{sessionTaskBrief}}"))
    }

    func testRenderFillsVars() throws {
        let out = try loader.render(
            name: "session-world-model", locale: "zh",
            vars: ["sessionTaskBrief": "修登录 bug", "crewTitle": "Alpha", "crewId": "local-1"])
        XCTAssertTrue(out.contains("修登录 bug"))
        XCTAssertTrue(out.contains("Alpha"))
        XCTAssertFalse(out.contains("{{sessionTaskBrief}}"), "slots must be filled")
        // 未提供的槽（如 humanRoster）被清空，不泄漏模板语法。
        XCTAssertFalse(out.contains("{{"), "no template syntax may leak to the agent")
    }

    func testGUIAutomationBanPresentInEnglishAndCaptain() throws {
        // 同一条禁令必须三份 prompt 都有：zh 世界观（见 LocalSessionWorldModelTests
        // 的渲染断言）、en 世界观、机长。
        let en = try loader.rawTemplate(name: "session-world-model", locale: "en")
        XCTAssertTrue(en.contains("AXUIElement"), "en world model must carry the GUI automation ban")
        XCTAssertTrue(en.contains("wait for a human to agree"), "en must require human sign-off")

        let captain = try loader.rawTemplate(name: "crew-captain", locale: "zh")
        XCTAssertTrue(captain.contains("AXUIElement"), "captain must carry the GUI automation ban too")
        XCTAssertTrue(captain.contains("你先拦"), "captain must gate worker requests for GUI automation")
    }

    func testLocaleFallbackToZh() throws {
        // crew-captain 只有 zh；请求 en 应回退到 zh（非抛错）。
        let out = try loader.rawTemplate(name: "crew-captain", locale: "en")
        XCTAssertTrue(out.contains("机长"), "should fall back to bundled zh crew-captain")
    }

    /// 总机组不是一个亲自干活的项目组。普通机长规则会教 agent 在明确任务时
    /// `start_session`，所以必须有一段更高优先级的总机组特例把这条翻掉；否则
    /// 总机长拿到 VPS 迁移这类任务时会顺着通用自治规则直接跑 SSH / 改线上。
    func testChiefCaptainIsCoordinationOnly() throws {
        let captain = try loader.rawTemplate(name: "crew-captain", locale: "zh")
        XCTAssertTrue(captain.contains("pendingcrew-chief"), "总机组边界必须按稳定 id 判，不靠可改标题")
        XCTAssertTrue(captain.contains("只做任务识别"), "总机长职责应明确收窄到协调")
        XCTAssertTrue(captain.contains("不得亲自运行 SSH"), "必须点名拦住本次真实越界")
        XCTAssertTrue(captain.contains("不得调用 `start_session`"), "不能在总机组里偷起执行 worker")
        XCTAssertTrue(captain.contains("`create_child_crew`"), "缺执行组时必须给出可行的新建路径")
    }

    func testMissingThrows() {
        XCTAssertThrowsError(try loader.rawTemplate(name: "does-not-exist", locale: "zh")) { err in
            XCTAssertEqual(err as? LocalPromptLoader.LoaderError,
                           .notFound(name: "does-not-exist", locale: "zh"))
        }
    }
}

/// 行为守则里「`ask` vs 人类 Todo」的分工（Todo #62 ⑤）。
///
/// 不做这条等于没做功能：所有 agent（含机长）今天被教的是「要人拍板就
/// `post_to_crew` 喊一嗓子」，建了这本账之后默认路径必须改。这一族钉住三份 prompt
/// 都真的改了，且**边界写明白了** —— 不写明白 agent 会拿人类 Todo 当 ask 用。
final class HumanTodoBehaviourRulesTests: XCTestCase {
    private var loader: LocalPromptLoader {
        LocalPromptLoader(bundle: Bundle(for: Self.self))
    }

    func testWorldModelZhTeachesTheNonBlockingPath() throws {
        let zh = try loader.rawTemplate(name: "session-world-model", locale: "zh")
        XCTAssertTrue(zh.contains("add_human_todo"), "zh 世界观得教这个工具")
        XCTAssertTrue(zh.contains("非阻塞"), "得说清人类 Todo 是非阻塞的")
        XCTAssertTrue(zh.contains("**阻塞**"), "得说清 ask 是阻塞的")
        // 分界必须是「你等不等」，不是别的模糊标准。
        XCTAssertTrue(zh.contains("你等不等"), "边界得写死在「等不等」上")
    }

    func testWorldModelEnCarriesTheSameBoundary() throws {
        let en = try loader.rawTemplate(name: "session-world-model", locale: "en")
        XCTAssertTrue(en.contains("add_human_todo"))
        XCTAssertTrue(en.contains("non-blocking"))
        XCTAssertTrue(en.contains("blocking"))
    }

    /// 机长自己也得改 —— 他是最常「要人拍板」的那个。
    func testCaptainPromptRoutesDecisionsIntoTheHumanLedger() throws {
        let captain = try loader.rawTemplate(name: "crew-captain", locale: "zh")
        XCTAssertTrue(captain.contains("add_human_todo"), "机长守则得教这个工具")
        XCTAssertTrue(captain.contains("别只在群里喊一嗓子"), "默认路径得从「群里喊」改过来")
        // 回落转达那条得写进守则 —— 机长会收到别人提的条目的答复。
        XCTAssertTrue(captain.contains("转达"), "机长得知道回落到他手上的那条要转达")
    }

    /// 两本账在守则里必须能一眼分清 —— 不分清机长会拿 respond_todo 回错账。
    func testCaptainPromptDistinguishesTheTwoLedgers() throws {
        let captain = try loader.rawTemplate(name: "crew-captain", locale: "zh")
        XCTAssertTrue(captain.contains("「To do +1: #N」是**人类派给你们**的"))
        XCTAssertTrue(captain.contains("「人类 To do +1: #N」是**你们请人类拍板**的"))
    }

    /// `ask` 本身一个字没动 —— 它还是那条阻塞通道。
    func testAskItselfIsUnchanged() throws {
        let zh = try loader.rawTemplate(name: "session-world-model", locale: "zh")
        XCTAssertTrue(zh.contains("**`ask(question)`**"))
        XCTAssertTrue(zh.contains("**先到本 crew 的 captain**"))
    }
}

final class CockpitPlanBehaviourRulesTests: XCTestCase {
    private var loader: LocalPromptLoader {
        LocalPromptLoader(bundle: Bundle(for: Self.self))
    }

    func testCaptainPromptMakesAgentPlanMaintenancePartOfTheWorkflow() throws {
        let captain = try loader.rawTemplate(name: "crew-captain", locale: "zh")
        XCTAssertTrue(captain.contains("驾驶舱只写 Agent 当前的计划和想法"))
        XCTAssertTrue(captain.contains("`plan_add`"))
        XCTAssertTrue(captain.contains("`plan_update`"))
        XCTAssertTrue(captain.contains("派活、收活、给 Todo 翻牌时都顺手更新"))
        XCTAssertTrue(captain.contains("宣布完成前必须再读一眼"))
        XCTAssertTrue(captain.contains("不要让驾驶舱靠不存在的 `docs/roadmap.md` 才有内容"))
    }
}
