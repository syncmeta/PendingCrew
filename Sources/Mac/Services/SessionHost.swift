#if os(macOS)
import Foundation
import Combine

/// **长期职责的唯一所有者**（spec `docs/2026-08-19-backend-split-design.md` §6）。
///
/// 在这个类型出现之前，编排器 / 云端中继 / 三个唤醒器 / 用量监视 / 两个轮询中心
/// 是随 `MacThreePaneView` 和 `CrewSidebarView` 两个**视图**一起生出来的 ——
/// 这就是「关掉 app 就全停」的根，也是把 session 搬进常驻后台进程时最先撞上的墙。
///
/// 现在它们都归这里。视图退化成观察者：只读 `@Published`，不创建、不启动。
///
/// P0 阶段这个类还活在 GUI 进程里（`ProcessRole.current == .orchestrator`）；
/// P4 之后同一个类原样跑在 `--daemon` 进程里，GUI 侧变成 `.viewer` 不再持有它。
/// **所以这里不许出现任何 SwiftUI / AppKit 依赖** —— 它将来要在没有画面的进程里跑。
@MainActor
final class SessionHost: ObservableObject {
    let runner: CrewSessionRunner
    let relay: CrewRelayAgent
    let usage: LocalAgentUsageMonitor

    private var bag = Set<AnyCancellable>()
    private var started = false

    /// 三个依赖都收 `nil` 默认值而不是 `= CrewSessionRunner()` 这类默认实参：
    /// 默认实参在 **nonisolated** 上下文求值，而这三个类型都是 `@MainActor`。
    init(runner: CrewSessionRunner? = nil,
         relay: CrewRelayAgent? = nil,
         usage: LocalAgentUsageMonitor? = nil) {
        self.runner = runner ?? CrewSessionRunner()
        self.relay = relay ?? CrewRelayAgent()
        self.usage = usage ?? LocalAgentUsageMonitor()
    }

    /// 启动全部长期职责。**幂等** —— 重复调用是 no-op（SwiftUI 的 `.task` 会因
    /// 视图重挂而重跑，这在切 crew 时是常态）。
    ///
    /// 第一行的断言是 spec §6.2 的闸门 1：viewer 进程里误起一套定时器 = 当场崩，
    /// 不是悄悄跑起来变成双头。双头的症状（账被两个进程交替覆盖、唤醒发两遍）
    /// 事后极难定位，所以宁可在这里响。
    func start(model: AppModel, crewStore: CrewStore) {
        precondition(
            ProcessRole.current == .orchestrator,
            "SessionHost.start 只能在编排者进程里调用，当前角色=\(ProcessRole.current.rawValue)")
        guard !started else { return }
        started = true

        // app 重启后重挂持久化的定时唤醒（schedule_wakeup 不因重启失约）。
        runner.rearmWakeups()
        // 成员状态快照定时器（机长 list_sessions 的数据源）。
        runner.startSessionsSnapshotTimer()
        // 本地 mention 唤醒器（wake-resilience 根因修复）：session/机长
        // post_to_crew 的定向 @ → 注入 idle run / 拉起缺席目标。幂等。
        if runner.localMentionWaker == nil {
            let waker = CrewLocalMentionWaker(
                runner: runner, backendProvider: { [weak model] in model?.backend })
            runner.localMentionWaker = waker
            waker.start()
        }
        // 额度中心 + 可用模型表中心一起常开（都是幂等启动、都要落文件给 helper 读）。
        QuotaCenter.shared.start()
        ModelCatalogCenter.shared.start()
        // 本机 Claude / Codex 今日 token 用量（侧栏 footer 那行小字的数据源）。
        usage.start()
        // relay 同步代理常开（幂等启动）；未登录时 tick 是 no-op。
        // sessionRunner 一并注入 —— relay 拉到 task_request 时在本机自动
        // 起 session（#242 遥控 v1），与 inspector 手动起的 run 同一个切换条。
        relay.start(appModel: model, sessionRunner: runner)

        wire(crewStore: crewStore, model: model)
    }

    /// 承接 `CrewStore` 排空共享控制文件后发布的请求数组。
    /// Task 3 填充；先留空让 Task 2 可以单独编译通过。
    private func wire(crewStore: CrewStore, model: AppModel) {}
}
#endif
