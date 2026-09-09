import XCTest

/// **测试不许把 `McpServer` 的账落到人的真实数据目录里。**
///
/// ## 这不是假设，已经发生过两次
///
/// 1. 2026-09-08（`914794a` 的注释里原文）：分类落账的第一版用例没注入 `todos`，
///    **往真的 app 数据目录写进了三条 Todo，事后手工删掉**。当时的处置是**在两个
///    测试文件里各留一条警告注释**。
/// 2. 2026-09-09（本单）：我把 `ask` 改成写人类 Todo 之后，`McpServerTests` 那个
///    工厂漏传目录，在人的数据目录里造出 `c.human-todos.json`。**那两条注释没拦住我
///    —— 我改的是第三个文件，根本没读到它们。**
///
/// 两条注释立起来一天就被绕过一次，绕过的人还是读过这类规矩的。**再加第三条注释
/// 只会等第四个人。** 所以这里立一把会红的尺子。
///
/// ## 判据只看一个旋钮（`1d373b3` 之后才成立）
///
/// 以前 `McpServer.init` 里有 **3 个独立的目录根**（`quotaDirectory` / `todos` /
/// `plans`），漏任何一个都会落到真实目录。`1d373b3`（#115 D）把后两个改成跟着
/// `quotaDirectory` 走之后，**独立的根只剩 1 个** —— 于是判据可以简化成一句：
/// **构造 `McpServer` 必须传 `quotaDirectory:`**。
///
/// 这也是为什么这里**没有共享工厂、因而也没有豁免**。工厂的价值本来是「替你记住
/// 8 个参数」；现在只剩 1 个，而工厂必然要给自己开一个豁免 ——
/// **那个豁免就是谁都能在那儿绕过去的洞，而且它绕过之后尺子依然全绿。**
/// 一个不需要豁免的判据比一个有豁免的判据结实。
///
/// ## 只扫代码，不扫注释
///
/// 上面那两条警告注释里就写着 `McpServer(` —— 不跳注释的话，这把尺子会当场把
/// **专门用来防这个错的那两句话**判成违规。这个形状本组今天撞了四次。
/// 判据零判断，照抄 `ReleaseScriptSourceContractTests`：**整行第一个非空字符是
/// `//` 就是注释**。不做「行内 // 之后算注释」那种事 —— 字符串里的 `//` 不是注释。
final class McpServerTestDirectoryContractTests: XCTestCase {

    // MARK: - 探测器本身（内联样本把它钉死，红是长期在的，不是我跑一次看见过）

    func testDetectorFiresOnADirectConstructionWithoutTheDirectory() {
        let code = """
        let s = McpServer(store: LocalWhiteboardStore(directory: dir),
                          approvals: LocalApprovalStore(directory: dir),
                          crewId: "c", sessionId: "s")
        """
        XCTAssertTrue(Self.constructsMcpServerWithoutDirectory(code),
                      "漏传 quotaDirectory 的构造没被抓到 —— 那这把尺子什么都拦不住")
    }

    func testDetectorStaysQuietWhenTheDirectoryIsPassed() {
        let code = """
        let s = McpServer(store: LocalWhiteboardStore(directory: dir),
                          approvals: LocalApprovalStore(directory: dir),
                          crewId: "c", sessionId: "s", quotaDirectory: dir)
        """
        XCTAssertFalse(Self.constructsMcpServerWithoutDirectory(code),
                       "传了目录还被判违规 —— 一把会过报的尺子会被人关掉")
    }

    /// **这条是这把尺子最容易长歪的地方**：防这个错的注释里必然写着 `McpServer(`。
    func testDetectorIgnoresWarningComments() {
        let code = """
        // ⚠️ `todos` 必须显式注入 —— 否则 McpServer(store:…) 会落到真的数据目录
        /// 别写成 McpServer(store: …, crewId: "c", sessionId: "s") 这种漏目录的形状
        let s = McpServer(store: st, approvals: ap, crewId: "c", sessionId: "s", quotaDirectory: dir)
        """
        XCTAssertFalse(Self.constructsMcpServerWithoutDirectory(code),
                       """
                       尺子把**专门用来防这个错的注释**判成了违规。\
                       那样它会逼着人删掉最该留下的那两句话 —— 本组今天已经撞了四次这个形状。
                       """)
    }

    // MARK: - 真扫一遍

    func testNoTestFileConstructsMcpServerWithoutADirectory() throws {
        let files = try Self.testSources()
        XCTAssertGreaterThan(files.count, 20, "源码扫描没扫到东西，测试本身失效了")
        let offenders = files
            .filter { $0.0 != "McpServerTestDirectoryContractTests.swift" }
            .filter { Self.constructsMcpServerWithoutDirectory($0.1) }
            .map(\.0)
            .sorted()
        XCTAssertTrue(
            offenders.isEmpty,
            """
            这些测试文件构造 `McpServer` 时没传 `quotaDirectory:`，于是 todos / plans / \
            wakeups / humanTodos / continuations / attachmentRoot / agentSessions \
            全部落到**人的真实数据目录**（\(offenders.count) 个）：
            \(offenders.joined(separator: "\n"))

            传 `quotaDirectory: <你的 temp dir>` 一个参数就够 —— 其余七样都跟着它走。
            """)
    }

    // MARK: - 判据

    /// 一段 Swift 源码里有没有「构造 McpServer 却没给 quotaDirectory」。
    ///
    /// 逐个构造点各判各的：从 `McpServer(` 起，到**括号配平**为止就是这一次调用的
    /// 参数表。不用正则跨行拼，也不做「聪明」解析 —— 括号配平是零判断的。
    static func constructsMcpServerWithoutDirectory(_ source: String) -> Bool {
        let code = source.split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")
        var rest = Substring(code)
        while let hit = rest.range(of: "McpServer(") {
            var depth = 0
            var end = hit.upperBound
            var idx = hit.lowerBound
            // 从 `McpServer(` 的那个左括号开始配平
            idx = code.index(before: hit.upperBound)
            var cursor = idx
            while cursor < rest.endIndex {
                let ch = rest[cursor]
                if ch == "(" { depth += 1 }
                if ch == ")" {
                    depth -= 1
                    if depth == 0 { end = rest.index(after: cursor); break }
                }
                cursor = rest.index(after: cursor)
            }
            let call = rest[hit.upperBound..<max(end, hit.upperBound)]
            if !call.contains("quotaDirectory:") { return true }
            rest = rest[end...]
        }
        return false
    }

    private static func testSources() throws -> [(String, String)] {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: root.path)
        else { throw XCTSkip("读不到测试源码目录") }
        return names.filter { $0.hasSuffix(".swift") }.compactMap { name in
            guard let text = try? String(
                contentsOf: root.appendingPathComponent(name), encoding: .utf8) else { return nil }
            return (name, text)
        }
    }
}
