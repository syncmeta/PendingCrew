import XCTest

/// 根祖先判定（crew 名字后面那行黄字标注的数据源）。纯函数,不需要 store,
/// 所以直接喂父边表 —— 多层嵌套 / 多父 / 根自身 / 脏数据(父不存在) / 环全钉住。
final class CrewRootLineageTests: XCTestCase {
    // MARK: - rootIds

    func testRootItselfHasNoRootSuffix() {
        // 自己就是根 → 不标注（自己 @ 自己没意义）。
        let parents = ["a": [] as [String]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "a", parents: parents), [])
    }

    func testDirectChildPointsAtParent() {
        let parents = ["a": [], "b": ["a"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "b", parents: parents), ["a"])
    }

    func testMultiLevelReachesTopmostNotDirectParent() {
        // c ← b ← a：c 标的是 **a**（根），不是直接父 b。
        let parents = ["a": [], "b": ["a"], "c": ["b"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "c", parents: parents), ["a"])
    }

    func testDeepChainStillOneRoot() {
        let parents = ["a": [], "b": ["a"], "c": ["b"], "d": ["c"], "e": ["d"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "e", parents: parents), ["a"])
    }

    func testMultipleParentsReturnAllRootsInAttachOrder() {
        // c 认了两个父(p1 先、p2 后)，两个父各自是根 → 全显示，顺序 = 加入顺序。
        let parents = ["p1": [], "p2": [], "c": ["p1", "p2"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "c", parents: parents), ["p1", "p2"])
    }

    func testMultipleParentsOrderFollowsDeclaration() {
        let parents = ["p1": [], "p2": [], "c": ["p2", "p1"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "c", parents: parents), ["p2", "p1"])
    }

    func testMultiLevelAndMultiParentCollectsBothRoots() {
        //   rootA        rootB
        //     |            |
        //    mid1         mid2
        //      \          /
        //         leaf（双父，各自往上两层）
        let parents = [
            "rootA": [], "rootB": [],
            "mid1": ["rootA"], "mid2": ["rootB"],
            "leaf": ["mid1", "mid2"],
        ]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "leaf", parents: parents), ["rootA", "rootB"])
    }

    func testDiamondDedupesSharedRoot() {
        // 两条路都通到同一个根 → 只出现一次。
        let parents = ["r": [], "l": ["r"], "m": ["r"], "leaf": ["l", "m"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "leaf", parents: parents), ["r"])
    }

    func testMixedDepthParentsBothCount() {
        // 一个父自己就是根、另一个父还有上级 → 两个根都要。
        let parents = ["nearRoot": [], "far": [], "mid": ["far"], "leaf": ["nearRoot", "mid"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "leaf", parents: parents), ["nearRoot", "far"])
    }

    // MARK: - 脏数据

    func testUnknownCrewReturnsEmpty() {
        XCTAssertEqual(CrewRootLineage.rootIds(of: "nope", parents: ["a": []]), [])
    }

    func testEmptyGraphReturnsEmpty() {
        XCTAssertEqual(CrewRootLineage.rootIds(of: "a", parents: [:]), [])
    }

    func testDanglingParentMakesCrewItsOwnRoot() {
        // b 的父 ghost 不在表里 → 那条边当不存在 → b 自己就是根 → 不标注。
        let parents = ["b": ["ghost"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "b", parents: parents), [])
    }

    func testDanglingParentInTheMiddleFallsBackToNearestKnownAncestor() {
        // c ← b，b 的父是脏引用 → b 当根（别因为一条脏边把整条谱系判丢）。
        let parents = ["b": ["ghost"], "c": ["b"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "c", parents: parents), ["b"])
    }

    func testOneDanglingOneRealParentKeepsTheRealOne() {
        let parents = ["a": [], "c": ["ghost", "a"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "c", parents: parents), ["a"])
    }

    func testSelfParentDoesNotHang() {
        // 手改 JSON 才可能的自挂自：保证返回,不标注。
        let parents = ["a": ["a"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "a", parents: parents), [])
    }

    func testTwoNodeCycleTerminates() {
        // a ↔ b：纯环上没有根 → 空数组,但**一定返回**（不挂死、不崩）。
        let parents = ["a": ["b"], "b": ["a"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "a", parents: parents), [])
        XCTAssertEqual(CrewRootLineage.rootIds(of: "b", parents: parents), [])
    }

    func testCycleWithEscapeHatchStillFindsTheRealRoot() {
        // b ↔ c 成环,但 c 另有一条通到真根 r 的边 → 仍能标出 r。
        let parents = ["r": [], "b": ["c"], "c": ["b", "r"]]
        XCTAssertEqual(CrewRootLineage.rootIds(of: "b", parents: parents), ["r"])
    }

    // MARK: - rootTitles（展示版）

    private func summary(
        _ id: String,
        _ title: String,
        parents: [String] = [],
        wireRoots: [String] = []
    ) -> CrewSummary {
        CrewSummary(
            id: id,
            title: title,
            responsibleSubjectId: "subj",
            runtimeLocation: "local_host",
            captainBotId: nil,
            status: "active",
            createdAt: "2026-08-11T00:00:00Z",
            updatedAt: "2026-08-11T00:00:00Z",
            parentCrewIds: parents,
            rootCrewTitles: wireRoots
        )
    }

    func testRootTitlesMapsIdsToTitles() {
        let crews = [
            summary("a", "PendingCrew"),
            summary("b", "PendingBot发版"),
            summary("c", "Crew出iOS", parents: ["a", "b"]),
        ]
        XCTAssertEqual(
            CrewRootLineage.rootTitles(of: "c", in: crews),
            ["PendingCrew", "PendingBot发版"]
        )
    }

    func testRootTitlesEmptyForRootCrew() {
        let crews = [summary("a", "PendingCrew")]
        XCTAssertEqual(CrewRootLineage.rootTitles(of: "a", in: crews), [])
    }

    func testRootTitlesEmptyListReturnsEmpty() {
        XCTAssertEqual(CrewRootLineage.rootTitles(of: "a", in: []), [])
    }

    // MARK: - `CrewSummary.rootCrewTitles` 那条回退分支（本地父边算不出时才用）
    //
    // 它原本装的是服务端下发的根 crew 血缘，给看不到本地 DAG 的 iPad/iPhone 用；
    // #63 第二期删掉云端整层之后**恒空**，这条分支在真实数据上不再会被走到。
    // 判定本身留着（重建前后端时第二个来源会回到这个位置），下面两条钉的是判定。

    func testWireRootTitlesUsedWhenNoLocalParents() {
        // 本地 parentCrewIds 为空时，标注只能来自 rootCrewTitles 那份。
        let crews = [summary("c", "Crew出iOS", wireRoots: ["PendingCrew", "PendingBot发版"])]
        XCTAssertEqual(
            CrewRootLineage.rootTitles(of: "c", in: crews),
            ["PendingCrew", "PendingBot发版"]
        )
        XCTAssertEqual(
            CrewRootLineage.rootTitlesByCrew(in: crews)["c"],
            ["PendingCrew", "PendingBot发版"]
        )
    }

    func testLocalLineageWinsOverWireRootTitles() {
        // LocalBackend 路径：本地 DAG 是真源，服务端那份（若有）不许覆盖它。
        let crews = [
            summary("a", "本地根"),
            summary("c", "Crew出iOS", parents: ["a"], wireRoots: ["服务端给的"]),
        ]
        XCTAssertEqual(CrewRootLineage.rootTitles(of: "c", in: crews), ["本地根"])
        XCTAssertEqual(CrewRootLineage.rootTitlesByCrew(in: crews)["c"], ["本地根"])
    }

    func testWireRootTitlesEmptyStillNoBadge() {
        let crews = [summary("a", "PendingCrew")]
        XCTAssertEqual(CrewRootLineage.rootTitles(of: "a", in: crews), [])
        XCTAssertNil(CrewRootLineage.rootTitlesByCrew(in: crews)["a"])
    }

    // MARK: - rootTitlesByCrew（列表批量版）

    func testRootTitlesByCrewSkipsRootsAndKeepsOrder() {
        let crews = [
            summary("a", "PendingCrew"),
            summary("b", "PendingBot发版"),
            summary("mid", "中层", parents: ["a"]),
            summary("c", "Crew出iOS", parents: ["mid", "b"]),
        ]
        let map = CrewRootLineage.rootTitlesByCrew(in: crews)
        // 根自己不进字典（不画标注）。
        XCTAssertNil(map["a"])
        XCTAssertNil(map["b"])
        XCTAssertEqual(map["mid"], ["PendingCrew"])
        // 多层 + 多父：两条链的根都要，顺序 = 加入顺序。
        XCTAssertEqual(map["c"], ["PendingCrew", "PendingBot发版"])
    }

    func testRootTitlesByCrewMatchesPerCrewLookup() {
        // 批量版与逐个查的口径必须一致（列表走批量、详情走逐个，别漂）。
        let crews = [
            summary("r", "根"),
            summary("l", "左", parents: ["r"]),
            summary("m", "右", parents: ["r"]),
            summary("leaf", "叶", parents: ["l", "m"]),
        ]
        let map = CrewRootLineage.rootTitlesByCrew(in: crews)
        for crew in crews {
            let single = CrewRootLineage.rootTitles(of: crew.id, in: crews)
            XCTAssertEqual(map[crew.id] ?? [], single, "crew \(crew.id) 两条路径口径不一致")
        }
    }

    func testRootTitlesByCrewEmptyInput() {
        XCTAssertTrue(CrewRootLineage.rootTitlesByCrew(in: []).isEmpty)
    }

    // MARK: - 黄字完整降级规则

    func testBadgeMultipleParentsShowsAllWhenFullCandidateFits() {
        let candidates = CrewRootBadgePresentation.candidateTexts(
            rootTitles: ["PendingCrew", "PendingBot发版"]
        )
        XCTAssertEqual(candidates.first, "@PendingCrew @PendingBot发版")
        XCTAssertEqual(
            CrewRootBadgePresentation.candidateIndexThatFits(
                widths: [180, 96], availableWidth: 180
            ),
            0
        )
    }

    func testBadgeMultipleParentsFallsBackToExactRemainingCount() {
        let candidates = CrewRootBadgePresentation.candidateTexts(
            rootTitles: ["PendingCrew", "PendingBot发版", "第三个根"]
        )
        XCTAssertEqual(
            candidates,
            ["@PendingCrew @PendingBot发版 @第三个根", "@PendingCrew @PendingBot发版 +1", "@PendingCrew +2"]
        )
        XCTAssertEqual(
            CrewRootBadgePresentation.candidateIndexThatFits(
                widths: [240, 176, 92], availableWidth: 100
            ),
            2
        )
    }

    func testBadgeHidesEntirelyWhenFirstParentCannotFit() {
        let candidates = CrewRootBadgePresentation.candidateTexts(rootTitles: ["超长父 crew 名"])
        XCTAssertNil(
            CrewRootBadgePresentation.candidateIndexThatFits(
                widths: [220], availableWidth: 100
            )
        )
        XCTAssertTrue(candidates.allSatisfy { !$0.contains("…") && $0 != "@" })
    }

    func testCrewNameUsesMeasuredMinimumWithTailEllipsis() {
        XCTAssertTrue(CrewRootBadgePresentation.titleMinimumWidthProbe.hasSuffix("…"))
        for candidate in CrewRootBadgePresentation.candidateTexts(
            rootTitles: ["PendingCrew", "PendingBot发版"]
        ) {
            XCTAssertFalse(candidate.contains("…"))
            XCTAssertNotEqual(candidate, "@")
        }
    }

    func testCrewNameTruncatesToMeasuredMinimumBeforeWholeBadge() {
        let allocation = CrewRootBadgePresentation.rowAllocation(
            titleIdealWidth: 320,
            titleMinimumWidth: 84,
            badgeWidths: [92],
            availableWidth: 182,
            spacing: 6
        )
        XCTAssertEqual(allocation.badgeCandidateIndex, 0)
        XCTAssertEqual(allocation.titleWidth, 84)
    }

    func testExtremelyLongCrewNameWithSingleParentKeepsMinimumAndHidesBadge() {
        let allocation = CrewRootBadgePresentation.rowAllocation(
            titleIdealWidth: 320,
            titleMinimumWidth: 84,
            badgeWidths: [92],
            availableWidth: 160,
            spacing: 6
        )
        XCTAssertNil(allocation.badgeCandidateIndex)
        XCTAssertEqual(allocation.titleWidth, 160)
        XCTAssertGreaterThanOrEqual(allocation.titleWidth, 84)
    }

    func testExtremelyLongCrewNameWithMultipleParentsKeepsMinimumAndHidesBadge() {
        let allocation = CrewRootBadgePresentation.rowAllocation(
            titleIdealWidth: 320,
            titleMinimumWidth: 84,
            badgeWidths: [230, 160, 92],
            availableWidth: 160,
            spacing: 6
        )
        XCTAssertNil(allocation.badgeCandidateIndex)
        XCTAssertEqual(allocation.titleWidth, 160)
        XCTAssertGreaterThanOrEqual(allocation.titleWidth, 84)
    }

    // MARK: - badgeText

    func testBadgeTextJoinsWithAtAndSpace() {
        XCTAssertEqual(
            CrewRootLineage.badgeText(rootTitles: ["PendingCrew", "PendingBot发版"]),
            "@PendingCrew @PendingBot发版"
        )
    }

    func testBadgeTextNilWhenNoRoots() {
        XCTAssertNil(CrewRootLineage.badgeText(rootTitles: []))
    }

    func testBadgeTextSkipsBlankTitles() {
        XCTAssertEqual(CrewRootLineage.badgeText(rootTitles: ["  ", "PendingCrew"]), "@PendingCrew")
        XCTAssertNil(CrewRootLineage.badgeText(rootTitles: ["", "   "]))
    }
}

/// `LocalCrewStore.rootCrewIds` 的接线测试 —— 判定本身在上面钉过了，这里只验
/// store 真把自己的父边表喂对了（漏喂就会全员无标注，纯函数单测抓不到）。
@MainActor
final class LocalCrewStoreRootLineageTests: XCTestCase {
    private func freshStore() -> LocalCrewStore {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("crewroot-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return LocalCrewStore(baseDirectory: dir)
    }

    private func req(title: String) -> CreateCrewRequest {
        .make(responsibleSubjectId: "local-byok", title: title, machineId: nil,
              workingDirectory: "/tmp/x", captainAgentKind: "codex",
              captain: .systemGenerated(templateName: nil))
    }

    func testStoreResolvesRootThroughTwoLevels() throws {
        let s = freshStore()
        let a = s.createCrew(req(title: "a")).crewId
        let b = s.createCrew(req(title: "b")).crewId
        let c = s.createCrew(req(title: "c")).crewId
        try s.attachParent(crewId: b, parentCrewId: a)
        try s.attachParent(crewId: c, parentCrewId: b)
        XCTAssertEqual(s.rootCrewIds(of: a), [])
        XCTAssertEqual(s.rootCrewIds(of: b), [a])
        XCTAssertEqual(s.rootCrewIds(of: c), [a])
    }

    func testStoreReturnsBothRootsForTwoParents() throws {
        let s = freshStore()
        let p1 = s.createCrew(req(title: "p1")).crewId
        let p2 = s.createCrew(req(title: "p2")).crewId
        let c = s.createCrew(req(title: "c")).crewId
        try s.attachParent(crewId: c, parentCrewId: p1)
        try s.attachParent(crewId: c, parentCrewId: p2)
        XCTAssertEqual(s.rootCrewIds(of: c), [p1, p2])
    }

    func testStoreUnknownCrewIsEmpty() {
        XCTAssertEqual(freshStore().rootCrewIds(of: "local-nope"), [])
    }
}
