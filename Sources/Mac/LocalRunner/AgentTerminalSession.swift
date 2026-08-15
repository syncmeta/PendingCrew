#if os(macOS)
import Foundation
import AppKit
import Combine
import SwiftTerm

/// 一个内嵌真终端的 coding-agent 会话：在 PTY 里跑交互式 claude/codex(自动审批)。
/// 读=终端原样显示，控=键盘(SwiftTerm 原生) + 程序化 send()/interrupt() 字节注入。
/// `LocalProcessTerminalView` 子类，记录最近一次收到 PTY 输出的时刻 —— 用来近似
/// 「agent 在干活」：claude TUI 干活时持续重绘(spinner/输出)→ 频繁有数据;空闲等
/// 指令时几乎不吐数据(光标闪烁是 SwiftTerm 本地渲染，不经 PTY)。`dataReceived` 是
/// `open`，覆盖它记时间戳即可(主线程，UI feed 路径)。
final class ActivityTerminalView: LocalProcessTerminalView {
    /// 回滚历史行数上限。SwiftTerm 的默认值是 **500 行**（`TerminalOptions.scrollback`），
    /// agent session 跑一小会儿就顶满 —— 这是「往上滑只能滑一小段」的一半病根。
    /// 10000 行对齐 macOS Terminal.app 的默认历史长度。
    ///
    /// 代价（本机实测，见 Todo #34）：`CharData` stride 24B，每行占 `cols × 24B` ——
    /// 160 列约 3.8KB/行，装满约 38MB/session（100 列约 24MB）。而且**不是按需增长**：
    /// SwiftTerm 的 `Buffer.resize` 在列数变化时会遍历 `lines.maxLength` 把每个槽位都
    /// 实例化，所以窗口第一次改宽度就会一次性吃满这份内存。再往上取值不划算：50000 行
    /// 时单次改宽 resize 实测 28ms（10000 行是 4.5ms），拖窗口会掉帧。
    static let scrollbackLines = 10_000

    override init(frame: CGRect) {
        super.init(frame: frame)
        // macOS 的 `TerminalView` 没有收 options 的构造器（`setupOptions` 自己 new 了一份
        // 默认 `TerminalOptions`），所以只能构造完再改。`changeScrollback` 会同时更新
        // `terminal.options.scrollback`，后续若走到 `Terminal.setup()` 重建 buffer 也保得住。
        getTerminal().changeScrollback(Self.scrollbackLines)
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        getTerminal().changeScrollback(Self.scrollbackLines)
    }

    /// 一个零宽/零高的 frame **不是真布局**，是 SwiftUI 重挂 NSView 时必经的那一拍
    /// （切 crew 就会发生，见下面 `delegate.onSizeChanged` 处的注释）。但 SwiftTerm 照单
    /// 全收：`setFrameSize` → `processSizeChange` → `terminal.resize(cols: 0, …)`，被夹到
    /// `MINIMUM_COLS = 2`。2 列下 reflow 会把每条历史按 2 字宽重新折行（100 列的一行炸成
    /// ~50 行），行数瞬间冲破 scrollback 上限、顶部被**永久**裁掉；折回真实宽度时只能把
    /// 幸存的碎片拼回去，最老那条还是从半截字开始。这既吃掉历史也把排版拼乱 ——
    /// 实测（Todo #34）：100 列 400 行历史过一次 `.zero`，500 行上限下只剩 12 行、
    /// 10000 行上限下只剩 223 行，且首行都是断在半截的碎片。
    ///
    /// 所以零尺寸一律不往下传：视图保持上一次的真实尺寸，等真尺寸到了再走正常 resize。
    /// 用户自己把栏拖窄那种**真实**窄布局照常 reflow —— 那是终端应有的行为，不在这里拦。
    override func setFrameSize(_ newSize: NSSize) {
        guard Self.isRealLayout(newSize) else { return }
        super.setFrameSize(newSize)
    }

    /// 「这尺寸是不是一次真布局」的判定（`setFrameSize` 守卫的纯函数部分，供单测直接钉）。
    /// 只拦真正退化的尺寸（放不下任何一个字符格），不猜阈值。
    static func isRealLayout(_ size: NSSize) -> Bool {
        size.width >= 1 && size.height >= 1
    }

    /// 最近一次收到子进程输出的时刻；busy 判定 = now - lastOutputAt < 阈值。
    var lastOutputAt = Date.distantPast
    /// PTY 输出旁路（健康扫描用）——`dataReceived` 在主线程 UI feed 路径上调,
    /// 回调必须够便宜（SessionHealthScanner 是字节循环 + 尾窗子串查,满足）。
    var onData: ((ArraySlice<UInt8>) -> Void)?
    override func dataReceived(slice: ArraySlice<UInt8>) {
        super.dataReceived(slice: slice)
        lastOutputAt = Date()
        onData?(slice)
    }

    // MARK: - 外置 overlay 滚动条支撑

    /// 终端滚动位置变化回调（外置 overlay 条据此刷新 knob）。
    /// `userInitiated` 区分「用户滚轮/拖动」与「输出自动滚屏」——只有前者才淡入显示条，
    /// 后者只静默同步几何，避免 agent 持续输出时条一直闪。
    var onScroll: ((_ userInitiated: Bool) -> Void)?

    /// SwiftTerm 每次 yDisp 变化都会调这里（用户滚 + 输出自动滚都算）。`scrollWheel` 在
    /// SwiftTerm 里非 open、外部模块不可 override，故不能直接标记「用户滚轮」；改用启发式：
    /// 输出驱动的自动滚屏总是紧跟一拍 `dataReceived`（lastOutputAt≈now），而用户滚轮/触控板
    /// 在空闲期滚动时最近没有 PTY 输出。>0.2s 没收到输出 = 判为用户主动 → 点亮条。
    override func scrolled(source terminal: Terminal, yDisp: Int) {
        super.scrolled(source: terminal, yDisp: yDisp)
        let userInitiated = Date().timeIntervalSince(lastOutputAt) > 0.2
        onScroll?(userInitiated)
    }

    /// SwiftTerm 内部那条焊死的 NSScroller —— 外置 overlay 条取代它，这里把它藏了。
    /// 网格宽度已扣掉 scrollerWidth（`getEffectiveWidth`），藏掉后右侧那条 ~15pt 空当
    /// 正好留给 overlay，且不占字。scroller 在 `setupScroller()` 里 addSubview，时机不定，
    /// 故 didAddSubview 抓一次 + 外部可再 sweep 一次兜底。
    private var didHideNativeScroller = false
    private var shouldHideNativeScroller = true
    override func didAddSubview(_ subview: NSView) {
        super.didAddSubview(subview)
        if let scroller = subview as? NSScroller {
            scroller.isHidden = shouldHideNativeScroller
            didHideNativeScroller = shouldHideNativeScroller
        }
    }

    /// Agent TUI 使用外置 overlay；普通终端保留 SwiftTerm 原生滚动条。
    func useNativeScroller() {
        shouldHideNativeScroller = false
        didHideNativeScroller = false
        for case let scroller as NSScroller in subviews { scroller.isHidden = false }
    }

    /// 兜底扫一遍 subviews 藏掉 NSScroller；返回是否已确认藏到（供 fail-loud 用）。
    @discardableResult
    func hideNativeScroller() -> Bool {
        shouldHideNativeScroller = true
        if didHideNativeScroller { return true }
        if let native = subviews.compactMap({ $0 as? NSScroller }).first {
            native.isHidden = true
            didHideNativeScroller = true
        }
        return didHideNativeScroller
    }
}

@MainActor
final class AgentTerminalSession: ObservableObject, SessionBackend {
    let kind: LocalCodingAgentKind
    let terminalView: ActivityTerminalView
    @Published private(set) var status: SessionStatus = .running
    var statusPublisher: Published<SessionStatus>.Publisher { $status }
    /// 唤醒注入门禁用：PTY 终端无可编程 turn-state，交互式 claude 自带输入排队，
    /// 注入随时安全 → 恒 false（main 语义保留，别让 @我的定向消息因 busy 漏注入）。
    var isBusy: Bool { false }
    /// UI 头像「干活中/空闲」用（与 isBusy 解耦）：最近 ~1s 还在吐 PTY 输出=干活，
    /// 安静=空闲。0.6s 轮询 `terminalView.lastOutputAt` 重算；status 非 running 时恒 false。
    @Published private(set) var isWorking = false
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }
    /// 群聊「正在输入」气泡专用的显示态（Todo #24）—— **故意与 `isWorking` 分家**。
    /// `isWorking` 是「最近 1s 有 PTY 输出」的原始活跃信号，被唤醒回执采样
    /// （`CrewSessionRunner` 的注入确认）、SessionHealth 的 working/idle 上报、
    /// 状态点聚合、限额恢复 streak 一起吃着；给它加迟滞会让那些判定一起变钝
    /// （回执要的正是「注入后马上有反应」这个即时性）。所以原始信号原样保留，
    /// 另出一条只服务 UI 的：见 `TypingActivityTracker`（指纹 + 不对称迟滞）。
    @Published private(set) var displayIsTyping = false
    var displayIsTypingUpdates: AnyPublisher<Bool, Never> { $displayIsTyping.eraseToAnyPublisher() }
    /// 「正在输入」判定用的明文提取器（每 chunk 清一次窗，只要这一笔的可见文本）。
    private let typingStripper = AnsiPlainTextTail(tailLimit: 1024)
    private var typingActivity = TypingActivityTracker()
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
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { $health }
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
    /// 当前正卡着的那个菜单（nil = 没在等）。run 侧观察它发群 + 翻状态。
    @Published private(set) var pendingDecision: PendingTerminalDecision?
    var pendingDecisionUpdates: AnyPublisher<PendingTerminalDecision?, Never> {
        $pendingDecision.eraseToAnyPublisher()
    }
    /// 切模型/effort 在途时挂上的回显扫描器（`applyProfileSwitch` 持有，用完置 nil）。
    private var profileEchoScanner: SessionProfileEchoScanner?
    /// 启动参数没被 CLI 接受的首屏扫描器（Todo #36）。只在拉起窗口内活着，
    /// 过期/报完两类就置 nil 停扫。没显式传 model/effort 时压根不建。
    private var launchParameterScanner: SessionLaunchParameterScanner?
    /// 扫到的「传了但没生效」问题。run 侧观察它 fail-loud 进白板 ——
    /// 进程活着、也在吐字，`SessionLaunchProbe` 判 `.alive` 不会报，只有这条会。
    @Published private(set) var launchParameterProblem: SessionLaunchParameterProblem?
    var launchParameterProblems: AnyPublisher<SessionLaunchParameterProblem, Never> {
        $launchParameterProblem.compactMap { $0 }.eraseToAnyPublisher()
    }
    private var busyTimer: Timer?

    /// 外置 overlay 滚动条的几何 + 用户主动信号（SwiftUI `TerminalScrollbarOverlay` 观察）。
    /// 全部取自 SwiftTerm 公开滚动接口，语义：position/thumbSize 都是 0…1。
    struct ScrollState: Equatable {
        /// 是否可滚（有回滚历史且非 alternate buffer）——决定 overlay 是否挂出。
        var canScroll = false
        /// viewport 相对位置 0(最顶)…1(最底)。
        var position: Double = 0
        /// knob 占轨道的比例 0…1（= SwiftTerm `scrollThumbsize`/knobProportion）。
        var thumbSize: Double = 1
        /// 单调递增计数：每次「用户主动滚动」bump 一次。overlay 据此触发淡入 + 重置
        /// 自动隐藏计时；输出自动滚屏不 bump，条不会因 agent 持续吐字一直闪。
        var userScrollTick: Int = 0
    }
    @Published private(set) var scrollState = ScrollState()

    private final class Delegate: NSObject, LocalProcessTerminalViewDelegate {
        var onExit: ((Int32?) -> Void)?
        /// 终端行列数变了（SwiftTerm 已就地发完 `TIOCSWINSZ`）。
        var onSizeChanged: (() -> Void)?
        func processTerminated(source: TerminalView, exitCode: Int32?) { onExit?(exitCode) }
        func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {
            onSizeChanged?()
        }
        func setTerminalTitle(source: LocalProcessTerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
    }
    private let delegate = Delegate()

    /// `executable` = 已 resolve 的 claude/codex 绝对路径；`workdir` = 工作目录。
    /// argv（含首条指令的 positional prompt、auto mode、effort）由 `config.argv()` 构建。
    init(config: SessionConfig, executable: String, workdir: String, env: [String: String]) {
        self.kind = config.kind
        self.terminalView = ActivityTerminalView(frame: .zero)
        self.terminalView.processDelegate = delegate
        // 健康扫描旁路：PTY 输出喂 SessionHealthScanner,命中(每 Kind 一次)翻 health。
        // dataReceived 在主线程,init 也是 @MainActor —— assumeIsolated 安全。
        self.terminalView.onData = { [weak self] slice in
            guard let self else { return }
            MainActor.assumeIsolated {
                // 半死判定的自我纠正（#541）：`stalled` 是「到点还没吐字」的推断，
                // 万一它只是启动特别慢，第一个字节到达就证明其实活着 —— 立刻撤掉
                // 红点，别让状态从「谎报空闲」翻成「谎报拉起失败」。
                if self.status == .running, self.health?.kind == .launchFailed {
                    self.health = nil
                }
                // 逐条赋值(而非只取一条)——同一 chunk 罕见地同时命中 auth+quota 时,
                // 两次 @Published 变更都要发出去,下游(run 的白板 fail-loud)按 kind 去重。
                for hit in self.healthScanner.feed(slice) { self.health = hit }
                if self.rateLimitScanner.feed(slice) { self.answerRateLimitMenu() }
                // 待决策跟踪只喂字节；「出现/消失」的判定按时间走，挂在下面
                // 0.6s 的 busyTimer 上（见 recomputeWorking 旁的 pollPendingDecision）。
                self.decisionTracker.feed(slice)
                // 「正在输入」判定：只喂这一笔的**可见文本**（清窗→喂→取窗），
                // 纯控制序列/重绘同一帧都会被 tracker 判成心跳，不点亮气泡。
                self.typingStripper.clear()
                if self.typingStripper.feed(slice) {
                    self.typingActivity.feed(plainText: self.typingStripper.tail, at: Date())
                }
                // 切模型/effort 在途时旁路一份：核对 claude 的生效/拒绝回显（#544）。
                self.profileEchoScanner?.feed(slice)
                // 首屏旁路一份：启动参数有没有被 CLI 悄悄忽略（Todo #36）。
                // 这条**必须**独立于 SessionLaunchProbe —— 那边看的是「活没活」，
                // 而参数没生效的 session 活得好好的。
                if let scanner = self.launchParameterScanner {
                    if scanner.isExpired() {
                        self.launchParameterScanner = nil       // 过窗就停扫，别为长回合里的偶然重现付误报代价
                    } else {
                        for hit in scanner.feed(slice) { self.launchParameterProblem = hit }
                    }
                }
            }
        }
        // 视口行列数变化（切 crew 让终端视图卸载/重挂 → SwiftUI 必经
        // frame .zero → 真实尺寸 → 两次行列数变化 → 两次 SIGWINCH）。子进程随后
        // 吐的整屏重绘**不是它在干活**，是我们要求它重画的 —— 打个宽限窗，
        // 窗内的输出不点亮「正在输入」气泡（Todo #32，证据见 TypingActivityTracker）。
        // AppKit 布局在主线程调这条，与 init 同隔离域。
        delegate.onSizeChanged = { [weak self] in
            MainActor.assumeIsolated {
                self?.typingActivity.noteViewportChange(at: Date())
            }
        }
        delegate.onExit = { [weak self] code in
            Task { @MainActor in
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
                    exitCode: code.map { Self.decodeWaitStatus($0) }) { return }
                self.status = .exited(code)
            }
        }
        // 启动参数没被接受的首屏扫描（Todo #36）—— 必须在 startProcess **之前**挂好，
        // 那两条警告是子进程吐的第一批字节。没显式传 model/effort 就不建（`isIdle`）。
        let paramScanner = SessionLaunchParameterScanner(model: config.model, effort: config.effort)
        launchParameterScanner = paramScanner.isIdle ? nil : paramScanner
        var envArr = env.map { "\($0.key)=\($0.value)" }
        if !envArr.contains(where: { $0.hasPrefix("TERM=") }) { envArr.append("TERM=xterm-256color") }
        terminalView.startProcess(
            executable: executable,
            args: config.argv(),
            environment: envArr,
            currentDirectory: workdir
        )
        // 拉起自检（#541）：**别把「构造完了」当成「跑起来了」**。SwiftTerm 的
        // forkpty 分支失败时静默 return（不置 running、不回调 delegate），且它的
        // 退出事件源 `activate()` 早于 `setEventHandler` —— 秒退的子进程（额度耗尽
        // 的 CLI 启动即退）退出事件可能没人接。两种情况都会让 status 永远停在
        // `.running`、`isWorking` 恒假 → 点名推导出「🟡 空闲」，机长照常派活。
        // 这里自己核实 spawn，并起看门狗盯「进程没了却没回调」「活着但零输出」。
        let spawned = terminalView.process?.running == true
            && (terminalView.process?.shellPid ?? 0) > 0
        startLaunchWatchdog(spawned: spawned, executable: executable)
        // 终端滚动（用户滚 + 输出自动滚）→ 刷新外置条几何。userInitiated 决定是否点亮。
        terminalView.onScroll = { [weak self] userInitiated in
            guard let self else { return }
            MainActor.assumeIsolated { self.refreshScrollState(userInitiated: userInitiated) }
        }
        // 0.6s 轮询输出活跃度 → 翻 isBusy。定时器在 main runloop 触发(init 是
        // @MainActor)；[weak self] 不成环，exit/stop/deinit 时 invalidate。
        busyTimer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.recomputeWorking()
                self?.pollPendingDecision()
                // canScroll/thumbSize 会因输出增长而变，但只在 yDisp 变化时才有 scrolled
                // 回调；顶到底持续吐字时 yDisp 每行都动能覆盖，静止但 buffer 变化的边角用
                // 轮询兜一层（非用户主动，只同步几何不点亮）。
                self?.refreshScrollState(userInitiated: false)
            }
        }
        // 内部 scroller 兜底隐藏 + fail-loud：didAddSubview 一般已抓到；若 SwiftTerm 换了
        // 布局（不再用 NSScroller / 私有结构变），这里大声报，别让它静默贴右缘压字。
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 800_000_000)
            guard let self else { return }
            if !self.terminalView.hideNativeScroller() {
                assertionFailure("SwiftTerm 内部 NSScroller 未找到——外置滚动条方案需复核 SwiftTerm 布局")
                NSLog("[AgentTerminalSession] 警告：未能隐藏 SwiftTerm 内部滚动条，右缘可能压字")
            }
        }
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
            sawOutput: terminalView.lastOutputAt != .distantPast,
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
                let pid = self.terminalView.process?.shellPid ?? 0
                let reap = spawned ? Self.reapIfExited(pid: pid) : .gone(nil)
                let verdict = SessionLaunchProbe.verdict(
                    spawned: spawned,
                    processAlive: reap.isAlive,
                    everAlive: spawned,   // fork 后现场量过 running，见上
                    // 收到过任何一个 PTY 字节 = 确定活着（TUI 已经在画了）。
                    sawOutput: self.terminalView.lastOutputAt != .distantPast,
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

    /// 从 SwiftTerm 公开接口重算外置条几何；仅在变化时发布，避免输出流时高频 @Published。
    private func refreshScrollState(userInitiated: Bool) {
        let next = ScrollState(
            canScroll: terminalView.canScroll,
            position: terminalView.scrollPosition,
            thumbSize: Double(terminalView.scrollThumbsize),
            userScrollTick: scrollState.userScrollTick + (userInitiated ? 1 : 0)
        )
        if next != scrollState { scrollState = next }
    }

    /// overlay 拖动/点击 → 驱动 SwiftTerm 滚到 0…1 位置，并立即回刷几何（算用户主动）。
    func scrollTerminal(toPosition position: Double) {
        terminalView.scroll(toPosition: max(0, min(1, position)))
        refreshScrollState(userInitiated: true)
    }

    /// 干活中 = 仍 running 且最近 1s 内还有 PTY 输出。
    private func recomputeWorking() {
        let now = Date()
        let working = status == .running && now.timeIntervalSince(terminalView.lastOutputAt) < 1.0
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

    deinit {
        busyTimer?.invalidate()
        launchWatchdog?.cancel()
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
            if Date().timeIntervalSince(terminalView.lastOutputAt) >= policy.idleQuiet { return true }
            try? await Task.sleep(nanoseconds: UInt64(policy.poll * 1_000_000_000))
        }
        return false
    }

    /// 机长 nudge_session 用：向 PTY 发裸按键字节（Enter=0x0d / Esc=0x1b 等），
    /// **不**追加回车 —— 与 `send` 的「正文+隔拍回车」区分，用于模态菜单选择。
    func sendRaw(_ bytes: [UInt8]) { inject(bytes) }

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

    /// 额度类 health 清除 + 扫描器重新武装（额度重置唤醒到点后由 runner 调）：
    /// 恢复干活后别让「限额中」红点谎报下去；下个限额窗再撞墙能再次首报。
    func clearQuotaHealth() {
        healthScanner.rearmQuota()
        if health?.isQuotaRelated == true { health = nil }
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
        let pid = terminalView.process?.shellPid ?? 0
        terminalView.terminate()            // SIGTERM + close PTY（但不回调 processTerminated）
        status = .exited(nil)               // ← 自己翻状态，否则 UI 永远 running
        launchWatchdog?.cancel()            // 主动停的别被自检倒打一耙报成「拉起失败」
        Task { await terminateTree(pid: pid, graceSeconds: 2.0) }
    }

    private func inject(_ bytes: [UInt8]) { terminalView.process?.send(data: bytes[...]) }
}

/// SIGTERM 已由调用方（SwiftTerm `terminate()`）发出；本函数等一个宽限期，
/// 若进程仍在就升级到 **SIGKILL**。`killpg` 杀整个进程组——pty 子进程是 setsid
/// 的会话首（组 id == pid），能连带杀掉它拉起的 bash/MCP 子进程；非组首则
/// fallback 单杀。SIGKILL 不可捕获，保证停得掉。
func terminateTree(pid: pid_t, graceSeconds: Double) async {
    guard pid > 0 else { return }
    try? await Task.sleep(nanoseconds: UInt64(max(0, graceSeconds) * 1_000_000_000))
    guard kill(pid, 0) == 0 else { return }     // 已退出，无需升级
    if killpg(pid, SIGKILL) != 0 { kill(pid, SIGKILL) }
}
#endif
