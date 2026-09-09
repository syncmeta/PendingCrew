import XCTest

/// 权限类也并进 Todo：**当场拒绝 + 提一条 Todo 引导**，不阻塞（驾驶舱计划 #75 ②）。
///
/// ## 人类原话
///
/// > 权限类可也可以用 todo 来做，如果实在需要人类，也应该通过 todo 引导。
///
/// 现状：gate 命中 → raise 一条待审批 → **阻塞 long-poll 最多一小时** → 到点判 deny。
/// 新：gate 命中 → **立刻 deny** + 提一条人类 Todo 说清「我要跑 X、为了 Y」→ agent
/// 去干别的 → 人同意后它再来跑。
///
/// ## 承重点：放行凭据。没有它，这个改动比现状更糟
///
/// hook 每次都重新判、**不知道人已经同意过**。所以「拒 → 提 Todo → 人同意 → agent
/// 重跑」会变成：重跑又被拒、又提一条 Todo、又被拒…… **无限循环，而且每转一圈往
/// 人类账上加一条垃圾**。人类刚因为「Todo 账上挂着一堆」质问过。
/// **做成那样等于我们亲手造了一台往他账上灌垃圾的机器。**
///
/// 所以这里钉两件事：
/// 1. **同一个工具已经有一条待处理的请求时，不许再提第二条。**
/// 2. **人同意之后要留下一张一次性放行票**，hook 见票放行并当场作废它。
///
/// ## 活体面很小，火力集中在循环上
///
/// 权限 gate 现在**只有一个**：`LocalSessionLaunch.swift` 里写死的 `computer-use`。
/// 其余全走 runner 自己的自动权限。所以覆盖面不是风险，循环才是。
///
/// ## 量不到的
///
/// 「人同意了」这件事只能从**自由文本**里读。这里的读法是保守的（读不准就当没同意），
/// 但它**读不出反讽、读不出条件同意**（「可以，但先备份」会被当成同意）。
/// 这条不是这一层能解决的，别声称它解决了。
final class PermissionIntoTodoTests: XCTestCase {

    // MARK: - ① 三种局面各走各的

    func testFirstTimeDeniesAndFilesATodo() {
        XCTAssertEqual(
            PermissionRequestFlow.decide(hasGrant: false, hasPendingRequest: false),
            .denyAndFile,
            "第一次撞到 gate，既没提过也没票 —— 该当场拒掉并提一条 Todo 引导人")
    }

    /// **这条是承重点的一半：不许灌垃圾。**
    func testDoesNotFileASecondTodoWhileOneIsStillPending() {
        XCTAssertEqual(
            PermissionRequestFlow.decide(hasGrant: false, hasPendingRequest: true),
            .denyWithoutFiling,
            """
            同一个工具已经有一条待处理的请求，还要再提一条 —— agent 每重试一次就往\
            人类账上加一条垃圾。人类刚因为「账上挂着一堆」质问过，这条做错等于\
            亲手造一台灌垃圾的机器。
            """)
    }

    /// **承重点的另一半：人同意过就得放行**，否则重跑又被拒 = 死循环。
    func testGrantLetsItThroughAndIsConsumed() {
        XCTAssertEqual(
            PermissionRequestFlow.decide(hasGrant: true, hasPendingRequest: false),
            .allowConsumingGrant,
            "人已经同意了，重跑还被拒 —— 那就是无限循环，比现状糟")
    }

    /// 有票时**票优先**：那条 Todo 还挂着不该挡住已经拿到的同意。
    func testGrantWinsOverAStillPendingRequest() {
        XCTAssertEqual(
            PermissionRequestFlow.decide(hasGrant: true, hasPendingRequest: true),
            .allowConsumingGrant,
            "人同意了但那条 Todo 还没被标完，就把它挡在外面 —— 同意会被自己的记录吃掉")
    }

    // MARK: - ② 从自由文本里读「同意了没有」——保守

    func testReadsAPlainYesAsGranted() {
        for reply in ["同意", "可以", "好，跑吧", "allow", "批准了"] {
            XCTAssertEqual(PermissionGrantReading.read(reply), .granted, reply)
        }
    }

    func testReadsARefusalAsRefused() {
        for reply in ["不行", "别跑", "拒绝", "deny", "不同意"] {
            XCTAssertEqual(PermissionGrantReading.read(reply), .refused, reply)
        }
    }

    /// **读不准就当没同意。** 权限这件事上，把「看不懂」当成「同意」是不可接受的方向；
    /// 当成「没同意」最坏只是让 agent 再问一次。
    func testUnclearRepliesAreNotAGrant() {
        for reply in ["嗯？", "这个我再想想", "", "你先看看别的"] {
            XCTAssertNotEqual(PermissionGrantReading.read(reply), .granted,
                              "「\(reply)」被当成同意了 —— 权限上把看不懂读成同意是错的方向")
        }
    }

    /// ⚠️ 已知读不出来的一类，**写下来免得它被当成已解决**。
    func testKnownBlindSpotConditionalApprovalReadsAsGranted() {
        XCTAssertEqual(PermissionGrantReading.read("可以，但先备份"), .granted,
                       """
                       这条是**故意钉住的盲区**，不是期望行为：条件同意会被读成无条件同意。\
                       真要治得让人在 Todo 里点「同意/不同意」而不是写自由文本 —— 那是另一单。
                       """)
    }

    // MARK: - ③ 一次性票：用掉就没了

    func testGrantIsOneShot() {
        let dir = tempDir()
        let store = PermissionGrantStore(directory: dir)
        store.grant(crewId: "c", tool: "computer-use")
        XCTAssertTrue(store.consume(crewId: "c", tool: "computer-use"), "第一次该放行")
        XCTAssertFalse(store.consume(crewId: "c", tool: "computer-use"),
                       """
                       一张票放行了两次。一次同意 = 一次放行；要长期放行是另一个决定，\
                       不该由「人回了一句可以」顺手变成常设权限。
                       """)
    }

    func testGrantsAreScopedToTheTool() {
        let dir = tempDir()
        let store = PermissionGrantStore(directory: dir)
        store.grant(crewId: "c", tool: "computer-use")
        XCTAssertFalse(store.consume(crewId: "c", tool: "rm-rf"),
                       "给 A 工具的同意被 B 工具用掉了 —— 那是把一次授权扩大成了通行证")
    }

    // MARK: - ④ 旧的阻塞路径必须真拆掉

    func testHookNoLongerBlocksWaitingForAHuman() throws {
        let source = Self.codeOnly(try Self.text(of: "McpPermissionHook.swift"))
        XCTAssertFalse(source.contains("awaitDecision("),
                       "权限 hook 还在 long-poll 等人 —— 那是「怎么又停了」的另一半")
        XCTAssertFalse(source.contains("kind: \"permission\""),
                       "还在往待审批列表 raise —— 权限类该走 Todo，不是两套并存")
    }

    // MARK: - 小工具

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("perm-todo-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
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
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录") }
        for case let url as URL in walker where url.lastPathComponent == fileName {
            return try String(contentsOf: url, encoding: .utf8)
        }
        throw XCTSkip("找不到 \(fileName)")
    }
}
