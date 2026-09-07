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
    ]

    func testNoMacOnlyAPIInFilesThatAlsoCompileForIOS() throws {
        let sources = try Self.sourceFiles()
        XCTAssertGreaterThan(sources.count, 50, "源码扫描没扫到东西，测试本身失效了")

        var offenders: [String] = []
        for (url, text) in sources {
            // 只看**会被编进 iOS 的那些行**：`#if os(macOS)` 块里的整段拿掉。
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
            这些文件没有 `#if os(macOS)`，却用了只有 macOS 才有的 API —— iOS 端**必然**编不过：
            \(offenders.joined(separator: "\n"))
            仓库里已有现成写法（`CockpitTaskLedger.currentHome`）：macOS 用
            `FileManager.default.homeDirectoryForCurrentUser`，其余用
            `URL(fileURLWithPath: NSHomeDirectory())`。照抄，别发明第二种。
            """)
    }

    /// 把 `#if os(macOS)` 分支里的行剔掉，剩下的就是 iOS 端也要编的那些行。
    /// 只认 `#if os(macOS)` / `#elseif os(macOS)` 这一种写法（本仓库统一用它）；
    /// 别的条件一律当成「两端都编」—— 尺子宁可多看，不可少看。
    static func linesCompiledForIOS(_ text: String) -> String {
        var macOnlyStack: [Bool] = []
        var kept: [Substring] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("#if ") {
                macOnlyStack.append(t == "#if os(macOS)")
                continue
            }
            if t.hasPrefix("#elseif ") {
                if !macOnlyStack.isEmpty { macOnlyStack[macOnlyStack.count - 1] = (t == "#elseif os(macOS)") }
                continue
            }
            if t == "#else" {
                if !macOnlyStack.isEmpty { macOnlyStack[macOnlyStack.count - 1].toggle() }
                continue
            }
            if t == "#endif" {
                if !macOnlyStack.isEmpty { macOnlyStack.removeLast() }
                continue
            }
            if macOnlyStack.contains(true) { continue }
            kept.append(Substring(Self.strippingComment(line)))
        }
        return kept.joined(separator: "\n")
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
