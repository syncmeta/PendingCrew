#if os(macOS)
import Foundation
import XCTest

/// **编排闸门（spec §6.2 闸门 2）的验收条件，不是它的附属品。**
///
/// 这一组存在的理由，得从它跑不起来的那段时间说起：在 `OrchestrationGate` 出现
/// 之前，app 那副身份通往编排的唯一入口是 `MacRootView` 的 `.task`（一个 SwiftUI
/// 视图钩子），**没有一条不开窗口的路能进去** —— 于是这道闸门在 app 侧无法被证明，
/// 而它恰恰是 2026-08-26 那次「app 在跑、daemon 照样起来、两个编排者同写三个单
/// writer 文件 27 秒」逼出来的东西。把闸门搬到进程身份上之后，下面这些才第一次
/// 变成**跑得出来的读数**，而不是「读代码读到的」。
///
/// ## 这一组的验法：把拒绝关掉，它必须红
///
/// `OrchestrationGate.refuse(_:dataRoot:)` 改成恒返回 `.takeOver`，
/// `test_锁被另一个app窗口占着时报冲突` / `test_锁被daemon占着时退化成viewer` /
/// `test_拒绝里三样齐` 三条当场红。**这个验法本身跑过一趟**（2026-08-26），
/// 不是「按理说会红」。
///
/// ## 每一条都跑在临时数据根上
///
/// 这台机器上随时有一个真 PendingCrew 在跑。**单测碰真 `Application Support`
/// 就是活生生的双头**，正是这道闸门要防的那件事 —— 用真目录测防双头，是这一期
/// 最不能犯的错。所以每条都自带 `dataRoot`。
final class OrchestrationGateTests: XCTestCase {
    private var dataRoot: URL!
    /// 测试里假扮「已经在编排的那个进程」的那把锁。**必须被持有**，释放即解锁。
    private var incumbent: SessionOrchestratorLock.Handle?

    override func setUpWithError() throws {
        dataRoot = URL(fileURLWithPath: "/tmp/pcrew-gate-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        // 顺序要紧：先松开进程级那一份（它也持着锁），再松开假扮的那把。
        OrchestrationGate.shared = nil
        incumbent = nil
        try? FileManager.default.removeItem(at: dataRoot)
    }

    /// 让 `dataRoot` 上先有一个编排者占着，返回它自称的身份。
    @discardableResult
    private func occupy(kind: String) throws -> SessionOrchestratorLock.Holder {
        let outcome = SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: kind)
        guard case let .acquired(handle) = outcome else {
            throw XCTSkip("测试自己都没拿到锁，后面几条无意义：\(outcome)")
        }
        incumbent = handle
        return handle.holder
    }

    // MARK: - 没人占着：app 那副身份**真的**去取了锁

    /// 这一条是那份「app 到底取没取锁」的读数本体。
    ///
    /// 在闸门搬到进程入口之前，它写不出来 —— 没有一个不开窗口的入口能进到那段
    /// 初始化。现在它跑得出来，而且断言的不是「函数返回了什么」，是**锁文件里
    /// 真的躺着本进程**（`currentHolder` 只看不写）。
    func test_没人占着时app接管并且锁文件里真的是本进程() {
        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        XCTAssertEqual(gate.decision, .takeOver)

        let holder = SessionOrchestratorLock.currentHolder(dataRoot: dataRoot)
        XCTAssertEqual(holder?.kind, "app", "app 那副身份取锁时必须自称 app —— "
            + "`SessionDaemonControl.runningDaemonPid` 靠这个字段分辨该不该 SIGTERM 它")
        XCTAssertEqual(holder?.pid, ProcessInfo.processInfo.processIdentifier)
        XCTAssertEqual(holder?.dataRoot, dataRoot.path)
    }

    /// 约束 4：锁跟着数据根走。**锁文件落在给定的数据根下，不在真目录下。**
    func test_锁跟着数据根走() {
        OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        let expected = dataRoot.appendingPathComponent(SessionOrchestratorLock.fileName)
        XCTAssertTrue(FileManager.default.fileExists(atPath: expected.path), expected.path)
        XCTAssertNotEqual(
            expected.path,
            PendingCrewDataRoot.url.appendingPathComponent(SessionOrchestratorLock.fileName).path,
            "这条要是相等，说明测试正在往真数据根里写锁 —— 立刻停手")
    }

    // MARK: - 有人占着：**是谁**决定 app 该怎么办

    /// 锁被一个 **daemon** 占着 → 退化成 viewer（那边真的在 socket 上听）。
    func test_锁被daemon占着时退化成viewer() throws {
        try occupy(kind: "daemon")
        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        guard case let .followDaemon(detail) = gate.decision else {
            return XCTFail("锁被 daemon 占着时应当退化成 viewer，实际 \(gate.decision)")
        }
        XCTAssertTrue(detail.contains("常驻后台进程"), detail)
    }

    /// 锁被**另一个 app 窗口**占着 → **不退化**，报冲突。
    ///
    /// 这一条守的是这道闸门自己会造出来的那个新静默态：inproc 的 app 不在 socket
    /// 上听，退化过去只会得到「界面在、什么都不动、不报错」的窗口 ——
    /// `ViewerSessionClient` 会去拉 daemon，那个 daemon 因为锁被占着当场 exit 0，
    /// 于是连不上、退避重连、永远循环。**方向反过来的同一种静默。**
    func test_锁被另一个app窗口占着时报冲突不退化() throws {
        try occupy(kind: "app")
        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        guard case .conflict = gate.decision else {
            return XCTFail("锁被另一个 app 占着时不许退化成 viewer，实际 \(gate.decision)")
        }
    }

    /// 「谁占着」必须回答三样：pid、启动时刻、数据根。**缺一样人就得再查一轮。**
    func test_拒绝里三样齐() throws {
        let holder = try occupy(kind: "app")
        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        guard case let .conflict(detail) = gate.decision else {
            return XCTFail("应当是冲突，实际 \(gate.decision)")
        }
        XCTAssertTrue(detail.contains("pid \(holder.pid)"), "缺 pid：\(detail)")
        XCTAssertTrue(detail.contains("启动于："), "缺启动时刻：\(detail)")
        XCTAssertTrue(detail.contains(dataRoot.path), "缺数据根：\(detail)")
    }

    /// 拿不到锁时**一把锁都不能留下** —— 冲突的那个进程不该持着任何东西，
    /// 否则占着它的那个 daemon 退出后没人接得上。
    func test_拒绝之后原持有者仍然是原来那个() throws {
        let holder = try occupy(kind: "daemon")
        OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        XCTAssertEqual(SessionOrchestratorLock.currentHolder(dataRoot: dataRoot)?.kind, "daemon")
        XCTAssertEqual(SessionOrchestratorLock.currentHolder(dataRoot: dataRoot)?.acquiredAt,
                       holder.acquiredAt,
                       "锁文件被后来者改写了 —— 下一个进程读到的「谁占着」就是假的")
    }

    // MARK: - 不是编排者身份的进程不来取锁

    func test_viewer身份不取锁() {
        let gate = OrchestrationGate.installForGUIProcess(
            role: .viewer, dataRoot: dataRoot, log: { _ in })
        XCTAssertEqual(gate.decision, .notOrchestrator)
        XCTAssertNil(SessionOrchestratorLock.currentHolder(dataRoot: dataRoot))
        XCTAssertFalse(
            FileManager.default.fileExists(
                atPath: dataRoot.appendingPathComponent(SessionOrchestratorLock.fileName).path),
            "viewer 连锁文件都不该建 —— 建了就意味着它去 open 了那把锁")
    }

    func test_helper身份不取锁() {
        let gate = OrchestrationGate.installForGUIProcess(
            role: .helper, dataRoot: dataRoot, log: { _ in })
        XCTAssertEqual(gate.decision, .notOrchestrator)
        XCTAssertNil(SessionOrchestratorLock.currentHolder(dataRoot: dataRoot))
    }

    // MARK: - 约束 6：启动时把数据根打进日志一行（**app 侧**）

    /// 在这之前只有 `--daemon` 那条路打这一行，GUI 不打。
    ///
    /// 这一行不是装饰：2026-08-26 那次事故是「daemon 悄悄跑在了真目录上」，而这套
    /// 隔离机制自己的失败形态是**方向相反的同一种静默** —— 「它悄悄跑在了临时目录
    /// 上」：人以为在动真数据，其实在动一个空壳，**而所有操作都会成功**。
    func test_app启动时把数据根打出来() {
        var lines: [String] = []
        OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { lines.append($0) })
        XCTAssertEqual(lines.count, 2, "启动两行：数据根 + 闸门裁决。实际 \(lines)")
        XCTAssertTrue(lines[0].hasPrefix("数据根 = "), lines[0])
        XCTAssertTrue(lines[0].contains(dataRoot.path), lines[0])
        XCTAssertTrue(lines[1].contains("编排闸门"), lines[1])
    }

    // MARK: - 闸门之后的角色（这一条挡的是「修一个双头顺手造出另一个」）

    /// 拿不到锁的 app 进程，`ProcessRole.requested` 仍是 `.orchestrator`，
    /// 但 **`effective` 必须是 `.viewer`**。
    ///
    /// 不分开的话 `CrewStore.ownsSharedControlChannel` 会放这个进程去排空共享控制
    /// 通道 —— 那三条通道是「一文件一命令、排空后删」的无锁模型，两边都排会让机长的
    /// `start_session` 被随机一方吞掉：不报错、不重试、命令文件已经删了。
    /// **不许依赖测试进程自己的身份。** 翻默认（2026-09-04）之后测试进程的
    /// `ProcessRole.requested` 就是 `.viewer` 了 —— 原来那句
    /// `XCTSkipUnless(requested == .orchestrator)` 会让这两条从此静默 skip，
    /// 而它们守的正是「修一个双头顺手造出另一个」。所以喂纯判定版，把 `requested`
    /// 当参数给进去。
    func test_拿不到锁的进程在effective上不再是编排者() throws {
        XCTAssertEqual(
            ProcessRole.effective(requested: .orchestrator, decision: nil,
                                  localFallbackActive: false),
            .orchestrator, "闸门没装时应当退回 requested")

        try occupy(kind: "daemon")
        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        XCTAssertEqual(
            ProcessRole.effective(requested: .orchestrator, decision: gate.decision,
                                  localFallbackActive: false),
            .viewer)
    }

    func test_拿到锁的进程在effective上仍是编排者() throws {
        let gate = OrchestrationGate.installForGUIProcess(
            role: .orchestrator, dataRoot: dataRoot, log: { _ in })
        XCTAssertEqual(
            ProcessRole.effective(requested: .orchestrator, decision: gate.decision,
                                  localFallbackActive: false),
            .orchestrator)
    }

    /// §9.2 的临时接管：`requested` 仍是 `.viewer`（身份不被改写），但
    /// **`effective` 必须变成编排者** —— 否则本地接管起来的那套长期定时器会被
    /// 各个 `start` 里的 precondition 当场打死，回退等于没有。
    func test_临时本地接管时effective是编排者() {
        XCTAssertEqual(
            ProcessRole.effective(requested: .viewer, decision: nil,
                                  localFallbackActive: true),
            .orchestrator)
        XCTAssertEqual(
            ProcessRole.effective(requested: .viewer, decision: nil,
                                  localFallbackActive: false),
            .viewer, "没接管的 viewer 仍然只是 viewer")
    }
}
#endif
