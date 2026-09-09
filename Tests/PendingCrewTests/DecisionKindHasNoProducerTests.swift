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
