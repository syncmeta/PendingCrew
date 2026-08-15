import XCTest
// 不 @testable import —— PromptTemplate.swift 直接编进 test bundle（见 project.yml），
// 纯 Foundation，同 LocalWhiteboardStore 模式。

final class PromptTemplateTests: XCTestCase {
    func testReplacesProvidedKeys() {
        let out = PromptTemplate.render("hi {{name}}, in {{crew}}",
                                        vars: ["name": "Ada", "crew": "Alpha"])
        XCTAssertEqual(out, "hi Ada, in Alpha")
    }

    func testReplacesAllOccurrences() {
        let out = PromptTemplate.render("{{x}}-{{x}}-{{x}}", vars: ["x": "z"])
        XCTAssertEqual(out, "z-z-z")
    }

    func testStripsUnfilledPlaceholders() {
        // 未提供的槽清成空串，不泄漏 {{...}} 给 agent。
        let out = PromptTemplate.render("a {{filled}} b {{missing}} c", vars: ["filled": "X"])
        XCTAssertEqual(out, "a X b  c")
    }

    func testNoVarsStripsAllPlaceholders() {
        let out = PromptTemplate.render("keep {{gone}} text", vars: [:])
        XCTAssertEqual(out, "keep  text")
    }

    func testPlainTextUnchanged() {
        XCTAssertEqual(PromptTemplate.render("no slots here", vars: ["a": "b"]), "no slots here")
    }

    func testEmptyTemplate() {
        XCTAssertEqual(PromptTemplate.render("", vars: ["a": "b"]), "")
    }
}
