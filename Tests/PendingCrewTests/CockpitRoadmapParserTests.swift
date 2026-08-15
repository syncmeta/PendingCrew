import XCTest
// CockpitModel.swift 直接编进 test bundle（见 project.yml），所以不需要 @testable import。

final class CockpitRoadmapParserTests: XCTestCase {
    let sample = """
    > 路线账。阶段按文件顺序排先后。

    当前主线:v1 收口。

    ## v1 收口
    status: doing
    target: 2026-08
    目标: 本地闭环全链路真机可用。
    - pendingcrew/concepts/cockpit — 路线段本身
    - pendingcrew/concepts/local-first

    ## v1.1 远端 runner
    status: planned
    目标: fly machine 开通。
    - pendingcrew/concepts/runtime-location-and-machine
    """

    func testParsePreambleAndPhases() {
        let r = CockpitParser.parseRoadmapFile(sample)
        XCTAssertTrue(r.preamble.contains("当前主线:v1 收口。"))
        XCTAssertTrue(r.preamble.contains("路线账"))
        XCTAssertEqual(r.phases.map(\.name), ["v1 收口", "v1.1 远端 runner"])
        let p1 = r.phases[0]
        XCTAssertEqual(p1.status, "doing")
        XCTAssertEqual(p1.target, "2026-08")
        XCTAssertEqual(p1.goal, "本地闭环全链路真机可用。")
        XCTAssertEqual(p1.entries, [
            CockpitPhaseEntry(relpath: "pendingcrew/concepts/cockpit", note: "路线段本身"),
            CockpitPhaseEntry(relpath: "pendingcrew/concepts/local-first", note: ""),
        ])
        XCTAssertEqual(r.phases[1].target, "")
        XCTAssertEqual(r.phases[1].entries.count, 1)
    }

    func testParseEmptyAndNoPhases() {
        XCTAssertEqual(CockpitParser.parseRoadmapFile("").phases.count, 0)
        let r = CockpitParser.parseRoadmapFile("只有一段话,没有阶段。")
        XCTAssertEqual(r.preamble, "只有一段话,没有阶段。")
        XCTAssertTrue(r.phases.isEmpty)
    }

    // MARK: - 分组（地图的「比例尺」那一层）

    /// 阶段下没有 `###` 时，条目归进一个**隐式默认组**（组名空）——旧格式的路线账
    /// 不用改也照渲，`phase.entries` 仍是拍平后的全部条目。
    func testImplicitDefaultGroup() {
        let p = CockpitParser.parseRoadmapFile(sample).phases[0]
        XCTAssertEqual(p.groups.count, 1)
        XCTAssertEqual(p.groups[0].name, "")
        XCTAssertTrue(p.groups[0].isImplicit)
        XCTAssertEqual(p.groups[0].entries.count, 2)
        XCTAssertEqual(p.entries.count, 2)
    }

    func testParseGroups() {
        let r = CockpitParser.parseRoadmapFile("""
        ## v1 收口
        status: doing
        目标: 收口。
        ### 核心机制
        - pendingcrew/concepts/crew — 骨架
        - pendingcrew/concepts/captain
        ### 界面与交互
        - pendingcrew/features/three-pane-ui
        ### 空组
        """)
        let p = r.phases[0]
        XCTAssertEqual(p.groups.map(\.name), ["核心机制", "界面与交互", "空组"])
        XCTAssertFalse(p.groups[0].isImplicit)
        XCTAssertEqual(p.groups[0].entries, [
            CockpitPhaseEntry(relpath: "pendingcrew/concepts/crew", note: "骨架"),
            CockpitPhaseEntry(relpath: "pendingcrew/concepts/captain", note: ""),
        ])
        XCTAssertEqual(p.groups[1].entries.count, 1)
        XCTAssertTrue(p.groups[2].entries.isEmpty, "写了 ### 但还没挂条目的组要留着（人看得见这块空着）")
        XCTAssertEqual(p.entries.count, 3, "phase.entries = 全组拍平")
    }

    /// `###` 之前先写了条目（混写）：那几条归隐式默认组，后面的照常进各自的组。
    func testEntriesBeforeFirstGroup() {
        let p = CockpitParser.parseRoadmapFile("""
        ## v1
        - a/b
        ### 组一
        - c/d
        """).phases[0]
        XCTAssertEqual(p.groups.count, 2)
        XCTAssertTrue(p.groups[0].isImplicit)
        XCTAssertEqual(p.groups[0].entries.map(\.relpath), ["a/b"])
        XCTAssertEqual(p.groups[1].name, "组一")
        XCTAssertEqual(p.groups[1].entries.map(\.relpath), ["c/d"])
    }

    /// 组不跨阶段：下一个 `##` 关掉上一阶段的所有组。
    func testGroupsDoNotLeakAcrossPhases() {
        let r = CockpitParser.parseRoadmapFile("""
        ## 一
        ### 组一
        - a/b
        ## 二
        - c/d
        """)
        XCTAssertEqual(r.phases[0].groups.map(\.name), ["组一"])
        XCTAssertEqual(r.phases[1].groups.count, 1)
        XCTAssertTrue(r.phases[1].groups[0].isImplicit)
        XCTAssertEqual(r.phases[1].entries.map(\.relpath), ["c/d"])
    }

    // MARK: - 聚合进度（比例尺上的刻度）

    /// 两档分开数（Todo #31）：已验证只认 done 桶，做完待验单独一档，两者不合并。
    func testProgressSplitsVerifiedAndAwaitingQA() {
        let entries = ["a/b", "c/d", "e/f", "g/h"].map { CockpitPhaseEntry(relpath: $0, note: "") }
        let status = [
            "a/b": "done",
            "c/d": "partial, pending-qa",   // 做完待验
            "e/f": "done, pending-qa",      // pending-qa 桶优先 —— 仍算待验，不算已验证
            // g/h 压根没入现状账 —— 两档都不算，只进 total
        ]
        let p = CockpitRoadmapProgress.of(entries, statusByRelpath: status)
        XCTAssertEqual(p.verified, 1)
        XCTAssertEqual(p.awaitingQA, 2)
        XCTAssertEqual(p.total, 4)
        XCTAssertEqual(p.fraction, 0.25, accuracy: 0.0001)
        XCTAssertEqual(p.settledFraction, 0.75, accuracy: 0.0001)
        XCTAssertEqual(p.label, "1+2/4")
        XCTAssertEqual(p.longLabel, "已验证 1 · 做完待验 2 · 共 4")
    }

    /// 全是待验时进度**不为零** —— 这正是改口径要修的那个场景（v1 收口显示 0/15）。
    func testProgressAllPendingQAIsNotZero() {
        let entries = ["a/b", "c/d"].map { CockpitPhaseEntry(relpath: $0, note: "") }
        let p = CockpitRoadmapProgress.of(
            entries, statusByRelpath: ["a/b": "partial, pending-qa", "c/d": "done, pending-qa"])
        XCTAssertEqual(p.verified, 0)
        XCTAssertEqual(p.awaitingQA, 2)
        XCTAssertEqual(p.fraction, 0, accuracy: 0.0001)
        XCTAssertEqual(p.settledFraction, 1, accuracy: 0.0001)
        XCTAssertEqual(p.label, "0+2/2")
    }

    /// 没有待验时标签退回单档写法，不显示多余的 `+0`。
    func testProgressLabelDropsPlusWhenNoPendingQA() {
        let entries = ["a/b", "c/d"].map { CockpitPhaseEntry(relpath: $0, note: "") }
        let p = CockpitRoadmapProgress.of(entries, statusByRelpath: ["a/b": "done"])
        XCTAssertEqual(p.label, "1/2")
        XCTAssertEqual(p.settledFraction, 0.5, accuracy: 0.0001)
    }

    func testProgressEmpty() {
        let p = CockpitRoadmapProgress.of([], statusByRelpath: [:])
        XCTAssertEqual(p.total, 0)
        XCTAssertEqual(p.fraction, 0, accuracy: 0.0001)
        XCTAssertEqual(p.settledFraction, 0, accuracy: 0.0001)
        XCTAssertTrue(p.isEmpty)
    }
}
