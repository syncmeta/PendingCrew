import XCTest
import Foundation

/// 总机组的工作目录：判据 + 它真的被填进了那条记录（人类 Todo #141 / #145 前提）。
///
/// 背景见 `ChiefCrewWorkingDirectory` 的文档注释：没有 cwd → 四道 guard 挡死 →
/// 总机组永远起不了机长 → 「总机长排序/写摘要」整条线不成立。
@MainActor
final class ChiefCrewWorkingDirectoryTests: XCTestCase {

    private var baseDir: URL!

    private func store() -> LocalCrewStore {
        baseDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("chief-workdir-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: baseDir, withIntermediateDirectories: true)
        return LocalCrewStore(baseDirectory: baseDir)
    }

    private func makeCrew(_ s: LocalCrewStore, _ title: String, dir: String?) -> String {
        s.createCrew(CreateCrewRequest(
            responsibleSubjectId: "local", title: title, machineId: nil,
            workingDirectory: dir, captainAgentKind: "claude_code",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))).crewId
    }

    // MARK: - 判据

    func testPicksTheMostCommonDirectory() {
        let out = ChiefCrewWorkingDirectory.resolve(
            existing: ["/a", "/b", "/a", "/c", "/a", "/b"], exists: { _ in true })
        XCTAssertEqual(out, "/a", "该取「这台机器的活主要在哪」——出现最多的那个")
    }

    /// **不存在的目录不算数**：路径可能早就被删了/搬走了。
    /// 编一个不存在的路径出来，会把「还没设」变成「设了但是错的」，后者更难查。
    func testSkipsDirectoriesThatAreGone() {
        let out = ChiefCrewWorkingDirectory.resolve(
            existing: ["/gone", "/gone", "/gone", "/here"],
            exists: { $0 == "/here" })
        XCTAssertEqual(out, "/here", "出现最多但已经不存在的那个不该被选中")
    }

    /// 一个候选都没有 → nil，**不编**。
    func testReturnsNilRatherThanInventingAPath() {
        XCTAssertNil(ChiefCrewWorkingDirectory.resolve(existing: [], exists: { _ in true }))
        XCTAssertNil(ChiefCrewWorkingDirectory.resolve(
            existing: [nil, "", "   "], exists: { _ in true }))
        XCTAssertNil(ChiefCrewWorkingDirectory.resolve(
            existing: ["/a", "/b"], exists: { _ in false }),
            "全都不存在时宁可没有，也不给一个错的")
    }

    /// **全序**：并列时同一份输入必须每次给同一个答案，否则总机组的目录会随机跳。
    func testTiesAreBrokenDeterministically() {
        let input = ["/b", "/a"]
        let first = ChiefCrewWorkingDirectory.resolve(existing: input, exists: { _ in true })
        XCTAssertEqual(first, "/a", "并列取字典序最小")
        for _ in 0..<20 {
            XCTAssertEqual(
                ChiefCrewWorkingDirectory.resolve(existing: input, exists: { _ in true }), first,
                "同一份输入两次给出不同答案 —— 字典序那一手没生效")
        }
    }

    // MARK: - 它真的被填进了那条记录（接线）

    /// 这条是本组的重点：判据算得对，**但没填进去就等于没做**。
    func testTheChiefLayerActuallyGetsAWorkingDirectory() {
        let s = store()
        let dir = baseDir.path   // 真实存在的目录
        _ = makeCrew(s, "甲", dir: dir)
        _ = makeCrew(s, "乙", dir: dir)
        // 重开：造总机组那一步在 `init` 里，要它看见上面两条才算数。
        let reopened = LocalCrewStore(baseDirectory: baseDir)
        XCTAssertEqual(
            reopened.getCrew(LocalCrew.chiefCrewId)?.crew.workingDirectory, dir,
            "总机组没有拿到工作目录 —— 那四道 guard 会继续挡死它，机长永远起不来")
    }

    /// 已经有目录的总机组**不许被覆盖**（人可能已经 `change_workdir` 指到别处了）。
    func testAnExistingWorkingDirectoryIsNeverOverwritten() throws {
        let s = store()
        let chosen = baseDir.appendingPathComponent("chosen")
        try FileManager.default.createDirectory(at: chosen, withIntermediateDirectories: true)
        s.setWorkingDirectory(LocalCrew.chiefCrewId, chosen.path)
        _ = makeCrew(s, "甲", dir: baseDir.path)
        _ = makeCrew(s, "乙", dir: baseDir.path)
        let reopened = LocalCrewStore(baseDirectory: baseDir)
        XCTAssertEqual(
            reopened.getCrew(LocalCrew.chiefCrewId)?.crew.workingDirectory, chosen.path,
            "人自己指过的目录被开机那一步覆盖掉了")
    }
}
