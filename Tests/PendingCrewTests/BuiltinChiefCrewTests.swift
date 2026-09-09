import XCTest
import Foundation

/// **总机组那一层**：它在名册里有一条，但组织操作一律拒绝它（人类 Todo #130 / #137）。
///
/// ## 这几条测试为什么必须存在
/// 备选方案是「不进名册，靠它不存在所以碰不到」。那条**测不了** ——
/// 「因为它不存在所以没被删」写不出一条会红的断言，只写得出「断言它不在名册里」，
/// 而那是把实现照抄一遍，改坏了照样绿。
///
/// 所以选了「进名册 + 四处显式拒绝」，**代价就是这四条测试必须真的会红**。
/// 每一条都做过变异自证：把对应的那道 guard 拆掉，它必须红。
@MainActor
final class BuiltinChiefCrewTests: XCTestCase {

    private var baseDir: URL!

    private func store() -> LocalCrewStore {
        baseDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("builtin-chief-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: baseDir, withIntermediateDirectories: true)
        return LocalCrewStore(baseDirectory: baseDir)
    }

    private func makeCrew(_ s: LocalCrewStore, _ title: String) -> String {
        s.createCrew(CreateCrewRequest(
            responsibleSubjectId: "local", title: title, machineId: nil,
            workingDirectory: nil, captainAgentKind: "claude_code",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))).crewId
    }

    /// 造一条内建的 + 一条普通的，返回 (内建 id, 普通 id)。
    private func seed(_ s: LocalCrewStore) -> (String, String) {
        let normal = makeCrew(s, "普通机组")
        s.upsertBuiltinChiefCrew()
        return (LocalCrew.chiefCrewId, normal)
    }

    // MARK: - 四处拒绝，每条都变异自证过

    func testAdoptRefusesTheBuiltinLayerAsChild() {
        let s = store()
        let (builtin, normal) = seed(s)
        XCTAssertThrowsError(try s.adopt(crewId: builtin, underParent: normal)) { error in
            guard case .builtinCrewNotOrganizable(let id)? = error as? LocalCrewStoreError else {
                return XCTFail("拒绝的理由要说得出是「内建那一层」，不能只是随便一个错：\(error)")
            }
            XCTAssertEqual(id, builtin)
        }
    }

    func testAdoptRefusesTheBuiltinLayerAsParent() {
        // 两个方向都要拦：它既不能被收编，也不能收编别人。
        let s = store()
        let (builtin, normal) = seed(s)
        XCTAssertThrowsError(try s.adopt(crewId: normal, underParent: builtin)) { error in
            guard case .builtinCrewNotOrganizable(let id)? = error as? LocalCrewStoreError else {
                return XCTFail("拒绝的理由要说得出是「内建那一层」，不能只是随便一个错：\(error)")
            }
            XCTAssertEqual(id, builtin)
        }
    }

    func testReleaseRefusesTheBuiltinLayer() {
        let s = store()
        let (builtin, normal) = seed(s)
        XCTAssertThrowsError(try s.release(crewId: builtin, from: normal, to: nil)) { error in
            guard case .builtinCrewNotOrganizable(let id)? = error as? LocalCrewStoreError else {
                return XCTFail("拒绝的理由要说得出是「内建那一层」，不能只是随便一个错：\(error)")
            }
            XCTAssertEqual(id, builtin)
        }
    }

    func testAttachParentRefusesTheBuiltinLayer() {
        // `adopt` 已经拦过一道，这里再拦是因为它是那条**原语** ——
        // 将来多一个调用方，不该指望那个调用方记得也拦一次。
        let s = store()
        let (builtin, normal) = seed(s)
        XCTAssertThrowsError(try s.attachParent(crewId: normal, parentCrewId: builtin)) { error in
            guard case .builtinCrewNotOrganizable(let id)? = error as? LocalCrewStoreError else {
                return XCTFail("拒绝的理由要说得出是「内建那一层」，不能只是随便一个错：\(error)")
            }
            XCTAssertEqual(id, builtin)
        }
    }

    func testDeleteRefusesTheBuiltinLayerAndSaysWhy() {
        let s = store()
        let (builtin, normal) = seed(s)
        XCTAssertEqual(s.deleteCrew(builtin), .refusedBuiltin)
        XCTAssertEqual(s.deleteCrew(normal), .deleted)
        XCTAssertEqual(s.deleteCrew(normal), .notFound,
                       "「本来就没有」和「拒绝删」不许压成同一个答案")
    }

    // MARK: - 口径：全机 crew 数不含它

    func testTheBuiltinLayerIsNotCountedAsACrew() {
        let s = store()
        let (builtin, normal) = seed(s)
        XCTAssertEqual(s.listCrews().map(\.id), [normal],
                       "它是一层不是机组，不该被算进「全机 N 个 crew」")
        XCTAssertTrue(s.listCrews(includingBuiltin: true).map(\.id).contains(builtin),
                      "要它的地方显式要，得要得到")
    }

    func testOrgTreeDoesNotContainTheBuiltinLayer() {
        let s = store()
        let (builtin, _) = seed(s)
        let rows = LocalCrewStore.orgTreeLines(
            whiteboardDirectory: baseDir.appendingPathComponent("whiteboards"))
        XCTAssertFalse(rows.contains { $0.id == builtin })
    }

    // MARK: - 普通 crew 一个字都没变

    func testOrdinaryCrewsAreUnaffected() {
        // 老 JSON 没有 `isBuiltin` 键 → nil → 一律当普通 crew。
        // 这条守的是「加了个新字段，把既有行为带偏了」。
        let s = store()
        let (_, normal) = seed(s)
        let other = makeCrew(s, "另一个")
        XCTAssertNoThrow(try s.adopt(crewId: other, underParent: normal))
        XCTAssertNoThrow(try s.release(crewId: other, from: normal, to: nil))
        XCTAssertEqual(s.deleteCrew(other), .deleted)
    }
}
