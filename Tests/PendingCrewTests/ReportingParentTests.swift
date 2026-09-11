import XCTest
import Foundation

/// **汇报线上的父** 和 **组织树上的父** 是两件事（人类 Todo #141 / #137）。
///
/// ## 为什么不能是同一个
/// #137 的原话是「总机组是全局的一**层**，顶层机组就是它的子机组」。照字面实现
/// 会让 13 个顶层 crew 每个**存**一条指向总机组的父边 —— 然后某个 crew 被 `adopt`
/// 走，它就同时挂着真父和总机组两条边，**得有人记得去掉那条**；`release` 回顶层
/// 时又得记得加回来。忘一次，组织树多一根假枝，而且**没有任何尺子会响**。
/// 一层不该有边。
///
/// 所以这里分成两个访问器，区别必须**一直**测着：
/// - `parentIds` = **存下来的边**。树、深度、成环、adopt/release、机长交接授权
///   全走它，它**永远不返回内建那一层**。
/// - `reportingParentIds` = **汇报该送到谁**。存下来的父为空时派生出总机组。
///
/// ## 这里最危险的变异
/// 不是某个分支写错，是**两个函数被写成同一个**（谁图省事让 `parentIds` 去调
/// `reportingParentIds`）。那一刀下去组织树会当场塌：没有任何 crew 还是根、
/// 深度全体 +1、每个顶层 crew 都变成「已有父」于是 adopt 语义改变。
/// `testStoredParentsNeverIncludeTheBuiltinLayer` 就是挡这一刀的，它必须会红。
@MainActor
final class ReportingParentTests: XCTestCase {

    private var baseDir: URL!

    private func store() -> LocalCrewStore {
        baseDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("reporting-parent-\(UUID().uuidString)")
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

    // MARK: - 尺子 ①：存下来的边里永远没有内建那一层

    /// **挡「两个函数被写成同一个」那一刀。**
    func testStoredParentsNeverIncludeTheBuiltinLayer() {
        let s = store()
        let top = makeCrew(s, "顶层机组")
        XCTAssertEqual(s.parentIds(of: top), [],
                       "顶层 crew 就是根 —— 存下来的父边必须是空的，不许把总机组派生进来")
        XCTAssertFalse(s.parentIds(of: top).contains(LocalCrew.chiefCrewId))
    }

    /// 同一件事的另一面：挂了真父之后，存下来的边里**只有**真父。
    func testStoredParentsAreExactlyTheRealEdges() throws {
        let s = store()
        let parent = makeCrew(s, "父机组")
        let child = makeCrew(s, "子机组")
        try s.attachParent(crewId: child, parentCrewId: parent)
        XCTAssertEqual(s.parentIds(of: child), [parent])
    }

    // MARK: - 尺子 ②：汇报该送到谁

    /// 没有真父 → 汇报落到总机组那一层。
    func testReportingParentOfARootCrewIsTheBuiltinLayer() {
        let s = store()
        let top = makeCrew(s, "顶层机组")
        XCTAssertEqual(s.reportingParentIds(of: top), [LocalCrew.chiefCrewId],
                       "顶层机组往上汇报，该落到总机组 —— 而不是回一句「你已是根」")
    }

    /// 有真父 → 原样返回真父，**且不含**总机组。
    /// 这一半同样重要：别把总机组顺手塞进每一次汇报。
    func testReportingParentOfAChildCrewIsJustItsRealParent() throws {
        let s = store()
        let parent = makeCrew(s, "父机组")
        let child = makeCrew(s, "子机组")
        try s.attachParent(crewId: child, parentCrewId: parent)
        XCTAssertEqual(s.reportingParentIds(of: child), [parent])
        XCTAssertFalse(s.reportingParentIds(of: child).contains(LocalCrew.chiefCrewId),
                       "有真父的 crew 不该再往总机组汇报一份")
    }

    /// 总机组**自己**往上汇报：没有更上一层了，不许派生出自己（那是自己汇报给自己）。
    func testTheBuiltinLayerDoesNotReportToItself() {
        let s = store()
        XCTAssertEqual(s.reportingParentIds(of: LocalCrew.chiefCrewId), [],
                       "总机组没有上级 —— 派生出自己就是一条自环")
    }

    // MARK: - 判据本身（纯函数，三个分支）
    //
    // 「那一层不在名册里」这一支**在 store 上造不出来** —— `init` 一定会把它造好。
    // 所以判据抽成纯函数，这一支在那儿测。不把它当成「测不到所以不管」：
    // 往一个不存在的 crew 汇报，消息会落进一本没人看的群聊，而回执仍说送达了。

    func testRuleDerivesTheBuiltinLayerOnlyWhenItExists() {
        XCTAssertEqual(
            CrewReportingParent.resolve(stored: [], selfId: "top", builtinLayerExists: true),
            [LocalCrew.chiefCrewId])
        XCTAssertEqual(
            CrewReportingParent.resolve(stored: [], selfId: "top", builtinLayerExists: false),
            [], "那一层不在名册里就别派生它")
    }

    func testRuleKeepsRealParentsUntouched() {
        XCTAssertEqual(
            CrewReportingParent.resolve(stored: ["p1", "p2"], selfId: "c",
                                        builtinLayerExists: true),
            ["p1", "p2"], "有真父就只给真父，不抄送总机组")
    }

    func testRuleNeverSendsTheBuiltinLayerToItself() {
        XCTAssertEqual(
            CrewReportingParent.resolve(stored: [], selfId: LocalCrew.chiefCrewId,
                                        builtinLayerExists: true),
            [], "派生出自己就是一条自环")
    }

    /// 查无此 crew → 两个访问器都回空，不许一个回空一个派生。
    func testUnknownCrewGetsNothingFromEither() {
        let s = store()
        XCTAssertEqual(s.parentIds(of: "no-such-crew"), [])
        XCTAssertEqual(s.reportingParentIds(of: "no-such-crew"), [])
    }
}
