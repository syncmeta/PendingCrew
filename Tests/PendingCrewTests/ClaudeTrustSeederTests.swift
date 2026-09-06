import XCTest

/// 执行层：真的去改一份 `~/.claude.json`。跑**真文件系统**（tmp 下一个假 home）——
/// 要钉的正是「会不会把别人的条目改坏 / 写前备没备份 / 没落住会不会静默当成功」，
/// 这几件事只有真读真写才算数。
///
/// ⚠️ 这里所有用例都只碰自己造的临时 home，**一个字都不会写到真实的 `~/.claude.json`**。
final class ClaudeTrustSeederTests: XCTestCase {

    private var home: URL!
    private var backups: URL!

    override func setUpWithError() throws {
        let root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("claude-trust-seed-" + UUID().uuidString, isDirectory: true)
        home = root.appendingPathComponent("home", isDirectory: true)
        backups = root.appendingPathComponent("backups/seed", isDirectory: true)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: home.deletingLastPathComponent())
    }

    private var claudeJSONURL: URL { home.appendingPathComponent(".claude.json") }

    private func writeClaudeJSON(_ text: String) throws {
        try Data(text.utf8).write(to: claudeJSONURL)
    }

    private func projects() throws -> [String: [String: Any]] {
        let root = try JSONSerialization.jsonObject(
            with: Data(contentsOf: claudeJSONURL)) as! [String: Any]
        var out: [String: [String: Any]] = [:]
        for (k, v) in (root["projects"] as? [String: Any]) ?? [:] {
            out[k] = v as? [String: Any]
        }
        return out
    }

    private func rootObject() throws -> [String: Any] {
        try JSONSerialization.jsonObject(with: Data(contentsOf: claudeJSONURL)) as! [String: Any]
    }

    @discardableResult
    private func seed(_ workdir: String,
                      _ authorization: ClaudeTrustSeedPlan.Authorization = .granted,
                      io: ClaudeTrustSeeder.IO? = nil) -> ClaudeTrustSeeder.Receipt {
        ClaudeTrustSeeder.seed(workdir: workdir, authorization: authorization,
                               home: home, backupDirectory: backups, io: io,
                               attempts: 2, waitBetween: 0)
    }

    /// 一份「已经有别人条目」的真实形状（值照 2026-09-06 现场：条目在、信任位 false）。
    private let existing = """
    {
      "numStartups": 400,
      "oauthAccount": {"accountUuid": "abc"},
      "projects": {
        "/Users/x/dev": {"hasTrustDialogAccepted": true, "allowedTools": ["Bash(ls:*)"]},
        "/Users/x/CrewGround/Bhadrak": {
          "hasTrustDialogAccepted": false,
          "allowedTools": [],
          "exampleFiles": []
        }
      }
    }
    """

    // MARK: - 补种

    /// 全新目录（`projects` 里没有条目）→ 建条目 + 写信任位，别人的东西一个字不动。
    func testSeedsTrustBitForBrandNewDirectory() throws {
        try writeClaudeJSON(existing)
        let receipt = seed("/Users/x/CrewGround/Lisbon")

        XCTAssertTrue(receipt.succeeded, receipt.failure ?? "")
        XCTAssertEqual(receipt.seededKeys, ["hasTrustDialogAccepted"])
        XCTAssertTrue(receipt.warnings.isEmpty, "落住了就不该有警告：\(receipt.warnings)")

        let p = try projects()
        XCTAssertEqual(p["/Users/x/CrewGround/Lisbon"]?["hasTrustDialogAccepted"] as? Bool, true)
        XCTAssertEqual(p["/Users/x/dev"]?["allowedTools"] as? [String], ["Bash(ls:*)"],
                       "别人的条目一个字都不许动")
        XCTAssertEqual(try rootObject()["numStartups"] as? Int, 400,
                       "projects 以外的设置要原样留着")
    }

    /// **踩坑现场**：条目在、信任位是 `false` → 翻成 `true`，同条目里别的键不动。
    func testSeedsOverExistingFalseTrustBitWithoutTouchingSiblingKeys() throws {
        try writeClaudeJSON(existing)
        let receipt = seed("/Users/x/CrewGround/Bhadrak")

        XCTAssertEqual(receipt.seededKeys, ["hasTrustDialogAccepted"])
        let entry = try projects()["/Users/x/CrewGround/Bhadrak"]
        XCTAssertEqual(entry?["hasTrustDialogAccepted"] as? Bool, true)
        XCTAssertEqual(entry?["allowedTools"] as? [String], [], "工具权限不许被顺手写点什么进去")
        XCTAssertNotNil(entry?["exampleFiles"], "同条目里的其它键要原样留着")
    }

    /// 尾斜杠归一 —— claude 按路径字面量分家，`/a/b/` 写成第二个条目等于白补。
    func testSeedsNormalizedPath() throws {
        try writeClaudeJSON(existing)
        seed("/Users/x/CrewGround/Cypress/")
        let p = try projects()
        XCTAssertEqual(p["/Users/x/CrewGround/Cypress"]?["hasTrustDialogAccepted"] as? Bool, true)
        XCTAssertNil(p["/Users/x/CrewGround/Cypress/"], "别造出一个带尾斜杠的第二条目")
    }

    /// 已经信任过 → 不写、不备份，文件逐字节不动。
    func testAlreadyTrustedLeavesFileUntouched() throws {
        try writeClaudeJSON(existing)
        let before = try Data(contentsOf: claudeJSONURL)
        let receipt = seed("/Users/x/dev")

        XCTAssertEqual(receipt.skip, .alreadyTrusted(path: "/Users/x/dev"))
        XCTAssertTrue(receipt.seededKeys.isEmpty)
        XCTAssertEqual(try Data(contentsOf: claudeJSONURL), before)
        XCTAssertFalse(FileManager.default.fileExists(atPath: backups.path),
                       "什么都没做就不该留下备份目录")
    }

    // MARK: - 授权闸

    /// **没授权 = 一个字都不写。** 这一条是红线：替人写信任位等于替人做了他没授权的事。
    func testWritesNothingWithoutAuthorization() throws {
        try writeClaudeJSON(existing)
        let before = try Data(contentsOf: claudeJSONURL)
        let receipt = seed("/Users/x/CrewGround/Lisbon", .notGranted)

        XCTAssertEqual(receipt.skip, .notAuthorized(path: "/Users/x/CrewGround/Lisbon"))
        XCTAssertTrue(receipt.seededKeys.isEmpty)
        XCTAssertEqual(try Data(contentsOf: claudeJSONURL), before, "文件必须逐字节不动")
        XCTAssertFalse(FileManager.default.fileExists(atPath: backups.path))
    }

    // MARK: - 写前备份 / fail-loud

    /// 动 `~/.claude.json` 之前先整份备份 —— 撞了别的 claude 进程还能原样找回。
    func testBacksUpClaudeJSONBeforeWriting() throws {
        try writeClaudeJSON(existing)
        let before = try Data(contentsOf: claudeJSONURL)
        let receipt = seed("/Users/x/CrewGround/Lisbon")

        XCTAssertEqual(receipt.backupPath, backups.path)
        let copied = backups.appendingPathComponent(".claude.json")
        XCTAssertEqual(try Data(contentsOf: copied), before, "备份必须是改动**之前**那一份")
    }

    /// 备份不成 → 一步都不走（备份目录的位置被一个普通文件占住）。
    func testDoesNotWriteWhenBackupFails() throws {
        try writeClaudeJSON(existing)
        try FileManager.default.createDirectory(
            at: backups.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("not a directory".utf8).write(to: backups)
        let before = try Data(contentsOf: claudeJSONURL)

        let receipt = seed("/Users/x/CrewGround/Lisbon")
        XCTAssertFalse(receipt.succeeded, "备份失败必须报失败，不许接着写")
        XCTAssertTrue(receipt.seededKeys.isEmpty)
        XCTAssertEqual(try Data(contentsOf: claudeJSONURL), before)
    }

    /// `~/.claude.json` 压根不存在（claude 还没在这台机器上跑过）→ 如实报失败，
    /// **不替 claude 造一份**（那份文件里还有账号等东西，不是我们该造的）。
    func testFailsLoudWhenClaudeJSONMissing() {
        let receipt = seed("/Users/x/CrewGround/Lisbon")
        XCTAssertFalse(receipt.succeeded)
        XCTAssertFalse(FileManager.default.fileExists(atPath: claudeJSONURL.path),
                       "不许凭空造一份 ~/.claude.json")
    }

    /// 文件在、但还没有 `projects` 表（装了 claude、没进过任何项目）→ 建表，别拒绝。
    /// 这是与迁移那边**故意的不同**：迁移要从源条目里取值，没有表就没有源；
    /// 补种没有源，没有表只说明「还没有任何目录被记过」。
    func testCreatesProjectsTableWhenAbsent() throws {
        try writeClaudeJSON(#"{"numStartups": 1}"#)
        let receipt = seed("/Users/x/CrewGround/Lisbon")

        XCTAssertTrue(receipt.succeeded, receipt.failure ?? "")
        XCTAssertEqual(try projects()["/Users/x/CrewGround/Lisbon"]?["hasTrustDialogAccepted"] as? Bool,
                       true)
        XCTAssertEqual(try rootObject()["numStartups"] as? Int, 1)
    }

    /// 原文件是 600（里面有 oauth 账号）。原子替换会带默认权限 —— 必须按原样恢复，
    /// 别把凭证类文件放宽。
    func testPreservesFilePermissions() throws {
        try writeClaudeJSON(existing)
        try FileManager.default.setAttributes([.posixPermissions: 0o600],
                                              ofItemAtPath: claudeJSONURL.path)
        seed("/Users/x/CrewGround/Lisbon")
        let perms = try FileManager.default
            .attributesOfItem(atPath: claudeJSONURL.path)[.posixPermissions] as? NSNumber
        XCTAssertEqual(perms?.int16Value, 0o600)
    }

    // MARK: - 回读校验

    /// **写完要回读。** `~/.claude.json` 是全机 claude 共写的一份，我们刚写进去的
    /// 真可能被别的进程覆盖回去。这里模拟「写了、但立刻被覆盖回原样」：
    /// 回执必须出警告，**不许静默当成功**。
    func testWarnsWhenWriteDoesNotStick() throws {
        try writeClaudeJSON(existing)
        let original = try Data(contentsOf: claudeJSONURL)
        // 别的 claude 进程的样子：我们写完，它立刻用自己内存里那份覆盖回去。
        let clobbering = ClaudeTrustSeeder.IO(
            read: { try Data(contentsOf: self.claudeJSONURL) },
            write: { _ in try original.write(to: self.claudeJSONURL, options: .atomic) })

        let receipt = seed("/Users/x/CrewGround/Lisbon", .granted, io: clobbering)

        XCTAssertTrue(receipt.seededKeys.isEmpty, "没落住就不许算作补上了")
        XCTAssertFalse(receipt.warnings.isEmpty, "没落住必须出警告")
        XCTAssertNil(try projects()["/Users/x/CrewGround/Lisbon"])
    }

    // MARK: - 回执文案（警告得有地方说出去）

    /// 补上了 → 说清补的是哪个目录。
    func testReceiptTextSaysWhatLanded() throws {
        try writeClaudeJSON(existing)
        let text = ClaudeTrustSeeder.receiptText(seed("/Users/x/CrewGround/Lisbon"))
        XCTAssertNotNil(text)
        XCTAssertTrue(text?.contains("/Users/x/CrewGround/Lisbon") == true, text ?? "")
        XCTAssertTrue(text?.contains("信任") == true, text ?? "")
    }

    /// **没落住的时候，回执不许读起来像成功。** 静默当成功正是今天这条 P0 最贵的部分。
    func testReceiptTextCarriesWarningInsteadOfClaimingSuccess() throws {
        try writeClaudeJSON(existing)
        let original = try Data(contentsOf: claudeJSONURL)
        let clobbering = ClaudeTrustSeeder.IO(
            read: { try Data(contentsOf: self.claudeJSONURL) },
            write: { _ in try original.write(to: self.claudeJSONURL, options: .atomic) })
        let text = ClaudeTrustSeeder.receiptText(
            seed("/Users/x/CrewGround/Lisbon", .granted, io: clobbering))

        XCTAssertTrue(text?.contains("没落住") == true, text ?? "")
        XCTAssertFalse(text?.contains("已补上") == true, "没落住不许说「已补上」：\(text ?? "")")
    }

    /// 失败要说出来。
    func testReceiptTextSaysFailure() {
        let text = ClaudeTrustSeeder.receiptText(seed("/Users/x/CrewGround/Lisbon"))
        XCTAssertTrue(text?.contains("读不到") == true, text ?? "")
    }

    /// 什么都没做（本来就信任过 / 没授权）→ 没什么可说的，别往群里刷屏。
    func testReceiptTextIsNilWhenNothingWorthSaying() throws {
        try writeClaudeJSON(existing)
        XCTAssertNil(ClaudeTrustSeeder.receiptText(seed("/Users/x/dev")))
        XCTAssertNil(ClaudeTrustSeeder.receiptText(
            seed("/Users/x/CrewGround/Lisbon", .notGranted)))
    }

    // MARK: - 建 crew 那条路的一次调用

    /// 备份落在数据根下、按时间戳命名（跟迁移那条路同一个口径）。
    func testSeedForNewCrewBacksUpUnderDataRoot() throws {
        try writeClaudeJSON(existing)
        let dataRoot = home.deletingLastPathComponent()
            .appendingPathComponent("data", isDirectory: true)
        let receipt = ClaudeTrustSeeder.seedForNewCrew(
            workdir: "/Users/x/CrewGround/Lisbon", authorization: .granted,
            home: home, dataRoot: dataRoot)

        XCTAssertEqual(receipt.seededKeys, ["hasTrustDialogAccepted"])
        let backupRoot = dataRoot.appendingPathComponent("backups")
        let made = try FileManager.default.contentsOfDirectory(atPath: backupRoot.path)
        XCTAssertEqual(made.count, 1, "应当只建一个带时间戳的备份目录：\(made)")
        XCTAssertTrue(made[0].hasPrefix("claude-trust-seed-"), made[0])
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: backupRoot.appendingPathComponent(made[0] + "/.claude.json").path))
    }

    /// 没授权时这条路也是一个字不写、连备份目录都不建。
    func testSeedForNewCrewWritesNothingWithoutAuthorization() throws {
        try writeClaudeJSON(existing)
        let before = try Data(contentsOf: claudeJSONURL)
        let dataRoot = home.deletingLastPathComponent()
            .appendingPathComponent("data", isDirectory: true)
        let receipt = ClaudeTrustSeeder.seedForNewCrew(
            workdir: "/Users/x/CrewGround/Lisbon", authorization: .notGranted,
            home: home, dataRoot: dataRoot)

        XCTAssertEqual(receipt.skip, .notAuthorized(path: "/Users/x/CrewGround/Lisbon"))
        XCTAssertEqual(try Data(contentsOf: claudeJSONURL), before)
        XCTAssertFalse(FileManager.default.fileExists(atPath: dataRoot.path))
    }
}
