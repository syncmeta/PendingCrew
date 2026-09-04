#if os(macOS)
import XCTest

/// **别的进程刚建的 crew，本进程 `getCrew` 认不出来**（2026-09-04）。
///
/// ## 病历
///
/// `loadFromDisk` **只在 `init` 跑一次**，之后内存那份不再刷新。翻默认之后编排搬进
/// daemon，而**人在界面上新建 crew 走的是 GUI 进程** —— 于是 daemon 在自己下一次
/// 写盘之前，`getCrew(新建的 id)` 拿到的是 nil。
///
/// 后果不是「显示不新鲜」，是**功能失败**：`start_session` 排空之后
/// `SessionHost` 消费时 `crewStore.details` 缓存 miss → `refreshDetail` →
/// `backend.getCrew` → 读到 nil → **给这个新 crew 派活直接失败**。
///
/// 好在那条路上有 fail-loud（白板落一句「拉不到 crew 详情，brief 已丢弃」），
/// 所以症状是**明说失败**而不是「什么都没发生」—— 比静默丢活轻，比显示问题重。
///
/// ## 范围（机长钉死的，别扩大）
///
/// 只在 `getCrew` **内存 miss** 时、在同一把跨进程锁下重读一次再判 miss；
/// **命中不读盘**；`listCrews` 与其余读路径一律不动，不改成轮询或每次读盘。
@MainActor
final class LocalCrewStoreReadMissTests: XCTestCase {

    private var dir: URL!

    override func setUpWithError() throws {
        dir = URL(fileURLWithPath: "/tmp/pcrew-readmiss-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.setAttributes([.posixPermissions: 0o700],
                                               ofItemAtPath: dir.path)
        try? FileManager.default.removeItem(at: dir)
    }

    private func makeStore() -> LocalCrewStore { LocalCrewStore(baseDirectory: dir) }

    private func makeCrew(_ store: LocalCrewStore, _ title: String) -> String {
        store.createCrew(.make(
            responsibleSubjectId: "local-byok", title: title, machineId: nil,
            workingDirectory: dir.path, captainAgentKind: "claude_code",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))).crewId
    }

    // MARK: -

    /// **主条**：B 先起（此后内存不再刷新），A 之后建了一个 crew ——
    /// B 必须认得出它。这正是「人在界面上建 crew → 机长在里面派活」那条路。
    ///
    /// 注意 **B 全程没有做过任何写入** —— 它不该靠「下一次自己写盘顺带重读」才看见。
    func test_别的进程刚建的crew本进程也要认得出来() {
        let seed = makeStore()
        _ = makeCrew(seed, "已有的 crew")

        let b = makeStore()          // 后台：init 读一次，之后内存不再刷新
        let a = makeStore()          // 界面：人在这儿建了一个新 crew
        let fresh = makeCrew(a, "人刚新建的 crew")

        XCTAssertNotNil(
            b.getCrew(fresh),
            """
            别的进程刚建的 crew 认不出来 —— 给它派活会直接失败（brief 被丢弃）。
            这条路上 `loadFromDisk` 只在 init 跑过一次。
            """)
        XCTAssertEqual(b.getCrew(fresh)?.crew.title, "人刚新建的 crew")
    }

    /// 真·不存在的 id 仍然是 nil —— 重读一次之后**还是没有**，那就是没有。
    func test_不存在的id仍然返回nil() {
        let store = makeStore()
        _ = makeCrew(store, "存在的 crew")
        XCTAssertNil(store.getCrew("local-根本没这个 crew"))
    }

    /// **命中不读盘。** 把文件整个删掉之后，内存里已有的那个仍然读得到 ——
    /// 读得到就证明这一次没去碰盘（去碰了只会读到「文件不在」）。
    func test_命中时不读盘() throws {
        let store = makeStore()
        let id = makeCrew(store, "已有的 crew")
        try FileManager.default.removeItem(at: dir.appendingPathComponent("local-crews.json"))

        XCTAssertNotNil(store.getCrew(id), "命中的那次不该依赖磁盘")
    }

    /// **读失败不许把已知的 crew 变成「不存在」。**
    ///
    /// 最危险的写法是「miss → 重读 → 重读失败时把内存清空 → 还是 miss → nil」：
    /// 一次读失败会把**一屋子已知的 crew 全变成不存在**，而调用方分不出
    /// 「这个 crew 没了」和「这次没读着」。收口里的 `reloadUnderLock` 守的正是
    /// 「读失败 ≠ 内容损坏」（`MultiProcessJSONStore` 第 ④ 条，一次真事故换来的），
    /// 这条把它钉在**读**这一侧。
    ///
    /// ⚠️ **钉得住的和钉不住的**：钉得住「已知的不许变成不存在」；
    /// **钉不住「未知的别谎称不存在」** —— `getCrew` 返回的是 optional，
    /// 文件读不出来时对一个从没见过的 id 给不出第三种态。要区分得改签名，
    /// 超出这次范围。
    func test_文件坏掉时已知的crew不许变成不存在() throws {
        let store = makeStore()
        let known = makeCrew(store, "已有的 crew")
        // 写进一段解不开的字节（不是删文件 —— 那是另一条路径）。
        try Data("{ 这不是 JSON".utf8)
            .write(to: dir.appendingPathComponent("local-crews.json"))

        XCTAssertNotNil(
            store.getCrew(known),
            "一次读失败把已知的 crew 报成「不存在」—— 调用方分不出「没了」和「没读着」")
        XCTAssertNil(store.getCrew("local-根本没这个 crew"), "不存在的仍然是 nil")
    }

    /// 目录不可读（权限）时同样不许把已知的报成不存在。
    func test_读不出来时已知的crew不许变成不存在() throws {
        let store = makeStore()
        let known = makeCrew(store, "已有的 crew")
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o000],
            ofItemAtPath: dir.appendingPathComponent("local-crews.json").path)

        XCTAssertNotNil(store.getCrew(known), "读不出来 ≠ 这个 crew 不存在")
    }
}
#endif
