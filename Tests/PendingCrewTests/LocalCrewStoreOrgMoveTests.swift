import XCTest

/// 组织架构调整数据层单测（Todo #22/#25）：`adopt` / `release` / `height` /
/// `resolveAnyCrew` / `orgTreeLines`。照 `LocalCrewStoreDepthTests` 的注入目录模式。
@MainActor
final class LocalCrewStoreOrgMoveTests: XCTestCase {
    private var dir: URL!

    override func setUp() {
        super.setUp()
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("creworg-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    private func freshStore() -> LocalCrewStore { LocalCrewStore(baseDirectory: dir) }

    private func req(title: String) -> CreateCrewRequest {
        .make(responsibleSubjectId: "local-byok", title: title, machineId: nil,
              workingDirectory: "/tmp/x", captainAgentKind: "codex",
              captain: .systemGenerated(templateName: nil))
    }

    // MARK: - height

    func testHeightLeafZeroAndChainCounts() throws {
        let s = freshStore()
        let a = s.createCrew(req(title: "a")).crewId
        let b = s.createCrew(req(title: "b")).crewId
        let c = s.createCrew(req(title: "c")).crewId
        XCTAssertEqual(s.height(of: a), 0)
        try s.attachParent(crewId: b, parentCrewId: a)
        try s.attachParent(crewId: c, parentCrewId: b)
        XCTAssertEqual(s.height(of: a), 2)
        XCTAssertEqual(s.height(of: b), 1)
        XCTAssertEqual(s.height(of: c), 0)
    }

    // MARK: - adopt（收编/认父共用落地）

    func testAdoptAttachesAndIsIdempotent() throws {
        let s = freshStore()
        let hq = s.createCrew(req(title: "总部")).crewId
        let a = s.createCrew(req(title: "鉴权重构")).crewId
        try s.adopt(crewId: a, underParent: hq)
        XCTAssertEqual(s.parentIds(of: a), [hq])
        try s.adopt(crewId: a, underParent: hq) // 已是其子 → 幂等
        XCTAssertEqual(s.parentIds(of: a), [hq])
    }

    func testAdoptRejectsCycleAndSelf() throws {
        let s = freshStore()
        let a = s.createCrew(req(title: "a")).crewId
        let b = s.createCrew(req(title: "b")).crewId
        try s.adopt(crewId: b, underParent: a)
        XCTAssertThrowsError(try s.adopt(crewId: a, underParent: a))
        // a 收编自己的后代当父 → 环。
        XCTAssertThrowsError(try s.adopt(crewId: a, underParent: b)) { err in
            guard case LocalCrewStoreError.wouldCreateCycle = err else {
                return XCTFail("expected wouldCreateCycle, got \(err)")
            }
        }
    }

    /// 组织树**层数不设上限**（PendingCrew 是给大规模 agent 组织用的）：挂多深都行；
    /// 唯一的结构约束是禁环 —— 不能把 crew 挂进自己的子树。
    func testDeepNestingAllowedButCycleStillRejected() throws {
        let s = freshStore()
        // 链式挂到 depth 7，一路都不该被拒。
        var chain: [String] = [s.createCrew(req(title: "c0")).crewId]
        for i in 1...7 {
            let next = s.createCrew(req(title: "c\(i)")).crewId
            XCTAssertNoThrow(try s.adopt(crewId: next, underParent: chain[i - 1]))
            chain.append(next)
        }
        XCTAssertEqual(s.depth(of: chain[7]), 7)

        // 带子树的 crew 挂到最深处也不拒（旧 cap 会在这拒）。
        let x = s.createCrew(req(title: "x")).crewId
        let y = s.createCrew(req(title: "y")).crewId
        try s.adopt(crewId: y, underParent: x)
        XCTAssertNoThrow(try s.adopt(crewId: x, underParent: chain[7]))
        XCTAssertEqual(s.depth(of: y), 9)

        // 但把链顶挂到自己的后代下 → 成环，仍要拒。
        XCTAssertThrowsError(try s.adopt(crewId: chain[0], underParent: y)) { err in
            guard case LocalCrewStoreError.wouldCreateCycle = err else {
                return XCTFail("expected wouldCreateCycle, got \(err)")
            }
        }
    }

    func testAdoptUnknownCrewThrowsNotFound() {
        let s = freshStore()
        let a = s.createCrew(req(title: "a")).crewId
        XCTAssertThrowsError(try s.adopt(crewId: "local-nope", underParent: a))
        XCTAssertThrowsError(try s.adopt(crewId: a, underParent: "local-nope"))
    }

    // MARK: - release（摘出/转挂,仅限直系子）

    func testReleaseToTopLevel() throws {
        let s = freshStore()
        let hq = s.createCrew(req(title: "总部")).crewId
        let a = s.createCrew(req(title: "a")).crewId
        try s.adopt(crewId: a, underParent: hq)
        try s.release(crewId: a, from: hq, to: nil)
        XCTAssertEqual(s.parentIds(of: a), [])
    }

    func testReleaseTransfersToNewParent() throws {
        let s = freshStore()
        let hq = s.createCrew(req(title: "总部")).crewId
        let a = s.createCrew(req(title: "a")).crewId
        let b = s.createCrew(req(title: "b")).crewId
        try s.adopt(crewId: a, underParent: hq)
        try s.adopt(crewId: b, underParent: hq)
        try s.release(crewId: a, from: hq, to: b)
        XCTAssertEqual(s.parentIds(of: a), [b])
    }

    func testReleaseNonChildThrows() throws {
        let s = freshStore()
        let hq = s.createCrew(req(title: "总部")).crewId
        let stranger = s.createCrew(req(title: "无关")).crewId
        XCTAssertThrowsError(try s.release(crewId: stranger, from: hq, to: nil)) { err in
            guard case LocalCrewStoreError.notDirectChild = err else {
                return XCTFail("expected notDirectChild, got \(err)")
            }
        }
    }

    /// 转挂校验失败（环）时原父边必须不动 —— 先挂新边再摘旧边的顺序保证。
    func testReleaseFailedTransferKeepsOriginalEdge() throws {
        let s = freshStore()
        let hq = s.createCrew(req(title: "总部")).crewId
        let a = s.createCrew(req(title: "a")).crewId
        let a1 = s.createCrew(req(title: "a1")).crewId
        try s.adopt(crewId: a, underParent: hq)
        try s.adopt(crewId: a1, underParent: a)
        // 把 a 转挂到自己的后代 a1 → 环,拒;a 仍在 hq 下。
        XCTAssertThrowsError(try s.release(crewId: a, from: hq, to: a1))
        XCTAssertEqual(s.parentIds(of: a), [hq])
    }

    func testReleaseToSameParentIsNoop() throws {
        let s = freshStore()
        let hq = s.createCrew(req(title: "总部")).crewId
        let a = s.createCrew(req(title: "a")).crewId
        try s.adopt(crewId: a, underParent: hq)
        try s.release(crewId: a, from: hq, to: hq)
        XCTAssertEqual(s.parentIds(of: a), [hq])
    }

    // MARK: - resolveAnyCrew（全局解析）

    func testResolveAnyCrewByIdTitlePrefixAndAmbiguity() throws {
        let s = freshStore()
        let auth = s.createCrew(req(title: "鉴权重构")).crewId
        let dark = s.createCrew(req(title: "深色模式")).crewId
        _ = s.createCrew(req(title: "深色文案")).crewId
        _ = dark
        XCTAssertEqual(s.resolveAnyCrew(hint: auth), auth)        // id 精确
        XCTAssertEqual(s.resolveAnyCrew(hint: "鉴权重构"), auth)   // title 精确
        XCTAssertEqual(s.resolveAnyCrew(hint: "鉴权"), auth)       // 唯一前缀
        XCTAssertNil(s.resolveAnyCrew(hint: "深色"))               // 歧义前缀
        XCTAssertNil(s.resolveAnyCrew(hint: "语音"))               // 无匹配
        XCTAssertNil(s.resolveAnyCrew(hint: "  "))                 // 空
    }

    func testResolveAnyCrewDuplicateExactTitleIsAmbiguous() throws {
        let s = freshStore()
        _ = s.createCrew(req(title: "重构"))
        _ = s.createCrew(req(title: "重构"))
        XCTAssertNil(s.resolveAnyCrew(hint: "重构"))
    }

    // MARK: - orgTreeLines（跨进程树概览）

    func testOrgTreeLinesRendersDepthAndOrphanRoots() throws {
        let s = freshStore()
        let hq = s.createCrew(req(title: "总部")).crewId
        let a = s.createCrew(req(title: "鉴权重构")).crewId
        let a1 = s.createCrew(req(title: "登录闪退")).crewId
        let solo = s.createCrew(req(title: "独立组")).crewId
        try s.adopt(crewId: a, underParent: hq)
        try s.adopt(crewId: a1, underParent: a)
        // nonisolated static 从 whiteboardDirectory 的**父目录**找 local-crews.json
        // （与 title(ofCrew:) 同布局）——store 的 baseDirectory 对应白板目录的父级,
        // 所以传一个 dir 下的子目录进去。
        let wbDir = dir.appendingPathComponent("whiteboard", isDirectory: true)
        let lines = LocalCrewStore.orgTreeLines(whiteboardDirectory: wbDir)
        let byId = Dictionary(uniqueKeysWithValues: lines.map { ($0.id, $0) })
        XCTAssertEqual(byId[hq]?.depth, 0)
        XCTAssertEqual(byId[a]?.depth, 1)
        XCTAssertEqual(byId[a1]?.depth, 2)
        XCTAssertEqual(byId[solo]?.depth, 0)
        XCTAssertEqual(byId[a]?.title, "鉴权重构")
        // 前序：父行在子行前面。
        let idx = { (id: String) in lines.firstIndex { $0.id == id }! }
        XCTAssertLessThan(idx(hq), idx(a))
        XCTAssertLessThan(idx(a), idx(a1))
    }

    func testOrgTreeLinesMissingFileEmpty() {
        let wbDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("creworg-none-\(UUID().uuidString)/whiteboard")
        XCTAssertTrue(LocalCrewStore.orgTreeLines(whiteboardDirectory: wbDir).isEmpty)
    }
}
