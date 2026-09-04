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

    private let runner: CrewSessionRunner
    private let paths: PendingCrewDaemonPaths
    private let spawnDaemon: () -> Bool
    private var link: UnixSocketTransport?
    private var client: SessionProtocolClient?
    private var heartbeat: Timer?
    private var reconnectAttempt = 0
    private var stopped = false

    init(runner: CrewSessionRunner,
         paths: PendingCrewDaemonPaths? = nil,
         spawnDaemon: (() -> Bool)? = nil) {
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
        link?.close()
        link = nil
        client = nil
        isConnected = false
    }

    // MARK: -

    private func connect() {
        guard !stopped else { return }
        if SessionDaemonControl.runningDaemonPid(paths: paths) == nil {
            guard spawnDaemon() else {
                lastError = "拉不起后台进程（PendingCrew --daemon）。"
                scheduleReconnect()
                return
            }
        }
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
            self.link = link
            self.client = client
            runner.attachViewer(client: client)
            client.connect()
            client.requestSessionList()
            isConnected = true
            lastError = nil
            reconnectAttempt = 0
            startHeartbeat()
        } catch {
            lastError = "连不上后台进程：\(error)"
            scheduleReconnect()
        }
    }

    private func linkClosed() {
        guard !stopped else { return }
        isConnected = false
        heartbeat?.invalidate()
        heartbeat = nil
        link = nil
        client = nil
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
                    self.link?.close()
                    self.linkClosed()
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
    private static func launchBundledDaemon() -> Bool {
        guard let executable = Bundle.main.executableURL else { return false }
        let process = Process()
        process.executableURL = executable
        process.arguments = [SessionDaemonMain.flag]
        do {
            try process.run()
            return true
        } catch {
            NSLog("[ViewerSessionClient] 拉起 daemon 失败：\(error)")
            return false
        }
    }
}
#endif
