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

    func testMissingThrows() {
        XCTAssertThrowsError(try loader.rawTemplate(name: "does-not-exist", locale: "zh")) { err in
            XCTAssertEqual(err as? LocalPromptLoader.LoaderError,
                           .notFound(name: "does-not-exist", locale: "zh"))
        }
    }
}
