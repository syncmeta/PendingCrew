import XCTest

/// **产品一个信任键都不写** —— 这一整个文件存在的理由，是把「不写」从
/// 「这一刻代码里恰好没有」变成「以后也不许有」。
///
/// 信任的单位是路径，那是 claude / codex 两家定的规矩，不是我们定的。人对 `/a` 点的
/// 那一下头，我们复制到 `/b`，`/b` 那份授权就是我们签的，不是他签的 ——「搬」这个动词
/// 听起来像守恒，其实凭空多了一份。
///
/// 为什么光把代码删掉不够：**「显式选择不做」和「不小心漏了」在代码上长得一模一样，
/// 在半年后长得完全不一样。** 删掉只在这一刻为真；这几条断言才让它以后也为真，
/// 并且说明白这是我们选的。
final class TrustNeverWrittenTests: XCTestCase {

    private var home: URL!
    private var oldDir: String!
    private var newDir: String!

    override func setUpWithError() throws {
        home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("trust-never-written-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        oldDir = home.appendingPathComponent("old", isDirectory: true).path
        newDir = home.appendingPathComponent("new", isDirectory: true).path
        for d in [oldDir!, newDir!] {
            try FileManager.default.createDirectory(atPath: d, withIntermediateDirectories: true)
        }
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

    // MARK: - 迁移那条路

    /// 端到端（真文件 → `make` → `execute` → 回读真文件）：迁完之后
    /// **新路径的 claude 信任位仍然没有**，而别的权限照旧搬过去了。
    ///
    /// 两句都要：只断言「没搬信任位」，一个把整条迁移搞坏的改动也能让它绿。
    func test_迁移之后新路径的claude信任位保持未写() throws {
        try write("""
        {"projects": {"\(oldDir!)": {
            "hasTrustDialogAccepted": true,
            "allowedTools": ["Bash(ls:*)"]
        }}}
        """, to: ".claude.json")

        let receipt = try migrate()
        XCTAssertNil(receipt.failure, "不该失败：\(String(describing: receipt.failure))")

        let entry = try newClaudeEntry()
        XCTAssertNil(entry["hasTrustDialogAccepted"],
                     "迁移不许给新路径写信任位 —— 那是人对这个目录的授权，不是我们的技术步骤")
        XCTAssertNotNil(entry["allowedTools"],
                        "别的权限照旧要搬，否则这条断言用一个「迁移整个坏掉」也能骗绿")
    }

    /// 即使**调用方点名要**这个键，执行层也不写。挡在最里面那一层：
    /// 规划层将来被谁改回去，也漏不出去。
    func test_即使点名要信任键执行层也不写() throws {
        try write("""
        {"projects": {"\(oldDir!)": {
            "hasTrustDialogAccepted": true,
            "allowedTools": ["Bash(ls:*)"]
        }}}
        """, to: ".claude.json")

        try WorkdirMigrationExecutor.copyClaudeProjectSettings(
            home: home, from: oldDir, to: newDir,
            keys: ["hasTrustDialogAccepted", "allowedTools"])

        let entry = try newClaudeEntry()
        XCTAssertNil(entry["hasTrustDialogAccepted"], "点名要也不给")
        XCTAssertNotNil(entry["allowedTools"])
    }

    /// codex 那半：迁移**一个字节都不碰** `~/.codex/config.toml`。
    func test_迁移全程不碰codex的config_toml() throws {
        try write("""
        {"projects": {"\(oldDir!)": {"allowedTools": ["Bash(ls:*)"]}}}
        """, to: ".claude.json")
        let toml = """
        model = "gpt-5"

        [projects."\(oldDir!)"]
        trust_level = "trusted"

        """
        try write(toml, to: ".codex/config.toml")
        let before = try Data(contentsOf: home.appendingPathComponent(".codex/config.toml"))

        let receipt = try migrate()
        XCTAssertNil(receipt.failure, "不该失败：\(String(describing: receipt.failure))")

        let after = try Data(contentsOf: home.appendingPathComponent(".codex/config.toml"))
        XCTAssertEqual(after, before, "config.toml 必须逐字节不动")
    }

    /// 信任键**不在**迁移会搬的键名单里 —— 名单本身就是这条选择的落点。
    func test_信任键不在迁移的键名单里() {
        XCTAssertFalse(
            WorkdirMigrationPlan.claudeSettingsKeys.contains("hasTrustDialogAccepted"),
            "它不该在这张名单里；要挡的原因见 neverWrittenClaudeKeys")
        XCTAssertTrue(
            WorkdirMigrationPlan.neverWrittenClaudeKeys.contains("hasTrustDialogAccepted"),
            "而且要显式写在「永不写」那张名单里 —— 让后来的人看得出这是选的，不是漏的")
    }

    // MARK: - 建 crew 那条路

    /// 建 crew 那条路上**不存在**任何写信任位的代码：补种器整套已经删掉，
    /// sheet 里也没有它的调用。
    ///
    /// 这条是源码层的尺子，因为那条路挂在 SwiftUI 的 `submit()` 里、还要连网建 crew，
    /// 跑不动 —— 但「那套写入器还在不在」是它能不能写的**前提**，这一条盖得住。
    func test_建crew那条路上没有写信任位的代码() throws {
        let root = repoRoot()
        let fm = FileManager.default
        for gone in ["Sources/Mac/LocalRunner/ClaudeTrustSeeder.swift",
                     "Sources/Mac/LocalRunner/ClaudeTrustSeedPlan.swift"] {
            XCTAssertFalse(fm.fileExists(atPath: root.appendingPathComponent(gone).path),
                           "整套补种器应已删除（人类否掉的是这个能力本身）：\(gone)")
        }
        let sheet = try String(
            contentsOf: root.appendingPathComponent("Sources/Mac/Views/CreateCrewSheet.swift"),
            encoding: .utf8)
        XCTAssertFalse(sheet.contains("ClaudeTrustSeeder"), "建 crew 不该再补种信任位")
        XCTAssertFalse(sheet.contains("hasTrustDialogAccepted"), "建 crew 这条路不写这个键")
    }

    // MARK: - 小工具

    private func repoRoot() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // PendingCrewTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // repo root
    }

    /// 真 probe → 真 plan → 真 execute。中间不喂任何我自己捏的判定。
    private func migrate() throws -> WorkdirMigrationExecutor.Receipt {
        let inputs = WorkdirMigrationPlan.Inputs(
            crews: [.init(id: "c1", title: "本群", workingDirectory: oldDir, parentCrewIds: [])],
            rootCrewId: "c1", selectedCrewIds: ["c1"], newWorkdir: newDir,
            runningSessions: [], callerSessionId: nil, home: home)
        let plan = WorkdirMigrationPlan.make(
            inputs, probe: WorkdirMigrationExecutor.probe(home: home))
        XCTAssertTrue(plan.isExecutable, "计划应当可执行：\(plan.blockers)")
        return WorkdirMigrationExecutor.execute(
            plan: plan, home: home,
            backupDirectory: home.appendingPathComponent("backup", isDirectory: true),
            applyCrewWorkingDirectory: { _, _ in })
    }

    private func newClaudeEntry() throws -> [String: Any] {
        let data = try Data(contentsOf: home.appendingPathComponent(".claude.json"))
        let root = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        return (root?["projects"] as? [String: Any])?[newDir] as? [String: Any] ?? [:]
    }
}
