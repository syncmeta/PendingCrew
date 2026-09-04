#if os(macOS)
import XCTest

/// **「这一轮该不该再拉一个 daemon」**（判据第 1–6 条落地时从接线里挪下来的那一段）。
///
/// 为什么它必须在这一层：P4 那次的教训是「闸门挂错对象 → 在 app 侧无法被证明」。
/// 这两条判断原来长在 `ViewerSessionClient.connect` 里 —— 那个文件在
/// `Sources/Mac/Services`、**进不了 test bundle**，于是它们只有「编译过」，
/// 没有任何人能证明它们对。**逻辑一律不许留在接线里。**
///
/// 而第二条（「上次拉起来的那个还活着就别再拉」）尤其该被测：它不是从设计文档抄
/// 下来的契约，是实现时为了防「每次说不准都再造一个不回话的 daemon」自己加的规则 ——
/// 自己发明的规则更需要有人盯着，因为没有第二份文档能对照。
final class DaemonLaunchPlanTests: XCTestCase {

    private func plan(daemonHoldsLock: Bool = false,
                      lastChild: DaemonLaunchRace.ChildState? = nil)
        -> DaemonLaunchPlan.Step {
        DaemonLaunchPlan.next(daemonHoldsLock: daemonHoldsLock, lastSpawnedChild: lastChild)
    }

    /// 什么都没有 —— 该拉一个。
    func test_没有后台也没拉过就拉一个() {
        XCTAssertEqual(plan(), .launch)
    }

    /// 锁上写着有 daemon 在跑 —— **不拉**，直接连。
    ///
    /// ⚠️ 这一条的理由是「再拉一个纯属白拉」（它起来就会撞上单实例锁、当场退掉），
    /// **不是**「拦住第二个编排者」—— 防双头是锁的事，不是这一层的事。两件事混着
    /// 写，以后就分不清哪条约束在生效。
    func test_锁上写着有daemon在跑就不拉直接连() {
        guard case let .connectOnly(reason) = plan(daemonHoldsLock: true) else {
            return XCTFail("实际：\(plan(daemonHoldsLock: true))")
        }
        XCTAssertTrue(reason.contains("在跑") || reason.contains("daemon"), reason)
    }

    /// **上一次拉起来的那个还活着 → 不拉。**
    ///
    /// 这一条防的是：超时判成「说不准」之后照常重连，而重连又走到拉起这一步 ——
    /// 于是每一轮都再造一个不回话的 daemon，越攒越多，日志里全是它们。
    func test_上次拉起来的还活着就不再拉一个() {
        guard case let .connectOnly(reason) = plan(lastChild: .alive) else {
            return XCTFail("实际：\(plan(lastChild: .alive))")
        }
        XCTAssertTrue(reason.contains("还活着"), reason)
    }

    /// 上一次那个已经退了 —— 该再拉一次（它没起成，而现在也没人在编排）。
    func test_上次拉起来的已经退了就再拉一次() {
        XCTAssertEqual(plan(lastChild: .exited(0)), .launch)
        XCTAssertEqual(plan(lastChild: .exited(nil)), .launch)
    }

    /// 两条同时成立时仍然不拉 —— 「有人在跑」优先，理由也报那一条
    /// （它是更有信息量的那个：告诉人锁在谁手上，而不是「我上次拉过」）。
    func test_两条同时成立时报更有信息量的那条() {
        guard case let .connectOnly(reason) = plan(daemonHoldsLock: true, lastChild: .alive) else {
            return XCTFail("实际：\(plan(daemonHoldsLock: true, lastChild: .alive))")
        }
        XCTAssertFalse(reason.contains("上一次"), "两条都成立时该报「有人在跑」：\(reason)")
    }
}
#endif
