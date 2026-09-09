import XCTest

/// **整个测试进程必须跑在临时数据根上，够不着人的真数据目录。**
///
/// ## 这是第二层，跟 `McpServerTestDirectoryContractTests` 不是一件事
///
/// - 那一层是**测试级**：断言「代码里别忘了给 store 注入 temp dir」——「**别写错**」。
/// - 这一层是**进程级**：整趟测试的数据根被挪到临时目录——「**就算写错了也伤不到**」。
///
/// 前者是早点发现，后者才是隔离。两层都要，因为**第一层挡不住它自己没覆盖到的东西**
/// （它只管 `McpServer`；别的 store 在测试里直接构造漏目录，它看不见）。
///
/// ## 为什么这一层必须有：今天本机的现场
///
/// 2026-09-09 本机数据目录发生了一次读失败（EPERM）。`CrewMentionFilterRealWhiteboardTests`
/// 里那句 `guard let data = try? Data(contentsOf: url) else { continue }` 把读失败吞成
/// 「没数据」→ 变成 `XCTSkip`，**而且 skip 连原因都没有**。那 3 条前一天 passed、
/// 之后重跑也 passed，**只在那一趟 skip**，整套照报 `0 failures`。
///
/// **也就是说：测试套件当时正在读人的真实数据目录，而那个目录出问题时套件安静地报绿。**
///
/// ## 机制不是这里发明的
///
/// `PendingCrewDataRoot.overrideEnvKey`（`PENDINGCREW_DATA_DIR`）**早就存在** ——
/// 它是 2026-08-26 为「在这台机器上冒烟测 daemon」造的，注释里写着
/// 「**它不是配置项，是能不能验的前提**」。这一笔只是把两个测试入口接上它。
///
/// ## ⚠️ 这条会在你直接跑 `xcodebuild test` 时红，那是**故意的**
///
/// 走 `sh scripts/test-mac.sh` 或 `sh scripts/release-gate.sh` 才有隔离。
/// 裸跑 `xcodebuild test` = 整趟跑在人的真数据根上 —— 那正是要被拦住的事。
/// **「人会用漏的那一处」**，所以两个入口都设了，而这条断言逼你走它们。
final class TestProcessDataRootIsolationTests: XCTestCase {

    func testTestProcessRunsOnAnIsolatedDataRoot() {
        XCTAssertTrue(
            PendingCrewDataRoot.isOverridden,
            """
            这趟测试跑在**人的真实数据目录**上（\(PendingCrewDataRoot.url.path)）。

            用 `sh scripts/test-mac.sh` 或 `sh scripts/release-gate.sh` 跑，它们会把
            `PENDINGCREW_DATA_DIR` 指到临时目录。裸 `xcodebuild test` 没有这层隔离 ——
            任何一个漏注入目录的用例都会直接写进人的账本（2026-09-08/09 连发生两次）。
            """)
    }

    /// 隔离生效时，**白板目录必须真的跟着挪**。
    ///
    /// 只挪数据根、状态还留在老地方那种一半的隔离**比不隔离更难查** ——
    /// `PendingCrewDataRoot` 的注释里点名了这件事（运行时状态全住在 `whiteboards/` 里）。
    func testWhiteboardDirectoryFollowsTheDataRoot() throws {
        try XCTSkipUnless(PendingCrewDataRoot.isOverridden, "没开隔离时这条无从验起")
        XCTAssertTrue(
            LocalWhiteboardStore.defaultDirectory.path.hasPrefix(PendingCrewDataRoot.url.path),
            """
            白板默认目录没跟着数据根走 —— 那就是「挪了一半」：一部分账在临时目录、
            一部分还在真目录，而且不会响。
            """)
    }

    /// 隔离生效时，那个临时根**不能是**真数据根。（防「变量设了但设成了原路径」。）
    func testTheIsolatedRootIsNotTheRealOne() throws {
        try XCTSkipUnless(PendingCrewDataRoot.isOverridden, "没开隔离时这条无从验起")
        let real = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first?.appendingPathComponent("PendingCrew", isDirectory: true)
        XCTAssertNotEqual(
            PendingCrewDataRoot.url.standardizedFileURL, real?.standardizedFileURL,
            "PENDINGCREW_DATA_DIR 设了，但指的就是真目录 —— 隔离等于没开，而且看起来开了")
    }
}
