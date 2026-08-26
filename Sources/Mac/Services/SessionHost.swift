#if os(macOS)
import Foundation
import Combine

/// **长期职责的唯一所有者**（spec `docs/internal/2026-08-19-backend-split-design.md` §6）。
///
/// 在这个类型出现之前，编排器 / 三个唤醒器 / 用量监视 / 两个轮询中心
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
    let usage: LocalAgentUsageMonitor

    private var bag = Set<AnyCancellable>()
    private var started = false

    /// 两个依赖都收 `nil` 默认值而不是 `= CrewSessionRunner()` 这类默认实参：
    /// 默认实参在 **nonisolated** 上下文求值，而这两个类型都是 `@MainActor`。
    init(runner: CrewSessionRunner? = nil,
         usage: LocalAgentUsageMonitor? = nil) {
        self.runner = runner ?? CrewSessionRunner()
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
        // 有 session 在跑就别自动更新（P4 之后这条会随 A1 一起去掉 —— 那时更新
        // app 本就不打断后台的 session）。
        AppUpdater.shared.isBusy = { [weak runner] in
            runner?.runs.contains { $0.status == .running } ?? false
        }

        wire(crewStore: crewStore, model: model)
    }

    /// 承接 `CrewStore` 排空共享控制文件后发布的请求数组。
    ///
    /// 这些订阅在此之前是 `MacThreePaneView` 上的一串 `.onChange` 修饰符 ——
    /// 也就是说**编排逻辑长在界面上**。搬到这里是 P0 的主要工作量，逐条原样搬，
    /// 循环体 / 错误处理 / fail-loud 落白板 / refreshDetail 缓存 miss 兜底一个字没改。
    ///
    /// ⚠️ 每条都必须 `.receive(on: DispatchQueue.main)`：`@Published` 是 willSet
    /// 语义（赋值**前**发），不推一拍就会读到旧值、且「读完清空」会被随后的赋值
    /// 盖掉。`.onChange` 是 didSet 语义，推一拍才对得上。
    ///
    /// 推到主队列之后再 `MainActor.assumeIsolated` —— `.receive(on: .main)` 保证了
    /// 线程，assumeIsolated 只是把它翻译成编译器认的隔离，好让闭包体能原样保留
    /// `.onChange` 里的写法（含内部继承 MainActor 的 `Task { }`）。
    ///
    /// **为什么这样是安全的，逐条留证**（assumeIsolated 猜错就是当场崩，不是随手加的）：
    /// 1. 本文件里**每一个** `.sink` 的上游都有 `.receive(on: DispatchQueue.main)`，
    ///    没有例外路径 —— 改这个方法时请保持这条不变量。
    /// 2. 唯一容易漏想的坑是「`@Published` 订阅瞬间会同步发一次当前值」：那一发
    ///    **也**要过 `receive(on:)`。`receive(on:)` 是无条件调度（哪怕上游已经在
    ///    目标队列上也照样 async 一拍），所以首值同样是异步落到主队列的，不存在
    ///    「首值绕过 receive(on:) 直达」这条路。
    /// 3. `DispatchQueue.main` 上执行 = 主线程 = 主 actor 的执行器，这正是
    ///    `MainActor.assumeIsolated` 成立的条件。
    private func wire(crewStore: CrewStore, model: AppModel) {
        let sessionRunner = self.runner

        // 建 crew 后自动起机长（用户要的零摩擦：新建即启动 + 群里报到，无需手动点
        // 「启动 Captain」）。store 在 createCrew 完成后 append payload；这里持有
        // sessionRunner，捕获整批并立即清空，再逐条拉起。
        crewStore.$captainAutostartRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] reqs in
                MainActor.assumeIsolated {
                    guard !reqs.isEmpty, let crewStore, let model else { return }
                    crewStore.captainAutostartRequests = []
                    Task {
                        for req in reqs {
                            if crewStore.details[req.crewId] == nil {
                                await crewStore.refreshDetail(req.crewId)
                            }
                            guard let detail = crewStore.details[req.crewId], detail.captain != nil else {
                                if let receipt = req.deliveryFailureReceipt(reason: "找不到子机长信息") {
                                    crewStore.postSystemNotice(crewId: receipt.crewId, text: receipt.text)
                                }
                                continue
                            }
                            do {
                                try await sessionRunner.startCaptain(
                                    detail: detail, backend: model.backend, openingBrief: req.brief)
                            } catch {
                                // 不再静默吞错：落到 runner 的共享通道，inspector 成员列表模式会显
                                // （否则建完 crew captain 没起、用户也不知道为什么）。
                                sessionRunner.reportStartFailure(
                                    crewId: req.crewId, brief: req.brief,
                                    error: error, mentionCaptain: false)
                                if let receipt = req.deliveryFailureReceipt(
                                    reason: "子机长启动失败：\(error.localizedDescription)") {
                                    crewStore.postSystemNotice(crewId: receipt.crewId, text: receipt.text)
                                }
                            }
                        }
                    }
                }
            }
            .store(in: &bag)

        // 机长 `start_session` 命令排空后的待起 worker session 队列（chunk2 §2）。
        // **数组**：同一 tick 里连续多条命令落地时,单值 `@Published` 在 SwiftUI 合并
        // 同步赋值会丢掉中间几条 —— 见 `CrewStore.sessionSpawnRequests` 注释。这里立刻
        // 捕获 + 清空,避免同一批命令被 `.onChange` 重复触发处理。
        crewStore.$sessionSpawnRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] reqs in
                MainActor.assumeIsolated {
                    guard !reqs.isEmpty, let crewStore, let model else { return }
                    crewStore.sessionSpawnRequests = []
                    Task {
                        for req in reqs {
                            // detail 缓存只在 UI 打开过该 crew 后才有 —— app 刚启动时为空，
                            // 不现拉就丢请求（曾静默吞掉整批机长派工）。对齐
                            // executeCreateChildCrew：缓存 miss 就 refreshDetail，仍拿不到才
                            // fail-loud 落白板。
                            if crewStore.details[req.crewId] == nil {
                                await crewStore.refreshDetail(req.crewId)
                            }
                            guard let detail = crewStore.details[req.crewId] else {
                                crewStore.postSystemNotice(
                                    crewId: req.crewId,
                                    text: "起 session 失败：拉不到 crew 详情，brief 已丢弃：\(req.brief.prefix(60))…")
                                continue
                            }
                            let kind: LocalCodingAgentKind? = req.runner.flatMap {
                                $0 == "claude" ? .claudeCode : ($0 == "codex" ? .codex : nil)
                            }
                            do {
                                try await sessionRunner.startForBrief(
                                    detail: detail, backend: model.backend, brief: req.brief,
                                    runnerOverride: kind, isolation: req.isolation,
                                    model: req.model, effort: req.effort, title: req.title)
                            } catch {
                                // fail-loud（#541）：此前只落 lastStartError（UI 横幅），机长
                                // 那边毫无动静 —— 排队派出去的 brief 就此蒸发。现在同时 @机长。
                                sessionRunner.reportStartFailure(
                                    crewId: req.crewId, brief: req.brief, error: error)
                            }
                        }
                    }
                }
            }
            .store(in: &bag)

        // session 自切模型/effort（set_session_profile）：claude 注入 /model /effort,
        // codex 白板说明。数组语义同 sessionSpawnRequests（防同 tick 丢命令）。
        crewStore.$profileChangeRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] reqs in
                MainActor.assumeIsolated {
                    guard !reqs.isEmpty, let crewStore else { return }
                    crewStore.profileChangeRequests = []
                    Task { for req in reqs { await sessionRunner.applyProfileChange(req) } }
                }
            }
            .store(in: &bag)

        // 定时唤醒登记（schedule_wakeup）→ runner 持久化 + 挂定时器。
        crewStore.$wakeupRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] reqs in
                MainActor.assumeIsolated {
                    guard !reqs.isEmpty, let crewStore else { return }
                    crewStore.wakeupRequests = []
                    for req in reqs { sessionRunner.scheduleWakeup(req) }
                }
            }
            .store(in: &bag)

        // 机长 session 操作（inspect / nudge / stop）→ runner 执行 + 写应答文件。
        crewStore.$sessionOpsRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] reqs in
                MainActor.assumeIsolated {
                    guard !reqs.isEmpty, let crewStore else { return }
                    crewStore.sessionOpsRequests = []
                    for req in reqs { sessionRunner.applySessionOp(req) }
                }
            }
            .store(in: &bag)

        // 机长 change_workdir（改工作目录 + 迁 agent 上下文）。规划要看在跑的 run，
        // 那份状态只有 runner 有 —— 所以和 sessionOps 一样在这儿接线：算完/干完把
        // 文本写回应答文件，机长那侧的 long-poll 就拿到预览或回执了。
        crewStore.$workdirChangeRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] reqs in
                MainActor.assumeIsolated {
                    guard !reqs.isEmpty, let crewStore else { return }
                    crewStore.workdirChangeRequests = []
                    for req in reqs {
                        let text = WorkdirChangeCommand.run(req, runs: sessionRunner.runs)
                        LocalCrewControlStore.shared.writeCommandResponse(
                            crewId: req.crewId, commandId: req.commandId, text: text)
                        Task { await crewStore.refreshDetail(req.crewId) }
                    }
                }
            }
            .store(in: &bag)

        // 群聊收听登记（listen；#465）→ runner 登记 + 白板观察 + 广播直投。
        crewStore.$listenRequests
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] reqs in
                MainActor.assumeIsolated {
                    guard !reqs.isEmpty, let crewStore else { return }
                    crewStore.listenRequests = []
                    for req in reqs { sessionRunner.applyListen(req) }
                }
            }
            .store(in: &bag)

        // 跨 crew 汇报线消息 → 唤醒目标 crew 机长（#463）。idle 才直投注入（busy
        // 的机长下轮白板注入自然看到）；机长没在跑 → **直接拉起**（@ 唤醒语义：
        // 不在跑不能只留白板），开场 prompt 带上这条消息;拉起失败才落白板注记。
        crewStore.$crewMessageWakes
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] wakes in
                MainActor.assumeIsolated {
                    guard !wakes.isEmpty, let crewStore, let model else { return }
                    crewStore.crewMessageWakes = []
                    for wake in wakes {
                        let captainRun = sessionRunner.runs.first {
                            $0.crewId == wake.targetCrewId && $0.role == .captain && $0.status == .running
                        }
                        if let run = captainRun {
                            if !run.backend.isBusy {
                                run.send(CrewLocalMentionInjectLogic.renderInjection(
                                    messageText: wake.text, senderName: wake.senderLabel))
                            }
                        } else {
                            Task {
                                do {
                                    guard let backend = model.backend else { throw CancellationError() }
                                    let detail = try await backend.getCrew(wake.targetCrewId)
                                    try await sessionRunner.startCaptain(
                                        detail: detail, backend: backend,
                                        wakeText: "\(wake.senderLabel)：\(wake.text)")
                                } catch {
                                    LocalWhiteboardStore.shared.appendSessionMessage(
                                        crewId: wake.targetCrewId, sessionId: "system",
                                        text: "收到「\(wake.senderLabel)」的消息，但自动拉起机长失败：\(error.localizedDescription)。",
                                        senderName: "系统")
                                }
                            }
                        }
                    }
                }
            }
            .store(in: &bag)

        // 额度快照更新 → 过按档位计算的提醒线、且未临近重置时按 runner 分流广播
        // （按重置周期去重；门槛/时机/事实文案见 QuotaWarningPlan）。
        // 两家快照由 runner 自己从 QuotaCenter 现取，这里只招呼一声。
        QuotaCenter.shared.$claude
            .receive(on: DispatchQueue.main)
            .sink { [weak sessionRunner] _ in
                MainActor.assumeIsolated { sessionRunner?.broadcastQuotaWarningIfNeeded() }
            }
            .store(in: &bag)

        QuotaCenter.shared.$codex
            .receive(on: DispatchQueue.main)
            .sink { [weak sessionRunner] _ in
                MainActor.assumeIsolated { sessionRunner?.broadcastQuotaWarningIfNeeded() }
            }
            .store(in: &bag)
    }
}
#endif
