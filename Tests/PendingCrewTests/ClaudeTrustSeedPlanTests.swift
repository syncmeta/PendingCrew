import XCTest

/// 规划层：**要不要给这个目录补种 claude 的信任位、补哪个键**。
///
/// 钉的是踩坑现场的形状：`~/.claude.json` 里条目**在**、`hasTrustDialogAccepted`
/// 是 `false`（不是缺失）—— 2026-09-06 全机 21 条这样的路径里有 8 条是
/// `~/CrewGround/<地名>`，它们下面新起的 claude 全停在信任框上、外面看着像「一直空闲」。
final class ClaudeTrustSeedPlanTests: XCTestCase {

    private func settings(exists: Bool,
                          meaningful: Set<String> = []) -> WorkdirMigrationPlan.ClaudeProjectSettings {
        WorkdirMigrationPlan.ClaudeProjectSettings(exists: exists, meaningfulKeys: meaningful)
    }

    private func decide(_ path: String,
                        _ authorization: ClaudeTrustSeedPlan.Authorization,
                        _ existing: WorkdirMigrationPlan.ClaudeProjectSettings)
        -> ClaudeTrustSeedPlan.Decision {
        ClaudeTrustSeedPlan.decide(.init(workdir: path, authorization: authorization,
                                         existing: existing))
    }

    /// 全新目录：`projects` 里压根没有条目 —— 要补。
    func testSeedsWhenProjectEntryMissing() {
        XCTAssertEqual(decide("/Users/x/CrewGround/Lisbon", .granted, settings(exists: false)),
                       .seed(path: "/Users/x/CrewGround/Lisbon",
                             keys: ["hasTrustDialogAccepted"]))
    }

    /// **踩坑现场的形状**：条目已经在了、别的键也有，就是信任位是 `false`。
    /// 「已存在就跳过」会把人卡在信任框上，所以这一条必须也补。
    func testSeedsWhenEntryExistsButTrustBitIsFalse() {
        let existing = settings(exists: true, meaningful: [])
        XCTAssertEqual(decide("/Users/x/CrewGround/Bhadrak", .granted, existing),
                       .seed(path: "/Users/x/CrewGround/Bhadrak",
                             keys: ["hasTrustDialogAccepted"]))
    }

    /// 已经信任过 → 什么都不做（连授权都不必问）。
    func testSkipsWhenAlreadyTrusted() {
        let existing = settings(exists: true, meaningful: ["hasTrustDialogAccepted"])
        XCTAssertEqual(decide("/Users/x/dev", .granted, existing),
                       .skip(.alreadyTrusted(path: "/Users/x/dev")))
        XCTAssertEqual(decide("/Users/x/dev", .notGranted, existing),
                       .skip(.alreadyTrusted(path: "/Users/x/dev")),
                       "没授权也一样是「本来就不用做」，别报成「没授权」")
    }

    /// **授权是硬闸**：没拿到人的授权，一个字都不写。
    /// 「什么动作算授权」是 UI 层的事（人类 Todo #3），这一层只认 granted / notGranted。
    func testSkipsWhenNotAuthorized() {
        XCTAssertEqual(decide("/Users/x/CrewGround/Brick", .notGranted, settings(exists: false)),
                       .skip(.notAuthorized(path: "/Users/x/CrewGround/Brick")))
    }

    func testSkipsEmptyWorkdir() {
        XCTAssertEqual(decide("   ", .granted, settings(exists: false)), .skip(.emptyWorkdir))
    }

    /// claude 按**路径字面量**给 `projects` 分家 —— `/a/b` 和 `/a/b/` 是两个条目。
    /// 补种前必须归一，跟迁移那边同一把尺子。
    func testNormalizesPathBeforeSeeding() {
        XCTAssertEqual(decide("/Users/x/CrewGround/Cypress/", .granted, settings(exists: false)),
                       .seed(path: "/Users/x/CrewGround/Cypress",
                             keys: ["hasTrustDialogAccepted"]))
    }

    /// **只补信任位**。迁移那边搬 8 个键是因为源目录真有那些值；建 crew 这条路
    /// 没有源，凭空写 `allowedTools` / `mcpServers` 等于替人放行工具 —— 绝不做。
    func testSeededKeysAreOnlyTheTrustBit() {
        XCTAssertEqual(ClaudeTrustSeedPlan.seededKeys, ["hasTrustDialogAccepted"])
        XCTAssertTrue(WorkdirMigrationPlan.claudeSettingsKeys
            .contains(ClaudeTrustSeedPlan.seededKeys[0]),
            "补的键必须是迁移那份名单里的同一个，别自己造第二个键名")
    }
}
