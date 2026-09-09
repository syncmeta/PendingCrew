#if os(macOS)
import Combine
import Foundation

/// **app 退化成 viewer 之后，它与 daemon 之间的那条腿**（spec §4.5 / §9 P4）。
///
/// 只在总闸 `PENDINGCREW_BACKEND=daemon` 时存在。它做四件事，一件都不多：
/// 拉起 daemon（如果还没跑）、连上去、断了重连、把 roster 对齐到本地镜像。
///
/// **它不做编排。** 这个进程里一个定时器都不该属于编排（§6.2 闸门 1 会当场崩），
/// 下面唯一的定时器是心跳，那是链路的事，不是 session 的事。
@MainActor
final class ViewerSessionClient: ObservableObject {
    /// app 侧声明的能力集。与 daemon 那份取交集，缺什么少什么功能，不拒连（§4.4）。
    static let capabilities = SessionDaemonHost.defaultCapabilities

    /// 连上了没有。UI 可以据此显示「正在连接后台…」。
    @Published private(set) var isConnected = false
    /// 连不上时给人看的原因。**不静默重试到天荒地老** —— 那是「点了没反应」的经典形状。
    @Published private(set) var lastError: String?
    /// §9.2 的降级裁决（`nil` = 还没做过判定）。连不上只是现象，这一条是
    /// 「连不上之后我们决定怎么办」——而那正是不许静默的地方。
    @Published private(set) var fallback: OrchestrationFallback.Decision?

    /// 拿到独占编排锁、后台确实起不来时回调一次（§9.2 唯一允许的那一支）。
    /// 由 `SessionHost` 挂上去起本地编排 —— **这条腿自己不编排任何东西**。
    var onTakeOverLocally: (() -> Void)?

    /// 拉起 daemon 的结果。
    ///
    /// **不是 `Bool`**（2026-09-04 真机逮到的洞）：`Process.run()` 没抛错只说明
    /// exec 成了，而 daemon 在拿不到编排锁 / 打不开锁文件时会打一行原因然后
    /// **当场 `exit(0)`** —— 只看 run() 会把这两种都记成「起来了」，于是契约里
    /// 唯一允许接管的那一支在「后台起不来」最常见的原因下根本到不了。
    /// 所以这里交出的是一个**能一直问「它还活着吗」的探针**。
    enum DaemonLaunch {
        case threw(String)
        case started(child: () -> DaemonLaunchRace.ChildState)
    }

    /// 拉起之后最多等多久还没握上手就下「说不准」的结论（判据第 4 条）。
    static let launchRaceLimit: TimeInterval = 15
    private static let racePollInterval: TimeInterval = 0.25
    /// 「后台在跑，我继续重连」这句话的寿命（`decide` 的 `stallLimit`）。
    static let stallLimit: TimeInterval = 60

    private let runner: CrewSessionRunner
    private let paths: PendingCrewDaemonPaths
    /// 编排锁所在的数据根（锁文件就落在它下面，同 `SessionDaemonControl`）。
    private var dataRoot: URL { paths.lock.deletingLastPathComponent() }
    private let spawnDaemon: () -> DaemonLaunch
    /// 这一串连不上是从什么时候开始的（连上就清）。喂给 `decide` 的 `stalledFor`。
    private var stalledSince: Date?
    /// 上一次我们拉起来的那个子进程。**还活着就别再拉一个** —— 否则每次
    /// 「说不准」都会再造一个不回话的 daemon 出来。
    private var lastSpawnedChild: (() -> DaemonLaunchRace.ChildState)?
    private var raceStartedAt: Date?
    /// 这条链路走到哪了。**`socketOpen` 不等于连上** —— 只有收到 `daemonHello`
    /// 且协商兼容（`SessionProtocolClient.isConnected` 翻 true）才算握上手。
    /// 这两者差着一整个握手，而把前者当后者正是 2026-09-04 读代码逮到的那条偏离。
    private var linkState: DaemonLaunchRace.LinkState = .none
    private var link: UnixSocketTransport?
    private var client: SessionProtocolClient?
    private var heartbeat: Timer?
    private var reconnectAttempt = 0
    private var stopped = false

    init(runner: CrewSessionRunner,
         paths: PendingCrewDaemonPaths? = nil,
         spawnDaemon: (() -> DaemonLaunch)? = nil) {
        self.runner = runner
        let resolved = paths ?? .standard()
        self.paths = resolved
        self.spawnDaemon = spawnDaemon ?? { ViewerSessionClient.launchBundledDaemon() }
    }

    func start() {
        stopped = false
        connect()
    }

    func stop() {
        stopped = true
        heartbeat?.invalidate()
        heartbeat = nil
        teardownLink()
        raceStartedAt = nil
        isConnected = false
    }

    /// **放掉这条链路的唯一出口**（Todo #138 ①）。
    ///
    /// 以前三条自发关闭路径（`stop()` / 赛跑判定子进程先退了 / 心跳判定对端没回应）
    /// 各写各的 `link?.close(); link = nil; client = nil`，**谁都没通知过底下那批
    /// 后端**：它们的连接句柄和能力表原样留着最后一次成功的值。改 weak 之后不再崩，
    /// 但开始骗人 —— 「读屏」「投唤醒」这类先看能力表的调用会以为通道还在。
    ///
    /// 病根不在这三处各自写错，而在 `SessionMessageLink.close()` 按约定不触发
    /// `onClose`（这条约定是对的：重连策略不该被自己的 detach 触发），而 `onClose`
    /// 后面同时挂着**状态清理**和**重连策略**两件事。`SessionProtocolClient.close()`
    /// 把它们拆开了，这里只要走那一个口子。
    ///
    /// **这个方法是唯一允许出现 `client = nil` 的地方**，`ViewWiringTests` 里那条
    /// 断言盯着这一点 —— 多一处就是多一条会漏掉状态清理的路。
    private func teardownLink() {
        client?.close()
        link?.close()
        link = nil
        client = nil
        linkState = .none
    }

    // MARK: -

    private func connect() {
        guard !stopped else { return }
        if stalledSince == nil { stalledSince = Date() }
        raceStartedAt = Date()
        // **该不该拉一个新的，不在这里判** —— 判断在 `DaemonLaunchPlan`（那一层进得了
        // test bundle）。这里只执行。P4 的教训：判断留在接线上就只有「编译过」。
        switch DaemonLaunchPlan.next(
            daemonHoldsLock: SessionDaemonControl.runningDaemonPid(paths: paths) != nil,
            lastSpawnedChild: lastSpawnedChild?()) {
        case let .connectOnly(reason):
            NSLog("[ViewerSessionClient] 不拉后台进程：%@", reason)
            pollRace(child: nil)
        case .launch:
            launchAndRace()
        }
    }

    /// 拉起 daemon，然后**并行等两件事**：首次协议握手 / 子进程终止，谁先到算谁
    /// （判据第 1–4 条，判定本身在 `DaemonLaunchRace`）。
    ///
    /// **这不是「睡一个宽限窗再看它还在不在」** —— 那是在赌时序：窗给短了会把
    /// 「正要退的」判成起成了，给长了每次启动都白等。这里每一拍都问一次真实观测。
    private func launchAndRace() {
        switch spawnDaemon() {
        case let .threw(reason):
            lastError = "拉不起后台进程：\(reason)"
            applyFallback(
                spawn: OrchestrationFallback.spawn(
                    launchThrew: reason, race: .pending, limit: Self.launchRaceLimit),
                linkFailure: nil)
        case let .started(child):
            lastSpawnedChild = child
            pollRace(child: child)
        }
    }

    /// 赛跑的一拍。`child` 为 nil = 这一轮我们没拉过任何进程。
    private func pollRace(child: (() -> DaemonLaunchRace.ChildState)?) {
        guard !stopped, let startedAt = raceStartedAt else { return }
        openSocketIfNeeded()
        let outcome = DaemonLaunchRace.step(
            link: linkState,
            child: child?() ?? .notSpawned,
            elapsed: Date().timeIntervalSince(startedAt),
            limit: Self.launchRaceLimit)
        switch outcome {
        case .pending:
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.racePollInterval) {
                [weak self] in
                MainActor.assumeIsolated { self?.pollRace(child: child) }
            }
        case .handshake:
            raceStartedAt = nil
            finishConnected()
        case .exitedBeforeHandshake, .timedOutStillAlive, .timedOutWithoutSpawn:
            raceStartedAt = nil
            if case .exitedBeforeHandshake = outcome {
                // 它没了，那条 socket 也就没意义了 —— 下一轮重开。
                lastSpawnedChild = nil
                teardownLink()
            }
            applyFallback(
                spawn: OrchestrationFallback.spawn(
                    launchThrew: nil, race: outcome, limit: Self.launchRaceLimit),
                linkFailure: nil)
        }
    }

    /// 开一条 socket 并把 hello 发出去（幂等：已经开着就什么都不做）。
    ///
    /// **它不宣布「连上了」** —— 那一拍归 `onDaemonHello`。`connect()` 成功与
    /// 「hello 发出去了」都是**本端单方面就能达成**的事，不能拿来当「对端回过话」
    /// 的证据；判准只有一条：**这个信号必须只有对端真的回过话才可能出现**。
    private func openSocketIfNeeded() {
        guard link == nil else { return }
        do {
            let link = try UnixSocketTransport.connect(toPath: paths.socket)
            let client = SessionProtocolClient(
                link: link, capabilities: Self.capabilities,
                appBuild: SessionDaemonHost.currentBuild)
            client.onLinkClosed = { [weak self] in
                MainActor.assumeIsolated { self?.linkClosed() }
            }
            client.onSessionList = { [weak self] list in
                MainActor.assumeIsolated { self?.runner.applyRemoteRoster(list) }
            }
            // **握上手的那一拍**：daemon 回了 hello 且协议/能力协商兼容
            // （`SessionProtocolClient` 只在这两条都成立时才调它）。
            client.onDaemonHello = { [weak self] _ in
                MainActor.assumeIsolated { self?.linkState = .handshaken }
            }
            self.link = link
            self.client = client
            runner.attachViewer(client: client)
            client.connect()
            client.requestSessionList()
            linkState = .socketOpen
        } catch {
            lastError = "连不上后台进程：\(error)"
            linkState = .none
        }
    }

    /// 真握上手之后才做的那些事。
    private func finishConnected() {
        isConnected = true
        lastError = nil
        // 连上了 = 上一轮那条降级裁决过期了，别在屏幕上留一条过期横幅。
        fallback = nil
        stalledSince = nil
        lastSpawnedChild = nil
        reconnectAttempt = 0
        startHeartbeat()
    }

    /// **连不上之后决定怎么办**（设计 §9.2）。顺序那一段在
    /// `OrchestrationFallbackCoordinator`（本文件在 `Sources/Mac/Services`、
    /// 进不了 test bundle，而「什么时候才许取锁 / 接管后有没有真的停腿 / 没接管
    /// 有没有真的放锁」三件事做错都是**安静地坏**，必须有测试盯着）。
    private func applyFallback(spawn: OrchestrationFallback.Spawn,
                               linkFailure: OrchestrationFallback.LinkFailure?) {
        let coordinator = OrchestrationFallbackCoordinator(
            dataRoot: dataRoot,
            hooks: .init(
                acquireLock: { SessionOrchestratorLock.acquire(dataRoot: $0, kind: "app") },
                takeOver: { handle, reason in
                    LocalOrchestrationFallback.shared.takeOver(handle: handle, reason: reason)
                },
                stopViewerLeg: { [weak self] in
                    self?.stop()
                    self?.onTakeOverLocally?()
                },
                scheduleReconnect: { [weak self] in self?.scheduleReconnect() }))
        fallback = coordinator.handle(
            spawn: spawn, linkFailure: linkFailure,
            stalledFor: stalledSince.map { Date().timeIntervalSince($0) })
    }

    private func linkClosed() {
        guard !stopped else { return }
        isConnected = false
        heartbeat?.invalidate()
        heartbeat = nil
        teardownLink()
        runner.viewerLinkClosed()
        scheduleReconnect()
    }

    private func scheduleReconnect() {
        guard !stopped else { return }
        let delay = SessionReconnectPolicy.delay(forAttempt: reconnectAttempt)
        reconnectAttempt += 1
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            MainActor.assumeIsolated { self?.connect() }
        }
    }

    /// §4.5：10 秒一 ping，30 秒没 pong 判定断线。
    ///
    /// **不能只靠 `onClose`**：对端进程被 SIGKILL 时 socket 会收到 FIN，那条走得通；
    /// 但半开连接（对端卡死、链路默默没了）不会有 FIN —— socket 一直「open」着，
    /// 一个字节都不来，而右栏看起来一切正常。心跳是唯一能把这种情况变成可见的东西。
    private func startHeartbeat() {
        heartbeat?.invalidate()
        client?.noteHeartbeat()
        heartbeat = Timer.scheduledTimer(
            withTimeInterval: SessionReconnectPolicy.pingInterval, repeats: true
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, let client = self.client else { return }
                if Date().timeIntervalSince(client.lastPongAt) > SessionReconnectPolicy.pongTimeout {
                    self.lastError = "后台进程 \(Int(SessionReconnectPolicy.pongTimeout)) 秒没有回应，正在重连。"
                    self.linkClosed()   // teardownLink() 会关 link
                    return
                }
                client.ping()
            }
        }
    }

    /// 拉起 `PendingCrew --daemon`。
    ///
    /// **同一个二进制**（§2.3）—— 不需要 embed 第二个可执行文件，签名/公证零改动。
    /// 拉起后它会自己 `setsid()` 脱离本进程的会话，所以 app 退出/被杀不会带走它；
    /// 那正是 A1（更新 app 不打断在跑的 session）的前提。
    ///
    /// P5 会换成 `SMAppService.agent`（开机自启 + 崩溃自拉）；这里是 P4 的手动通道。
    private static func launchBundledDaemon() -> DaemonLaunch {
        guard let executable = Bundle.main.executableURL else {
            return .threw("找不到本 app 的可执行文件路径")
        }
        let process = Process()
        process.executableURL = executable
        process.arguments = [SessionDaemonMain.flag]
        do {
            try process.run()
        } catch {
            NSLog("[ViewerSessionClient] 拉起 daemon 失败：\(error)")
            return .threw(error.localizedDescription)
        }
        // **交出探针，不交出「成了」。** daemon 不做 double-fork（只 setsid），
        // 所以这个进程就是 daemon 本身，「它还在不在」是直接观测得到的事实。
        // `terminationStatus` 在进程还活着时会 trap，所以必须先问 `isRunning`。
        return .started(child: {
            process.isRunning ? .alive : .exited(process.terminationStatus)
        })
    }
}
#endif
