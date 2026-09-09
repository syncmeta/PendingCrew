import XCTest
import Foundation

/// **总机组的界面入口**（人类 Todo #130 / #137 的最后一截）。
///
/// ## 这一摊之前卡在哪
/// 「总机组那一层」已经进名册、四处组织操作也拒绝它（见 `BuiltinChiefCrewTests`）。
/// 但接手复核时量到两件前任清单里没有的事：
///
/// 1. **那条记录根本没被造出来过** —— `upsertBuiltinChiefCrew` 全仓只有测试在调，
///    生产代码零调用点。本机名册 48 条，带内建标记的 0 条。
///    所以这不是「有屋子没有门」，是屋子还没盖。
/// 2. **就算加了入口，选中也站不住** —— 列表刷新时会把「不在列表里的选中项」清掉，
///    而总机组按设计**永远不在那份列表里**。
///
/// ## 所以这里钉三件事，每件都做过变异自证
/// - ① 它**不出现在 crew 列表里**（既有行为，别让入口把它带回列表）；
/// - ② 入口指向的是 `pendingcrew-chief` 那一条，而且那条**真的存在、开得出详情**；
/// - ③ 列表刷新不许把它从选中位上弹下来。
///
/// ⚠️ **这几条测不到的那一层**：SwiftUI 那一行有没有真的画出来、点下去手感对不对。
/// 这里测的是「入口该指向谁、那个谁在不在、选中站不站得住」。渲染在目视清单上。
@MainActor
final class ChiefLayerEntryTests: XCTestCase {

    private var baseDir: URL!

    private func store() -> LocalCrewStore {
        baseDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("chief-entry-\(UUID().uuidString)")
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

    private func summary(_ id: String, title: String? = nil,
                         updatedAt: String = "2020-01-01T00:00:00Z") -> CrewSummary {
        CrewSummary(id: id, title: title ?? id, responsibleSubjectId: "s",
                    runtimeLocation: "local_host", captainBotId: nil, status: nil,
                    createdAt: "", updatedAt: updatedAt, parentCrewIds: [],
                    captainAgentKind: nil, machineId: nil)
    }

    // MARK: - ② 那条记录得真的在，入口才有东西可开

    /// **开机就得有它**。这条在接手时是红的：`upsertBuiltinChiefCrew` 没有任何
    /// 生产调用点，谁也没造过它。
    func testAFreshStoreAlreadyHasTheChiefLayer() {
        let s = store()
        XCTAssertTrue(
            s.listCrews(includingBuiltin: true).contains { $0.id == LocalCrew.chiefCrewId },
            "总机组那一层要在 store 起来的那一刻就在名册里 —— 不然入口点下去开的是个空")
    }

    /// **入口点下去开得出详情**。中栏画的是 `getCrew` 拿到的那份 detail，
    /// 拿不到就只是一个转圈的加载态。
    func testTheChiefLayerOpensAsARealChat() {
        let s = store()
        let detail = s.getCrew(LocalCrew.chiefCrewId)
        XCTAssertNotNil(detail, "开得出详情，才叫「点得开」")
        XCTAssertEqual(detail?.crew.id, LocalCrew.chiefCrewId)
    }

    /// 造它是**幂等**的：重开一次 app 不许把标题、成员表冲掉。
    func testBootstrappingTwiceKeepsWhatIsAlreadyThere() {
        let s = store()
        let before = s.getCrew(LocalCrew.chiefCrewId)?.crew.createdAt
        let s2 = LocalCrewStore(baseDirectory: baseDir)
        XCTAssertEqual(s2.getCrew(LocalCrew.chiefCrewId)?.crew.createdAt, before,
                       "第二次起来不许重造 —— 重造等于把那本群聊的归属换了一条记录")
        XCTAssertEqual(
            s2.listCrews(includingBuiltin: true).filter { $0.id == LocalCrew.chiefCrewId }.count, 1)
    }

    // MARK: - ① 它仍然不算一个机组

    /// 入口存在，**但它不许因此回到 crew 列表里**。
    /// 这条守的是「为了做入口，顺手把那道排除拆了」。
    func testTheChiefLayerStillDoesNotShowUpInTheCrewList() {
        let s = store()
        let normal = makeCrew(s, "普通机组")
        XCTAssertEqual(s.listCrews().map(\.id), [normal],
                       "它是一层不是机组：有了入口也不该混进 crew 列表")
    }

    // MARK: - ② 侧栏第三视图的固定入口

    /// 固定入口排在**第一行**，且就是总机组那一条。
    func testChiefViewPinsTheChiefLayerAtTheTop() {
        let rows = CrewChiefOverview.rows(
            chiefLayer: summary(LocalCrew.chiefCrewId, title: "总机组"),
            crews: [summary("a"), summary("b")],
            activity: { _ in nil })
        guard case .chiefLayer(let crew)? = rows.first else {
            return XCTFail("第三视图顶上那一行必须是总机组的固定入口，实际是 \(String(describing: rows.first))")
        }
        XCTAssertEqual(crew.id, LocalCrew.chiefCrewId)
    }

    /// 固定入口**不吃掉**下面那份列表：另外两个 crew 一条不少、顺序不变。
    func testPinningTheEntryDoesNotEatTheListBelowIt() {
        let rows = CrewChiefOverview.rows(
            chiefLayer: summary(LocalCrew.chiefCrewId),
            crews: [summary("a"), summary("b")],
            activity: { _ in nil })
        XCTAssertEqual(rows.dropFirst().compactMap(\.crewId), ["a", "b"])
    }

    /// 名册里还没有那一层时（老数据、造它那步失败），**只是不画这一行**，
    /// 剩下的列表照常。这一档是这个视图的地基：入口挂了不该让侧栏一起挂。
    func testWithoutTheChiefLayerTheViewStillWorks() {
        let rows = CrewChiefOverview.rows(
            chiefLayer: nil, crews: [summary("a")], activity: { _ in nil })
        XCTAssertEqual(rows.compactMap(\.crewId), ["a"])
        XCTAssertFalse(rows.contains { if case .chiefLayer = $0 { return true } else { return false } })
    }

    /// 纵深：就算它**漏进了**那份 crew 列表（哪个调用方忘了默认口径），
    /// 下半段也不许把它再画一遍 —— 一个入口出现两次，人分不清点哪个。
    func testTheChiefLayerIsNeverDrawnTwice() {
        let chief = summary(LocalCrew.chiefCrewId)
        let rows = CrewChiefOverview.rows(
            chiefLayer: chief, crews: [chief, summary("a")], activity: { _ in nil })
        XCTAssertEqual(rows.dropFirst().compactMap(\.crewId), ["a"],
                       "下半段不许再出现总机组")
    }

    // MARK: - 它进名册，但不占通讯录的号

    /// **不占号**。号码是通讯录里「第 7 号机组」那个 7，而且**终身不变、永不
    /// 回收** —— 让内建那一层占掉 1 号，全新一台机器上人建的第一个机组就永远
    /// 是 2 号了。这条是造它那一步（`init` 里）最容易带出来的暗伤。
    ///
    /// ⚠️ **这条必须重开一次 store 才测得到**。发号在 `backfillNumbers` 里，
    /// 而它跑在 `loadFromDisk` 那一步 —— **早于**同一个 `init` 里造总机组那一句。
    /// 所以第一个实例上它压根走不到那条记录：只在下一次打开时才轮到它。
    /// 第一版这条测试没重开，于是「把 builtin 跳过那句删掉」一刀下去照样全绿。
    func testTheChiefLayerDoesNotConsumeADirectoryNumber() {
        let base: URL
        do {
            let s = store()
            base = baseDir
            _ = makeCrew(s, "第一个机组")
        }
        // 重开：这一次 `backfillNumbers` 才看得见总机组那条记录。
        let reopened = LocalCrewStore(baseDirectory: base)
        XCTAssertNil(reopened.crewNumber(of: LocalCrew.chiefCrewId),
                     "它不是一个机组，不该占号")
        let fresh = reopened.createCrew(CreateCrewRequest(
            responsibleSubjectId: "local", title: "第二个机组", machineId: nil,
            workingDirectory: nil, captainAgentKind: "claude_code",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))).crewId
        XCTAssertEqual(reopened.crewNumber(of: fresh), 2,
                       "第二个机组就该是 2 号 —— 内建那一层不许在中间吃掉一个号")
    }

    /// 不占号的**下游**：它因此也不出现在通讯录里（`CrewDirectory` 收的正是
    /// 有号的那批）。这条把「不占号」和「看得见的后果」钉在一起。
    func testTheChiefLayerIsNotListedInTheDirectory() {
        let base: URL
        do {
            let s = store()
            base = baseDir
            _ = makeCrew(s, "第一个机组")
        }
        // 同上：重开之后才是「它有没有被发到号」真正落定的那一刻。
        let reopened = LocalCrewStore(baseDirectory: base)
        XCTAssertFalse(reopened.directory().render().contains("总机组"),
                       "通讯录列的是机组，不该出现内建那一层")
    }

    // MARK: - ③ 选中站得住

    /// 总机组**永远不在** crew 列表里，所以「不在列表里就清掉选中」那条规则
    /// 会正好把它弹下来。这条守的就是那个短路。
    func testRefreshDoesNotKickTheChiefLayerOutOfTheSelection() {
        XCTAssertFalse(
            CrewSelectionRule.shouldClearSelection(
                selected: LocalCrew.chiefCrewId, listedIds: ["a", "b"]),
            "总机组按设计不在列表里，不该因此被弹出选中位")
    }

    /// 但**真的没了**的那种（被删了、被藏了）仍要清掉 —— 别为了放行总机组
    /// 把这条规则整个关掉。
    func testRefreshStillClearsASelectionThatIsReallyGone() {
        XCTAssertTrue(
            CrewSelectionRule.shouldClearSelection(selected: "deleted", listedIds: ["a"]))
        XCTAssertFalse(
            CrewSelectionRule.shouldClearSelection(selected: "a", listedIds: ["a"]))
        XCTAssertFalse(
            CrewSelectionRule.shouldClearSelection(selected: nil, listedIds: []))
    }
}
