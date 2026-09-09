import XCTest

/// **`kind: "decision"` 已经没有生产者了 —— 那就不许再有人教别人去处理它。**
///
/// ## 现场（2026-09-09，#75 收尾时被机长撞到）
///
/// `CrewCenterView` 里有一段：扫审批账本里 `kind == "decision"` 的 pending，注入机长
/// 「用 `answer_decision` 工具答它」。#75 之后**两头都不成立**：
///
/// - `ask` 改成写人类 Todo，**不再产生 `decision`**；
/// - codex 原生审批走的是 `kind: "permission"`（`CodexManualApprovalBridge`）；
/// - `answer_decision` 这个工具**已经删了**。
///
/// 于是它只可能命中 #75 **之前**遗留的旧行，然后把机长送进死胡同：机长照着答，
/// 工具回「找不到待决策」，而 worker 还停在那儿等。**两边都不知道对方在等什么** ——
/// 正是 #75 要消灭的形状换了个位置活下来。
///
/// ## 这条尺子钉的是什么
///
/// **拆掉一条老路时，「还有谁在教这条老路」要当成拆除清单的一部分。** 实现删干净了，
/// 而提示语、文案、文档里的指路牌会继续把人送进死胡同，**且不会有任何报错**。
///
/// 所以这里钉一对：**没有生产者** ⇒ **也不许有消费者**。谁哪天要重新引入
/// `decision`，这条会红，逼他把两头一起接上。
final class DecisionKindHasNoProducerTests: XCTestCase {

    func testNothingRaisesADecisionApprovalAnyMore() throws {
        let offenders = try Self.sources()
            .filter { Self.codeOnly($0.1).contains(#"kind: "decision""#) }
            .map(\.0).sorted()
        XCTAssertTrue(
            offenders.isEmpty,
            """
            这些文件又开始产生 `kind: "decision"` 了（\(offenders.joined(separator: "、"))）。
            要重新引入它，就得连**谁来处理、用什么工具处理**一起接上 —— 上一次少了这半，
            机长被指去用一个已删的工具，两边一起卡住。
            """)
    }

    func testNothingTellsAnyoneToUseTheDeletedTool() throws {
        let offenders = try Self.sources()
            .filter { Self.codeOnly($0.1).contains("answer_decision") }
            .map(\.0).sorted()
        XCTAssertTrue(
            offenders.isEmpty,
            """
            还有代码在教别人用 `answer_decision`（\(offenders.joined(separator: "、"))）——
            那个工具 #75 ① 已经删掉，照做的人会拿到「找不到待决策」。
            **注释里提它没问题**（这里只扫代码）；**运行时说给 agent 听的话不行**。
            """)
    }

    /// 反面：codex 原生那条**必须还在**。#75 拆的是 agent 问人类那一半，
    /// codex 协议自己要的审批留着 —— 拆了它 codex 在需要审批时无处可去，
    /// 那是新造一个「停」，跟这一单目的正相反。
    func testCodexNativeApprovalPathIsStillThere() throws {
        let store = try Self.text(of: "LocalApprovalStore.swift")
        XCTAssertTrue(store.contains(#"kind: "permission""#),
                      "codex 原生审批那条也被拆了 —— 它需要审批时会无处可去")
    }

    // MARK: - 扫描面不止 `Sources/`：**任何教人怎么做事的地方**

    /// 拆掉一条老路之后，**「还有谁在教这条老路」是拆除清单的一部分**。
    ///
    /// 上一笔（`18b5e82`）只扫了 `Sources/`，于是漏掉了真正在教人的那一处：
    /// **`Resources/Prompts/crew-captain.zh.md` —— 机长自己的提示词**，里面白纸黑字写着
    /// 「直接用工具 `answer_decision(reqId, reply)` 答它」。机长照做，工具回「找不到
    /// 待决策」，而 worker 还停在那儿等。**实现删干净了，指路牌还在把人往死胡同送，
    /// 而且不会有任何报错。**
    ///
    /// ## 判据：可以提它，但同一行必须说清它没了
    ///
    /// Markdown **没有注释语法**，所以「跳过注释」那招在文档上不成立 —— 而文档里
    /// 记录「这条路被删了」又是**应该**做的事。本仓已经因为这个形状红过三次
    /// （拆老路时留的注释里必然写着老路的名字）。
    ///
    /// 所以规则不是「不许出现」，是 **「出现时同一行必须带作废标记」**（`\(Self.retiredMarker)`）：
    /// - 教人用它 → 红；
    /// - 记录它没了 → 绿，**而且强制作者把「它没了」写出来**。
    ///
    /// 判据仍是零判断的 `contains`，不区分「叙述」和「指令」——那种区分需要判断，
    /// 而需要判断的判据会在第一次争议时被绕过。
    ///
    /// ## 扫描面的边界
    ///
    /// 扫：`Sources/`、`Resources/Prompts/`、`docs/` 顶层、`scripts/`、README / CONTRIBUTING。
    /// **不扫 `docs/internal/`** —— 那是按日期归档的事故与审计记录，**按构造就是历史**，
    /// 不是指导文本。把归档也纳进来，等于要求每一份事故报告在提到旧路时改写它的原话。
    func testNothingTeachesTheDeletedToolAnyMore() throws {
        var offenders: [String] = []
        var pendingHuman: [String] = []
        for (name, path, text) in try Self.instructionalFiles() {
            for (i, line) in Self.stripComments(text, ext: Self.ext(name)).enumerated()
            where line.contains("answer_decision") && !line.contains(Self.retiredMarker) {
                let hit = "\(path):\(i + 1)"
                if name == Self.humanOnlyFile { pendingHuman.append(hit) } else { offenders.append(hit) }
            }
        }
        if !pendingHuman.isEmpty {
            // **扫得到、但不红**（机长 2026-09-09 定）：那个文件只有人类能动，
            // 一把因为谁都改不了的文件而永远红的尺子，两周内会被人关掉。
            print("［已知不一致·等人处理］\(Self.humanOnlyFile) 仍在教已删的 answer_decision：\n  "
                  + pendingHuman.joined(separator: "\n  "))
        }
        XCTAssertTrue(
            offenders.isEmpty,
            """
            这些**写给人或 agent 看的**地方还在教 `answer_decision`，而那个工具 #75 ① 已删：
            \(offenders.joined(separator: "\n"))

            要保留这句话（比如记录它被删过），在**同一行**加上「\(Self.retiredMarker)」。
            """)
    }

    /// 豁免必须**恰好一处**。上次学到的：豁免就是那个洞，第二个悄悄出现时尺子就瞎了、
    /// 而且**依然全绿**。
    ///
    /// ⚠️ **这个豁免此刻是空转的，说出来免得它被当成一层保护**：实测
    /// `session-world-model.zh.md` 里 **一处 `answer_decision` 都没有**（我先前跟机长
    /// 报过两次「它还在教 answer_decision」，**那是错的，没开文件就说了**）。
    ///
    /// 那个文件里真正过期的是**另一件事**：`ask` 仍被描述成「阻塞」（第 30 / 149 / 156 行
    /// 一带），而 #75 之后它不阻塞了。**这把尺子不覆盖那一条** —— 它只认
    /// `answer_decision` 这个名字。要覆盖「口径过期」得能区分「描述现状」和「描述历史」，
    /// 那需要判断，而需要判断的判据会在第一次争议时被绕过。
    ///
    /// 所以那处不一致**不靠尺子治，靠人**：已经在给人类的清单里（只有人类能动那个文件）。
    func testTheHumanOnlyExemptionIsExactlyOne() {
        XCTAssertEqual(Self.humanOnlyFile, "session-world-model.zh.md",
                       "豁免的目标变了 —— 换目标要有人明确决定，不该是顺手改的")
    }

    /// 扫描面自己不许缩水：**扫不到的地方跟没有问题长得一模一样。**
    func testTheScanReachesPromptsAndDocsNotJustSources() throws {
        let files = try Self.instructionalFiles()
        for expected in ["crew-captain.zh.md", "architecture.md"] {
            XCTAssertTrue(files.contains { $0.0 == expected },
                          "扫描面里没有 \(expected) —— 上一笔漏的正是这一类")
        }
        XCTAssertTrue(files.contains { $0.0.hasSuffix(".swift") }, "连 Sources 都不扫了")
        XCTAssertFalse(files.contains { $0.1.contains("docs/internal/") },
                       "把归档也扫进来了 —— 那会要求每份事故报告改写它的原话")
    }

    /// 作废标记；同一行带上它就算「记录」而不是「教」。
    static let retiredMarker = "已删"
    /// 唯一的豁免：只有人类能动它。
    static let humanOnlyFile = "session-world-model.zh.md"

    private static func ext(_ name: String) -> String {
        name.contains(".") ? String(name.split(separator: ".").last!) : ""
    }

    /// 按后缀剥注释。**零判断**，照 `ReleaseScriptSourceContractTests` 那条：
    /// 整行第一个非空字符是注释符才算注释；不做行内解析（字符串里的 `//` 不是注释）。
    /// `.md` 没有注释语法 —— 一行都不剥，这正是需要作废标记那条规则的原因。
    private static func stripComments(_ text: String, ext: String) -> [String] {
        let marker: String?
        switch ext {
        case "swift": marker = "//"
        case "sh": marker = "#"
        default: marker = nil
        }
        return text.split(separator: "\n", omittingEmptySubsequences: false).map { line in
            guard let marker,
                  line.trimmingCharacters(in: .whitespaces).hasPrefix(marker) else { return String(line) }
            return ""
        }
    }

    /// 「会教人怎么做事」的全部文件。(文件名, 仓库相对路径, 内容)
    private static func instructionalFiles() throws -> [(String, String, String)] {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        var out: [(String, String, String)] = []
        func add(_ url: URL) {
            guard let text = try? String(contentsOf: url, encoding: .utf8) else { return }
            let rel = url.path.replacingOccurrences(of: root.path + "/", with: "")
            out.append((url.lastPathComponent, rel, text))
        }
        for sub in ["Sources", "Resources/Prompts", "scripts"] {
            guard let walker = FileManager.default.enumerator(
                at: root.appendingPathComponent(sub), includingPropertiesForKeys: nil,
                options: [.skipsHiddenFiles]) else { continue }
            for case let url as URL in walker
            where ["swift", "md", "sh"].contains(url.pathExtension) { add(url) }
        }
        // docs 只取顶层 —— `docs/internal/` 是按日期归档的事故记录，不是指导文本。
        if let names = try? FileManager.default.contentsOfDirectory(
            atPath: root.appendingPathComponent("docs").path) {
            for n in names where n.hasSuffix(".md") {
                add(root.appendingPathComponent("docs").appendingPathComponent(n))
            }
        }
        for n in ["README.md", "CONTRIBUTING.md"] { add(root.appendingPathComponent(n)) }
        guard out.count > 50 else { throw XCTSkip("扫描面异常小（\(out.count)），测试本身失效了") }
        return out
    }

    private static func codeOnly(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    private static func text(of fileName: String) throws -> String {
        guard let hit = try sources().first(where: { $0.0 == fileName })
        else { throw XCTSkip("找不到 \(fileName)") }
        return hit.1
    }

    private static func sources() throws -> [(String, String)] {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录") }
        return walker.compactMap { any in
            guard let url = any as? URL, url.pathExtension == "swift",
                  let text = try? String(contentsOf: url, encoding: .utf8) else { return nil }
            return (url.lastPathComponent, text)
        }
    }
}
