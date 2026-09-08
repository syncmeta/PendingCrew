import XCTest

/// 执行层。这里跑**真文件系统**（tmp 下造一个假 home），因为要钉的正是
/// 「会不会覆盖别人的东西 / 会不会把用户的配置改坏 / 炸了之后回执说不说得清」——
/// 这几件事只有真读真写才算数。
final class WorkdirMigrationExecutorTests: XCTestCase {

    private var home: URL!

    override func setUpWithError() throws {
        home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("workdir-migration-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: home)
    }

    private func write(_ text: String, to relative: String) throws {
        let url = home.appendingPathComponent(relative)
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url)
    }

    private func read(_ relative: String) throws -> String {
        try String(contentsOf: home.appendingPathComponent(relative), encoding: .utf8)
    }

    // MARK: - ~/.claude.json

    private let claudeJSON = """
    {
      "numStartups": 400,
      "projects": {
        "/old": {
          "mcpServers": {"thing": {}},
          "allowedTools": ["Bash(ls:*)"],
          "lastCost": 1.5
        },
        "/new": {
          "mcpServers": {},
          "allowedTools": ["Bash(git:*)"]
        },
        "/somebody-else": { "allowedTools": ["Bash(rm:*)"] }
      }
    }
    """

    /// 只补点名的键；目标已有实质值的**不覆盖**；旧条目和别人的条目原样留着；
    /// 统计类字段（lastCost）不跟着搬。
    func testCopyClaudeProjectSettingsOnlyFillsRequestedGaps() throws {
        try write(claudeJSON, to: ".claude.json")
        try WorkdirMigrationExecutor.copyClaudeProjectSettings(
            home: home, from: "/old", to: "/new",
            keys: ["mcpServers", "allowedTools"])

        let root = try JSONSerialization.jsonObject(
            with: Data(read(".claude.json").utf8)) as! [String: Any]
        let projects = root["projects"] as! [String: Any]
        let new = projects["/new"] as! [String: Any]
        XCTAssertEqual((new["mcpServers"] as? [String: Any])?.count, 1, "空字典应当被源补上")
        XCTAssertEqual(new["allowedTools"] as? [String], ["Bash(git:*)"], "目标已有的不许被覆盖")
        XCTAssertNil(new["lastCost"], "统计字段不该跟着搬")

        let old = projects["/old"] as! [String: Any]
        XCTAssertEqual((old["mcpServers"] as? [String: Any])?.count, 1, "旧条目要原样留着")
        XCTAssertNotNil(projects["/somebody-else"], "别人的条目一个字都不许动")
        XCTAssertEqual(root["numStartups"] as? Int, 400, "projects 以外的设置要原样留着")
    }

    func testCopyClaudeProjectSettingsCreatesTargetEntryWhenAbsent() throws {
        try write(#"{"projects":{"/old":{"allowedTools":["a"]}}}"#, to: ".claude.json")
        try WorkdirMigrationExecutor.copyClaudeProjectSettings(
            home: home, from: "/old", to: "/new", keys: ["allowedTools"])
        let root = try JSONSerialization.jsonObject(
            with: Data(read(".claude.json").utf8)) as! [String: Any]
        let new = (root["projects"] as! [String: Any])["/new"] as! [String: Any]
        XCTAssertEqual(new["allowedTools"] as? [String], ["a"])
    }

    func testCopyClaudeProjectSettingsFailsLoudWhenSourceEntryGone() throws {
        try write(#"{"projects":{}}"#, to: ".claude.json")
        XCTAssertThrowsError(try WorkdirMigrationExecutor.copyClaudeProjectSettings(
            home: home, from: "/old", to: "/new", keys: ["allowedTools"]))
    }

    // MARK: - 目录枚举

    func testRelativeFilePathsIsRecursiveAndFilesOnly() throws {
        try write("a", to: "mem/MEMORY.md")
        try write("b", to: "mem/sub/deep.md")
        let found = Set(WorkdirMigrationExecutor.relativeFilePaths(
            under: home.appendingPathComponent("mem").path))
        XCTAssertEqual(found, ["MEMORY.md", "sub/deep.md"])
    }

    // MARK: - 整体执行

    /// 一条完整的成功路径：备份先落地 → 工具权限 → 记忆复制（旧的还在）→
    /// crew 字段最后改。
    func testExecuteBacksUpThenCopiesMemoryAndSettings() throws {
        try write(claudeJSON, to: ".claude.json")
        let oldProj = home.appendingPathComponent(".claude/projects/-old").path
        let newProj = home.appendingPathComponent(".claude/projects/-new").path
        try write("mem", to: ".claude/projects/-old/memory/MEMORY.md")

        var plan = WorkdirMigrationPlan.Plan()
        plan.actions = [
            .copyClaudeProjectSettings(fromPath: "/old", toPath: "/new",
                                       keys: ["mcpServers"]),
            .copyClaudeMemoryFile(relativePath: "MEMORY.md",
                                  from: oldProj + "/memory/MEMORY.md",
                                  to: newProj + "/memory/MEMORY.md"),
            .setCrewWorkingDirectory(crewId: "c1", title: "本群", from: "/old", to: "/new"),
        ]
        let backup = home.appendingPathComponent("backup", isDirectory: true)
        var applied: [(String, String)] = []
        let receipt = WorkdirMigrationExecutor.execute(
            plan: plan, home: home, backupDirectory: backup,
            applyCrewWorkingDirectory: { applied.append(($0, $1)) })

        XCTAssertNil(receipt.failure, "不该失败：\(String(describing: receipt.failure))")
        let fm = FileManager.default
        XCTAssertTrue(fm.fileExists(atPath: backup.appendingPathComponent(".claude.json").path),
                      "改 claude.json 之前必须先有备份")
        XCTAssertTrue(fm.fileExists(atPath: newProj + "/memory/MEMORY.md"))
        XCTAssertTrue(fm.fileExists(atPath: oldProj + "/memory/MEMORY.md"),
                      "记忆是共享的，只准复制")
        XCTAssertEqual(applied.map(\.0), ["c1"])
        XCTAssertEqual(receipt.copiedMemoryFiles, ["MEMORY.md"])
        XCTAssertEqual(receipt.claudeSettingsKeysCopied, ["mcpServers"])
    }

    /// 中途炸了：停在那一步，**已经做完的照实报**，后面的不做。
    func testExecuteStopsAtFirstFailureAndReportsWhatWasDone() throws {
        try write(claudeJSON, to: ".claude.json")
        let oldProj = home.appendingPathComponent(".claude/projects/-old").path
        let newProj = home.appendingPathComponent(".claude/projects/-new").path
        try write("mem", to: ".claude/projects/-old/memory/a.md")

        var plan = WorkdirMigrationPlan.Plan()
        plan.actions = [
            .copyClaudeMemoryFile(relativePath: "a.md",
                                  from: oldProj + "/memory/a.md", to: newProj + "/memory/a.md"),
            // 源不存在 → copyItem 抛
            .copyClaudeMemoryFile(relativePath: "b.md",
                                  from: oldProj + "/memory/b.md", to: newProj + "/memory/b.md"),
            .setCrewWorkingDirectory(crewId: "c1", title: "本群", from: "/old", to: "/new"),
        ]
        var applied: [(String, String)] = []
        let receipt = WorkdirMigrationExecutor.execute(
            plan: plan, home: home, backupDirectory: home.appendingPathComponent("backup"),
            applyCrewWorkingDirectory: { applied.append(($0, $1)) })

        XCTAssertEqual(receipt.copiedMemoryFiles, ["a.md"])
        XCTAssertEqual(receipt.failure?.step, "复制记忆文件 b.md")
        XCTAssertTrue(applied.isEmpty, "炸了就不许再改 crew 字段（crew 得还指着旧目录）")
        XCTAssertTrue(WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
            .contains("中途停了"))
    }

    /// 有 blocker 一律不执行（UI 该拦住，执行层再拦一道）。
    func testExecuteRefusesWhenPlanHasBlockers() {
        var plan = WorkdirMigrationPlan.Plan()
        plan.blockers = [.newWorkdirMissing("/nope")]
        plan.actions = [.setCrewWorkingDirectory(crewId: "c1", title: "本群", from: nil, to: "/new")]
        var applied = 0
        let receipt = WorkdirMigrationExecutor.execute(
            plan: plan, home: home, backupDirectory: home.appendingPathComponent("backup"),
            applyCrewWorkingDirectory: { _, _ in applied += 1 })
        XCTAssertEqual(receipt.failure?.step, "预检")
        XCTAssertEqual(applied, 0)
    }

    // MARK: - 写回校验（~/.claude.json 是 claude 自己也在写的文件）

    /// 正常路径：写完读回来确认，键真的落住了。
    func testVerifiedCopyConfirmsKeysLanded() throws {
        try write(claudeJSON, to: ".claude.json")
        let (confirmed, tries) = try WorkdirMigrationExecutor.copyClaudeProjectSettingsVerified(
            home: home, from: "/old", to: "/new", keys: ["mcpServers"])
        XCTAssertEqual(confirmed, ["mcpServers"])
        XCTAssertEqual(tries, 1)
    }

    /// 源那边这个键本来就是空的 → 写不进去也确认不了。**不许当成功**：
    /// 返回的 confirmed 里没有它，调用方据此往回执里写「没落住」。
    func testVerifiedCopyReportsKeysThatNeverLanded() throws {
        try write(#"{"projects":{"/old":{"allowedTools":[]},"/new":{}}}"#,
                  to: ".claude.json")
        let (confirmed, tries) = try WorkdirMigrationExecutor.copyClaudeProjectSettingsVerified(
            home: home, from: "/old", to: "/new",
            keys: ["allowedTools"], attempts: 2, waitBetween: 0)
        XCTAssertTrue(confirmed.isEmpty)
        XCTAssertEqual(tries, 2, "没落住要重试，不是写一次就算完")
    }

    /// 没落住 → 回执必须出现 ⚠️ 并把「这几项要重新授权」说出来。
    func testExecuteWarnsWhenSettingsKeyDidNotLand() throws {
        try write(#"{"projects":{"/old":{"allowedTools":[]},"/new":{}}}"#,
                  to: ".claude.json")
        var plan = WorkdirMigrationPlan.Plan()
        plan.actions = [.copyClaudeProjectSettings(
            fromPath: "/old", toPath: "/new", keys: ["allowedTools"])]
        let receipt = WorkdirMigrationExecutor.execute(
            plan: plan, home: home, backupDirectory: home.appendingPathComponent("backup"),
            applyCrewWorkingDirectory: { _, _ in })
        XCTAssertNil(receipt.failure)
        XCTAssertTrue(receipt.claudeSettingsKeysCopied.isEmpty)
        XCTAssertEqual(receipt.warnings.count, 1)
        let text = WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
        XCTAssertTrue(text.contains("没落住"))
        XCTAssertTrue(text.contains("重新授权"))
    }

    // MARK: - 预览文案

    /// 预览必须说清「还没动手」「怎么才算真执行」，以及生效边界。
    func testPreviewTextSaysNothingHappenedYet() {
        var plan = WorkdirMigrationPlan.Plan()
        plan.crews = [.init(id: "c1", title: "本群")]
        plan.actions = [.setCrewWorkingDirectory(crewId: "c1", title: "本群", from: "/old", to: "/new")]
        let text = WorkdirMigrationExecutor.previewText(plan, newWorkdir: "/new")
        XCTAssertTrue(text.contains("还没动手"))
        XCTAssertTrue(text.contains("confirm"))
        XCTAssertTrue(text.contains("生效边界"))
        XCTAssertFalse(text.contains("清扫"), "清扫模式已经删了，预览不该还提它：\(text)")
    }

    /// 有人在干活 → 预览要点名，并说明现在不能执行。
    func testPreviewTextNamesBusySessions() {
        var plan = WorkdirMigrationPlan.Plan()
        plan.blockers = [.sessionsBusy([
            .init(crewId: "c1", sessionId: "s1", displayName: "打杂的", isWorking: true)])]
        let text = WorkdirMigrationExecutor.previewText(plan, newWorkdir: "/new")
        XCTAssertTrue(text.contains("打杂的"))
        XCTAssertTrue(text.contains("现在不能执行"))
    }

    /// 无事可做时不能显示成「可以执行」。
    func testPreviewTextSaysNothingToDo() {
        let text = WorkdirMigrationExecutor.previewText(
            WorkdirMigrationPlan.Plan(), newWorkdir: "/new")
        XCTAssertTrue(text.contains("没有要做的动作"))
    }

    // MARK: - 回执

    /// 撞名跳过的必须出现在回执里 —— 不然人以为记忆全复制过去了。
    func testReceiptTextSurfacesSkippedItems() {
        var receipt = WorkdirMigrationExecutor.Receipt(backupDirectory: "/b")
        receipt.copiedMemoryFiles = ["b.md"]
        receipt.skips = [
            .memoryTargetExists(relativePath: "a.md", path: "/y"),
            .crewAlreadyAtNewWorkdir(crewId: "c9", title: "已经在新目录的"),
        ]
        let text = WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
        XCTAssertTrue(text.contains("a.md"))
        XCTAssertFalse(text.contains("已经在新目录的"), "本来就不用做的不进回执，别刷屏")
        XCTAssertTrue(text.contains("/b"), "备份位置要写清楚")
        XCTAssertTrue(text.contains("生效边界"), "别让人以为点完当场全员换了目录")
    }

    /// 回执里**不许再出现「留待清扫 / 再调一次」这类话** —— 会话日志不搬了，
    /// 没有尾巴可补；留着那句话等于给机长一条指向虚空的指令（它照做，什么也不会发生）。
    func testReceiptTextNeverPromisesASweep() {
        var receipt = WorkdirMigrationExecutor.Receipt(backupDirectory: "/b")
        receipt.crewsUpdated = [.init(id: "c1", title: "本群")]
        let text = WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
        for word in ["留待清扫", "再调一次", "清扫完成", "会话"] {
            XCTAssertFalse(text.contains(word), "回执里不该再出现「\(word)」：\(text)")
        }
    }
}
