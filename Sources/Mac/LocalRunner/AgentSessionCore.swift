#if os(macOS)
import Foundation
import Combine
import SwiftTerm

/// **无画面的终端内核**（spec `docs/internal/2026-08-19-backend-split-design.md` §5.2）。
///
/// 在这个类型出现之前，PTY、屏幕缓冲区、渲染、以及「它在忙 / 撞额度了 / 卡在
/// 选择菜单等人按」那套从画面上认状态的逻辑，全长在**同一个 AppKit 视图对象**上、
/// 全跑主线程。那既是「关掉 app 就全停」的一半，也是 tech-debt 里
/// 「PTY 每批输出都要过主线程、代价随 session 数线性涨」那条的结构成因。
///
/// 现在它们在这里，而这里**没有一行 AppKit**：`Terminal` 与 `LocalProcess` 都是
/// 纯 Foundation（SwiftTerm 自带的 `HeadlessTerminal` 就是这个组合）。
/// 所以 P4 时这个类可以原样搬进没有画面的后台进程。
///
/// ⚠️ **不许在本文件 import AppKit / SwiftUI。** 有一条测试盯着这件事。
@MainActor
final class AgentSessionCore: NSObject, TerminalDelegate, LocalProcessDelegate {

    /// 内核的两种用法。**不是配置项，是两套本来就不同的行为**，劈分前分别长在
    /// `AgentTerminalSession` 与 `PlainTerminalSession` 上，这里逐字保留：
    /// - `.agent`：全套状态扫描器 + 拉起自检看门狗；退出回调先过一遍「起来即死」判定。
    /// - `.plainShell`：人直接操作的 shell —— 什么都不扫（普通 shell 的输出里出现
    ///   「usage limit」之类字样是它自己的事，不该翻成 session 健康异常），
    ///   退出码按 `wait(2)` 位布局解码，spawn 失败当场落 `.exited(127)`。
    enum Mode {
        case agent
        case plainShell
    }

    /// 回滚历史行数上限。理由与代价见 `TerminalMirrorView` 上同名常量的长注释
    /// （两侧必须一致，否则 mirror 能滚到的历史比 core 记得的多/少 —— 所以那边
    /// 直接引用这里的值，不另写一个数）。
    static let scrollbackLines = 10_000

    let kind: LocalCodingAgentKind
    private let mode: Mode

    private(set) var terminal: Terminal!
    private(set) var process: LocalProcess!

    /// mirror 挂这里拿字节。**core 自己不知道有没有人在看** —— 没人挂就没人收，
    /// 那正是 P4 之后「没人看的 session 不占主线程」的形状。
    var onOutput: ((ArraySlice<UInt8>) -> Void)?

    /// `status` 翻成 `.exited` 的那一拍（门面据此收窄两份回滚缓冲）。
    var onExited: (() -> Void)?

    /// 0.6s 轮询的每一拍（门面挂在这里刷新外置滚动条几何 —— 那是视图的事，
    /// 但节拍必须与劈分前同一个 timer 一致，所以由内核统一发）。
    var onTick: (() -> Void)?

    /// 最近一次收到子进程输出的时刻；busy 判定 = now - lastOutputAt < 阈值。
    private(set) var lastOutputAt = Date.distantPast
    /// 当前视口尺寸（没有 mirror 在看时保持最后一次的值，**绝不 resize 成 0**）。
    /// 初值对齐 `TerminalOptions.default` 的 80×25 —— 劈分前视图以 `.zero` frame
    /// 构造、零尺寸守卫让它停在同一个默认值上，PTY 拿到的初始 winsize 因此不变。
    private(set) var cols = TerminalOptions.default.cols
    private(set) var rows = TerminalOptions.default.rows

    var isProcessRunning: Bool { process?.running == true }

    // MARK: - 派生状态（劈分前长在 AgentTerminalSession 上）

    /// 进程终止的那一拍顺手把回滚缓冲收窄（2026-08-18 第二条）——
    /// 挂在 `didSet` 上而不是各个退出路径里：正常退出 / 用户停 / 拉起失败自检
    /// 三条路都各自翻 `status`，逐个去加容易漏，且漏掉的那条就是内存不放的那条。
    @Published private(set) var status: SessionStatus = .running {
        didSet {
            if case .exited = status { onExited?() }
        }
    }
    /// UI 头像「干活中/空闲」用（与 isBusy 解耦）：最近 ~1s 还在吐 PTY 输出=干活，
    /// 安静=空闲。0.6s 轮询 `lastOutputAt` 重算；status 非 running 时恒 false。
    @Published private(set) var isWorking = false
    /// 群聊「正在输入」气泡专用的显示态（Todo #24）—— **故意与 `isWorking` 分家**。
    /// `isWorking` 是「最近 1s 有 PTY 输出」的原始活跃信号，被唤醒回执采样
    /// （`CrewSessionRunner` 的注入确认）、SessionHealth 的 working/idle 上报、
    /// 状态点聚合、限额恢复 streak 一起吃着；给它加迟滞会让那些判定一起变钝
    /// （回执要的正是「注入后马上有反应」这个即时性）。所以原始信号原样保留，
    /// 另出一条只服务 UI 的：见 `TypingActivityTracker`（指纹 + 不对称迟滞）。
    @Published private(set) var displayIsTyping = false
    /// PTY 输出健康扫描（未登录/额度到顶）—— 每 Kind 只翻一次;`health` 保留
    /// 最近一条(auth 优先级高于 quota 的覆盖顺序由到达先后决定,展示层不细分)。
    @Published private(set) var health: CrewSessionHealth? {
        didSet {
            // 额度类 health 置上的时刻 —— 恢复判定只认「此刻之后才开始」的干活 streak，
            // 免得把「打印限额报错那一阵输出」当成已恢复。见 `QuotaHealthRecovery`。
            if health?.isQuotaRelated == true {
                quotaHealthAt = Date()
                workingSince = nil
            } else if health == nil {
                quotaHealthAt = nil
            }
        }
    }
    /// 当前正卡着的那个菜单（nil = 没在等）。run 侧观察它发群 + 翻状态。
    @Published private(set) var pendingDecision: PendingTerminalDecision?
    /// 扫到的「传了但没生效」问题。run 侧观察它 fail-loud 进白板 ——
    /// 进程活着、也在吐字，`SessionLaunchProbe` 判 `.alive` 不会报，只有这条会。
    @Published private(set) var launchParameterProblem: SessionLaunchParameterProblem?

    /// 「正在输入」判定用的明文提取器（每 chunk 清一次窗，只要这一笔的可见文本）。
    private let typingStripper = AnsiPlainTextTail(tailLimit: 1024)
    private var typingActivity = TypingActivityTracker()
    /// 额度类 health 被置上的时刻（`QuotaHealthRecovery` 用）。
    private var quotaHealthAt: Date?
    /// 当前这段**连续**干活的起点（`isWorking` false→true 时置，true→false 时清）。
    private var workingSince: Date?
    private let healthScanner = SessionHealthScanner()
    /// rate-limit 模态菜单检测（Todo #10 层1）—— 命中即自动应答，别让 session
    /// 卡死等人按键（2026-07-19 全员卡一天的根因之一）。
    private let rateLimitScanner = RateLimitMenuScanner()
    /// 「终端在等人选」跟踪（Todo #6）—— 认出 claude 自己弹的菜单（命令审批 /
    /// 计划确认 / 信任文件夹 / 选登录方式）。这些**不经过** PreToolUse 权限钩子
    /// （那个只 gate computer-use），所以此前群里一个字都没有，人不盯右栏这个
    /// session 就一直干等。与 rate-limit 那个的分工：那个能替它答（固定选 1），
    /// 这个是拍板类，**只报不答**。
    private let decisionTracker = PendingDecisionTracker()
    /// 切模型/effort 在途时挂上的回显扫描器（`applyProfileSwitch` 持有，用完置 nil）。
    private var profileEchoScanner: SessionProfileEchoScanner?
    /// 启动参数没被 CLI 接受的首屏扫描器（Todo #36）。只在拉起窗口内活着，
    /// 过期/报完两类就置 nil 停扫。没显式传 model/effort 时压根不建。
    private var launchParameterScanner: SessionLaunchParameterScanner?
    private var busyTimer: Timer?

    /// `executable` = 已 resolve 的 claude/codex 绝对路径（`.plainShell` 时是用户的
    /// 登录 shell）；`workdir` = 工作目录。argv（含首条指令的 positional prompt、
    /// auto mode、effort）由 `config.argv()` 构建。
    init(config: SessionConfig,
         mode: Mode = .agent,
         executable: String,
         workdir: String,
         env: [String: String]) {
        self.kind = config.kind
        self.mode = mode
        super.init()

        var opts = TerminalOptions.default
        opts.scrollback = Self.scrollbackLines
        terminal = Terminal(delegate: self, options: opts)
        // dispatchQueue 留空 = `DispatchQueue.main`，与劈分前那个「视图自己开进程」
        // 的 SwiftTerm 基类默认完全一致。P1 仍是单进程、字节仍走主线程；把它挪到 per-session 队列
        // 是 P4 的事（那时候没有主线程可挪回去了）。
        process = LocalProcess(delegate: self)

        // 启动参数没被接受的首屏扫描（Todo #36）—— 必须在 startProcess **之前**挂好，
        // 那两条警告是子进程吐的第一批字节。没显式传 model/effort 就不建（`isIdle`）。
        if mode == .agent {
            let paramScanner = SessionLaunchParameterScanner(model: config.model, effort: config.effort)
            launchParameterScanner = paramScanner.isIdle ? nil : paramScanner
        }

        var envArr = env.map { "\($0.key)=\($0.value)" }
        if !envArr.contains(where: { $0.hasPrefix("TERM=") }) { envArr.append("TERM=xterm-256color") }
        process.startProcess(
            executable: executable,
            args: config.argv(),
            environment: envArr,
            currentDirectory: workdir)

        switch mode {
        case .plainShell:
            if !isProcessRunning || process.shellPid <= 0 { status = .exited(127) }
        case .agent:
            // 拉起自检（#541）：**别把「构造完了」当成「跑起来了」**。SwiftTerm 的
            // forkpty 分支失败时静默 return（不置 running、不回调 delegate），且它的
            // 退出事件源 `activate()` 早于 `setEventHandler` —— 秒退的子进程（额度耗尽
            // 的 CLI 启动即退）退出事件可能没人接。两种情况都会让 status 永远停在
            // `.running`、`isWorking` 恒假 → 点名推导出「🟡 空闲」，机长照常派活。
            // 这里自己核实 spawn，并起看门狗盯「进程没了却没回调」「活着但零输出」。
            let spawned = isProcessRunning && process.shellPid > 0
            startLaunchWatchdog(spawned: spawned, executable: executable)
            // 0.6s 轮询输出活跃度 → 翻 isBusy。定时器在 main runloop 触发(init 是
            // @MainActor)；[weak self] 不成环，exit/stop/deinit 时 invalidate。
            busyTimer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.recomputeWorking()
                    self?.pollPendingDecision()
                    // canScroll/thumbSize 会因输出增长而变，但只在 yDisp 变化时才有 scrolled
                    // 回调；顶到底持续吐字时 yDisp 每行都动能覆盖，静止但 buffer 变化的边角用
                    // 轮询兜一层（非用户主动，只同步几何不点亮）。
                    self?.onTick?()
                }
            }
        }
    }

    deinit {
        busyTimer?.invalidate()
        launchWatchdog?.cancel()
    }

    // MARK: - LocalProcessDelegate

    /// SwiftTerm 的这三条 delegate 回调都在 `LocalProcess` 的 dispatchQueue 上 ——
    /// 我们没传队列，所以那就是主队列（见 init 里的注释）。协议本身不是
    /// main-actor 的，标 `nonisolated` + `assumeIsolated` 是把这个事实写明白，
    /// 顺带让 Swift 6 语言模式下也不报「conformance crosses into main actor」。
    nonisolated func processTerminated(_ source: LocalProcess, exitCode: Int32?) {
        MainActor.assumeIsolated { self.handleProcessTerminated(exitCode: exitCode) }
    }

    private func handleProcessTerminated(exitCode: Int32?) {
        switch mode {
        case .plainShell:
            Task { @MainActor [weak self] in
                guard let self, self.status == .running else { return }
                self.status = .exited(exitCode.map(Self.decodeWaitStatus))
            }
        case .agent:
            Task { @MainActor [weak self] in
                guard let self else { return }
                // 退出回调**先**过一遍拉起自检（#541）：秒退且从没吐过一个字 =
                // 「起来即死」，不是「跑完了」。这条回调有时比看门狗先到，若直接
                // 落 exited 就只剩一句「已退出」，机长拿不到任何原因 —— 所以这里
                // 也要报，`reportLaunchFailure` 会连 status 一起翻。
                self.launchWatchdog?.cancel()
                self.busyTimer?.invalidate()
                self.isWorking = false
                self.displayIsTyping = false
                if self.reportLaunchFailureIfStillborn(
                    exitCode: exitCode.map { Self.decodeWaitStatus($0) }) { return }
                self.status = .exited(exitCode)
            }
        }
    }

    /// PTY 有新字节：**先喂自己那份权威缓冲区，再转给看的人**。顺序不能反 ——
    /// mirror 可能不存在，权威那份必须无条件收到。
    nonisolated func dataReceived(slice: ArraySlice<UInt8>) {
        MainActor.assumeIsolated { self.handleDataReceived(slice) }
    }

    private func handleDataReceived(_ slice: ArraySlice<UInt8>) {
        terminal.feed(buffer: slice)
        lastOutputAt = Date()
        if mode == .agent { scanOutput(slice) }
        onOutput?(slice)
    }

    /// 报给 PTY 的窗口尺寸。像素维度报 16×16 的常量（SwiftTerm 自带的
    /// `HeadlessTerminal` 也是这么报的）—— 真实 cell 尺寸只有视图知道，而
    /// SwiftTerm 的 `cellDimension` 是 internal，模块外拿不到。行列数是准的，
    /// TUI 只吃这两个；像素维度仅图形协议（sixel / kitty）会看，两个 agent 都不用。
    nonisolated func getWindowSize() -> winsize {
        MainActor.assumeIsolated { self.currentWindowSize() }
    }

    private func currentWindowSize() -> winsize {
        winsize(ws_row: UInt16(rows), ws_col: UInt16(cols), ws_xpixel: 16, ws_ypixel: 16)
    }

    // MARK: - TerminalDelegate

    /// 终端要往「主机」写字节（回复设备查询之类）——原路送回子进程。
    /// **这是权威那份 Terminal 的回复，唯一一份**：mirror 那份的同名回复被它自己
    /// 拦掉了（见 `TerminalMirrorView.send(source:data:)` 上的注释），否则同一个
    /// 设备查询会被答两次。
    nonisolated func send(source: Terminal, data: ArraySlice<UInt8>) {
        MainActor.assumeIsolated { self.routeTerminalResponse(data) }
    }

    /// 终端往「主机」写字节时的分流。
    ///
    /// **默认原样送进子进程，拦截必须窄到只认自己问的那一条。** 这条路上跑的**不只是**
    /// 我们拍快照时问出来的答复 —— agent 自己问的光标位置报告、设备属性、括号粘贴
    /// 回应全从这儿走。多吞一条，agent 就会去等一个永远不来的回答，而那种坏法几乎
    /// 查不出来（它不报错，只是不动了）。
    ///
    /// 所以判据是**精确匹配**：正在问 `?N` 的时候，只吞 `\u{1b}[?N;<d>$y` 这一种形状，
    /// **连模式号不同的同类答复都放行**。宁可漏拿一个模式（快照少一个模式没人会死），
    /// 也不许错吞一个字节。
    ///
    /// **这两个条件的顺序不许调换。** 这是热路径 —— 每一次按键、每一条程序化写入都
    /// 过这里。`snapshotProbe` 是标志位，为 nil 时 Swift 直接短路，`isDecrpmAnswer`
    /// 一次都不会被调用，**正常情况下这道分流的成本是零**。反过来写（每次都先扫一遍
    /// 字节找 DECRQM 形状）就是往回走：#59 刚把这条路的单价从 253ms 打到 0.94ms，
    /// 而前后端分离的一个顺带目标正是解掉那条结构债。
    ///
    /// 顺带一个结构上的好处：没有查询挂着的时候，那段判断根本不执行 —— 误吞在结构上
    /// 就更不可能了，不是靠判据写得准。
    private func routeTerminalResponse(_ data: ArraySlice<UInt8>) {
        if let probe = snapshotProbe, Self.isDecrpmAnswer(data, forMode: probe.mode) {
            snapshotProbe?.sink.append(contentsOf: data)
            return
        }
        process.send(data: data)
    }

    /// `\u{1b}[?<mode>;<value>$y` —— DECRQM 的答复形状，**且模式号必须是问的那一个**。
    static func isDecrpmAnswer(_ data: ArraySlice<UInt8>, forMode mode: Int) -> Bool {
        let prefix = Array("\u{1b}[?\(mode);".utf8)
        let suffix = Array("$y".utf8)
        guard data.count >= prefix.count + suffix.count else { return false }
        return data.starts(with: prefix) && data.suffix(suffix.count).elementsEqual(suffix)
    }

    // MARK: - 缓冲区快照（前后端分离 P3）

    /// 正在问的那个模式，以及收到的答复。非 nil = 正在拍快照。
    private var snapshotProbe: (mode: Int, sink: [UInt8])?

    /// 「问一句、收一句」，一次只问一个模式。
    ///
    /// **不会挂住**：`Terminal.feed` 是同步的，答复在这一行之内就已经回来了或者没有 ——
    /// 所以这里没有等待、也不需要超时。没答上来就返回空，调用方回落到默认值。
    /// 快照发生在 attach 那一刻，窗口宁可少一个模式也不能连不上。
    func probeTerminal(mode: Int, query: [UInt8]) -> [UInt8] {
        snapshotProbe = (mode: mode, sink: [])
        defer { snapshotProbe = nil }
        // 直接喂 terminal，不走 handleDataReceived —— 这不是子进程的输出，
        // 六个扫描器一个都不该看见它。
        terminal.feed(byteArray: query)
        return snapshotProbe?.sink ?? []
    }

    /// 把当前权威缓冲区拍成一段「能把干净终端喂成同一副样子」的字节。
    ///
    /// P4 之后这是 attach 的第一件事：窗口连上来先吃这一份，才轮到实时字节。
    ///
    /// 这里额外做的一件事是**把模式查询的问答闭环接上**。SwiftTerm 里 `cursorHidden`
    /// 是 internal、`mouseProtocol` 是 private，`?25` 与 `?1006` 都没有公开读口 ——
    /// 唯一的公开问法是 DECRQM，而答复从 delegate 的 `send` 出来、也就是从我们手里过。
    /// **只有在这儿才拦得住它**，不然 agent 的输入框里会凭空多出一串 `\u{1b}[?25;2$y`。
    ///
    /// 鼠标上报的 `?1000/?1002/?1003` **不用问** —— `Terminal.mouseMode` 是公开的。
    /// 查询面越小，错吞的风险越小。
    func snapshot() -> TerminalSnapshotEncoder.Snapshot {
        TerminalSnapshotEncoder.encode(terminal) { [weak self] query in
            guard let self, let mode = TerminalSnapshotEncoder.decrqmMode(of: query) else {
                return []
            }
            return self.probeTerminal(mode: mode, query: query)
        }
    }

    // MARK: - PTY 输出旁路（六个扫描器）

    /// 劈分前这一整段是终端视图 `onData` 旁路闭包的闭包体，逐字搬过来 ——
    /// 旁路顺序不许改（launchFailed 自我纠正 → health → rate-limit → 待决策 →
    /// 正在输入 → 切档回显 → 启动参数）。
    private func scanOutput(_ slice: ArraySlice<UInt8>) {
        // 半死判定的自我纠正（#541）：`stalled` 是「到点还没吐字」的推断，
        // 万一它只是启动特别慢，第一个字节到达就证明其实活着 —— 立刻撤掉
        // 红点，别让状态从「谎报空闲」翻成「谎报拉起失败」。
        if status == .running, health?.kind == .launchFailed {
            health = nil
        }
        // 逐条赋值(而非只取一条)——同一 chunk 罕见地同时命中 auth+quota 时,
        // 两次 @Published 变更都要发出去,下游(run 的白板 fail-loud)按 kind 去重。
        for hit in healthScanner.feed(slice) { health = hit }
        if rateLimitScanner.feed(slice) { answerRateLimitMenu() }
        // 待决策跟踪只喂字节；「出现/消失」的判定按时间走，挂在下面
        // 0.6s 的 busyTimer 上（见 recomputeWorking 旁的 pollPendingDecision）。
        decisionTracker.feed(slice)
        // 「正在输入」判定：只喂这一笔的**可见文本**（清窗→喂→取窗），
        // 纯控制序列/重绘同一帧都会被 tracker 判成心跳，不点亮气泡。
        typingStripper.clear()
        if typingStripper.feed(slice) {
            typingActivity.feed(plainText: typingStripper.tail, at: Date())
        }
        // 切模型/effort 在途时旁路一份：核对 claude 的生效/拒绝回显（#544）。
        profileEchoScanner?.feed(slice)
        // 首屏旁路一份：启动参数有没有被 CLI 悄悄忽略（Todo #36）。
        // 这条**必须**独立于 SessionLaunchProbe —— 那边看的是「活没活」，
        // 而参数没生效的 session 活得好好的。
        if let scanner = launchParameterScanner {
            if scanner.isExpired() {
                launchParameterScanner = nil       // 过窗就停扫，别为长回合里的偶然重现付误报代价
            } else {
                for hit in scanner.feed(slice) { launchParameterProblem = hit }
            }
        }
    }

    /// 视口行列数变化（切 crew 让终端视图卸载/重挂 → SwiftUI 必经
    /// frame .zero → 真实尺寸 → 两次行列数变化 → 两次 SIGWINCH）。子进程随后
    /// 吐的整屏重绘**不是它在干活**，是我们要求它重画的 —— 打个宽限窗，
    /// 窗内的输出不点亮「正在输入」气泡（Todo #32，证据见 TypingActivityTracker）。
    /// 由 mirror 在 `sizeChanged` 里调。
    func noteViewportChange() {
        typingActivity.noteViewportChange(at: Date())
    }

    // MARK: - 拉起自检（#541）

    /// 拉起看门狗：出裁决就停（终局 → fail-loud；alive → 收工）。
    private var launchWatchdog: Task<Void, Never>?
    /// 拉起时刻 / spawn 结果 / 可执行路径 —— 退出回调那条路也要拿它们判「起来即死」。
    private var launchStartedAt = Date()
    private var launchSpawned = false
    private var launchExecutable = ""
    /// 用户/机长主动停 —— 停掉的别被自检倒打一耙报成「拉起失败」。
    private var userStopped = false

    /// 退出回调路的「起来即死」判定：从没吐过字、且还在拉起观察窗内就没了 =
    /// 拉起失败（不是「跑完了」）。报了返回 true（status 已由报告方翻）。
    @discardableResult
    private func reportLaunchFailureIfStillborn(exitCode: Int32?) -> Bool {
        guard !userStopped, status == .running else { return false }
        let verdict = SessionLaunchProbe.verdict(
            spawned: launchSpawned, processAlive: false,
            // claude 的 `spawned` 是 fork 之后现场量的 `process.running == true`，
            // 所以「spawn 成功」本身就等于「观测到活过」——不像 codex 要逐轮累积。
            everAlive: launchSpawned,
            sawOutput: lastOutputAt != .distantPast,
            elapsed: Date().timeIntervalSince(launchStartedAt))
        guard SessionLaunchProbe.isTerminal(verdict) else { return false }
        reportLaunchFailure(verdict, executable: launchExecutable, exitCode: exitCode)
        return true
    }

    /// 每秒喂一次观测量给 `SessionLaunchProbe`，拿到终局裁决就翻 health（+ 视情况
    /// 翻 status）。只在 `status == .running` 期间盯 —— 正常退出归 delegate 回调管。
    private func startLaunchWatchdog(spawned: Bool, executable: String) {
        let startedAt = Date()
        launchStartedAt = startedAt
        launchSpawned = spawned
        launchExecutable = executable
        launchWatchdog = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                guard let self, self.status == .running else { return }
                let pid = self.process?.shellPid ?? 0
                let reap = spawned ? Self.reapIfExited(pid: pid) : .gone(nil)
                let verdict = SessionLaunchProbe.verdict(
                    spawned: spawned,
                    processAlive: reap.isAlive,
                    everAlive: spawned,   // fork 后现场量过 running，见上
                    // 收到过任何一个 PTY 字节 = 确定活着（TUI 已经在画了）。
                    sawOutput: self.lastOutputAt != .distantPast,
                    elapsed: Date().timeIntervalSince(startedAt))
                if SessionLaunchProbe.isTerminal(verdict) {
                    self.reportLaunchFailure(
                        verdict, executable: executable, exitCode: reap.exitCode)
                    return
                }
                if verdict == .alive { return }
                try? await Task.sleep(
                    nanoseconds: UInt64(SessionLaunchProbe.pollInterval * 1_000_000_000))
            }
        }
    }

    /// 终局裁决 → health（`launchFailed`）+ 该退的翻 status。
    /// **health 先于 status** —— run 侧 finalize 会收掉 health 观察，先发才不会丢；
    /// run 侧另有 finalize 时直读 backend.health 的兜底，两头都堵上。
    private func reportLaunchFailure(
        _ verdict: SessionLaunchVerdict, executable: String, exitCode: Int32?
    ) {
        let underlying: String?
        switch verdict {
        case .spawnFailed: underlying = executable
        case .diedSilently: underlying = exitCode.map { "exit \($0)" }
        default: underlying = nil
        }
        guard let detail = SessionLaunchProbe.failureDetail(
            verdict, kind: kind, underlying: underlying) else { return }
        health = CrewSessionHealth(kind: .launchFailed, detail: detail)
        isWorking = false
        displayIsTyping = false
        switch verdict {
        case .spawnFailed:
            // 127 = 惯例「起不来」，让退出原因分类落 `.failed` 而不是「正常完成」。
            status = .exited(127)
        case .diedSilently:
            status = .exited(exitCode ?? 127)
        case .stalled:
            // 进程还活着 —— 不替人做主杀掉（终端现场留着给人看 / inspect_session）。
            // health 已翻成异常，点名不再报「空闲」，机长据此改派。
            break
        case .alive, .pending:
            break
        }
    }

    /// 子进程收尸探测：**不能只用 `kill(pid, 0)`** —— 退出回调丢掉时没人 `waitpid`，
    /// 子进程会以僵尸态留着，`kill(pid, 0)` 照样返回 0（看起来还活着），正是这个
    /// 假信号让「起来即死」伪装成「在跑」。我们是它的父进程，直接 `WNOHANG` 收一次：
    /// 0 = 真在跑；>0 = 已退出（顺带拿到退出码）；<0 = 已被收走/不是我们的孩子 = 没了。
    private enum ReapResult {
        case alive
        case gone(Int32?)
        var isAlive: Bool { if case .alive = self { return true }; return false }
        var exitCode: Int32? { if case let .gone(c) = self { return c }; return nil }
    }
    /// SwiftTerm 的 `processTerminated` 把 **`waitpid` 的原始 status** 原样当
    /// exitCode 交出来（没走 WEXITSTATUS）—— 于是 exit 2 会显示成 512、exit 127
    /// 显示成 32512。给人看的原因里必须是真的退出码，这里解一次。
    static func decodeWaitStatus(_ st: Int32) -> Int32 {
        (st & 0x7f) == 0 ? (st >> 8) & 0xff : st & 0x7f
    }

    private static func reapIfExited(pid: pid_t) -> ReapResult {
        guard pid > 0 else { return .gone(nil) }
        var st: Int32 = 0
        let r = waitpid(pid, &st, WNOHANG)
        if r == 0 { return .alive }
        if r < 0 { return .gone(nil) }
        // WIFEXITED/WEXITSTATUS 的宏在 Swift 里没导出，按 wait(2) 的位布局取。
        let exited = (st & 0x7f) == 0
        return .gone(exited ? (st >> 8) & 0xff : nil)
    }

    // MARK: - 派生状态的 0.6s 轮询

    /// 干活中 = 仍 running 且最近 1s 内还有 PTY 输出。
    private func recomputeWorking() {
        let now = Date()
        let working = status == .running && now.timeIntervalSince(lastOutputAt) < 1.0
        if working != isWorking { isWorking = working }
        // UI 的「正在输入」走另一条判据（Todo #24），与上面的原始信号互不影响。
        let typing = typingActivity.isTyping(isRunning: status == .running, now: now)
        if typing != displayIsTyping { displayIsTyping = typing }
        // 干活 streak 起止 —— 「限额中」的恢复判定要它。
        if working {
            if workingSince == nil { workingSince = Date() }
        } else {
            workingSince = nil
        }
        // 「限额中」进得去也要出得来：撞限额后又连着干活够久 = 已恢复（换了模型、
        // 或限额窗自己到点），别让红点一路谎报到几小时后的重置唤醒才熄。
        if QuotaHealthRecovery.recovered(
            health: health, quotaHealthAt: quotaHealthAt, workingSince: workingSince) {
            clearQuotaHealth()
        }
    }

    /// 「终端在等人选」的出现/消失轮询（Todo #6），挂在 0.6s busyTimer 上。
    /// 判定本身在 `PendingDecisionTracker`（纯逻辑 + 单测），这里只把结果落到
    /// `@Published`，剩下的（发群 @人、翻状态、超时升级）归 `CrewSessionRun`。
    ///
    /// 退出的 session 不留待决策：进程都没了，「在等人选」是假的，必须清掉 ——
    /// 否则又是一个「进得去出不来」的谎报状态（#545）。
    private func pollPendingDecision() {
        guard status == .running else {
            if pendingDecision != nil { pendingDecision = nil }
            return
        }
        switch decisionTracker.poll() {
        case let .appeared(d): pendingDecision = d
        case .cleared:         pendingDecision = nil
        case nil:              break
        }
    }

    // MARK: - 控制面

    /// 写字节进 PTY（键盘、程序化 send、菜单按键都走这里）。
    func write(_ bytes: [UInt8]) {
        guard isProcessRunning else { return }
        process.send(data: bytes[...])
    }

    /// 程序化发文本(prompt / steer)：先写正文，隔一拍再单发回车。
    /// claude TUI 按字节到达时序区分「粘贴 vs 敲键」——正文+回车一笔写入会被
    /// 整段判成粘贴，回车被吞进输入框文本 → 不提交（用户得手动按 Enter）。
    /// 回车必须作为独立的一次按键、在粘贴判定窗口之外到达才触发提交。
    func send(_ text: String) {
        inject(Array(text.utf8))
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 200_000_000)
            guard let self, self.status == .running else { return }
            self.inject([0x0d])
        }
    }

    /// 打断进行中的回合(Esc)。
    func interrupt() { inject([0x1b]) }

    /// 普通 shell 的打断是 Ctrl-C（劈分前 `PlainTerminalSession.interrupt` 就发这个）。
    func interruptPlainShell() {
        guard status == .running else { return }
        inject([0x03])
    }

    /// 协议要求的程序化写入；普通使用路径直接由 SwiftTerm 接收人的键盘输入。
    func sendPlainShell(_ text: String) {
        guard status == .running else { return }
        inject(Array(text.utf8))
    }

    /// 机长 nudge_session 用：向 PTY 发裸按键字节（Enter=0x0d / Esc=0x1b 等），
    /// **不**追加回车 —— 与 `send` 的「正文+隔拍回车」区分，用于模态菜单选择。
    func sendRaw(_ bytes: [UInt8]) { inject(bytes) }

    /// 视口尺寸变了：同步自己那份 `Terminal`，并把 winsize 推给 PTY。
    /// 零/退化尺寸一律忽略 —— 2 列下 reflow 会把历史按 2 字宽重排、顶部被永久裁掉
    /// （Todo #34 实测：100 列 400 行历史过一次退化尺寸，10000 行上限下只剩 223 行，
    /// 且首行断在半截）。守卫在 mirror 侧也有一道，这里是第二道。
    func resize(cols newCols: Int, rows newRows: Int) {
        guard newCols >= 2, newRows >= 1 else { return }
        guard newCols != cols || newRows != rows else { return }
        cols = newCols
        rows = newRows
        terminal.resize(cols: newCols, rows: newRows)
        guard isProcessRunning, process.childfd >= 0 else { return }
        var size = currentWindowSize()
        _ = PseudoTerminalHelpers.setWinSize(masterPtyDescriptor: process.childfd, windowSize: &size)
    }

    /// 收窄权威那份的回滚缓冲（进程终止后，门面用 mirror 算出的保留行数调）。
    /// 劈分前只有一份 `Terminal`，收窄它就等于全放掉；现在两份都要收，否则
    /// 已终止的 session 仍会攥着内核那份 10000 行（`TerminatedScrollbackPlan`
    /// 的整段理由对两份同样成立）。
    func changeScrollback(_ lines: Int) {
        terminal.changeScrollback(lines)
    }

    /// **权威画面**（`inspect_session` 用）：当前屏幕内容，与任何窗口滚到哪无关。
    func screenText(maxLines: Int) -> String {
        var lines: [String] = []
        for row in 0..<terminal.rows {
            guard let line = terminal.getLine(row: row) else { continue }
            lines.append(line.translateToString(trimRight: true))
        }
        while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
            lines.removeLast()
        }
        return lines.suffix(maxLines).joined(separator: "\n")
    }

    /// 终止子进程。"停不掉"的真因：SwiftTerm `terminate()` 调 `childStopped()`，
    /// 后者 **cancel 掉退出监听 childMonitor 且 close io**，于是进程真退出后
    /// `processTerminated` 回调**永不触发** → `status` 永远停在 `.running` →
    /// UI 停止键/badge/终端留屏不变 = 看着"停不掉"。所以这里**自己翻 `status`**，
    /// 不依赖被吞掉的回调。另外 `terminate()` 只发 SIGTERM；万一交互式 agent 截获
    /// 它，宽限后 `terminateTree` 升级 SIGKILL（不可捕获）兜底，连带杀子进程。
    func stop() {
        guard status == .running else { return }
        userStopped = true
        let pid = process.shellPid
        process.terminate()                 // SIGTERM + close PTY（但不回调 processTerminated）
        status = .exited(nil)               // ← 自己翻状态，否则 UI 永远 running
        launchWatchdog?.cancel()            // 主动停的别被自检倒打一耙报成「拉起失败」
        Task { await terminateTree(pid: pid, graceSeconds: 2.0) }
    }

    /// rate-limit 菜单自动应答（Todo #10 层1）：替 session 选「1. Stop and wait
    /// for limit to reset」——先发 "1" 钉住选项（菜单默认 ❯ 在 1，但不赌默认位），
    /// 隔一拍再 Enter 确认（同 `send()` 的粘贴判定原因，两笔分开发）。同时翻
    /// health `.rateLimited`（层2 状态如实）：卡限额不再装「空闲」，红点亮起、
    /// run 侧据此自动挂额度重置唤醒。菜单若没被按掉，扫描器冷却期满会再触发重试。
    private func answerRateLimitMenu() {
        inject(Array("1".utf8))
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard let self, self.status == .running else { return }
            self.inject([0x0d])
        }
        if health?.kind != .rateLimited {
            health = CrewSessionHealth(
                kind: .rateLimited,
                detail: "Claude Code 撞到额度上限（rate-limit 菜单已自动应答「Stop and wait」）—— 等额度窗口重置后自动继续。")
        }
    }

    /// 额度类 health 清除 + 扫描器重新武装（额度重置唤醒到点后由 runner 调）：
    /// 恢复干活后别让「限额中」红点谎报下去；下个限额窗再撞墙能再次首报。
    func clearQuotaHealth() {
        healthScanner.rearmQuota()
        if health?.isQuotaRelated == true { health = nil }
    }

    // MARK: - 中途切模型 / effort（#544）

    /// 等终端空闲 → 注入 `/model` `/effort` → **核对 claude 回显**，回真实结果。
    ///
    /// 为什么必须等空闲（这是 #544 的根因）：claude 在跑 turn 时，写进 PTY 的整行
    /// 会被它当成「下一条用户消息」收进消息队列（实测：会话 transcript 里留下
    /// `queue-operation enqueue content="/model opus"`），**斜杠命令永远不会执行**。
    /// 而 `set_session_profile` 按定义总是 session 自己在回合中调的 —— 于是老实现
    /// 100% 必然落空，还回了「已生效」的假成功。空闲时注入则实测可用（claude
    /// 2.1.220 PTY 实跑：`/model haiku` → `Set model to Haiku 4.5 …`）。
    ///
    /// 回显核对是第二道保险：没等到生效回显就绝不报成功（`.noConfirmation`）。
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        await applyProfileSwitch(cmd, policy: SessionProfileSwitchPolicy())
    }

    func applyProfileSwitch(
        _ cmd: SessionProfileSwitchCommand, policy: SessionProfileSwitchPolicy
    ) async -> SessionProfileSwitchOutcome {
        var sawIdle = false
        for _ in 0..<max(1, policy.attempts) {
            guard status == .running else { return sawIdle ? .noConfirmation : .neverIdle }
            guard await waitForTerminalIdle(policy: policy) else { continue }
            sawIdle = true

            let scanner = SessionProfileEchoScanner(knob: cmd.knob)
            profileEchoScanner = scanner
            send(cmd.line)

            let deadline = Date().addingTimeInterval(policy.confirmWait)
            var verdict: SessionProfileSwitchOutcome?
            while verdict == nil, Date() < deadline, status == .running {
                try? await Task.sleep(nanoseconds: UInt64(policy.poll * 1_000_000_000))
                verdict = scanner.outcome
            }
            profileEchoScanner = nil
            if let verdict { return verdict }
        }
        return sawIdle ? .noConfirmation : .neverIdle
    }

    /// 连续 `idleQuiet` 没有 PTY 输出 = 终端空闲（claude 干活时 TUI 每秒重绘计时器，
    /// 静默窗口不会被误判）。等到 `idleWait` 还不空闲就放弃这一轮。
    private func waitForTerminalIdle(policy: SessionProfileSwitchPolicy) async -> Bool {
        let deadline = Date().addingTimeInterval(policy.idleWait)
        while Date() < deadline {
            guard status == .running else { return false }
            if Date().timeIntervalSince(lastOutputAt) >= policy.idleQuiet { return true }
            try? await Task.sleep(nanoseconds: UInt64(policy.poll * 1_000_000_000))
        }
        return false
    }

    private func inject(_ bytes: [UInt8]) { write(bytes) }
}
#endif
