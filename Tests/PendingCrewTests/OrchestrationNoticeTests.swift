#if os(macOS)
import Foundation
import XCTest

/// **闸门拒绝之后，界面上到底有没有东西** —— 这一组是那条链的最后一段。
///
/// 上一笔落 `OrchestrationGate` 时留了一个自己点出来的缺口：拒绝的理由只到
/// `@Published` 为止，**全仓没有任何视图读**。那等于闸门存在、但对用户静默 ——
/// 而「窗口在、什么都不动、不报错」正是这一整期在修的那种失败。
///
/// 光「接上界面」不够，因为 GUI 我们验不了（不许为验证开窗口），「接上了」很容易
/// 变成第二个没人看的字段。所以这一组**从锁一路串到界面态**：
/// 真占一把锁 → 真跑 `installForGUIProcess` → 拿到的裁决喂给
/// `OrchestrationNotice.resolve` → **断言屏幕上是错误态而不是正常态**。
///
/// 副作用是这条链把「把拒绝关掉」那个验法延长了一截：`OrchestrationGate.refuse`
/// 改成恒 `.takeOver` 时，`test_锁被别人占着时界面必须是错误态` 会跟着红 ——
/// **不只是闸门红，界面层一起红。**
final class OrchestrationNoticeTests: XCTestCase {
    private var dataRoot: URL!
    private var incumbent: SessionOrchestratorLock.Handle?

    override func setUpWithError() throws {
        dataRoot = URL(fileURLWithPath: "/tmp/pcrew-notice-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        OrchestrationGate.shared = nil
        incumbent = nil
        try? FileManager.default.removeItem(at: dataRoot)
    }

    // MARK: - 从锁串到界面态

    /// **这一组的主条。** 锁被一个不听 socket 的东西占着时，界面必须是错误态。
    func test_锁被别人占着时界面必须是错误态() throws {
        let outcome = SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: "app")
        guard case let .acquired(handle) = outcome else {
            throw XCTSkip("测试自己都没拿到锁：\(outcome)")
        }
        incumbent = handle

        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        let notice = OrchestrationNotice.resolve(decision: gate.decision, viewer: nil)

        guard case let .conflict(detail) = notice else {
            return XCTFail("闸门拒绝了，界面却是 \(notice) —— 拒绝没有出口，等于没拒绝")
        }
        // 屏幕上必须给全「谁占着」三样，缺一样人就得再查一轮。
        XCTAssertTrue(detail.contains("pid \(handle.holder.pid)"), detail)
        XCTAssertTrue(detail.contains("启动于："), detail)
        XCTAssertTrue(detail.contains(dataRoot.path), detail)
    }

    /// 反面：没人占着时界面不占屏。**没有这条，上面那条可以靠「永远报错」通过。**
    func test_没人占着时界面不占屏() {
        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        XCTAssertEqual(gate.decision, .takeOver)
        XCTAssertEqual(
            OrchestrationNotice.resolve(decision: gate.decision, viewer: nil), .none)
    }

    // MARK: - 纯判定本身

    func test_退化成viewer且已连上时不占屏() {
        XCTAssertEqual(
            OrchestrationNotice.resolve(
                decision: .followDaemon("谁占着的那段话"),
                viewer: .init(isConnected: true, lastError: nil)),
            .none)
    }

    /// 退化了但没连上：提示态，而且**要带上「本来该连谁」** ——
    /// 只说「连不上」，人还得再查一轮。
    func test_退化成viewer但没连上时是提示态并带上本来该连谁() {
        let notice = OrchestrationNotice.resolve(
            decision: .followDaemon("- 谁：常驻后台进程（--daemon） pid 4242"),
            viewer: .init(isConnected: false, lastError: "连不上后台进程：ECONNREFUSED"))
        guard case let .connecting(detail) = notice else {
            return XCTFail("应当是提示态，实际 \(notice)")
        }
        XCTAssertTrue(detail.contains("ECONNREFUSED"), detail)
        XCTAssertTrue(detail.contains("pid 4242"), "缺「本来该连谁」：\(detail)")
    }

    /// 冲突压过链路状态：这个窗口什么都不管，连不连得上已经不是重点。
    func test_冲突压过连不上() {
        let notice = OrchestrationNotice.resolve(
            decision: .conflict("有人占着"),
            viewer: .init(isConnected: false, lastError: "连不上"))
        XCTAssertEqual(notice, .conflict(detail: "有人占着"))
    }

    /// 闸门没装（`--daemon` 进程 / 单测）且没有 viewer 那条腿 → 不占屏，行为同从前。
    func test_闸门没装时不占屏() {
        XCTAssertEqual(OrchestrationNotice.resolve(decision: nil, viewer: nil), .none)
    }

    /// 总闸 `PENDINGCREW_BACKEND=daemon` 那条老路：没有裁决（身份本来就是 viewer），
    /// 但腿断了照样要说 —— 这一态在这一笔之前**屏幕上什么都没有**。
    func test_老viewer路径断线时也要说() {
        let notice = OrchestrationNotice.resolve(
            decision: nil, viewer: .init(isConnected: false, lastError: "后台进程 30 秒没有回应"))
        XCTAssertEqual(notice, .connecting(detail: "后台进程 30 秒没有回应"))
    }
}
#endif
