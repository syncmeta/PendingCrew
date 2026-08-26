#if os(macOS)
import Foundation

/// **编排闸门（spec §6.2 闸门 2）：同一个数据根只能有一个长期编排者。**
///
/// ## 它为什么长在进程入口，而不是 `SessionHost.begin` 里
///
/// 2026-08-26 P4 收尾时量出来的一条：在这个类型出现之前，app 那副身份通往编排的
/// **唯一**入口是 `MacRootView.swift` 的 `.task { sessionHost.begin(...) }` ——
/// 一个 SwiftUI 视图钩子。也就是说「本进程是不是编排者」这个问题，要等到有窗口、
/// 有视图、视图挂上去了才被问第一次。两个后果，第二个才是要命的：
///
/// 1. **单测进不去** —— 那条路上没有一个不开窗口的入口，于是这道闸门无法被证明。
///    （这不是「没想到怎么测」：`SessionOrchestratorLock.acquire` 当时全仓只有
///    `SessionDaemonHost` 一个调用点，GUI 那条链上一个都没有。）
/// 2. **闸门挂错了对象。** 「谁是编排者」是**进程身份**的属性（`ProcessRole`），
///    不是某个视图的属性。挂在视图上就意味着：换一个入口（第二个窗口、菜单栏
///    extra、将来某个 headless 模式）都得各自记得再问一遍 —— 而当时连第一个入口
///    都没问。
///
/// 所以闸门搬到 `PendingCrewEntry.main()`（`installForGUIProcess`），跟
/// `ProcessRole.requested` 同一拍解析。副产品正是第 1 条的解药：它现在可以在单测里
/// 真跑一遍，见 `OrchestrationGateTests`。
///
/// ## 拿不到锁：daemon 和 app 的正确反应**不是同一个**
///
/// 这不是不一致，这就是设计：
///
/// - **daemon 拿不到 = 拒绝启动**（`SessionDaemonHost.start` 抛
///   `.alreadyOrchestrated`）。第二个 daemon 就是双头本身，安静退出是正确结局。
/// - **app 拿不到 ≠ 拒绝启动。** 「app 起不来」会把一个后台架构改动变成「用户双击
///   图标没反应」—— 那是这条线上最贵的一种翻车。app 拿不到锁的语义是「**已经有人
///   在编排了**」，所以正确反应要看**是谁**在编排：
///
/// | 锁被谁占着 | app 怎么办 | 为什么 |
/// |---|---|---|
/// | `kind == "daemon"` | `.followDaemon` → 退化成 viewer 连上去 | 那边真的在 socket 上听 |
/// | 别的 / 读不出 / 打不开锁文件 | `.conflict` → 两样都不做，摆到用户面前 | 见下 |
///
/// **最后一行是刻意的，它堵的是这道闸门自己会造出来的那个新静默态。** 锁被一个
/// **不听 socket** 的东西占着时（另一个 inproc 的 app 窗口、崩到一半的进程、别的
/// 什么拿了同名锁），要是也退化成 viewer，会发生这一串：`ViewerSessionClient`
/// 发现没有 daemon 在跑 → 去拉一个 → 那个 daemon 因为锁被占着**当场退出（exit 0）**
/// → 连不上 → 退避重连 → 永远循环。用户看到的是**一个界面在、什么都不动、
/// 不报错的窗口**，而那正是我们这一整期在修的那种静默，只是方向反过来。
/// 所以这一态不退化，直接把「谁占着」（pid + 启动时刻 + 数据根，三样照给）
/// 交给上层显示（`SessionHost.orchestrationConflict`）。
///
/// 退化成 viewer 之后仍然连不上（比如 daemon 刚好在这中间死了）由
/// `ViewerSessionClient.isConnected` / `.lastError` 负责说出来 —— 那两个
/// `@Published` 在这一笔之前**全仓没有任何视图读**，同一笔一起接上。
final class OrchestrationGate {
    /// 闸门的裁决。**上层只读它，不许自己再去问一遍锁** —— 再问一遍就等于把闸门
    /// 挂回调用方身上，那正是这个类型要修的病。
    enum Decision: Equatable {
        /// 锁在手，本进程就是这个数据根的编排者。
        case takeOver
        /// 本进程根本不是编排者身份（`.viewer` / `.helper`），本来就不该来取锁。
        case notOrchestrator
        /// 锁被一个 **daemon** 占着 —— 退化成 viewer 连上去。字符串是「谁占着」。
        case followDaemon(String)
        /// 锁被一个**不听 socket** 的东西占着，或者读不出、打不开 —— 既不编排也不
        /// 退化，把冲突摆到用户面前。字符串是「谁占着」。
        case conflict(String)

        /// 给日志的一行。
        var logLine: String {
            switch self {
            case .takeOver: return "本进程接管编排（锁在手）"
            case .notOrchestrator: return "本进程不是编排者身份，不取锁"
            case let .followDaemon(detail): return "退化成 viewer —— \(detail)"
            case let .conflict(detail): return "⚠️ 编排冲突 —— \(detail)"
            }
        }
    }

    /// GUI 进程那一份。`PendingCrewEntry` 在入口处装一次。
    ///
    /// **必须一直被持有** —— `Handle` 释放即解锁，把它置回 `nil` 就是放锁
    /// （单测的 `tearDown` 正是靠这个还原）。
    static var shared: OrchestrationGate?

    let decision: Decision

    /// 锁的持有句柄。**只存不用** —— 它存在的全部意义就是活着。
    private let handle: SessionOrchestratorLock.Handle?

    init(role: ProcessRole, dataRoot: URL, kind: String = "app") {
        guard role == .orchestrator else {
            decision = .notOrchestrator
            handle = nil
            return
        }
        let outcome = SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: kind)
        if case let .acquired(acquired) = outcome {
            handle = acquired
            decision = .takeOver
            return
        }
        handle = nil
        decision = Self.refuse(outcome, dataRoot: dataRoot)
    }

    /// **拒绝的那一半单独拿出来，因为它是这道闸门的全部意义。**
    ///
    /// 把这个函数改成恒返回 `.takeOver`，`OrchestrationGateTests` 里那几条会红 ——
    /// 那是这道闸门唯一的验法（「把拒绝关掉，测试必须红」）。**它不是附属品，
    /// 是验收条件**：没有它，闸门落完我们手上又只有一句「应该挡住了」，而
    /// 2026-08-26 已经证过一次，「应该挡住了」和「挡住了」之间是 27 秒双头。
    static func refuse(_ outcome: SessionOrchestratorLock.Outcome, dataRoot: URL) -> Decision {
        let detail = SessionOrchestratorLock.describe(outcome, dataRoot: dataRoot)
        // **只有 daemon 才值得退化过去** —— 判据是锁文件里持有者自称的 kind，
        // 不是「锁被占着」这个事实本身。分不清的话就会退化成一个连不上的 viewer。
        if case let .heldBy(holder) = outcome, holder?.kind == "daemon" {
            return .followDaemon(detail)
        }
        return .conflict(detail)
    }

    /// **GUI 进程在入口处要做的那两件事**，`PendingCrewEntry.main()` 调它一次。
    ///
    /// 1. 把解析出的数据根打进日志一行（`PENDINGCREW_DATA_DIR` 六条约束的第 6 条 ——
    ///    在这之前只有 `--daemon` 那条路打，GUI 不打）。
    /// 2. 取编排闸门。
    ///
    /// `role` / `dataRoot` / `log` 三个参数只为一件事：**让单测能真跑这个函数**
    /// —— 不碰人的真数据目录，而且能把打出来的那两行接住核对（约束 6 的 app 侧
    /// 「启动时把数据根打进日志一行」，不接住就只能靠人眼看 Console）。
    /// 生产上永远是默认值，调用点只有 `PendingCrewEntry` 一处。
    @discardableResult
    static func installForGUIProcess(
        role: ProcessRole = ProcessRole.requested,
        dataRoot: URL = PendingCrewDataRoot.url,
        log: (String) -> Void = { NSLog("[PendingCrew] %@", $0) }
    ) -> OrchestrationGate {
        log(PendingCrewDataRoot.startupLine(root: dataRoot))
        let gate = OrchestrationGate(role: role, dataRoot: dataRoot, kind: "app")
        log("编排闸门：" + gate.decision.logLine)
        shared = gate
        return gate
    }
}
#endif
