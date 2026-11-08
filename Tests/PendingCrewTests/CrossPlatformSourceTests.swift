import XCTest

/// **跨平台文件不许用 macOS 独占 API** —— 用一条会红的尺子代替一条没人读的注释。
///
/// 病根（2026-09-07 第二次）：`Sources/` 整个目录被 **同一个 target** 同时编进
/// macOS 和 iOS（`project.yml` 里 `supportedDestinations: [iOS, macOS]`，
/// `sources: - path: Sources`）。所以一个**没有 `#if os(macOS)` 的文件**里出现
/// macOS 独占 API，iOS 端**必然**编不过 —— 不是「碰巧」，是构造上跑不掉。
///
/// 这次红的是 `SessionOutputEvidence.swift` 的 `homeDirectoryForCurrentUser`。
/// **而同一个坑仓库里已经踩过一次**，踩过的人还专门在 `CockpitTaskLedger.swift`
/// 留了注释说「它在 iOS 上不可用、整个 iOS target 都因此编不过」——
/// **注释就在那儿，没拦住第二次。**
///
/// 所以这里加的不是第二条注释、也不是一个「大家记得去用」的 shim（那要靠记性，
/// 而记性今天已经被证伪过一次）：本机 `homeDirectoryForCurrentUser` 有十来处用法，
/// **绝大多数都合法** —— 它们在 `#if os(macOS)` 后面。会出事的只有「跨平台文件」
/// 那一类。这条测试就只钉那一类，在本机跑 macOS 测试时就红，不用等 iOS 构建。
final class CrossPlatformSourceTests: XCTestCase {

    /// macOS 独占、在 iOS 上根本不存在的符号。只收**真的在这个仓库里炸过 iOS 构建**
    /// 的那些 —— 凭想象往里加只会让这把尺子噪音变大、然后被人忽略。
    private static let macOnlyAPIs = [
        "homeDirectoryForCurrentUser",
        // 2026-09-07：`TodoEvidence.swift` 用它跑 `git cat-file -e`，iOS 端
        // `cannot find 'Process' in scope`，**挡住了一次发版**。
        // 这条也是这把尺子自己的教训：**名单式的尺子，漏的永远是名单外的那个。**
        // 红样本取它真漏过的那次 —— 见 `testRulerCatchesTheProcessRegression`。
        "Process(",
    ]

    func testNoMacOnlyAPIInFilesThatAlsoCompileForIOS() throws {
        let sources = try Self.sourceFiles()
        XCTAssertGreaterThan(sources.count, 50, "源码扫描没扫到东西，测试本身失效了")

        var offenders: [String] = []
        for (url, text) in sources {
            // 只看**可能被编进 iOS 的那些行**：仅剔除条件能证明 iOS 不可达的块。
            // 不是「文件里出现过 #if 就整份放过」—— 那样一个文件只要在别处有个
            // 平台开关，它其余地方怎么乱用都不再被看住，尺子会在最需要它的那次变绿。
            let iosLines = Self.linesCompiledForIOS(text)
            for api in Self.macOnlyAPIs where iosLines.contains(api) {
                offenders.append("\(url.lastPathComponent) 用了 \(api)")
            }
        }

        XCTAssertEqual(
            offenders, [],
            """
            这些可能进入 iOS 的源码行用了 macOS 独占 API；请核对真实编译守卫：
            \(offenders.joined(separator: "\n"))
            仓库里已有现成写法（`CockpitTaskLedger.currentHome`）：macOS 用
            `FileManager.default.homeDirectoryForCurrentUser`，其余用
            `URL(fileURLWithPath: NSHomeDirectory())`。照抄，别发明第二种。
            """)
    }

    /// Conservative iOS reachability. A branch is dropped only when the
    /// condition and its ancestors prove it cannot compile for iOS. Unknown
    /// flags/canImport/compiler conditions keep every reachable alternative.
    private enum IOSCondition: Equatable {
        case yes, no, unknown
        var negated: IOSCondition {
            switch self { case .yes: return .no; case .no: return .yes; case .unknown: return .unknown }
        }
        func and(_ other: IOSCondition) -> IOSCondition {
            if self == .no || other == .no { return .no }
            if self == .yes && other == .yes { return .yes }
            return .unknown
        }
        func or(_ other: IOSCondition) -> IOSCondition {
            if self == .yes || other == .yes { return .yes }
            if self == .no && other == .no { return .no }
            return .unknown
        }
    }
    private struct IOSConditionParser {
        let tokens: [String]
        var position = 0
        var malformed = false
        init(_ source: String) {
            let chars = Array(source)
            var result: [String] = []
            var i = 0
            while i < chars.count {
                if chars[i].isWhitespace { i += 1; continue }
                if i + 1 < chars.count,
                   (chars[i] == "&" && chars[i + 1] == "&" || chars[i] == "|" && chars[i + 1] == "|") {
                    result.append(String(chars[i...(i + 1)])); i += 2
                } else if chars[i].isLetter || chars[i].isNumber || chars[i] == "_" {
                    let start = i
                    while i < chars.count && (chars[i].isLetter || chars[i].isNumber || chars[i] == "_") { i += 1 }
                    result.append(String(chars[start..<i]))
                } else { result.append(String(chars[i])); i += 1 }
            }
            tokens = result
        }
        // This ruler supports identifier tokens made of letters, digits and
        // underscores, with a nonnumeric first character. Unsupported lexical
        // syntax is malformed, not an unknown feature that Boolean rules may
        // simplify to a platform exemption.
        private static func isIdentifier(_ token: String) -> Bool {
            guard let first = token.first, first.isLetter || first == "_" else { return false }
            return token.allSatisfy { $0.isLetter || $0.isNumber || $0 == "_" }
        }
        mutating func eat(_ token: String) -> Bool {
            guard position < tokens.count, tokens[position] == token else { return false }
            position += 1
            return true
        }
        mutating func evaluate() -> IOSCondition {
            let value = disjunction()
            return malformed || position != tokens.count ? .unknown : value
        }
        mutating func disjunction() -> IOSCondition {
            var value = conjunction()
            while eat("||") { let rhs = conjunction(); value = value.or(rhs) }
            return value
        }
        mutating func conjunction() -> IOSCondition {
            var value = unary()
            while eat("&&") { let rhs = unary(); value = value.and(rhs) }
            return value
        }
        mutating func unary() -> IOSCondition {
            if eat("!") { return unary().negated }
            if eat("(") {
                let value = disjunction()
                if !eat(")") { malformed = true }
                return value
            }
            guard position < tokens.count else { malformed = true; return .unknown }
            let name = tokens[position]; position += 1
            guard Self.isIdentifier(name) else { malformed = true; return .unknown }
            guard eat("(") else {
                if name == "true" { return .yes }
                if name == "false" { return .no }
                return .unknown
            }
            let start = position
            var depth = 1
            while position < tokens.count {
                let token = tokens[position]
                if token == "(" { depth += 1 }
                if token == ")" {
                    depth -= 1
                    if depth == 0 {
                        let arguments = tokens[start..<position]
                        position += 1
                        if name == "os" {
                            // A known platform is exactly one identifier token.
                            // Joining split tokens would turn invalid mac OS into
                            // macOS and incorrectly exempt its body from scanning.
                            guard arguments.count == 1, let argument = arguments.first,
                                  Self.isIdentifier(argument) else {
                                malformed = true
                                return .unknown
                            }
                            if argument == "iOS" { return .yes }
                            if ["macOS", "tvOS", "watchOS", "visionOS", "Linux", "Windows", "Android", "FreeBSD", "OpenBSD"].contains(argument) { return .no }
                        }
                        return .unknown
                    }
                }
                position += 1
            }
            malformed = true
            return .unknown
        }
    }
    private struct IOSConditionalFrame {
        let parent: IOSCondition
        var matched: IOSCondition
    }
    static func linesCompiledForIOS(_ text: String) -> String {
        var frames: [IOSConditionalFrame] = []
        var active = IOSCondition.yes
        var kept: [String] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let code = strippingComment(line)
            let directive = code.trimmingCharacters(in: .whitespaces)
                .split(maxSplits: 1, whereSeparator: { $0.isWhitespace })
            let name = directive.first.map { String($0) } ?? ""
            let expression = directive.count == 2 ? String(directive[1]) : ""
            if name == "#if" {
                var parser = IOSConditionParser(expression)
                let condition = parser.evaluate()
                frames.append(.init(parent: active, matched: condition))
                active = active.and(condition)
                continue
            }
            if name == "#elseif" {
                guard var frame = frames.popLast() else { active = .unknown; continue }
                var parser = IOSConditionParser(expression)
                let condition = parser.evaluate()
                active = frame.parent.and(frame.matched.negated).and(condition)
                frame.matched = frame.matched.or(condition)
                frames.append(frame)
                continue
            }
            if name == "#else" {
                guard var frame = frames.popLast() else { active = .unknown; continue }
                active = frame.parent.and(frame.matched.negated)
                frame.matched = .yes
                frames.append(frame)
                continue
            }
            if name == "#endif" {
                active = frames.popLast()?.parent ?? .unknown
                continue
            }
            if active != .no { kept.append(code) }
        }
        return kept.joined(separator: "\n")
    }

    func testCompoundMacGuardAndNestedUnknownBranchesAreExcludedOnlyUnderMacParent() {
        let kept = Self.linesCompiledForIOS("""
        #if os(macOS) && PENDINGCREW_LOCAL_DEMO
        FileManager.default.homeDirectoryForCurrentUser
        #if UNKNOWN
        Process()
        #else
        Process()
        #endif
        #else
        portableIOSPath
        #endif
        outsideAllGuards
        """)
        XCTAssertFalse(kept.contains("homeDirectoryForCurrentUser"))
        XCTAssertFalse(kept.contains("Process("))
        XCTAssertTrue(kept.contains("portableIOSPath"))
        XCTAssertTrue(kept.contains("outsideAllGuards"))
    }

    func testUnknownFlagKeepsBothBranchesIncludingElse() {
        let kept = Self.linesCompiledForIOS("""
        #if UNKNOWN_FLAG
        let first = Process()
        #else
        let second = FileManager.default.homeDirectoryForCurrentUser
        #endif
        """)
        XCTAssertTrue(kept.contains("Process("))
        XCTAssertTrue(kept.contains("homeDirectoryForCurrentUser"))
    }

    func testUnknownElseifChainRetainsAllPossibleIOSAlternatives() {
        let kept = Self.linesCompiledForIOS("""
        #if FEATURE
        firstUnknown
        #elseif os(macOS)
        macOnly
        #elseif canImport(UnspecifiedModule)
        secondUnknown
        #else
        finalUnknown
        #endif
        """)
        XCTAssertTrue(kept.contains("firstUnknown"))
        XCTAssertFalse(kept.contains("macOnly"))
        XCTAssertTrue(kept.contains("secondUnknown"))
        XCTAssertTrue(kept.contains("finalUnknown"))
    }

    func testKnownIOSBranchCannotHideAnUnguardedMacAPIAndSkipsUnreachableElse() {
        let kept = Self.linesCompiledForIOS("""
        #if os(iOS)
        Process()
        #elseif UNKNOWN_FLAG
        unreachableUnknown
        #else
        unreachableElse
        #endif
        """)
        XCTAssertTrue(kept.contains("Process("))
        XCTAssertFalse(kept.contains("unreachableUnknown"))
        XCTAssertFalse(kept.contains("unreachableElse"))
    }

    func testOrUnknownCannotBeMistakenForAMacOnlyGuard() {
        for expression in ["os(macOS) || FEATURE", "!os(macOS) && FEATURE", "canImport(AppKit)", "swift(>=6.0)"] {
            let kept = Self.linesCompiledForIOS("#if \(expression)\nProcess()\n#else\notherBranch\n#endif")
            XCTAssertTrue(kept.contains("Process("), expression)
            XCTAssertTrue(kept.contains("otherBranch"), expression)
        }
    }

    func testBooleanPrecedenceAndParenthesesKeepOnlyProvablyReachableBranches() {
        let hidden = Self.linesCompiledForIOS("#if (os(macOS) || os(Linux)) && FLAG\nProcess()\n#else\nportable\n#endif")
        XCTAssertFalse(hidden.contains("Process("))
        XCTAssertTrue(hidden.contains("portable"))
        let visible = Self.linesCompiledForIOS("#if os(macOS) && FLAG || os(iOS)\nProcess()\n#else\nunreachable\n#endif")
        XCTAssertTrue(visible.contains("Process("))
        XCTAssertFalse(visible.contains("unreachable"))
    }

    func testCompoundElseIfWhitespaceAndTrailingCommentDoNotHidePortableCode() {
        let kept = Self.linesCompiledForIOS("""
        #if\tos(macOS) && FEATURE // legal Mac-only condition
        homeDirectoryForCurrentUser
        #elseif\t!os(macOS)
        Process()
        #endif
        """)
        XCTAssertFalse(kept.contains("homeDirectoryForCurrentUser"))
        XCTAssertTrue(kept.contains("Process("))
    }

    func testMalformedConditionIsUnknownRatherThanAnExemption() {
        for condition in ["os(macOS) &&", "(os(macOS)", "os(macOS) unsupported tokens"] {
            let kept = Self.linesCompiledForIOS("#if \(condition)\nProcess()\n#else\nhomeDirectoryForCurrentUser\n#endif")
            XCTAssertTrue(kept.contains("Process("), condition)
            XCTAssertTrue(kept.contains("homeDirectoryForCurrentUser"), condition)
        }
    }

    /// These expected outputs come from the Swift condition's platform meaning,
    /// independently of the parser/tokenizer's representation.
    func testKnownOSIdentifierHasExplicitReachabilityExpectations() {
        let cases: [(condition: String, expected: String)] = [
            ("os(macOS)", "otherPlatform"),
            ("os ( macOS )", "otherPlatform"),
            ("os(iOS)", "Process()"),
            ("os ( iOS )", "Process()"),
            ("os(tvOS)", "otherPlatform"),
            ("os(watchOS)", "otherPlatform"),
            ("os(visionOS)", "otherPlatform"),
            ("os(Linux)", "otherPlatform"),
            ("os(Windows)", "otherPlatform"),
            ("os(Android)", "otherPlatform"),
            ("os(FreeBSD)", "otherPlatform"),
            ("os(OpenBSD)", "otherPlatform"),
        ]
        for (condition, expected) in cases {
            let source = "#if \(condition)\nProcess()\n#else\notherPlatform\n#endif"
            XCTAssertEqual(Self.linesCompiledForIOS(source), expected, condition)
        }
    }

    func testSplitMalformedAndUnknownOSArgumentsCannotGrantAnExemption() {
        // Intentionally invalid Swift source is scanner input, not compiled
        // fixtures. Neither branch may disappear based on a repaired spelling.
        let conditions = [
            "os(mac OS)", "os(i OS)", "os(mac\tOS)", "os(ma cOS)",
            "os()", "os(macOS, iOS)", "os((macOS))", "os(\"macOS\")",
            "os(macOS.extra)", "os(macOS())", "os(UnknownOS)", "os(_macOS)", "os(123)",
        ]
        for condition in conditions {
            let source = "#if \(condition)\nProcess()\n#else\nhomeDirectoryForCurrentUser\n#endif"
            XCTAssertEqual(Self.linesCompiledForIOS(source), "Process()\nhomeDirectoryForCurrentUser", condition)
        }
    }

    func testMalformedOSArgumentsStayUnknownAcrossBooleanOperators() {
        for condition in ["os(mac OS) && false", "!os(i OS) || true",
                          "os(macOS) && os(mac OS)", "os(iOS) || os(i OS)",
                          "os(123) && false", "os(.) || true"] {
            let source = "#if \(condition)\nProcess()\n#else\nhomeDirectoryForCurrentUser\n#endif"
            XCTAssertEqual(Self.linesCompiledForIOS(source), "Process()\nhomeDirectoryForCurrentUser", condition)
        }
    }

    func testInvalidAtomAndCallNamesCannotBooleanSimplifyToAnExemption() {
        // External expected output: malformed lexical names retain both API
        // lines even beside a decisive Boolean, including on its right side.
        let conditions = [
            "@ && false", ". || true", "@(Feature) && false",
            "false && @", "true || .", "false && @(Feature)",
            "!@ && false", "!(.) || true", ".(Feature) || true",
            "1 && false", "123(Feature) || true", "4FEATURE && false",
            "false && 1", "true || 123(Feature)", "+ || true", "? && false",
            "false && (@ || false)", "true || (. && true)",
        ]
        for condition in conditions {
            let source = "#if \(condition)\nProcess()\n#else\nhomeDirectoryForCurrentUser\n#endif"
            XCTAssertEqual(Self.linesCompiledForIOS(source), "Process()\nhomeDirectoryForCurrentUser", condition)
        }
    }

    func testLegalUnknownConditionsRetainOrdinaryThreeValuedSimplification() {
        // A valid but unresolved feature/call/platform is different from bad
        // syntax: known false AND / true OR still prove their Boolean result.
        let cases: [(condition: String, expected: String)] = [
            ("FEATURE", "Process()\nhomeDirectoryForCurrentUser"),
            ("canImport(AppKit)", "Process()\nhomeDirectoryForCurrentUser"),
            ("os(FutureOS)", "Process()\nhomeDirectoryForCurrentUser"),
            ("unrecognized(Feature)", "Process()\nhomeDirectoryForCurrentUser"),
            ("FEATURE && false", "homeDirectoryForCurrentUser"),
            ("false && FEATURE", "homeDirectoryForCurrentUser"),
            ("FEATURE || true", "Process()"),
            ("true || FEATURE", "Process()"),
            ("canImport(AppKit) && false", "homeDirectoryForCurrentUser"),
            ("false && canImport(UnspecifiedModule)", "homeDirectoryForCurrentUser"),
            ("canImport(UnspecifiedModule) || true", "Process()"),
            ("true || canImport(AppKit)", "Process()"),
            ("os(FutureOS) && false", "homeDirectoryForCurrentUser"),
            ("false && os(FutureOS)", "homeDirectoryForCurrentUser"),
            ("os(FutureOS) || true", "Process()"),
            ("true || os(FutureOS)", "Process()"),
            ("unrecognized(Feature) && false", "homeDirectoryForCurrentUser"),
            ("unrecognized(Feature) || true", "Process()"),
            ("_FEATURE1 && false", "homeDirectoryForCurrentUser"),
            ("FEATURE_2 || true", "Process()"),
        ]
        for (condition, expected) in cases {
            let source = "#if \(condition)\nProcess()\n#else\nhomeDirectoryForCurrentUser\n#endif"
            XCTAssertEqual(Self.linesCompiledForIOS(source), expected, condition)
        }
    }

    /// 去掉行内注释。**注释里提到这些 API 名是正常的** —— 解释「为什么这里要加平台
    /// 开关」的那句话本身就写着它；把注释算进来，尺子会对着自己的说明文字发红
    /// （第一版就是这么红的）。裸 `//` 之后一律砍掉：会误伤 URL 字面量里的 `//`，
    /// 但那半行里不会出现我们要找的符号。
    static func strippingComment(_ line: Substring) -> String {
        guard let marker = line.range(of: "//") else { return String(line) }
        return String(line[line.startIndex..<marker.lowerBound])
    }

    /// 尺子自己的自检：`#else` 那一半是 iOS 也要编的，不许被当成 macOS 段一起吞掉；
    /// 注释里的符号不算数。
    func testStripperKeepsTheNonMacBranchAndDropsComments() {
        let sample = """
        #if os(macOS)
        let a = macOnlyThing
        #else
        let b = portableThing
        #endif
        /// 说明里提一句 macOnlyThing 不算用它
        let c = alwaysCompiled
        """
        let kept = Self.linesCompiledForIOS(sample)
        XCTAssertFalse(kept.contains("macOnlyThing"),
                       "macOS 段或注释没被剔掉，尺子会对着自己的说明文字发红")
        XCTAssertTrue(kept.contains("portableThing"), "#else 那一半被误删了，尺子会瞎")
        XCTAssertTrue(kept.contains("alwaysCompiled"))
    }

    private static func sourceFiles() throws -> [(URL, String)] {
        let root = URL(fileURLWithPath: #filePath)      // .../Tests/PendingCrewTests/<self>.swift
            .deletingLastPathComponent()                 // .../Tests/PendingCrewTests
            .deletingLastPathComponent()                 // .../Tests
            .deletingLastPathComponent()                 // 仓库根
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
