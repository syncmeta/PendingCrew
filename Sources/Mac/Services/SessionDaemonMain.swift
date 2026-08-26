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
    static let flag = "--daemon"

    /// 不是 daemon 就返回 false，调用方照常起 GUI。
    ///
    /// **这个函数不返回 true 之后还继续跑** —— 它在里面把 runloop 跑起来，
    /// 一直到进程退出。
    static func runIfDaemon(_ argv: [String]) -> Bool {
        guard argv.contains(flag) else { return false }
        MainActor.assumeIsolated { run() }
        return true
    }

    @MainActor
    private static func run() {
        let host = SessionDaemonHost()
        do {
            try host.start()
        } catch {
            // 拿不到单实例锁是**正常结局**，不是错误：说明已经有一个 daemon 在跑，
            // 本进程安静退出即可（第二个 daemon 就是双头本身，§6.2）。
            FileHandle.standardError.write(Data(("PendingCrew daemon：\(error)\n").utf8))
            exit(error is SessionDaemonHost.StartError ? 0 : 1)
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

        installGracefulShutdown(host: host, runner: runner)
        host.log.write("编排已就位，进入 runloop")
        RunLoop.main.run()
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

    /// SIGTERM → 先停掉所有 session，再放锁退出。
    ///
    /// 不这么做的话，「清除本机所有数据」（先停 daemon 再删，§6.1）会留下一地
    /// 没人管的 agent 子进程 —— 而且 registry 刚好也被那次删除清掉了，**连回收
    /// 它们的依据都没了**。
    @MainActor
    private static func installGracefulShutdown(
        host: SessionDaemonHost, runner: CrewSessionRunner
    ) {
        signal(SIGTERM, SIG_IGN)      // 交给 DispatchSource，别让默认动作抢先
        signal(SIGINT, SIG_IGN)
        for sig in [SIGTERM, SIGINT] {
            let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            source.setEventHandler {
                MainActor.assumeIsolated {
                    host.log.write("收到信号 \(sig)，停掉 \(runner.runs.count) 个 session 后退出")
                    for run in runner.runs where run.status == .running { run.stop() }
                    host.stop()
                    // 给 SIGTERM → SIGKILL 的升级留一点时间（`terminateTree` 是
                    // 异步的），再退。停不掉的那些由下一轮的孤儿核对兜底。
                    DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { exit(0) }
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
