import XCTest
import Foundation

/// 销号凭据（Todo #102 第四刀）。
///
/// 起因是一笔真账：群里记着「已修好，合进 main（`2cde0da9`）」，那个 hash 在两个仓
/// 都不是合法 git 对象，代码一字未变，账挂了 191 小时。**它带了凭据，只是那个凭据
/// 从来没有被解析过。**
///
/// 所以这里钉的都是「不这么做就会再出一笔假账」的那几条：三种结果不许压成两种、
/// 验不了不许当成验过、验不过不许动账、两个都不给不许放行。
final class TodoEvidenceTests: XCTestCase {

    private func judge(commit: String? = nil, prose: String? = nil,
                       _ resolution: TodoEvidence.Resolution = .resolved) -> TodoEvidence.Verdict {
        TodoEvidence.judge(commit: commit, prose: prose) { _ in resolution }
    }

    // MARK: - 三种结果必须分得开

    func testResolvedCommitIsAccepted() {
        XCTAssertEqual(judge(commit: "2cde0da", .resolved), .commitResolved("2cde0da"))
    }

    func testMissingObjectIsNotTheSameAsCannotVerify() {
        // 这两条在 agent 那边要做完全不同的事：前者「你给的东西是假的，去改」，
        // 后者「你给的可能是真的，但我这儿证不了，换条路」。压成一个 false，
        // 它只会瞎重试 —— 撤回那扇门上已经判过一次，这里不许退回去。
        XCTAssertEqual(judge(commit: "2cde0da", .notFound), .commitNotFound("2cde0da"))
        XCTAssertEqual(judge(commit: "2cde0da", .unavailable("没有登记的工作目录")),
                       .cannotVerify(commit: "2cde0da", why: "没有登记的工作目录"))
    }

    func testCannotVerifyIsNotSilentlyAccepted() {
        // 「验不了」绝不能落成 `.commitResolved`。这是整条设计的地基：
        // 「先记下来以后再核」正是那笔假账的形状。
        let verdict = judge(commit: "abcdef1", .unavailable("这儿不是仓库"))
        if case .commitResolved = verdict { XCTFail("验不了不许当成验过了") }
    }

    // MARK: - 形状先于跑 git

    func testMalformedCommitIsRejectedWithoutTouchingGit() {
        var probed = false
        let verdict = TodoEvidence.judge(commit: "做完了", prose: nil) { _ in
            probed = true
            return .resolved
        }
        XCTAssertEqual(verdict, .malformedCommit("做完了"))
        XCTAssertFalse(probed, "形状就不对，不该白起一个 git 进程")
    }

    func testCommitShapeBounds() {
        XCTAssertFalse(TodoEvidence.looksLikeCommit("abc123"))       // 6 位，太短必然歧义
        XCTAssertTrue(TodoEvidence.looksLikeCommit("abc1234"))       // 7 位，下限
        XCTAssertTrue(TodoEvidence.looksLikeCommit(String(repeating: "a", count: 40)))
        XCTAssertFalse(TodoEvidence.looksLikeCommit(String(repeating: "a", count: 41)))
        XCTAssertFalse(TodoEvidence.looksLikeCommit("abcdefg"))      // g 不是十六进制
    }

    // MARK: - 文字凭据那条路

    func testProseEvidenceIsAcceptedWhenNoCommitGiven() {
        XCTAssertEqual(judge(prose: "在 iPhone 15 真机上装了 0.1.25 走了一遍登录"),
                       .prose("在 iPhone 15 真机上装了 0.1.25 走了一遍登录"))
    }

    func testBlankProseIsNotEvidence() {
        XCTAssertEqual(judge(prose: "   "), .missing)
    }

    func testNeitherGivenIsRefused() {
        XCTAssertEqual(judge(), .missing)
        XCTAssertEqual(judge(commit: "  ", prose: nil), .missing)
    }

    func testCommitWinsWhenBothGiven() {
        // 两个都给了以 hash 为准（文字那条留在回执里，不丢）——
        // 否则「给个 hash 再随便写句话」就能绕开解析。
        XCTAssertEqual(judge(commit: "abc1234", prose: "顺手说一句", .notFound),
                       .commitNotFound("abc1234"))
    }

    // MARK: - 真的去解引用（用本仓库当现场）

    func testProbeResolvesARealObjectInThisRepo() throws {
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let probe = GitObjectProbe(directory: repoRoot.path)
        // HEAD 一定解析得出来 —— 拿它当「真对象」的样本，不写死任何 sha
        // （写死的 sha 在别人 clone 里可能不存在，那就成了一把会误报的尺子）。
        let head = try headSHA(at: repoRoot)
        XCTAssertEqual(probe.resolve(head), .resolved)
    }

    func testProbeReportsNotFoundForAFabricatedSHA() throws {
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        // 40 个 f 是合法形状、几乎不可能存在 —— 就是那笔假账的形状。
        let fabricated = String(repeating: "f", count: 40)
        XCTAssertEqual(GitObjectProbe(directory: repoRoot.path).resolve(fabricated), .notFound)
    }

    func testProbeSaysCannotVerifyOutsideARepo() {
        // 「不是仓库」必须报成 `.unavailable`，不能报成 `.notFound` ——
        // 前者是环境问题、后者是凭据问题，说反了会让人去错的方向找。
        let outside = FileManager.default.temporaryDirectory
            .appendingPathComponent("no-repo-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: outside, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: outside) }
        guard case .unavailable = GitObjectProbe(directory: outside.path)
            .resolve(String(repeating: "a", count: 40)) else {
            return XCTFail("仓库外解析要报「我验不了」，不是「你的凭据不存在」")
        }
    }

    func testUnsupportedPlatformGetsCannotVerifyNotResolvedNorNotFound() {
        // iOS 上没有 git、没有工作副本，「验凭据」这件事本来就不成立。它必须落进
        // **已有的**「我验不了」那一态 —— 不许是「解析成功」（那是假绿），
        // 也不许是「不存在」（那会让人去改一个没错的凭据）。
        //
        // 这条用例**只靠平台这一项决定结果**：目录是真的、hash 形状是对的、
        // 本仓库确实是 git 仓库 —— 链上其它项全部成立，只把 support 换掉。
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let probe = GitObjectProbe(directory: repoRoot.path,
                                   support: .unsupported("这个平台上跑不了 git（凭据解析只在 Mac 上成立）"))
        XCTAssertEqual(probe.resolve(String(repeating: "a", count: 40)),
                       .unavailable("这个平台上跑不了 git（凭据解析只在 Mac 上成立）"))
    }

    func testMacCanRunGitSoTheGateIsRealHere() {
        // 反面：Mac 上必须是 `.canRunGit`，否则上一条证明不了什么 ——
        // 一把永远返回「验不了」的尺子当然永远不会放行假账，但它也永远没在工作。
        XCTAssertEqual(GitObjectProbe.current, .canRunGit)
    }

    func testProbeSaysCannotVerifyWhenTheDirectoryIsGone() {
        let gone = "/tmp/definitely-not-here-\(UUID().uuidString)"
        guard case .unavailable = GitObjectProbe(directory: gone)
            .resolve(String(repeating: "a", count: 40)) else {
            return XCTFail("登记的目录不在了要报「我验不了」")
        }
    }

    // MARK: - respond_todo 那道闸（先验后动账）

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("todo-evidence-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1", isCaptain: false,
                  sessionLabel: "机长", todos: LocalTodoStore(directory: dir))
    }

    private func respond(_ s: McpServer, _ args: [String: Any]) -> String {
        let json = String(data: try! JSONSerialization.data(withJSONObject: args),
                          encoding: .utf8)!
        return s.handleLine("""
            {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"respond_todo","arguments":\(json)}}
            """) ?? ""
    }

    @discardableResult
    private func seedAgentTodo(_ dir: URL) -> LocalTodoItem {
        LocalTodoStore(directory: dir).add(crewId: "c", text: "把那个搜索修好")!
    }

    func testCompletingWithoutEvidenceIsRefusedAndChangesNothing() {
        let dir = tempDir()
        let item = seedAgentTodo(dir)
        let receipt = respond(server(dir), ["number": item.number, "response": "做完了",
                                           "status": "completed"])
        XCTAssertTrue(receipt.contains("要带凭据"), receipt)
        // **先验后动账**：拒了就一个字都不许写进去 —— 状态没翻、回应没落。
        let after = LocalTodoStore(directory: dir).item(crewId: "c", number: item.number)
        XCTAssertEqual(after?.status, "pending")
        XCTAssertTrue(after?.responses.isEmpty == true)
    }

    func testCompletingWithProseEvidenceLandsItOnTheItem() {
        let dir = tempDir()
        let item = seedAgentTodo(dir)
        let receipt = respond(server(dir), [
            "number": item.number, "response": "修好了", "status": "completed",
            "evidence": "在共享目录跑了全量，2213 tests / 0 failures，日志归档在 .test-archive",
        ])
        XCTAssertTrue(receipt.contains("已回应"), receipt)
        let after = LocalTodoStore(directory: dir).item(crewId: "c", number: item.number)
        XCTAssertEqual(after?.status, "completed")
        XCTAssertTrue(after?.responses.first?.text.contains(".test-archive") == true,
                      "凭据要跟着条目走，不能只活在回执里")
    }

    func testCompletingWithAHashButNoRegisteredWorkdirIsRefusedNotRecorded() {
        // 临时目录里没有 local-crews.json，也就查不到本 crew 登记的工作目录 ——
        // 这时**不许**退回去从 cwd 往上找一个仓库来解析（那会在错的仓库里解析成功，
        // 生产一条带真 hash 的假账），必须明说验不了、并且**不销号、也不记下来**。
        let dir = tempDir()
        let item = seedAgentTodo(dir)
        let receipt = respond(server(dir), [
            "number": item.number, "response": "修好了", "status": "completed",
            "evidence_commit": String(repeating: "a", count: 40),
        ])
        XCTAssertTrue(receipt.contains("验不了"), receipt)
        XCTAssertTrue(receipt.contains("也没有把它记下来等以后再核"), receipt)
        let after = LocalTodoStore(directory: dir).item(crewId: "c", number: item.number)
        XCTAssertEqual(after?.status, "pending")
        XCTAssertTrue(after?.responses.isEmpty == true)
    }

    func testNonCompletionStatusesDoNotNeedEvidence() {
        // 这道闸只卡「宣布完成」。认领和报进展照旧，不然它会把日常沟通也变贵。
        let dir = tempDir()
        let item = seedAgentTodo(dir)
        XCTAssertTrue(respond(server(dir), ["number": item.number, "response": "我接了",
                                            "status": "in_progress"]).contains("已回应"))
        XCTAssertTrue(respond(server(dir), ["number": item.number, "response": "还在跑测试"])
            .contains("已回应"))
        XCTAssertEqual(LocalTodoStore(directory: dir)
            .item(crewId: "c", number: item.number)?.status, "in_progress")
    }

    func testReceiptNeverClaimsTheFixWasVerified() {
        // 我们能验的只有指针，不是内容。回执里出现「已验证该修复」这种话，
        // 下一个人就会把销号当成验收。
        let dir = tempDir()
        let item = seedAgentTodo(dir)
        let receipt = respond(server(dir), [
            "number": item.number, "response": "修好了", "status": "completed",
            "evidence": "人工核对过输出",
        ])
        XCTAssertFalse(receipt.contains("已验证"), receipt)
    }

    private func headSHA(at repoRoot: URL) throws -> String {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        p.arguments = ["-C", repoRoot.path, "rev-parse", "HEAD"]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        try p.run()
        p.waitUntilExit()
        let sha = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        XCTAssertEqual(sha.count, 40, "拿不到 HEAD，这台机器上的现场不对")
        return sha
    }
}
