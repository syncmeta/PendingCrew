#if os(macOS)
import Combine
import Foundation

/// **同一个二进制的第三副身份**（spec §2.3 / §9 P4）：`PendingCrew --daemon`。
///
/// 身份一是 GUI，身份二是 `--mcp-*` helper 子进程，这是第三个。不新增可执行文件
/// —— 签名/公证零改动，那是当初选「同一个二进制多副身份」的全部理由。
///
/// 这个进程里**没有窗口**：不碰 `NSApplication`，不造任何 NSView，跑的是
/// `RunLoop.main`。编排本体（`SessionHost` + `CrewSessionRunner`）与 GUI 那条路
/// 是同一份代码，差别全在 `SessionProtocolPublishing` 那一个接缝后面（§10）。
enum SessionDaemonMain {
    /// 字面值在 `SessionDaemonMainFlag`（LocalRunner 那层）—— LaunchAgent 的 plist
    /// 要带同一个 flag，而它进得了 test bundle、这里进不去。两边各写一份字面量的话，
    /// 改了一边另一边不会有任何反应。
    static let flag = SessionDaemonMainFlag.daemon

    /// 不是 daemon 就返回 false，调用方照常起 GUI。
    ///
    /// **这个函数不返回 true 之后还继续跑** —— 它在里面把 runloop 跑起来，
    /// 一直到进程退出。
    static func runIfDaemon(_ argv: [String]) -> Bool {
        guard argv.contains(flag) else { return false }
        MainActor.assumeIsolated { run(argv) }
        return true
    }

    @MainActor
    private static func run(_ argv: [String]) {
        // 脱离拉起我们的那个进程的会话/进程组。
        //
        // **A1 就靠这一行**：app 是用 `Process` 把我们拉起来的，不脱离的话我们和它
        // 同组 —— 从终端里 ⌃C 那个 app、或者 app 被整组信号带走时，daemon 跟着一起
        // 没，而「更新 app 不打断在跑的 session」当场不成立。已经是会话首时
        // `setsid()` 返回 -1（EPERM），那是正常的，不是错误。
        _ = setsid()
        let host = SessionDaemonHost()
        do {
            try host.start()
        } catch {
            // 退出码按「期望状态成立了没有」给，判定在 `DaemonExitCode`（那一层进得了
            // test bundle）：锁被**另一个 daemon** 占着 = 已经有一个在跑，安静退出是
            // 正确结局（0）；锁被 app 窗口占着 / 锁文件打不开 / 监听失败 = 一个 daemon
            // 都没有（非 0）。**这些码是给人和脚本看的，判据不许读它们**（见
            // `DaemonExitCode` 的类型注释）。
            FileHandle.standardError.write(Data(("PendingCrew daemon：\(error)\n").utf8))
            guard let start = error as? SessionDaemonHost.StartError else {
                exit(DaemonExitCode.failed)
            }
            exit(DaemonExitCode.forDaemonStart(
                start, launchedByLaunchd: PendingCrewLaunchAgent.launchedByLaunchd(argv)))
        }

        // 编排本体。**与 GUI 那条路同一份代码**，只是发布口换成了 socket 服务端。
        let model = AppModel()
        let crewStore = CrewStore(appModel: model)
        let runner = CrewSessionRunner(
            sessionPublisher: DaemonSessionPublisher(server: host.server))
        let sessionHost = SessionHost(runner: runner, ownsAppUpdater: false)
        trackRoster(of: runner, into: host)
        sessionHost.start(model: model, crewStore: crewStore)

        // 排空共享控制文件的那条监听挂在首刷上（`CrewStore.refreshList`）。
        // GUI 里由界面触发，daemon 里没有界面 —— 不主动刷一次的话，机长的
        // `start_session` 等命令永远没人接（§6.1：排空方从 app 搬到 daemon）。
        Task { await crewStore.refreshList() }

        wireOrchestrationRequests(host: host, runner: runner,
                                  crewStore: crewStore, model: model)
        installGracefulShutdown(host: host, runner: runner)
        host.log.write("编排已就位，进入 runloop")
        RunLoop.main.run()
    }

    /// viewer 那侧问「roster 长什么样」「帮我起一个 / 停一个」时的应答（P4）。
    ///
    /// **编排请求一律落在同一批 runner 方法上** —— 与 GUI 自己点的时候走的是同一条路，
    /// 不另开一套。另开一套的下场看 §2.5 那条：接线接在视图上，两条路悄悄分叉，
    /// 其中一条一次都没被调用过，而且没有任何报错。
    @MainActor
    private static func wireOrchestrationRequests(
        host: SessionDaemonHost, runner: CrewSessionRunner,
        crewStore: CrewStore, model: AppModel
    ) {
        host.server.runSummaryProvider = { [weak runner] sessionId in
            runner?.runs.first { $0.sessionId == sessionId }?.protocolSummary
        }
        host.server.onOrchestrationRequest = { control in
            MainActor.assumeIsolated {
                handle(control, runner: runner, crewStore: crewStore,
                       model: model, log: host.log)
            }
        }
        // roster 变了就把全量推给所有 viewer —— 起了新 session、某个退出了、
        // 档位切了，右栏都要立刻跟上。不推的话 viewer 要等下一次自己发 listSessions。
        runner.$runs
            .receive(on: DispatchQueue.main)
            .sink { _ in
                MainActor.assumeIsolated { host.server.broadcastSessionList() }
            }
            .store(in: &rosterBag)
    }

    @MainActor
    private static func handle(
        _ control: SessionControl, runner: CrewSessionRunner,
        crewStore: CrewStore, model: AppModel, log: SessionDaemonLog
    ) {
        func string(_ key: String) -> String? {
            guard case let .string(value)? = control.arguments[key], !value.isEmpty else {
                return nil
            }
            return value
        }
        func bool(_ key: String) -> Bool {
            if case let .bool(value)? = control.arguments[key] { return value }
            return false
        }
        func run(_ key: String = "sessionId") -> CrewSessionRun? {
            guard let sessionId = string(key) else { return nil }
            return runner.runs.first { $0.sessionId == sessionId }
        }

        switch control.op {
        case SessionOrchestrationOp.startSession:
            guard let crewId = string("crewId") else { return }
            let isCaptain = string("role") == "captain"
            Task { @MainActor in
                // detail 缓存只在 UI 打开过该 crew 后才有；daemon 里根本没有 UI，
                // 所以这里**总是**现拉（对齐 `SessionHost.wire` 里那两处缓存 miss 兜底）。
                if crewStore.details[crewId] == nil { await crewStore.refreshDetail(crewId) }
                guard let detail = crewStore.details[crewId] else {
                    crewStore.postSystemNotice(
                        crewId: crewId, text: "起 session 失败：拉不到 crew 详情。")
                    return
                }
                do {
                    if isCaptain {
                        try await runner.startCaptain(
                            detail: detail, backend: model.backend,
                            wakeText: string("wakeText"), openingBrief: string("brief"))
                    } else {
                        try await runner.startForBrief(
                            detail: detail, backend: model.backend,
                            brief: string("brief") ?? "",
                            runnerOverride: string("runner")
                                .flatMap(LocalCodingAgentKind.init(rawValue:)),
                            isolation: bool("isolation"),
                            model: string("model"), effort: string("effort"),
                            title: string("title"),
                            userInitiated: bool("userInitiated"))
                    }
                } catch {
                    runner.reportStartFailure(
                        crewId: crewId, brief: string("brief"), error: error,
                        mentionCaptain: !isCaptain)
                }
            }
        case SessionOrchestrationOp.stopRun:
            guard let target = run() else { return }
            runner.stop(target.runID)
        case SessionOrchestrationOp.removeRun:
            guard let target = run() else { return }
            runner.remove(target.runID)
        case SessionOrchestrationOp.sendText:
            guard let target = run(), let text = string("text") else { return }
            target.send(text)
        case SessionOrchestrationOp.interrupt:
            run()?.interrupt()
        case SessionOrchestrationOp.profileChange:
            guard let sessionId = string("sessionId"), let crewId = string("crewId") else { return }
            Task { @MainActor in
                await runner.applyProfileChange(.init(
                    crewId: crewId, sessionId: sessionId,
                    model: string("model"), effort: string("effort")))
            }
        case SessionOrchestrationOp.captainHandoff:
            // 整笔交接（Todo #101）。detail 同 startSession 那条：daemon 里没有 UI，
            // 缓存永远是 miss，所以总是现拉。
            guard let crewId = string("crewId") else { return }
            Task { @MainActor in
                if crewStore.details[crewId] == nil { await crewStore.refreshDetail(crewId) }
                guard let detail = crewStore.details[crewId] else {
                    crewStore.postSystemNotice(
                        crewId: crewId, text: "机长交接失败：拉不到 crew 详情。旧机长保持不变。")
                    return
                }
                await runner.performForwardedCaptainHandoff(
                    crewId: crewId, sessionId: string("sessionId"),
                    runnerRaw: string("runner"), brief: string("brief") ?? "",
                    detail: detail, backend: model.backend)
                await crewStore.refreshDetail(crewId)
            }
        case SessionOrchestrationOp.approvalMode:
            guard let target = run(), let raw = string("reviewer"),
                  let reviewer = CodexProtocol.ApprovalsReviewer(rawValue: raw) else { return }
            Task { @MainActor in
                await runner.applyCodexApprovalMode(to: target, reviewer: reviewer)
            }
        default:
            log.write("未知编排请求 \(control.op)，忽略（§4.4：新增能力不断连）")
        }
    }

    /// roster → registry（§8.2）。`SessionDaemonHost` 不认识 `CrewSessionRunner`
    /// （它编进单测 bundle，不许依赖编排层），所以三元组在这里摘。
    @MainActor
    private static func trackRoster(of runner: CrewSessionRunner, into host: SessionDaemonHost) {
        runner.$runs
            .receive(on: DispatchQueue.main)
            .sink { runs in
                MainActor.assumeIsolated {
                    host.updateRegistry(from: runs.compactMap { run in
                        guard run.status == .running,
                              let source = run.backend as? SessionProcessIdentifying else { return nil }
                        return SessionDaemonHost.RosterProcess(
                            sessionId: run.sessionId, crewId: run.crewId,
                            pid: source.agentProcessIdentifier)
                    })
                }
            }
            .store(in: &rosterBag)
    }

    @MainActor private static var rosterBag = Set<AnyCancellable>()

    /// SIGTERM → 先停掉所有 session，再放锁退出。**这也是唯一的停用通路**
    /// （`--daemon-stop` 就是替人找到 pid 再发这个信号）。
    ///
    /// 不停 session 就退的话，「清除本机所有数据」（先停 daemon 再删，§6.1）会留下
    /// 一地没人管的 agent 子进程 —— 而且 registry 刚好也被那次删除清掉了，**连回收
    /// 它们的依据都没了**。
    ///
    /// 收尾的三条约束（退出码、预算、**计时器不许挂在主队列上**）连同它们的理由都在
    /// `DaemonShutdown.swift`，并且有测试钉着 —— P5b 之后「正常退出」是人能不能关掉
    /// 这个后台的唯一依据，一句注释拦不住改它的人。
    @MainActor
    private static func installGracefulShutdown(
        host: SessionDaemonHost, runner: CrewSessionRunner
    ) {
        let shutdown = DaemonGracefulShutdown(
            stopSessions: {
                MainActor.assumeIsolated {
                    host.log.write("收尾：停掉 \(runner.runs.count) 个 session")
                    for run in runner.runs where run.status == .running { run.stop() }
                }
            },
            releaseHost: { MainActor.assumeIsolated { host.stop() } })
        signal(SIGTERM, SIG_IGN)      // 交给 DispatchSource，别让默认动作抢先
        signal(SIGINT, SIG_IGN)
        for sig in [SIGTERM, SIGINT] {
            let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            source.setEventHandler {
                MainActor.assumeIsolated {
                    host.log.write("收到信号 \(sig)")
                    shutdown.begin()
                }
            }
            source.resume()
            Self.signalSources.append(source)
        }
    }

    /// DispatchSource 必须被持有，否则装完就被回收、信号处理静默失效。
    @MainActor private static var signalSources: [DispatchSourceSignal] = []
}
#endif
