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
          "hasTrustDialogAccepted": true,
          "allowedTools": ["Bash(ls:*)"],
          "lastCost": 1.5
        },
        "/new": {
          "hasTrustDialogAccepted": false,
          "allowedTools": ["Bash(git:*)"]
        },
        "/somebody-else": { "hasTrustDialogAccepted": true }
      }
    }
    """

    /// 只补点名的键；目标已有实质值的**不覆盖**；旧条目和别人的条目原样留着；
    /// 统计类字段（lastCost）不跟着搬。
    func testCopyClaudeProjectSettingsOnlyFillsRequestedGaps() throws {
        try write(claudeJSON, to: ".claude.json")
        try WorkdirMigrationExecutor.copyClaudeProjectSettings(
            home: home, from: "/old", to: "/new",
            keys: ["hasTrustDialogAccepted", "allowedTools"])

        let root = try JSONSerialization.jsonObject(
            with: Data(read(".claude.json").utf8)) as! [String: Any]
        let projects = root["projects"] as! [String: Any]
        let new = projects["/new"] as! [String: Any]
        XCTAssertEqual(new["hasTrustDialogAccepted"] as? Bool, true, "false 应当被源的 true 补上")
        XCTAssertEqual(new["allowedTools"] as? [String], ["Bash(git:*)"], "目标已有的不许被覆盖")
        XCTAssertNil(new["lastCost"], "统计字段不该跟着搬")

        let old = projects["/old"] as! [String: Any]
        XCTAssertEqual(old["hasTrustDialogAccepted"] as? Bool, true, "旧条目要原样留着")
        XCTAssertNotNil(projects["/somebody-else"], "别人的条目一个字都不许动")
        XCTAssertEqual(root["numStartups"] as? Int, 400, "projects 以外的设置要原样留着")
    }

    func testCopyClaudeProjectSettingsCreatesTargetEntryWhenAbsent() throws {
        try write(#"{"projects":{"/old":{"hasTrustDialogAccepted":true}}}"#, to: ".claude.json")
        try WorkdirMigrationExecutor.copyClaudeProjectSettings(
            home: home, from: "/old", to: "/new", keys: ["hasTrustDialogAccepted"])
        let root = try JSONSerialization.jsonObject(
            with: Data(read(".claude.json").utf8)) as! [String: Any]
        let new = (root["projects"] as! [String: Any])["/new"] as! [String: Any]
        XCTAssertEqual(new["hasTrustDialogAccepted"] as? Bool, true)
    }

    func testCopyClaudeProjectSettingsFailsLoudWhenSourceEntryGone() throws {
        try write(#"{"projects":{}}"#, to: ".claude.json")
        XCTAssertThrowsError(try WorkdirMigrationExecutor.copyClaudeProjectSettings(
            home: home, from: "/old", to: "/new", keys: ["hasTrustDialogAccepted"]))
    }

    // MARK: - ~/.codex/config.toml

    private let codexTOML = """
    model = "gpt-5.6-sol"

    [plugins."github@x"]
    enabled = true

    [projects."/old"]
    trust_level = "trusted"

    [mcp_servers.thing]
    command = "x"
    args = ["mcp"]
    """

    /// 补一条，别的一个字都不动 —— 而且改完必须还是**合法 TOML**、内容恰好多这一条。
    func testAddCodexTrustAddsEntryAndKeepsEverythingElse() throws {
        try write(codexTOML, to: ".codex/config.toml")
        try WorkdirMigrationExecutor.addCodexTrust(home: home, path: "/new", trustLevel: "trusted")

        let levels = WorkdirMigrationExecutor.loadCodexTrustLevels(home: home)
        XCTAssertEqual(levels["/new"], "trusted")
        XCTAssertEqual(levels["/old"], "trusted", "旧条目要留着（别的 crew 还在用旧目录）")

        let text = try read(".codex/config.toml")
        XCTAssertTrue(text.contains("gpt-5.6-sol"), "顶层设置不能丢")
        XCTAssertTrue(text.contains("mcp_servers"), "别的表不能丢")
        XCTAssertTrue(text.contains("github@x"), "带引号的表名不能丢")
    }

    /// 新路径已经有条目 → 拒绝改写，不覆盖用户已有的信任级别。
    func testAddCodexTrustRefusesToOverwrite() throws {
        try write(codexTOML + "\n[projects.\"/new\"]\ntrust_level = \"untrusted\"\n",
                  to: ".codex/config.toml")
        XCTAssertThrowsError(
            try WorkdirMigrationExecutor.addCodexTrust(home: home, path: "/new", trustLevel: "trusted"))
        XCTAssertEqual(WorkdirMigrationExecutor.loadCodexTrustLevels(home: home)["/new"], "untrusted")
    }

    func testLoadCodexTrustLevelsOnMissingFileIsEmpty() {
        XCTAssertTrue(WorkdirMigrationExecutor.loadCodexTrustLevels(home: home).isEmpty)
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

    /// 一条完整的成功路径：备份先落地 → 记忆复制（旧的还在）→ 会话移动（旧的没了）→
    /// crew 字段最后改。
    func testExecuteBacksUpThenCopiesMemoryAndMovesTranscripts() throws {
        try write(claudeJSON, to: ".claude.json")
        try write(codexTOML, to: ".codex/config.toml")
        let oldProj = home.appendingPathComponent(".claude/projects/-old").path
        let newProj = home.appendingPathComponent(".claude/projects/-new").path
        try write("session", to: ".claude/projects/-old/aaa.jsonl")
        try write("mem", to: ".claude/projects/-old/memory/MEMORY.md")

        var plan = WorkdirMigrationPlan.Plan()
        plan.actions = [
            .copyClaudeProjectSettings(fromPath: "/old", toPath: "/new",
                                       keys: ["hasTrustDialogAccepted"]),
            .copyCodexTrust(fromPath: "/old", toPath: "/new", trustLevel: "trusted"),
            .copyClaudeMemoryFile(relativePath: "MEMORY.md",
                                  from: oldProj + "/memory/MEMORY.md",
                                  to: newProj + "/memory/MEMORY.md"),
            .moveClaudeTranscript(agentSessionId: "aaa", memberName: "小明",
                                  from: oldProj + "/aaa.jsonl", to: newProj + "/aaa.jsonl"),
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
        XCTAssertTrue(fm.fileExists(atPath: backup.appendingPathComponent("config.toml").path))
        XCTAssertTrue(fm.fileExists(atPath: newProj + "/memory/MEMORY.md"))
        XCTAssertTrue(fm.fileExists(atPath: oldProj + "/memory/MEMORY.md"),
                      "记忆是共享的，只准复制")
        XCTAssertTrue(fm.fileExists(atPath: newProj + "/aaa.jsonl"))
        XCTAssertFalse(fm.fileExists(atPath: oldProj + "/aaa.jsonl"), "会话是移动")
        XCTAssertEqual(applied.map(\.0), ["c1"])
        XCTAssertEqual(receipt.movedTranscripts, ["aaa"])
        XCTAssertEqual(receipt.copiedMemoryFiles, ["MEMORY.md"])
        XCTAssertTrue(receipt.codexTrustCopied)
    }

    /// 中途炸了：停在那一步，**已经做完的照实报**，后面的不做。
    func testExecuteStopsAtFirstFailureAndReportsWhatWasDone() throws {
        try write(claudeJSON, to: ".claude.json")
        let oldProj = home.appendingPathComponent(".claude/projects/-old").path
        let newProj = home.appendingPathComponent(".claude/projects/-new").path
        try write("session", to: ".claude/projects/-old/aaa.jsonl")

        var plan = WorkdirMigrationPlan.Plan()
        plan.actions = [
            .moveClaudeTranscript(agentSessionId: "aaa", memberName: "小明",
                                  from: oldProj + "/aaa.jsonl", to: newProj + "/aaa.jsonl"),
            // 源不存在 → moveItem 抛
            .moveClaudeTranscript(agentSessionId: "bbb", memberName: "小红",
                                  from: oldProj + "/bbb.jsonl", to: newProj + "/bbb.jsonl"),
            .setCrewWorkingDirectory(crewId: "c1", title: "本群", from: "/old", to: "/new"),
        ]
        var applied: [(String, String)] = []
        let receipt = WorkdirMigrationExecutor.execute(
            plan: plan, home: home, backupDirectory: home.appendingPathComponent("backup"),
            applyCrewWorkingDirectory: { applied.append(($0, $1)) })

        XCTAssertEqual(receipt.movedTranscripts, ["aaa"])
        XCTAssertEqual(receipt.failure?.step, "搬「小红」的会话 bbb")
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
            home: home, from: "/old", to: "/new", keys: ["hasTrustDialogAccepted"])
        XCTAssertEqual(confirmed, ["hasTrustDialogAccepted"])
        XCTAssertEqual(tries, 1)
    }

    /// 源那边这个键本来就是空的 → 写不进去也确认不了。**不许当成功**：
    /// 返回的 confirmed 里没有它，调用方据此往回执里写「没落住」。
    func testVerifiedCopyReportsKeysThatNeverLanded() throws {
        try write(#"{"projects":{"/old":{"hasTrustDialogAccepted":false},"/new":{}}}"#,
                  to: ".claude.json")
        let (confirmed, tries) = try WorkdirMigrationExecutor.copyClaudeProjectSettingsVerified(
            home: home, from: "/old", to: "/new",
            keys: ["hasTrustDialogAccepted"], attempts: 2, waitBetween: 0)
        XCTAssertTrue(confirmed.isEmpty)
        XCTAssertEqual(tries, 2, "没落住要重试，不是写一次就算完")
    }

    /// 没落住 → 回执必须出现 ⚠️ 并把「可能要手点信任框」说出来。
    func testExecuteWarnsWhenTrustKeyDidNotLand() throws {
        try write(#"{"projects":{"/old":{"hasTrustDialogAccepted":false},"/new":{}}}"#,
                  to: ".claude.json")
        var plan = WorkdirMigrationPlan.Plan()
        plan.actions = [.copyClaudeProjectSettings(
            fromPath: "/old", toPath: "/new", keys: ["hasTrustDialogAccepted"])]
        let receipt = WorkdirMigrationExecutor.execute(
            plan: plan, home: home, backupDirectory: home.appendingPathComponent("backup"),
            applyCrewWorkingDirectory: { _, _ in })
        XCTAssertNil(receipt.failure)
        XCTAssertTrue(receipt.claudeSettingsKeysCopied.isEmpty)
        XCTAssertEqual(receipt.warnings.count, 1)
        let text = WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
        XCTAssertTrue(text.contains("没落住"))
        XCTAssertTrue(text.contains("信任"))
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

    /// 清扫模式的预览要自报家门，别让人以为又要整迁一遍。
    func testPreviewTextAnnouncesSweepMode() {
        var plan = WorkdirMigrationPlan.Plan()
        plan.isSweep = true
        plan.actions = [.moveClaudeTranscript(agentSessionId: "aaa", memberName: "小明",
                                              from: "/a", to: "/b")]
        let text = WorkdirMigrationExecutor.previewText(plan, newWorkdir: "/new")
        XCTAssertTrue(text.contains("清扫模式"))
    }

    /// 无事可做时不能显示成「可以执行」。
    func testPreviewTextSaysNothingToDo() {
        let text = WorkdirMigrationExecutor.previewText(
            WorkdirMigrationPlan.Plan(), newWorkdir: "/new")
        XCTAssertTrue(text.contains("没有要做的动作"))
    }

    // MARK: - 回执

    /// 撞名跳过的必须出现在回执里 —— 不然人以为全搬过去了。
    func testReceiptTextSurfacesSkippedItems() {
        var receipt = WorkdirMigrationExecutor.Receipt(backupDirectory: "/b")
        receipt.movedTranscripts = ["aaa"]
        receipt.skips = [
            .transcriptTargetExists(agentSessionId: "bbb", memberName: "小红", path: "/x"),
            .memoryTargetExists(relativePath: "a.md", path: "/y"),
            .codexSessionNeedsNoMove(agentSessionId: "th", memberName: "codex 的"),
        ]
        let text = WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
        XCTAssertTrue(text.contains("小红"))
        XCTAssertTrue(text.contains("a.md"))
        XCTAssertFalse(text.contains("codex 的"), "本来就不用搬的不进回执，别刷屏")
        XCTAssertTrue(text.contains("/b"), "备份位置要写清楚")
        XCTAssertTrue(text.contains("生效边界"), "别让人以为点完当场全员换了目录")
    }

    /// 「留待清扫」的成员要点名，并告诉人怎么收尾（再调一次）。
    func testReceiptTextExplainsPendingSweep() {
        var receipt = WorkdirMigrationExecutor.Receipt(backupDirectory: "/b")
        receipt.skips = [
            .sessionStillLive(agentSessionId: "aaa", memberName: "机长"),
            .sessionStillLive(agentSessionId: "bbb", memberName: "打杂的"),
        ]
        let text = WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
        XCTAssertTrue(text.contains("留待清扫"))
        XCTAssertTrue(text.contains("机长"))
        XCTAssertTrue(text.contains("打杂的"))
        XCTAssertTrue(text.contains("再调一次"))
    }

    func testReceiptTextMarksSweepRun() {
        var receipt = WorkdirMigrationExecutor.Receipt(backupDirectory: "/b")
        receipt.isSweep = true
        XCTAssertTrue(WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: "/new")
            .contains("清扫完成"))
    }
}
