#if os(macOS)
import Foundation
import AppKit
import Combine

/// **长期职责的唯一所有者**（spec `docs/internal/2026-08-19-backend-split-design.md` §6）。
///
/// 在这个类型出现之前，编排器 / 三个唤醒器 / 用量监视 / 两个轮询中心
/// 是随 `MacThreePaneView` 和 `CrewSidebarView` 两个**视图**一起生出来的 ——
/// 这就是「关掉 app 就全停」的根，也是把 session 搬进常驻后台进程时最先撞上的墙。
///
/// 现在它们都归这里。视图退化成观察者：只读 `@Published`，不创建、不启动。
///
/// P0 阶段这个类还活在 GUI 进程里（`ProcessRole.requested == .orchestrator`）；
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
    /// `ownsAppUpdater` = 本进程是不是那个「更新 app」的进程。**daemon 传 false**：
    /// 更新是窗口那一侧的事，而且碰 `AppUpdater.shared` 会把 Sparkle 拉起来 ——
    /// 一个没有 `NSApplication` 的进程里不该有它。
    init(runner: CrewSessionRunner? = nil,
         usage: LocalAgentUsageMonitor? = nil,
         ownsAppUpdater: Bool = true) {
        self.runner = runner ?? CrewSessionRunner()
        self.usage = usage ?? LocalAgentUsageMonitor()
        self.ownsAppUpdater = ownsAppUpdater
    }

    private let ownsAppUpdater: Bool

    /// viewer 模式下那条腿（`PENDINGCREW_BACKEND=daemon`，或 inproc 但锁被一个 daemon
    /// 占着而退化过来）。真 inproc 编排时恒 nil。
    ///
    /// `@Published` 是有意的：退化那条路在 `.task` 里才决定，视图得跟着重画
    /// （连不上时那条横幅就挂在它上面）。
    @Published private(set) var viewer: ViewerSessionClient?

    /// 编排闸门的裁决，**原样发布给界面**（`nil` = 闸门没装：`--daemon` 进程 / 单测）。
    ///
    /// 发布的是裁决本身而不是两个预先格式化好的字符串：屏幕上显示什么由
    /// `OrchestrationNotice.resolve(decision:viewer:)` 这个**纯判定**算，
    /// 于是「锁被别人占着 → 界面必须是错误态」是一条跑得出来的测试，
    /// 而不是又一个没人读的 `@Published`。见 `OrchestrationNotice`。
    @Published private(set) var orchestrationDecision: OrchestrationGate.Decision?

    /// **唯一的入口。** 按进程角色分岔，别让视图去判断自己该走哪条 ——
    /// 判断散在视图里，就会有第 N 个视图哪天忘了判断，然后在 viewer 里起一套编排。
    ///
    /// - `.orchestrator`（inproc 的 GUI，或 `--daemon`）→ 起全部长期职责。
    /// - `.viewer`（总闸=daemon 的 GUI）→ 只连后台，**一个长期定时器都不起**。
    /// `begin` 收到的那两样，留着给 §9.2 的临时接管用 —— 接管发生在连不上之后，
    /// 那时早已不在 `begin` 的调用栈里了。
    private var orchestrationContext: (model: AppModel, crewStore: CrewStore)?

    func begin(model: AppModel, crewStore: CrewStore) {
        orchestrationContext = (model, crewStore)
        switch ProcessRole.requested {
        case .orchestrator:
            // 闸门在**进程入口**取好了（`OrchestrationGate.installForGUIProcess`）。
            // 这里只读它的裁决 —— **视图这条路不许自己再去问一遍锁**，再问一遍就等于
            // 把闸门挂回视图上，那正是 2026-08-26 量出来的病根。
            guard let gate = OrchestrationGate.shared else {
                // 走到这儿 = 进程入口那一步被删了或被绕过了。dev 当场响；release 保持
                // 从前的行为（照常编排），而不是让 app 变成一个什么都不做的窗口。
                assertionFailure(
                    "GUI 进程没有装编排闸门，见 PendingCrewEntry.main / OrchestrationGate")
                start(model: model, crewStore: crewStore)
                return
            }
            orchestrationDecision = gate.decision
            switch gate.decision {
            case .takeOver, .notOrchestrator:
                start(model: model, crewStore: crewStore)
            case let .followDaemon(detail):
                // 锁被一个 daemon 占着：那边真的在 socket 上听，连上去就是了。
                NSLog("[SessionHost] 本进程不接管编排，退化成 viewer：%@", detail)
                beginViewer()
            case let .conflict(detail):
                // 锁被一个不听 socket 的东西占着：**既不编排也不退化。**
                // 退化过去只会得到一个连不上的 viewer —— 界面在、什么都不动、
                // 不报错，比不做还难查。理由交给界面显示（`OrchestrationNotice`）。
                NSLog("[SessionHost] 编排冲突，本进程既不编排也不退化：%@", detail)
            }
        case .viewer:
            beginViewer()
        case .helper:
            assertionFailure("helper 进程不该起 GUI")
        }
    }

    /// 连上后台那条腿。**一个长期定时器都不起**（下面那两个的理由各自写在方法上）。
    private func beginViewer() {
        guard viewer == nil else { return }
        let viewer = ViewerSessionClient(runner: runner)
        self.viewer = viewer
        // §9.2 唯一允许的那一支：拿到独占编排锁、且后台确实起不来 → 本窗口临时接管。
        // **判断不在这里**（在 `OrchestrationFallback` 那个纯函数里），这里只执行。
        viewer.onTakeOverLocally = { [weak self] in
            MainActor.assumeIsolated { self?.takeOverLocally() }
        }
        viewer.start()
        // 这两个在 viewer 里照跑，理由各自写在方法上：一个只跟着 daemon 写好的
        // 文件走（不写），一个只读磁盘算个和（不写）。**闸门 1 防的是第二个
        // writer，不是第二个 reader。**
        QuotaCenter.shared.startFollowingFile()
        usage.startReadOnly()
    }

    /// **后台起不来时的临时本地接管**（设计 §9.2 表里唯一允许的那一支）。
    ///
    /// 走到这里时三件事都已经成立，缺一不可：本进程**确实拿到了**独占编排锁
    /// （= 确定没有别人在编排）、拉 daemon **确实失败**、viewer 那条腿**已经停了**
    /// （不会再重连出第二个 host）。判据全在 `OrchestrationFallback.decide`。
    ///
    /// **接管之后不许再交还。** 本进程从这一刻起会养真的 agent 子进程 —— 它们是
    /// 这个进程的孩子，交不给 daemon；半路把编排交还就是 §9.2 附加约束 2 点名的
    /// 「半停一半留 = 双头的另一种形状」。回到后台模式的唯一走法是重开 app，
    /// 界面上那条常驻横幅就是这么写的（`OrchestrationNotice.localFallback`）。
    private func takeOverLocally() {
        guard let context = orchestrationContext else {
            assertionFailure("临时接管时没有 begin 传下来的上下文")
            return
        }
        NSLog("[SessionHost] 后台起不来且编排锁在手，本窗口临时接管编排")
        // viewer 期间起的是**只读**那一版额度轮询（跟着 daemon 写的文件走）。
        // 现在我们自己是编排者了，得换成真的探针那一版 —— 不换的话
        // `QuotaCenter.start()` 会被它自己的 `guard timer == nil` 挡掉，
        // 于是额度那本账在整个回退期间**没有人写**，而且没有任何人会发现。
        QuotaCenter.shared.stop()
        start(model: context.model, crewStore: context.crewStore)
    }

    /// 启动全部长期职责。**幂等** —— 重复调用是 no-op（SwiftUI 的 `.task` 会因
    /// 视图重挂而重跑，这在切 crew 时是常态）。
    ///
    /// 第一行的断言是 spec §6.2 的闸门 1：viewer 进程里误起一套定时器 = 当场崩，
    /// 不是悄悄跑起来变成双头。双头的症状（账被两个进程交替覆盖、唤醒发两遍）
    /// 事后极难定位，所以宁可在这里响。
    /// 上一轮 GUI 是怎么结束的（恢复弹窗读它）。`start` 之后才有值。
    private(set) var lastExitOfPreviousRun: ProcessExitClassification = .noPriorRun
    /// 上一轮 GUI 是哪个版本 —— 「刚更新过」那一档比它，不需要第二套机制。
    private(set) var previousRunBuild: String?
    /// 要不要问人「恢复上次的 session」，以及要恢复哪些。
    /// **`shouldAsk == false` 时界面一个字都不许弹。**
    @Published private(set) var restoreOffer = SessionRestoreOffer.Decision(
        reason: nil, candidates: [], message: "")
    private var exitMarker: ProcessLifecycleMarker?
    private var terminationObserver: NSObjectProtocol?

    /// 上一轮在跑的那些。来源是 daemon 的 registry —— **不新造第二本账**。
    private static func restoreCandidates() -> [SessionRestoreOffer.Candidate] {
        let url = PendingCrewDaemonPaths.standard().registry
        guard let data = try? Data(contentsOf: url),
              let registry = try? JSONDecoder().decode(SessionProcessRegistry.self, from: data)
        else { return [] }
        return registry.entries.map {
            SessionRestoreOffer.Candidate(sessionId: $0.sessionId, crewId: $0.crewId)
        }
    }

    /// 人在弹窗里点了「恢复」。**只有这条路会恢复，没有任何自动恢复。**
    @discardableResult
    func restoreOfferedSessions(model: AppModel) async -> SessionRestoreOutcome {
        let candidates = restoreOffer.candidates
        dismissRestoreOffer()
        return await runner.restoreSessions(candidates, backend: model.backend)
    }

    /// 人点了「不恢复」，或者已经恢复过了。**问过一次就不再问** —— 同一次启动里
    /// 反复弹同一个窗，比不弹更糟。
    func dismissRestoreOffer() {
        restoreOffer = SessionRestoreOffer.Decision(reason: nil, candidates: [], message: "")
    }

    func start(model: AppModel, crewStore: CrewStore) {
        precondition(
            ProcessRole.effective == .orchestrator,
            "SessionHost.start 只能在编排者进程里调用，当前角色=\(ProcessRole.requested.rawValue)")
        guard !started else { return }
        started = true

        // 退出印记（恢复弹窗的承重件）。**GUI 也会崩** —— 2026-09-09 真崩过一次
        // （SIGABRT），只盯 daemon 的话人类撞到过的那个场景反而不弹窗。
        // **先读上一轮再写这一轮**，顺序反了就把结论盖掉了。
        let marker = ProcessLifecycleMarker(
            role: .app, build: SessionDaemonHost.currentBuild,
            onWriteFailure: { NSLog("[SessionHost] %@", $0) })
        lastExitOfPreviousRun = marker.classifyPreviousRun()
        previousRunBuild = marker.previousBuild
        marker.markRunning()
        exitMarker = marker
        // 上一轮在跑的那些（daemon 的 registry 本来就在记它们，收尸逻辑遍历的就是这份）。
        // **读不出来就是空** —— 这里读不到只会让我们少问一次，不会让我们乱恢复。
        let candidates = Self.restoreCandidates()

        // 前端更新了，本机后端跟着换代（人类 9-11 点名的那条）。
        // **必须在算恢复弹窗之前跑** —— 它可能亲手打断这些 session，
        // 而打断了就得问。判定在 `BackendUpdatePlan`（有测试）。
        let update = BackendUpdateCoordinator.runIfNeeded(
            log: { NSLog("[SessionHost] %@", $0) })

        if case let .replace(oldBuild, newBuild, _) = update {
            // 是我们自己把它打断的 —— 这一档 `decide` 判不出来（app 可能根本没更新），
            // 所以单独走一条。
            restoreOffer = SessionRestoreOffer.afterBackendReplaced(
                oldBuild: oldBuild, newBuild: newBuild, candidates: candidates)
        } else {
            restoreOffer = SessionRestoreOffer.decide(
                exit: lastExitOfPreviousRun,
                previousBuild: previousRunBuild,
                currentBuild: SessionDaemonHost.currentBuild,
                candidates: candidates)
        }
        if restoreOffer.shouldAsk {
            NSLog("[SessionHost] 上一轮：%@，有 %d 个 session 可以接回，等人决定",
                  lastExitOfPreviousRun.text, candidates.count)
        }
        // ⌘Q / 正常退出：收尾一开始记 draining，做完记 clean。崩溃走不到这里，
        // 盘上留的就还是 running —— 那正是我们要认出来的那一档。
        terminationObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak runner] _ in
            MainActor.assumeIsolated {
                marker.markDraining()
                for run in runner?.runs ?? [] where run.status == .running { run.stop() }
                marker.markClean()
            }
        }

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
        if ownsAppUpdater {
            AppUpdater.shared.isBusy = { [weak runner] in
                runner?.runs.contains { $0.status == .running } ?? false
            }
        }

        wire(crewStore: crewStore, model: model)

        // 安装/重启跨过去的机长交接：请求在旧 app 停 session 之前已单独落盘，
        // 新版编排器起来后第一时间续接被指定的 agent conversation。失败会在白板
        // fail-loud 且保留请求，下次启动继续，不会悄悄回退旧机长。
        Task { [weak runner, weak model] in
            await runner?.resumePendingCaptainReassignments(backend: model?.backend)
        }
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
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] _ in
                MainActor.assumeIsolated {
                    guard let crewStore, let model else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.captainAutostartRequests.take()
                    guard !reqs.isEmpty else { return }
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
        //
        // ⚠️ **别退回「订阅数组、处理收到的那份快照、再清空」那种写法**（2026-09-04
        // 真机实测：投 2 条命令起了 3 个 session）。那是发布「变化」、消费当「队列」：
        // 逐条 append 会发出两份快照，第二次 sink 拿到的仍是它被发出时的那份。
        // 现在脉冲只是「去看一眼」的信号，**整批从队列原子取走**。
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] _ in
                MainActor.assumeIsolated {
                    guard let crewStore, let model else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.sessionSpawnRequests.take()
                    guard !reqs.isEmpty else { return }
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

        // captain 自己发起的交接：helper 只把明确二选一请求放进共享队列；真正停旧、
        // 起新、持久化和失败回滚都在持有 live runs 的 runner 上执行。旧 captain 的
        // MCP 进程会在交接中被停掉，所以不 long-poll 工具调用；最终结果统一进群聊。
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] _ in
                MainActor.assumeIsolated {
                    guard let crewStore, let model else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.captainHandoffRequests.take()
                    guard !reqs.isEmpty else { return }
                    Task {
                        for req in reqs {
                            if crewStore.details[req.targetCrewId] == nil {
                                await crewStore.refreshDetail(req.targetCrewId)
                            }
                            guard let detail = crewStore.details[req.targetCrewId] else {
                                crewStore.postSystemNotice(
                                    crewId: req.sourceCrewId,
                                    text: "机长交接失败：拉不到 crew 详情。旧机长保持不变。")
                                continue
                            }
                            await sessionRunner.performCaptainHandoff(
                                req, detail: detail, backend: model.backend)
                            await crewStore.refreshDetail(req.targetCrewId)
                        }
                    }
                }
            }
            .store(in: &bag)

        // session 自切模型/effort（set_session_profile）：claude 注入 /model /effort,
        // codex 白板说明。数组语义同 sessionSpawnRequests（防同 tick 丢命令）。
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] _ in
                MainActor.assumeIsolated {
                    guard let crewStore else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.profileChangeRequests.take()
                    guard !reqs.isEmpty else { return }
                    Task { for req in reqs { await sessionRunner.applyProfileChange(req) } }
                }
            }
            .store(in: &bag)

        // 定时唤醒登记（schedule_wakeup）→ runner 持久化 + 挂定时器。
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] _ in
                MainActor.assumeIsolated {
                    guard let crewStore else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.wakeupRequests.take()
                    guard !reqs.isEmpty else { return }
                    for req in reqs { sessionRunner.scheduleWakeup(req) }
                }
            }
            .store(in: &bag)

        // 机长 session 操作（inspect / nudge / stop）→ runner 执行 + 写应答文件。
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] _ in
                MainActor.assumeIsolated {
                    guard let crewStore else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.sessionOpsRequests.take()
                    guard !reqs.isEmpty else { return }
                    for req in reqs { sessionRunner.applySessionOp(req) }
                }
            }
            .store(in: &bag)

        // 机长 change_workdir（改工作目录 + 迁 agent 上下文）。规划要看在跑的 run，
        // 那份状态只有 runner 有 —— 所以和 sessionOps 一样在这儿接线：算完/干完把
        // 文本写回应答文件，机长那侧的 long-poll 就拿到预览或回执了。
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] _ in
                MainActor.assumeIsolated {
                    guard let crewStore else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.workdirChangeRequests.take()
                    guard !reqs.isEmpty else { return }
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
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore] _ in
                MainActor.assumeIsolated {
                    guard let crewStore else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let reqs = crewStore.listenRequests.take()
                    guard !reqs.isEmpty else { return }
                    for req in reqs { sessionRunner.applyListen(req) }
                }
            }
            .store(in: &bag)

        // 跨 crew 汇报线消息 → 唤醒目标 crew 机长（#463）。idle 才直投注入（busy
        // 的机长下轮白板注入自然看到）；机长没在跑 → **直接拉起**（@ 唤醒语义：
        // 不在跑不能只留白板），开场 prompt 带上这条消息;拉起失败才落白板注记。
        crewStore.$pendingRequestsRevision
            .receive(on: DispatchQueue.main)
            .sink { [weak crewStore, weak model] _ in
                MainActor.assumeIsolated {
                    guard let crewStore, let model else { return }
                    // **取走语义**：不看脉冲的值，直接原子取走整批。
                    // 重复/滞后的脉冲只能取到空 —— 「同一批被处理两遍」
                    // 因此在结构上不可能，不是靠这里小心。
                    let wakes = crewStore.crewMessageWakes.take()
                    guard !wakes.isEmpty else { return }
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
