#if os(macOS)
import Combine
import Foundation
import SwiftUI

/// local coding agent 跑动的 view-side 视图模型（chunk2 T1：多 run 并存）。
///
/// 每次 `start(...)` 起一个新 `CrewSessionRun` 追加进 `runs` 并选中——旧 run
/// **不再**被自动停掉，N 个 session 并行各自跑各自的 PTY，无并发上限。
/// 右栏 session 切换条（T2）靠 `selectedRunId` 决定哪个 run 在前台。
///
/// 每个 run 内嵌一个真 PTY 终端（`AgentTerminalSession`），跑交互式
/// claude/codex（自动审批）。读=终端原样渲染；控=键盘 + 程序化 send/stop。
@MainActor
final class CrewSessionRunner: ObservableObject {
    @Published private(set) var runs: [CrewSessionRun] = []
    @Published var selectedRunId: UUID?
    private let sessionProtocolBridge = InProcessSessionProtocolBridge()

    /// 「新建 session」态：inspector 显示零配置 composer 而非某个 run。由 roster /
    /// 切换条的「+」置 true，启动成功或切到某个 run 时复位。UI 选择态（与
    /// `selectedRunId` 同类），放这里让 roster（开 inspector 的一端）与 inspector
    /// composer（消费的一端）无需额外 binding 即可共享。
    @Published var isComposingNew = false

    /// inspector 内的模式开关：`false`=成员列表模式（平时看 roster + 待审批），
    /// `true`=终端模式（看某个 session 的终端 + composer）。点 session 行 / 起新
    /// session 翻 true；inspector 顶「‹ 成员」返回翻 false。toolbar 的「Session 终端」
    /// 开关只翻 inspector 呈现态，不强制 true —— 让用户先落在成员列表。
    @Published var viewingTerminal = false

    /// 驾驶舱模式（cockpit.md）：`true` 时主窗口从「三栏群聊」整片切到「侧栏 | 驾驶舱」
    /// 两栏——驾驶舱铺满 content+detail（不是临时小窗口/info 面板）。侧栏仍在,作为
    /// **沿 DAG 选 crew 的范围选择器**。中栏 toolbar「驾驶舱」按钮翻 true,驾驶舱头部
    /// 「退出」翻 false。放 runner 上让 MacThreePaneView（决定布局）与中栏（入口）共享。
    @Published var showingCockpit = false

    /// 驾驶舱段深链请求（Todo #12：工具栏 Todo 独立按钮直达 Todo 段）。值为
    /// `CockpitView.Segment` 的 rawValue；CockpitView 出现/变更时消费并清 nil。
    /// 放 runner 上与 `showingCockpit` 同层——入口（中栏 toolbar）与呈现（CockpitView）
    /// 分属两棵子树，只有 runner 两边都够得着。
    @Published var cockpitSegmentRequest: String?

    /// 最近一次「启动 Captain」失败的人类可读原因（手动按钮 + 建 crew 自动起共用）。
    /// 错误从前台终端模式（composer 里渲染 `localError`）和成员列表模式（按钮所在
    /// 那屏）两处都看不到会让人误以为「点了没反应」—— 所以放在 runner 上，让按钮所在
    /// 的成员列表模式也能显，且自动起（MacRootView，无 view-local error）失败时同样有处可落。
    /// 成功启动或下一次尝试开始时清空。
    @Published var lastStartError: String?

    /// 前台 run：选中的那个；没选中（或选中的已被移除）时回退最后一个 ——
    /// 回退只在前台 crew 内找,不越 crew 顶出别人的终端（#481）。
    /// 保留这个名字让既有 call site（composer / auto-claim）继续编译。
    var current: CrewSessionRun? {
        if let sel = runs.first(where: { $0.runID == selectedRunId }) { return sel }
        if let crewId = activeCrewId { return runs.last { $0.crewId == crewId } }
        return runs.last
    }

    // MARK: - per-crew 右栏选中态（#481）

    /// 右栏当前对应的 crew（`MacThreePaneView` 在 `selectedCrewId` 变化时经
    /// `switchCrew` 同步）。`start()` 用它判断新 run 属不属于前台 crew。
    private(set) var activeCrewId: String?

    /// 单个 crew 记住的右栏 UI 态：打开着哪个 session、终端还是成员列表、新建态。
    private struct PaneState {
        var selectedRunId: UUID?
        var viewingTerminal = false
        var isComposingNew = false
    }

    /// 每个 crew 各自的右栏态 —— 切 crew 存旧恢新，互不串。
    private var paneStates: [String: PaneState] = [:]

    /// 切 crew（#481）：把右栏选中态存回旧 crew，恢复新 crew 自己记住的那份。
    /// 记住的 run 已被移除时退回成员列表默认态（不 fallback 到别的 crew 的 run）。
    func switchCrew(from oldCrewId: String?, to newCrewId: String?) {
        guard oldCrewId != newCrewId else { return }
        if let old = oldCrewId {
            paneStates[old] = PaneState(selectedRunId: selectedRunId,
                                        viewingTerminal: viewingTerminal,
                                        isComposingNew: isComposingNew)
        }
        activeCrewId = newCrewId
        var st = newCrewId.flatMap { paneStates[$0] } ?? PaneState()
        if let rid = st.selectedRunId,
           !runs.contains(where: { $0.runID == rid && $0.crewId == newCrewId }) {
            st = PaneState()
        }
        selectedRunId = st.selectedRunId
        viewingTerminal = st.viewingTerminal
        isComposingNew = st.isComposingNew
    }

    /// 本地 mention 唤醒器（wake-resilience 根因修复）：session/机长 post_to_crew
    /// 的定向 @ → 注入/拉起目标。由 MacRootView.task 创建并 start（需要 backend
    /// provider 拉 crew detail），runner 只持有 + 在 run 启动时通知钉游标。
    var localMentionWaker: CrewLocalMentionWaker?

    /// 所有本地直投共用的 Codex busy -> idle 补投账。Claude backend 的 `isBusy`
    /// 恒 false（交互式 CLI 自带安全排队），因此它仍保持立即发送；Codex 在 turn
    /// 运行期间只登记，`turn/completed` 发布 idle 后自动起下一 turn。
    private var deferredWakes = CrewDeferredWakeQueue()
    private var deferredWakeCallbacks: [String: () -> Void] = [:]

    /// 正在拉起中的目标（`captain:<crewId>` / `member:<sessionId>`）。
    ///
    /// `startCaptain` / `restartMember` 开头的「已经在跑就别重复起」只看 `runs`，
    /// 而 run 是在一串 `await`（拉 crew detail、列成员、起进程）**之后**才进
    /// `runs` 的 —— 两个并发调用会双双通过那道守卫，起出两个进程。**同一条消息
    /// 有两个投递者时这就是活的**（2026-08-11 通讯录把唤醒器的扫描面扩到全部 crew
    /// 之后，休眠 crew 的机长同时被唤醒器和 `crewMessageWakes` 拉，正好凑齐）。
    /// 这里补的是 await 窗口内的互斥；`runs` 那道守卫照旧留着（管已经起好的）。
    private var launchesInFlight: Set<String> = []

    init() {}

    /// 切换前台 run（退出「新建」态）。
    func select(_ runId: UUID) {
        selectedRunId = runId
        isComposingNew = false
    }

    /// 进入「新建 session」态（composer 显零配置启动面）。
    func composeNew() {
        isComposingNew = true
    }

    /// 停掉指定 run（保留在列表里，状态落 cancelled）。
    func stop(_ runId: UUID) {
        guard let run = runs.first(where: { $0.runID == runId }) else { return }
        let crewId = run.crewId
        discardDeferredWakes(sessionId: run.sessionId)
        run.stop()
    }

    /// 移除指定 run（仍在跑则先停）。被移除的是选中项时，回退选最后一个。
    func remove(_ runId: UUID) {
        guard let idx = runs.firstIndex(where: { $0.runID == runId }) else { return }
        let run = runs[idx]
        let crewId = run.crewId
        discardDeferredWakes(sessionId: run.sessionId)
        if run.status == .running { run.stop() }
        runs.remove(at: idx)
        if selectedRunId == runId {
            // 回退只在同 crew 内找（#481）—— 别把前台切到另一个 crew 的 run。
            selectedRunId = runs.last(where: { $0.crewId == crewId })?.runID
        }
    }

    /// 本地唤醒投递的唯一 busy 门禁。目标 idle 时立即 send；Codex 正在跑 turn 时
    /// 留账，不打断当前 turn。`onDelivered` 只在真正调用 `run.send` 后执行，供游标
    /// 推进 / 投递回执接线使用；排队阶段绝不提前把消息标成已消费。
    func deliverOrDeferWake(
        sourceKey: String,
        to run: CrewSessionRun,
        text: String,
        onDelivered: (() -> Void)? = nil
    ) {
        guard run.status == .running, run.kind.isAgent else { return }
        let delivery = CrewDeferredWakeQueue.Delivery(
            key: sourceKey + "|target:" + run.sessionId,
            targetSessionId: run.sessionId,
            text: text)
        switch deferredWakes.submit(delivery, isBusy: run.backend.isBusy) {
        case let .deliver(ready):
            run.send(ready.text)
            onDelivered?()
        case .deferred:
            if let onDelivered { deferredWakeCallbacks[delivery.key] = onDelivered }
        case .duplicate:
            break
        }
    }

    /// 后端发布 busy -> idle 时补一条。再读一次 `backend.isBusy`，挡住 idle 事件排队
    /// 到主线程期间目标已经开始另一 turn 的窄竞态。
    private func runBecameIdle(_ run: CrewSessionRun) {
        guard run.status == .running, !run.backend.isBusy else { return }
        if let delivery = deferredWakes.popWhenIdle(sessionId: run.sessionId) {
            let callback = deferredWakeCallbacks.removeValue(forKey: delivery.key)
            run.send(delivery.text)
            callback?()
            // 这次 idle 已经被本地补投占用；`send` 正在起下一 turn，别同时让
            // mailbox 的异步重拉抢同一空闲窗口。下一次 idle 再处理服务端 inbox。
            return
        }
    }

    private func discardDeferredWakes(sessionId: String) {
        let deliveries = deferredWakes.remove(sessionId: sessionId)
        for delivery in deliveries { deferredWakeCallbacks.removeValue(forKey: delivery.key) }
    }

    // MARK: - session 自我配置（set_session_profile；#455）

    /// Change one live Codex thread between native auto_review and manual review.
    /// Persist only after app-server acknowledges thread/settings/update, so a failed
    /// switch is never shown as active and is not silently applied on the next resume.
    func applyCodexApprovalMode(
        to run: CrewSessionRun, reviewer: CodexProtocol.ApprovalsReviewer
    ) async {
        guard run.kind == .codex, run.status == .running else { return }
        do {
            if let backend = run.backend as? CodexAppServerBackend {
                try await backend.updateApprovalsReviewer(reviewer)
            } else if let backend = run.backend as? RemoteSessionBackend {
                try await backend.updateApprovalsReviewer(reviewer)
            } else {
                return
            }
            run.approvalsReviewer = reviewer
            let scope: CodexApprovalModeStore.Scope = run.role == .captain
                ? .captain : .session(run.sessionId)
            CodexApprovalModeStore.shared.set(reviewer, crewId: run.crewId, scope: scope)
        } catch {
            lastStartError = "切换 Codex 审批模式失败：\(error.localizedDescription)"
        }
    }

    /// 应用一条 session 自切模型/effort 请求，**并核实到底切没切成**（#544）。
    ///
    /// 老实现的坑（人类实测撞到）：直接往 PTY 写 `/model opus` 就当切好了、还抢先
    /// 回写 `run.model`。但 session 调 `set_session_profile` 时**按定义正在忙**
    /// （工具是它自己在回合里调的），忙时写进去的整行会被 claude 收进消息队列、
    /// 斜杠命令永不执行 —— 机长于是一路用 Fable 跑到撞周额度上限。
    ///
    /// 现在：`applyProfileSwitch` 等终端空闲才注入、并核对 claude 的生效回显；
    /// - 真生效 → 回写 `run.model/effort` + 白板 ✅ + **在终端告诉 session**
    ///   （撞额度的那种场景，它据此换模型直接接着跑，不用等额度重置）；
    /// - 没切成 → 白板如实说明原因并 @机长，`run.model/effort` 保持原值（UI 不谎报）。
    /// - codex：无中途切换通道，白板说明（现状不变）。
    /// - 目标 run 不在跑：白板落说明（fail-loud，不静默吞）。
    func applyProfileChange(_ req: SessionProfileChangeRequest) async {
        guard let run = runs.first(where: { $0.sessionId == req.sessionId && $0.status == .running }) else {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: req.crewId, sessionId: "system",
                text: "set_session_profile 未生效：目标 session 已不在跑。", senderName: "系统")
            return
        }
        // 纯终端没有模型/effort，也不属于 crew agent 编排；不往白板伪造失败回执。
        guard run.kind.isAgent else { return }
        guard run.kind == .claudeCode else {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: req.crewId, sessionId: "system",
                text: "\(run.displayName) 请求切换模型/effort，但 codex 不支持中途切换，需另起 session 接手。",
                senderName: "系统")
            return
        }

        var commands: [SessionProfileSwitchCommand] = []
        if let m = req.model { commands.append(SessionProfileSwitchCommand(knob: .model, value: m)) }
        if let e = req.effort { commands.append(SessionProfileSwitchCommand(knob: .effort, value: e)) }
        guard !commands.isEmpty else { return }

        run.pendingProfile = commands.map(\.value).joined(separator: " / ")
        defer { run.pendingProfile = nil }

        var applied: [String] = []
        var failed: [String] = []
        for cmd in commands {
            switch await run.applyProfileSwitch(cmd) {
            case .applied:
                switch cmd.knob {
                case .model: run.model = cmd.value
                case .effort: run.effort = cmd.value
                }
                applied.append(cmd.summary)
                // 换模型正是「撞限额后自救」的手段 —— 切成了就别再挂着「⏳ 限额中」。
                // （活跃度那条恢复判定也会兜到，但这里是确定性的，不等 6s streak。）
                if run.health?.isQuotaRelated == true { run.rearmQuotaHealth() }
            case let .rejected(quote):
                failed.append("\(cmd.summary)：claude 拒绝了 —— \(quote)")
            case .noConfirmation:
                failed.append("\(cmd.summary)：`\(cmd.line)` 已注入终端，但没等到 claude 的生效回显（当作没切成）")
            case .neverIdle:
                failed.append("\(cmd.summary)：终端一直没空闲窗口（或 session 已退出），斜杠命令发不出去")
            case .unsupported:
                failed.append("\(cmd.summary)：该 runner 没有中途切换通道")
            }
        }
        reportProfileSwitch(run: run, crewId: req.crewId, applied: applied, failed: failed)
    }

    /// 切换结果回执：白板一条（成功/失败如实分开），外加在终端知会 session 本人 ——
    /// 它才是发起方，且「撞额度→换模型接着跑」这条自愈路必须由它继续。
    private func reportProfileSwitch(
        run: CrewSessionRun, crewId: String, applied: [String], failed: [String]
    ) {
        if failed.isEmpty {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system",
                text: "\(run.displayName) 已切换：\(applied.joined(separator: "、"))。",
                senderName: "系统")
        } else {
            let head = applied.isEmpty ? "\(run.displayName) 切换失败"
                                       : "\(run.displayName) 只切成一半"
            let okLine = applied.isEmpty ? "" : "\n已生效：\(applied.joined(separator: "、"))"
            // 报了「没切成」就把当前可用清单一并给出 —— 光说失败，机长下一次
            // 还是只能靠猜（Todo #37）。清单自带新鲜度警示，不必也不该在这里拆开。
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system",
                text: "\(head)：\n" + failed.map { "· \($0)" }.joined(separator: "\n") + okLine
                    + "\n" + currentCatalogLine(for: run.kind),
                category: "error", senderName: "系统",
                mentions: applied.isEmpty ? [LocalWhiteboardMention(kind: "captain", targetId: nil)] : nil)
        }
        guard run.status == .running else { return }
        var lines = ["set_session_profile 结果（切换只能在你回合结束、终端空闲时执行，所以是现在才落）："]
        if !applied.isEmpty { lines.append("已生效：\(applied.joined(separator: "、"))") }
        for f in failed { lines.append("未生效：\(f)") }
        if !failed.isEmpty { lines.append(currentCatalogLine(for: run.kind)) }
        lines.append(applied.isEmpty
            ? "没切成 —— 别再假定已经换了。要么换个值重试，要么让机长 start_session 另起一个带目标配置的 session 接手。"
            : "手上的活没做完就接着做（撞额度的话现在已经在新模型上，不用等重置）；已经做完就不用回复。")
        run.send(lines.joined(separator: "\n"))
    }

    /// 该 runner 当前的可用模型/effort 一行（切换失败时附给机长与 session）。
    /// 走 `AgentModelCatalog.summaryLine` —— 新鲜度警示串在同一行里，别拆。
    private func currentCatalogLine(for kind: LocalCodingAgentKind) -> String {
        let key = SessionLaunchOptions.agentKey(for: kind)
        guard let table = AgentModelCatalogFile.resolveTable(
            agent: key, file: ModelCatalogCenter.shared.file)
        else { return AgentModelCatalog.missingLine(for: key) }
        return AgentModelCatalog.summaryLine(for: table)
    }

    // MARK: - 定时唤醒（schedule_wakeup；#455 额度重置自唤醒）

    /// 待触发的定时唤醒（持久化在 wakeups.json —— app 重启后 `rearmWakeups()` 重挂,
    /// 不因重启丢约）。持久化在 `LocalWakeupStore`（#528 基座三件套：损坏归档
    /// fail-loud，不再静默清空全部在途约定）。
    typealias PendingWakeup = LocalWakeupStore.PendingWakeup

    private let wakeupStore = LocalWakeupStore()
    private var wakeupTimers: [String: Timer] = [:]

    /// wakeups.json 出事 → fail-loud @机长。发到有活跃 run 的 crew；一个都没有
    /// （如刚启动）→ 发到所有 crew。
    ///
    /// **两种事故两套文案**（2026-08-12）：以前不论「读不出来」还是「真的解不开」
    /// 都说成「损坏，已归档」——那晚 24 次全是读失败，十几个机长照着这句话跑去翻
    /// 归档，发现文件好好的。现在读不出来时明说「原件完好、别去翻归档」，
    /// 只有确认解不开才谈失约与找回。
    private func reportWakeupIncident(_ incident: MultiProcessJSONStore.LedgerIncident) {
        let tail = incident.isDataIntact
            ? "在途的定时唤醒**没丢**，只是这一次没读到；下次读得动就照常。"
            : "在途的定时唤醒可能失约，请机长核对归档并重约。"
        var targets = Set(runs.filter(\.kind.isAgent).map(\.crewId))
        if targets.isEmpty { targets = Set(LocalCrewStore.shared.listCrews().map(\.id)) }
        for crewId in targets {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system",
                text: "定时唤醒账本 wakeups.json：" + incident.summary + tail,
                category: "question",
                senderName: "系统",
                mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        }
    }

    /// 登记一条定时唤醒：持久化 + 挂定时器。同 id 重复登记忽略（drain 重放安全）。
    func scheduleWakeup(_ req: SessionWakeupRequest) {
        guard runs.contains(where: {
            $0.sessionId == req.sessionId && $0.status == .running && $0.kind.isAgent
        }) else { return }
        let w = PendingWakeup(id: req.id, crewId: req.crewId, sessionId: req.sessionId,
                              fireAt: req.fireAt, note: req.note)
        guard wakeupStore.register(w, onIncident: { self.reportWakeupIncident($0) }) else { return }
        arm(w)
    }

    /// app 启动时重挂所有持久化的唤醒（已过期的立即触发 —— 迟到好过失约）。
    func rearmWakeups() {
        let pending = wakeupStore.list(onIncident: { self.reportWakeupIncident($0) })
        for w in pending where wakeupTimers[w.id] == nil { arm(w) }
    }

    private func arm(_ w: PendingWakeup) {
        let fireAt = McpServer.parseISO(w.fireAt) ?? Date()
        let delay = max(1, fireAt.timeIntervalSinceNow)
        wakeupTimers[w.id] = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.fire(w) }
        }
    }

    private func fire(_ w: PendingWakeup) {
        wakeupTimers[w.id]?.invalidate()
        wakeupTimers[w.id] = nil
        wakeupStore.remove(id: w.id, onIncident: { self.reportWakeupIncident($0) })
        let now = ISO8601DateFormatter().string(from: Date())
        if let run = runs.first(where: {
            $0.sessionId == w.sessionId && $0.status == .running && $0.kind.isAgent
        }) {
            // 额度重置唤醒到点 → 先清限额态（红点熄灭 + 检测重新武装,下个窗
            // 再撞墙能再次报警/挂唤醒）,再注入唤醒词。
            if w.note.hasPrefix("[auto]") { run.rearmQuotaHealth() }
            // #484 微信式精简：短标头即可，schedule_wakeup 语义教学在 world-model。
            run.send("""
            定时唤醒（你用 schedule_wakeup 约的）：
            - 备注: \(w.note)
            - 现在: \(now)。先 get_quota 确认额度,然后按备注继续。
            """)
        } else {
            // 目标已退 → fail-loud @机长：这条"有约没人赴"要有人接（用原 session
            // 重新拉起或重派）,不能只静静躺白板（Todo #10 ①）。
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: w.crewId, sessionId: "system",
                text: "定时唤醒到点，但目标 session（\(w.sessionId.prefix(13))…）已不在跑。备注：\(w.note)\n请机长决定重起还是改派。",
                category: "question",
                senderName: "系统",
                mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        }
    }

    // MARK: - 群聊收听（listen；#465）

    /// 生效中的收听登记（每 session 至多一条，后写覆盖）。**内存态不持久化** ——
    /// 本地 run 不跨 app 重启，收听跟着 run 的生命周期走。
    private var listeners: [CrewListenLogic.Listener] = []
    /// sessionId → 所属 crewId（决定观察哪块白板）。
    private var listenerCrew: [String: String] = [:]
    /// 每 crew 的收听游标（白板位置）：只送「开启收听之后」的新消息。
    /// #595：位置是 (id, 时间戳) 复合而非裸 id —— 白板被归档重建换了一批 id 时，
    /// 裸 id 游标当场悬空，而旧实现里「悬空 = 全是新的」，几周的历史会整块灌进
    /// 收听中的 session。
    private var listenCursors: [String: WhiteboardCursorPosition] = [:]
    private var listenWatchers: [AnyCancellable] = []

    /// 应用一条 `listen` 请求（开/关群聊收听）。开 = 覆盖登记 + 游标钉到白板当前
    /// 末尾（历史消息不回放 —— 白板每轮整块注入本就覆盖）；关 = 撤销登记。
    /// 到期自动失效（`CrewListenLogic.decide` 按 `until` 判定 + 惰性清理）。
    func applyListen(_ req: SessionListenRequest) {
        guard runs.contains(where: {
            $0.sessionId == req.sessionId && $0.status == .running && $0.kind.isAgent
        }) else { return }
        listeners.removeAll { $0.sessionId == req.sessionId }
        if req.off {
            listenerCrew[req.sessionId] = nil
            return
        }
        guard let until = req.until.flatMap(McpServer.parseISO), until > Date() else { return }
        // 本 crew 还没有别的收听者时才钉游标 —— 已有收听者的游标在跟进中，
        // 重置会把没送完的窗口吞掉。
        if !listenerCrew.values.contains(req.crewId) {
            // 钉在当前尾。读失败的合成警示行不许当游标（磁盘上没有这一行，钉了当场
            // 悬空）—— 判定与唤醒器共用 `pinPosition`；这次钉不上就退回 nil，收听窗口
            // 本就允许「从此刻起」的近似。
            if case .pin(let position) = CrewLocalMentionWakeLogic.pinPosition(
                rows: LocalWhiteboardStore.shared.list(crewId: req.crewId)) {
                listenCursors[req.crewId] = position
            } else {
                listenCursors[req.crewId] = nil
            }
        }
        // 机长身份决定它吃不吃 `@captain` 的定向（#543 可见性判定入参）。
        let isCaptain = runs.contains {
            $0.sessionId == req.sessionId && $0.role == .captain
        }
        listeners.append(CrewListenLogic.Listener(
            sessionId: req.sessionId, until: until, senders: req.senders, isCaptain: isCaptain))
        listenerCrew[req.sessionId] = req.crewId
        startListenWatchIfNeeded()
        // 不再往白板发「👂 …开启了群聊收听…」系统回执 —— 群消息保持精简,收听是
        // session 自己的事,发起方的确认由 McpServer listen 的 toolResult 只回本
        // session（不进白板,用户明确要「只自己看到、不进群」）。登记/投递/到期清理
        // 都在此之上、独立于这条回执,删掉不影响收听功能。
    }

    /// 挂上白板观察（幂等）。进程内 append 走 `changes`（带 crewId）；helper 子进程
    /// 的 `post_to_crew` 写盘走 `directoryChanged`（无 crewId → 扫所有有收听者的
    /// crew，读增量很廉价）。两路对同一批消息先后触发时，游标推进让第二次成 no-op。
    private func startListenWatchIfNeeded() {
        guard listenWatchers.isEmpty else { return }
        LocalWhiteboardStore.shared.startWatching()
        LocalWhiteboardStore.shared.changes
            .sink { [weak self] crewId in
                Task { @MainActor in self?.deliverListens(crewId: crewId) }
            }
            .store(in: &listenWatchers)
        LocalWhiteboardStore.shared.directoryChanged
            .sink { [weak self] in
                Task { @MainActor in
                    guard let self else { return }
                    for crewId in Set(self.listenerCrew.values) {
                        self.deliverListens(crewId: crewId)
                    }
                }
            }
            .store(in: &listenWatchers)
    }

    /// 把该 crew 白板游标之后的新消息按收听登记交给统一投递门禁。busy 不打断，
    /// 但消息计划会留在 runner，idle 后无需第二条白板消息自动补投。
    private func deliverListens(crewId: String) {
        pruneExpiredListeners()
        let active = listeners.filter { listenerCrew[$0.sessionId] == crewId }
        guard !active.isEmpty else { return }
        let entries = LocalWhiteboardStore.shared.entries(
            crewId: crewId, after: listenCursors[crewId])
        guard let last = entries.last else { return }
        listenCursors[crewId] = WhiteboardCursorPosition(id: last.id, createdAt: last.createdAt)
        let runStates = runs.filter { $0.status == .running && $0.kind.isAgent }
            .map { CrewLocalMentionInjectLogic.RunState(sessionId: $0.sessionId, isBusy: $0.backend.isBusy) }
        let injections = CrewListenLogic.plannedInjections(
            entries: entries, listeners: active, runs: runStates, now: Date())
        for inj in injections {
            guard let run = runs.first(where: {
                $0.sessionId == inj.sessionId && $0.status == .running
            }) else { continue }
            deliverOrDeferWake(
                sourceKey: "listen:\(crewId):\(last.id)",
                to: run,
                text: inj.text)
        }
    }

    /// 惰性清理：过期登记 + 已无收听者的 crew 游标。到期不发通知（像人不再盯群，
    /// 静默回落到「仅 @ 唤醒」）。
    private func pruneExpiredListeners() {
        let now = Date()
        let expired = listeners.filter { $0.until <= now }.map(\.sessionId)
        guard !expired.isEmpty else { return }
        listeners.removeAll { $0.until <= now }
        for sid in expired { listenerCrew[sid] = nil }
        let liveCrews = Set(listenerCrew.values)
        listenCursors = listenCursors.filter { liveCrews.contains($0.key) }
    }

    // MARK: - 成员状态快照（机长 list_sessions 用;#463 组织能力）

    /// 把 runs 实时状态落 crew-sessions.json（离线 helper 的 `list_sessions` 读）。
    /// 2 秒节流定时器周期写 + start/remove 即写 —— run 的 working/health 变化
    /// 最迟 2 秒进快照,机长点名够用,不追帧。
    private var snapshotTimer: Timer?

    /// 「在等谁回话」那两样磁盘输入（审批账本 + 回合 marker）的指纹门控缓存。
    ///
    /// 2026-08-18「开久了卡」第三条：这一拍原本在 MainActor 上按 crew 逐个
    /// flock + 整份解码审批 JSON、再按 run 逐个读 turn marker，而 crew / run 只增
    /// 不减 —— 开一天下来每 2 秒几十次加锁读盘挂在主线程。现在两样都走指纹门控
    /// （没变就不读）且整段挪到后台队列，主线程只剩「把结果写回 @Published + 组装
    /// 快照」。判定口径一个字没改，详见 `SessionAwaitingReplyInputsCache`。
    private let awaitingInputs = SessionAwaitingReplyInputsCache(
        directory: LocalWhiteboardStore.defaultDirectory)

    /// 一拍还在后台跑时不再起第二拍（定时器 + start 即写两条来源会撞上）；
    /// 期间来的请求记一笔，跑完立刻补一拍 —— 「新成员立刻进快照」不能被吞掉。
    private var snapshotTickInFlight = false
    private var snapshotTickQueued = false

    func startSessionsSnapshotTimer() {
        guard snapshotTimer == nil else { return }
        snapshotTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.persistSessionsSnapshot() }
        }
    }

    /// 走一拍：后台读两样输入 → 回主线程重算「在等谁回话」+ 落点名快照。
    ///
    /// 重算而不是驻留仍是**每拍**做（口径没变，见 `SessionAwaitingReply`）；变的只是
    /// 「输入从哪来」——从主线程现读现解，改成后台指纹门控缓存。红点/点名的延迟因此
    /// 不会变差：定时器周期仍是 2 秒，多出来的只是一次后台 hop（微秒级），而省掉的是
    /// 主线程上几十次加锁读盘。
    func persistSessionsSnapshot() {
        guard !snapshotTickInFlight else {
            snapshotTickQueued = true
            return
        }
        snapshotTickInFlight = true
        let keys = runs.filter { $0.kind.isAgent }.map {
            SessionAwaitingReplyInputsCache.RunKey(crewId: $0.crewId, sessionId: $0.sessionId)
        }
        let cache = awaitingInputs
        Task.detached(priority: .utility) { [weak self] in
            let inputs = cache.refresh(runs: keys)
            await self?.applySnapshotTick(inputs)
        }
    }

    /// 一拍的主线程那半：把后台读到的输入写回 run，再组装 + 落盘点名快照。
    @MainActor
    private func applySnapshotTick(_ inputs: [SessionAwaitingReplyInputsCache.RunKey:
                                              SessionAwaitingReplyInputsCache.Inputs]) {
        snapshotTickInFlight = false
        refreshAwaitingReplies(inputs)
        writeSessionsSnapshot()
        if snapshotTickQueued {
            snapshotTickQueued = false
            persistSessionsSnapshot()
        }
    }

    /// 重算每个 run 的「在等谁回话」（人类 Todo #25 层 2）。跟着点名快照的 2 秒定时器
    /// 走 —— 红点晚 2 秒亮完全够用，换来的是**每拍重算**：不存在「进得去出不来」。
    ///
    /// 两样磁盘输入（审批账本里本 session 的 pending 摘要 / 上一轮的收尾问句）由
    /// `SessionAwaitingReplyInputsCache` 在后台按 crew、按 run 各取一次分发下来，
    /// 这里只做纯内存的判定与赋值。ask 与权限钩子写的是同一份，两类都算「它被挡在
    /// 那儿等人」。
    private func refreshAwaitingReplies(
        _ inputs: [SessionAwaitingReplyInputsCache.RunKey:
                   SessionAwaitingReplyInputsCache.Inputs]
    ) {
        for run in runs where run.kind.isAgent {
            let key = SessionAwaitingReplyInputsCache.RunKey(
                crewId: run.crewId, sessionId: run.sessionId)
            run.refreshAwaitingReply(
                pendingApprovalSummary: inputs[key]?.pendingApprovalSummary,
                trailingQuestion: inputs[key]?.trailingQuestion)
        }
    }

    /// 组装点名快照并落盘。编码在主线程（对象小、必须与 runs 同一拍取值），
    /// **写盘挪到后台** —— 那是 2 秒一次的同步磁盘 IO，没有理由占着主线程。
    private func writeSessionsSnapshot() {
        var snapshot = CrewSessionsSnapshot()
        snapshot.updatedAt = ISO8601DateFormatter().string(from: Date())
        for run in runs where run.kind.isAgent {
            // 状态推导走共享的那一份（`CrewSessionStateDerivation`）—— 额度类单列
            // "rateLimited"（Todo #10 层2）、拉起失败单列 "launchFailed"（#541）：
            // 机长点名一眼分得清「等额度重置中」「从来没起来」「真空闲」，不误派活。
            let state = CrewSessionStateDerivation.state(
                isRunning: run.status == .running, health: run.health, isWorking: run.isWorking,
                awaitingDecision: run.pendingDecision != nil,
                awaitingReply: run.awaitingReply != nil)
            let healthDetail: String?
            switch state {
            case CrewSessionStateDerivation.awaitingDecision:
                // 「在等什么」比「异常说明」更该出现在点名里 —— 机长一眼看清能不能代答。
                healthDetail = run.pendingDecision?.prompt
            case CrewSessionStateDerivation.awaitingReply:
                healthDetail = run.awaitingReply?.summary
            case "error", "rateLimited", CrewSessionStateDerivation.launchFailed:
                healthDetail = run.health?.detail
            default:
                healthDetail = nil
            }
            snapshot.crews[run.crewId, default: []].append(CrewSessionsSnapshot.Entry(
                sessionId: run.sessionId, name: run.displayName,
                role: run.role == .captain ? "captain" : "worker",
                brief: run.role == .captain ? "" : run.taskBrief,
                state: state, healthDetail: healthDetail))
        }
        guard let data = try? JSONEncoder().encode(snapshot) else { return }
        let url = LocalWhiteboardStore.defaultDirectory
            .appendingPathComponent(CrewSessionsSnapshot.fileName)
        Task.detached(priority: .utility) {
            try? data.write(to: url, options: .atomic)
        }
    }

    // MARK: - 撞墙自动挂钩 + 额度警戒广播（#455 增补：不只靠 session 自觉）

    /// session 撞到额度上限（健康感知首报 / rate-limit 菜单命中 / hit-limit 终止,
    /// 多路都汇到这,按待触发 [auto] 钩子去重）→ 自动挂一条「额度重置后唤醒」。
    /// 时刻计算走 `QuotaWakeupPlan`（纯函数,单测覆盖）：重置时刻解析成功 →
    /// **重置+1分钟**;解析不出/缺失/已过期 → 45 分钟退避重试并在白板 fail-loud
    /// 说清没拿到重置时刻（唤醒词让它先 get_quota 核实,醒来仍受限会自己顺延）。
    func autoScheduleQuotaWakeup(for run: CrewSessionRun) {
        guard run.kind.isAgent else { return }
        guard !wakeupStore.list(onIncident: { self.reportWakeupIncident($0) }).contains(where: {
            $0.sessionId == run.sessionId && $0.note.hasPrefix("[auto]")
        }) else { return }
        Task { @MainActor in
            await QuotaCenter.shared.refresh()   // 撞墙瞬间拿最新重置时刻
            let snap = run.kind == .claudeCode ? QuotaCenter.shared.claude : QuotaCenter.shared.codex
            let plan = QuotaWakeupPlan.compute(resetsAt: snap?.fiveHourWindow?.resetsAt)
            let iso = ISO8601DateFormatter().string(from: plan.fireAt)
            scheduleWakeup(SessionWakeupRequest(
                id: "autoq-\(run.sessionId)-\(Int(plan.fireAt.timeIntervalSince1970))",
                crewId: run.crewId, sessionId: run.sessionId, fireAt: iso,
                note: "[auto] 额度已重置，继续之前被打断的工作：你上次撞到额度上限被迫停下。先 get_quota 核实额度已恢复,再回顾白板与工作区接着干;若仍受限,自己 schedule_wakeup 顺延。"))
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: run.crewId, sessionId: "system",
                text: plan.isFallback
                    ? "\(run.displayName) 撞到额度上限，重置时刻不可信，已按 45 分钟退避挂唤醒（\(iso)）。"
                    : "\(run.displayName) 撞到额度上限，已挂重置唤醒（\(iso)）。",
                senderName: "系统")
        }
    }

    // MARK: - 投递回执（wake-resilience：修「假送达」）

    /// 唤醒注入后的回执编排（判定纯函数在 `CrewMailboxWakeLogic`）：先等注入
    /// 回显安静（2s），再窗内每秒采样目标工作态（isBusy||isWorking），见工作态
    /// 提前确认。confirmed → `onConfirmed`（消费：mark-delivered / 推游标）；
    /// failed → `onFailed` + 白板告警 @captain（system 身份 —— system 条目免
    /// 回执追踪，机长也唤不醒时不会告警成环）。
    func confirmWake(
        run: CrewSessionRun, crewId: String,
        onConfirmed: @escaping () -> Void, onFailed: (() -> Void)? = nil
    ) {
        let targetLabel = run.displayName
        Task { @MainActor in
            // 注入回显尾：send() 后 claude TUI 会重绘输入框（即便模态菜单吞了
            // 正文也有一次回显），isWorking 会短暂为真 —— 先跳过这段再采样，
            // 避免把回显误判成「转入工作态」。
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            var samples: [Bool] = []
            let deadline = Date().addingTimeInterval(CrewMailboxWakeLogic.receiptWindow)
            while Date() < deadline, run.status == .running {
                samples.append(run.backend.isBusy || run.backend.isWorking)
                if samples.last == true { break }   // 已见工作态，提前确认
                try? await Task.sleep(nanoseconds:
                    UInt64(CrewMailboxWakeLogic.receiptSampleInterval * 1_000_000_000))
            }
            switch CrewMailboxWakeLogic.receiptVerdict(workingSamples: samples) {
            case .confirmed:
                onConfirmed()
            case .failed:
                onFailed?()
                LocalWhiteboardStore.shared.appendSessionMessage(
                    crewId: crewId, sessionId: "system",
                    text: CrewMailboxWakeLogic.wakeFailureAlert(targetLabel: targetLabel),
                    category: "question", senderName: "系统",
                    mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
            }
        }
    }

    // MARK: - 机长 session 操作（inspect_session / nudge_session / stop_session）

    /// 执行一条机长 session 操作并写应答文件（helper 侧 long-poll 取走）。inspect =
    /// 状态 + 终端现场；nudge = 向目标发文本/按键；stop = 先落白板再真停进程。
    /// 所有分支都必须应答 —— helper
    /// 在等，静默吞会让机长白等到超时。
    func applySessionOp(_ req: SessionOpsRequest) {
        let respond = { (text: String) in
            LocalCrewControlStore.shared.writeCommandResponse(
                crewId: req.crewId, commandId: req.commandId, text: text)
        }
        if let reason = req.stopReason, let requester = req.requesterSessionId {
            let targets = runs.filter(\.kind.isAgent).map {
                SessionStopTarget(
                    sessionId: $0.sessionId, crewId: $0.crewId,
                    displayName: $0.displayName, isRunning: $0.status == .running)
            }
            let result = SessionStopCoordinator.execute(
                requestCrewId: req.crewId, requesterSessionId: requester,
                targetSessionId: req.targetSessionId, reason: reason, targets: targets,
                writeReceipt: { text in
                    LocalWhiteboardStore.shared.appendSessionMessage(
                        crewId: req.crewId, sessionId: requester, text: text,
                        category: "milestone", senderName: "机长", senderKind: "captain")
                },
                terminate: { target in
                    guard let run = self.runs.first(where: {
                        $0.sessionId == target.sessionId && $0.crewId == target.crewId
                            && $0.kind.isAgent
                    }) else { return }
                    self.stop(run.runID)
                })
            respond(result)
            return
        }
        guard let run = runs.first(where: {
            $0.sessionId == req.targetSessionId && $0.kind.isAgent
        }) else {
            respond("找不到 session \(req.targetSessionId)：不在本机在跑列表（可能已退出）。用 list_sessions 核对 id。")
            return
        }
        if let input = req.input {
            respond(nudge(run: run, input: input))
        } else {
            respond(inspect(run: run))
        }
    }

    /// 终端现场快照：状态行（与点名快照同一套推导）+ 最近若干行画面。
    private func inspect(run: CrewSessionRun) -> String {
        // 与点名快照同一套推导（`CrewSessionStateDerivation`），只是换成中文标签。
        let state: String
        switch CrewSessionStateDerivation.state(
            isRunning: run.status == .running, health: run.health, isWorking: run.isWorking,
            awaitingDecision: run.pendingDecision != nil,
            awaitingReply: run.awaitingReply != nil) {
        case CrewSessionStateDerivation.launchFailed:
            state = "拉起失败（\(run.health?.detail ?? "")）"
        case CrewSessionStateDerivation.awaitingDecision:
            state = "等人拍板（\(run.pendingDecision?.prompt ?? "")）—— 下面的画面里就是那个菜单，"
                + "你能拍就 nudge_session 发选项数字或 Enter，拍不了就发群 @人"
        case CrewSessionStateDerivation.awaitingReply:
            state = "\(run.awaitingReply?.label ?? "待回复")（\(run.awaitingReply?.summary ?? "")）"
                + " —— 它在等人回话，你答得了就 nudge_session 回它一句，答不了就发群 @人"
        case "exited":       state = "已退出"
        case "rateLimited":  state = "限额中（\(run.health?.detail ?? "")）"
        case "error":        state = "异常（\(run.health?.detail ?? "")）"
        case "working":      state = "干活中"
        default:             state = "空闲"
        }
        var lines = ["「\(run.displayName)」（\(run.sessionId)）状态：\(state)"]
        // 终端后端读的是**权威画面**（core 那份无画面 `Terminal` 的当前屏幕），
        // 与任何窗口滚到哪无关。改动前读的是 SwiftTerm 的当前可见区（按 `yDisp`）
        // —— 也就是**人把那个终端往上滚，机长看到的文本就跟着变**。那是实现细节
        // 漏出来的，不是设计：机长要的是「它现在卡在哪一屏」，不是「人正在看哪一屏」。
        // 改完之后同一个 sessionId 在任何时刻问到的都是同一份画面（spec §5.1）。
        // `getLine` / `translateToString` 在无画面的 `Terminal` 上一模一样，所以
        // P4 之后这段由 daemon 侧执行，app 连不连着都问得到同一份画面。
        let maxLines = run.kind == .codex ? 10 : 40
        if let tail = SessionAuthoritativeScreenText.read(
            from: run.backend, maxLines: maxLines
        ) {
            if run.kind == .codex {
                lines.append("transcript 尾部：\n\(tail)")
            } else {
                lines.append(tail.isEmpty
                    ? "（终端画面为空）"
                    : "终端画面（权威画面，尾部空行已去）：\n\(tail)")
            }
        } else {
            lines.append("（该后端类型无可读输出）")
        }
        return lines.joined(separator: "\n")
    }

    /// 向目标发文本/按键。"Enter"/"Esc" 是按键（解模态菜单）；其余文本走 send
    /// （claude=正文+隔拍回车提交；codex=起新 turn）。
    private func nudge(run: CrewSessionRun, input: String) -> String {
        guard run.status == .running else { return "「\(run.displayName)」已退出，无法注入。" }
        guard run.kind.isAgent else {
            return "「\(run.displayName)」是人的纯终端，不接受 crew agent 编排注入。"
        }
        // 有人来答了 —— 先熄掉「在等回复」，别让机长代答完界面还红着（Todo #25 层 2）。
        run.clearAwaitingQuestionMarker()
        let key = input.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if let term = run.backend as? AgentTerminalSession {
            switch key {
            case "enter", "回车":
                term.sendRaw([0x0d])
                return "已向「\(run.displayName)」发送 Enter。稍后 inspect_session 复查画面。"
            case "esc":
                term.sendRaw([0x1b])
                return "已向「\(run.displayName)」发送 Esc。稍后 inspect_session 复查画面。"
            default:
                term.send(input)
                return "已把文本发给「\(run.displayName)」（自动回车提交）。"
            }
        }
        if let remote = run.backend as? RemoteSessionBackend, run.kind == .claudeCode {
            switch key {
            case "enter", "回车":
                remote.sendRaw([0x0d])
                return "已向「\(run.displayName)」发送 Enter。稍后 inspect_session 复查画面。"
            case "esc":
                remote.sendRaw([0x1b])
                return "已向「\(run.displayName)」发送 Esc。稍后 inspect_session 复查画面。"
            default:
                remote.send(input)
                return "已把文本发给「\(run.displayName)」（自动回车提交）。"
            }
        }
        // codex（app-server）：无 PTY 按键语义。
        switch key {
        case "esc":
            run.backend.interrupt()
            return "已打断「\(run.displayName)」的当前 turn（codex interrupt）。"
        case "enter", "回车":
            return "「\(run.displayName)」是 codex session，无终端按键；发文本会作为新 turn 输入。"
        default:
            run.backend.send(input)
            return "已把文本作为新 turn 输入发给「\(run.displayName)」。"
        }
    }

    /// 已广播过的警戒键（agent|窗|重置时刻）——一个重置周期只喊一次,不刷屏。
    private var warnedQuotaKeys: Set<String> = []

    /// 额度阻断窗越过**按订阅档位计算的提醒线**，且尚未临近重置 → 按 runner 分流
    /// 广播事实（人类 Todo #26 管受众，#39 管门槛/时机/措辞）。session 每轮注入
    /// 白板，不用主动轮询 get_quota 也能看到档位、窗口、百分比与重置距离，再自行判断。
    ///
    /// 编排（发给谁、什么文案、@谁、怎么去重）全在纯函数 `QuotaWarningPlan` 里,
    /// 这里只负责喂数据 + 写白板。单模型周窗（周(Fable) 等）耗尽可切模型继续,
    /// 不算停摆风险 → 不发（`AgentQuotaWindow.isBlocking`,人类拍板 2026-07-26）。
    /// 两家快照都从 `QuotaCenter` 现取（判「两家都将尽」必须同时看两边），
    /// 调用方只需在任一家刷新后招呼一声。
    func broadcastQuotaWarningIfNeeded() {
        let sessions = runs.filter { $0.status == .running && $0.kind.isAgent }.map {
            QuotaWarningPlan.RunningSession(
                sessionId: $0.sessionId, crewId: $0.crewId,
                agent: $0.kind == .claudeCode ? "claude" : "codex")
        }
        let messages = QuotaWarningPlan.compute(
            claude: QuotaCenter.shared.claude,
            codex: QuotaCenter.shared.codex,
            sessions: sessions,
            alreadyWarned: warnedQuotaKeys)
        for m in messages {
            warnedQuotaKeys.insert(m.dedupeKey)
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: m.crewId, sessionId: "system", text: m.text,
                senderName: "系统",
                mentions: m.mentions.isEmpty ? nil : m.mentions)
        }
    }

    // MARK: - 事件驱动唤醒（Phase 4b）

    /// 起 session **没起成**的统一 fail-loud（#541）：`start*` 抛错的那几条来路
    /// （机长 start_session 排队、驾驶舱派活、手动起机长）都走这里，别各自吞。
    /// ① `lastStartError` → UI 横幅（沿用「人类发言自动拉起机长失败」那条留痕通道）；
    /// ② 白板一条 @机长 —— 机长看到才能立刻改派，否则 brief 就此人间蒸发。
    /// `brief` 非空时带上任务前缀，让机长认得出是哪份活没派出去。
    ///
    /// `mentionCaptain: false` 用于**起机长自己失败**那条来路 —— @机长会触发
    /// 「目标缺席拉起」，起不来又发一条，就此成环。那条改成广播，受众本来也是人。
    func reportStartFailure(
        crewId: String, brief: String?, error: Error, mentionCaptain: Bool = true
    ) {
        let reason = error.localizedDescription
        lastStartError = reason
        let briefPart = (brief?.isEmpty == false) ? "\n无人接手的活：\(brief!.prefix(80))" : ""
        LocalWhiteboardStore.shared.appendSessionMessage(
            crewId: crewId, sessionId: "system",
            text: "起 session 失败：\(reason)\(briefPart)\n请改派或重起。",
            category: "question", senderName: "系统",
            mentions: mentionCaptain
                ? [LocalWhiteboardMention(kind: "captain", targetId: nil)] : nil)
    }

    /// 起一个新 session。pre-flight 失败抛错（工具未装 / 超并发上限）。成功后
    /// 把新 run 追加进 `runs` 并选中；agent 在内嵌终端里交互式跑，旧 run 不受影响。
    func start(
        crewId: String,
        /// 本 session 的合成 id（BYOK localSessionId / logged 服务端 id）—— 存到 run 上，
        /// 右栏内联待办卡片靠它从 `LocalApprovalStore` 过滤出本 session 的待决策/待审批。
        sessionId: String,
        /// 完整的 per-session 启动配置（kind / model / effort / initialPrompt /
        /// permissionMode）。Claude 生成 argv；Codex 除 `app-server` 子命令外的配置
        /// 全部经 thread/start 或 thread/resume 发送（spec §1/§11）。
        config: SessionConfig,
        workingDirectory: URL,
        taskBrief: String,
        /// 精简标题（单一真值，见 `CrewSessionRun.title`）。nil = 从 `taskBrief` 兜底
        /// derive。captain 起 session 时把 `--label` 传的 title 一并透进来，让 run
        /// 上的 title 与 helper 白板署名逐字一致。
        title: String? = nil,
        additionalEnv: [String: String] = [:],
        /// codex 世界观（字符串）→ `thread/start.developerInstructions`。caller 在
        /// 拿得到 detail/members 处用 `LocalSessionLaunch.renderWorldModelString` 算好传入。
        /// claude 不用这个 —— claude 走 `config.appendSystemPromptFile`（文件路径）。
        developerInstructions: String? = nil,
        /// codex crew MCP 配置 dict → `thread/start.mcpServers`。caller 用
        /// `LocalSessionLaunch.codexMcpServers` 算好传入。claude 不用 —— 走
        /// `config.mcpConfigFile`（文件路径）。
        codexMcpServers: [String: Any]? = nil,
        /// captain / worker 角色标记（session 切换条展示 + captain 置顶）。
        role: CrewSessionRun.Role = .worker,
        /// **这次启动是不是人自己点出来的**（#42）。`true` 只给「人在新建面板里填完
        /// 提交」「人在驾驶舱按派单」这两条路 —— 人就是要看它，跳过去才对。
        /// 机长 `start_session`、@ 唤醒拉起、远程排队认领一律 `false`：它们在背后起，
        /// 抢走用户正开着的 session 就是 #40/#42 那个毛病。
        userInitiated: Bool = false
    ) async throws {
        // 没显式选 model → 解析一个具体默认别名显式落进 config（argv 带 --model、
        // run.model 永不为 nil）：显示=实际跑的模型，不再糊「默认」(#489)。codex 若
        // 读不到 ~/.codex/config.toml 的 model 仍为 nil（由 app-server 采用默认模型）。
        var config = config
        if config.kind.isAgent, config.model == nil {
            config.model = SessionLaunchOptions.defaultModel(
                for: config.kind, projectDir: workingDirectory)
        }
        // 1. 校验可执行路径。纯终端直接用用户默认 shell，不走 coding-agent CLI 发现。
        let executable: URL
        let env: [String: String]
        if config.kind == .terminal {
            guard let shell = PlainTerminalSession.defaultShell() else {
                throw RunnerError.defaultShellUnavailable
            }
            executable = URL(fileURLWithPath: shell)
            var shellEnv = PlainTerminalSession.shellEnvironment()
            for (key, value) in additionalEnv where !LocalCodingAgentEnv.isForbidden(key: key) {
                shellEnv[key] = value
            }
            env = shellEnv
        } else {
            guard let resolved = LocalCodingAgentExecutable.resolve(config.kind) else {
                throw RunnerError.toolNotInstalled(kind: config.kind)
            }
            executable = resolved
            // 2. agent env 白名单（spec v2 §8.4.2）。
            env = LocalCodingAgentEnv.build(additionalEnv: additionalEnv, kind: config.kind)
        }
        // 3. 按 kind 选后端：claude = agent PTY；codex = app-server；terminal = plain PTY。
        let directBackend: any SessionBackend
        switch config.kind {
        case .claudeCode:
            // Claude 的 PostToolUse 白板 hook 要等第一次工具调用后才会触发；首轮 brief
            // 必须在这里先带上同一游标的未读，否则新 session 第一拍就是空降。
            if let prompt = config.initialPrompt, !prompt.isEmpty {
                config.initialPrompt = LocalSessionLaunch.initialPromptWithWhiteboard(
                    prompt, crewId: crewId, sessionId: sessionId,
                    captain: role == .captain)
            }
            // Todo #28：claude 的会话号由我们指定（`--session-id`）并立刻记账，
            // 这样这个 session 关掉再点恢复时能 `--resume` 回同一条对话。续跑
            // （resumeSessionId 非空）时 claude 写回同一个会话号，账本不用改。
            if config.resumeSessionId == nil {
                config.newSessionId = AgentSessionResume.newClaudeSessionId()
            }
            if let agentId = config.resumeSessionId ?? config.newSessionId {
                // Todo #68：把**真实 cwd** 一并记下（isolation worktree 的成员记的就是
                // worktree 路径）。它只回答「当初在哪儿跑」——「日志在哪儿」跟目录无关
                // （claude `--resume` 按会话号找全盘，见 `AgentSessionResume` 的实测）。
                LocalAgentSessionStore.shared.record(
                    crewId: crewId, sessionId: sessionId,
                    kind: config.kind.rawValue, agentSessionId: agentId,
                    workingDirectory: workingDirectory.path)
            }
            directBackend = AgentTerminalSession(
                config: config,
                executable: executable.path,
                workdir: workingDirectory.path,
                env: env,
                protocolOutputSink: SessionBackendRouting.usesProtocolTransport
                    ? sessionProtocolBridge.terminalOutputSink(sessionId: sessionId) : nil
            )
        case .codex:
            // codex 没有 claude 的 hook/settings 通道：session 配置、世界观与 MCP
            // 全走 app-server thread/start（或 thread/resume），白板逐轮走 turn/start。
            // 后两者用 in-process provider 复用同一份本地 store（与 claude 的 helper 子进程
            // 读写同一份白板/审批 JSON —— 右栏内联卡片 / 群聊白板对齐）。
            let approvalScope: CodexApprovalModeStore.Scope = role == .captain
                ? .captain : .session(sessionId)
            let approvalsReviewer = CodexApprovalModeStore.shared.reviewer(
                crewId: crewId, scope: approvalScope)
            let cb = CodexAppServerBackend(
                executable: executable.path,
                argv: config.argv(),
                cwd: workingDirectory.path,
                env: env,
                model: config.model,
                effort: config.effort,
                resumeThreadId: config.resumeSessionId,
                approvalsReviewer: approvalsReviewer,
                developerInstructions: developerInstructions,
                mcpServers: codexMcpServers,
                whiteboardProvider: Self.makeWhiteboardProvider(
                    crewId: crewId, sessionId: sessionId, captain: role == .captain),
                approvalProvider: Self.makeApprovalProvider(crewId: crewId, sessionId: sessionId),
                notifyUnanswerable: Self.makeUnanswerableNotifier(
                    crewId: crewId, sessionId: sessionId, isCaptain: role == .captain),
                notifyTurnEnded: Self.makeTurnEndedNotifier(
                    crewId: crewId, sessionId: sessionId,
                    sessionName: CrewSessionTitle.resolve(explicit: title, brief: taskBrief),
                    isCaptain: role == .captain),
                // Todo #28：握手拿到 threadId 就记账，重启这个成员时 thread/resume 回来。
                notifyThreadId: { tid in
                    // Todo #68：同 claude 那处 —— 真实 cwd 一并记下（唤醒时定进程目录用）。
                    LocalAgentSessionStore.shared.record(
                        crewId: crewId, sessionId: sessionId,
                        kind: LocalCodingAgentKind.codex.rawValue, agentSessionId: tid,
                        workingDirectory: workingDirectory.path)
                },
                // 续不回来 → 已降级新起一条 thread，如实进群说明（不静默假装恢复）。
                notifyResumeFallback: { failedId, reason in
                    LocalWhiteboardStore.shared.appendSessionMessage(
                        crewId: crewId, sessionId: "system",
                        text: "「\(CrewSessionTitle.resolve(explicit: title, brief: taskBrief))」"
                            + "的原对话接不回来了（会话 \(failedId)：\(reason)），这一轮是**新开的**"
                            + "——群里的上下文它还在，终端里聊过的细节要重讲。",
                        category: "progress", senderName: "系统")
                },
                protocolNotificationSink: SessionBackendRouting.usesProtocolTransport
                    ? sessionProtocolBridge.codexNotificationSink(sessionId: sessionId) : nil)
            cb.boot(initialPrompt: config.initialPrompt)
            directBackend = cb
        case .terminal:
            // 人的工具：没有 initial prompt、世界观、MCP、白板 provider 或 agent 状态扫描。
            directBackend = PlainTerminalSession(
                shell: executable.path,
                workdir: workingDirectory.path,
                environment: env,
                protocolOutputSink: SessionBackendRouting.usesProtocolTransport
                    ? sessionProtocolBridge.terminalOutputSink(sessionId: sessionId) : nil)
        }
        let backend: any SessionBackend = SessionBackendRouting.usesProtocolTransport
            ? sessionProtocolBridge.expose(sessionId: sessionId, backend: directBackend)
            : directBackend
        // 4. 包成 view model
        let run = CrewSessionRun(
            crewId: crewId,
            sessionId: sessionId,
            kind: config.kind,
            taskBrief: taskBrief,
            title: CrewSessionTitle.resolve(explicit: title, brief: taskBrief),
            workingDirectory: workingDirectory,
            model: config.model,
            effort: config.effort,
            approvalsReviewer: config.kind == .codex
                ? CodexApprovalModeStore.shared.reviewer(
                    crewId: crewId,
                    scope: role == .captain ? .captain : .session(sessionId))
                : nil,
            permissionModeOverride: config.kind.isAgent ? config.permissionMode : nil,
            backend: backend,
            role: role
        )
        // 多 run 并存：追加进列表，不动旧 run（每个 run 在切换条上都有自己的
        // 停止/移除入口，不会变 orphan）。
        //
        // **选不选中它，看这次启动是不是人自己点出来的**（#42）：
        // - 人主动新建 / 驾驶舱派单（`userInitiated`）→ 跳过去，人就是要看它。
        // - 程序在背后起的（机长 `start_session`、@ 唤醒拉起、远程排队认领）→ 不抢，
        //   只记进 pane 态，等人自己切过去。跨 crew 那半 #481 已经这么做了，这里补的
        //   是**同 crew** 这半：你正开着 A 谈事，机长在同一个群里起了 B，右栏被切走。
        // - 唯一例外：那个 crew 还一个 run 都没选中（`selectedRunId == nil`）时选中它，
        //   否则右栏一片空白 —— 没有「正在看的东西」可打断，谈不上抢（与 #40 同一个口子）。
        runs.append(run)
        let paneState = paneStates[crewId] ?? PaneState()
        switch SessionForegroundClaim.decide(
            isForegroundCrew: activeCrewId == nil || crewId == activeCrewId,
            userInitiated: userInitiated,
            foregroundSelection: selectedRunId,
            paneSelection: paneState.selectedRunId
        ) {
        case .selectForeground:
            selectedRunId = run.runID
            isComposingNew = false
        case .rememberInPane:
            var st = paneState
            st.selectedRunId = run.runID
            st.isComposingNew = false
            paneStates[crewId] = st
        case .leaveAlone:
            break
        }
        // 撞额度上限（健康感知首报）→ 自动挂重置唤醒（用户定调:hit limit 才挂钩）。
        if config.kind.isAgent {
            run.onUsageLimit = { [weak self] r in self?.autoScheduleQuotaWakeup(for: r) }
        // 拉起失败（#541）→ 落进 lastStartError（UI 横幅）。白板 @机长 那条由 run
        // 自己发（它掌握去重），这里只补人面的留痕。
            run.onLaunchFailure = { [weak self] r, h in
                self?.lastStartError = "「\(r.displayName)」拉起失败：\(h.detail)"
            }
        // Codex turn 完成的这一拍冲刷 busy 期间的待投消息；不依赖第二条白板消息
        // 或 hub 事件来“碰醒”。Claude `isBusy` 恒 false，仍走即时投递。
            run.onBecameIdle = { [weak self] r in
                self?.runBecameIdle(r)
            }
            let launchedAt = Date()
            let launchedConfig = config
            run.onEnded = { [weak self] r in
                self?.discardDeferredWakes(sessionId: r.sessionId)
                // Todo #68：claude 自己拒了这个会话号 → 不带 --resume 重起一次。
                self?.retryWithoutResumeIfClaudeRefused(
                    run: r, config: launchedConfig, launchedAt: launchedAt,
                    crewId: crewId, sessionId: sessionId,
                    workingDirectory: workingDirectory, taskBrief: taskBrief,
                    title: title, additionalEnv: additionalEnv, role: role)
            }
        // 本地 mention 唤醒器钉该 crew 的白板游标（在 CLI 子进程能发首条
        // post_to_crew 之前）—— 该 crew 后续定向 @ 保证被扫到。
            localMentionWaker?.notifyRunStarted(crewId: crewId)
            persistSessionsSnapshot()   // 新成员立刻进机长的点名快照
        }
        // 本地 crew:把 worker session 登记成持久成员（成员列表/@ 名单退出后不丢）。
        // captain 已作为 captain 成员在 roster 里,不重复登记;登录态 crew 的
        // membership 归 edge(H1 迁移),store 里查无此 crew → no-op。
        if role != .captain, config.kind.isAgent {
            LocalCrewStore.shared.recordSessionMember(
                crewId: crewId, sessionId: sessionId, displayName: run.displayName)
        }
    }

    // MARK: - codex backend providers

    /// codex 每轮 turn 前注入未读白板的 provider —— 复用 claude PostToolUse hook 的
    /// 同一份 `HookEmitter`（per-session 游标读未读 + 渲染 + 推进游标），区别只是
    /// 这里在 app 进程内直接调，不经 helper 子进程。返回的字符串作为 `turn/start`
    /// 的首条 text input 前置注入。无未读 → nil（不注入）。best-effort：
    /// 任何失败（store 读不出 / 渲染失败）→ nil，turn 照常跑（只是这轮没白板）。
    nonisolated static func makeWhiteboardProvider(
        crewId: String, sessionId: String, captain: Bool = false
    ) -> () -> String? {
        let dir = LocalWhiteboardStore.defaultDirectory
        return {
            // captain → 注入多带全机 crew 组织树概览（#24 机长视野,与 claude 的
            // hook `--captain` 同语义）。
            let emitter = HookEmitter(
                store: LocalWhiteboardStore(directory: dir),
                crewId: crewId, sessionId: sessionId, cursorDir: dir, isCaptain: captain)
            // HookEmitter 吐的是 claude hook 信封 JSON；codex 这边只要里头的纯文本
            // additionalContext，剥一层拿渲染好的未读串。
            guard let json = emitter.emitAndAdvance(),
                  let data = json.data(using: .utf8),
                  let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  let hook = obj["hookSpecificOutput"] as? [String: Any],
                  let ctx = hook["additionalContext"] as? String, !ctx.isEmpty
            else { return nil }
            return ctx
        }
    }

    /// codex 审批 provider —— 镜像 `McpPermissionHook` 的 raise→long-poll→answer，
    /// 但在 app 进程内异步等（不阻塞线程）。codex 经 server request 发来审批，
    /// 这里 raise 一条 `permission` 待审批（归档在本 sessionId 下，右栏内联卡片
    /// 据此过滤）+ 通知半边贴本地白板，再 poll `LocalApprovalStore` 直到人类在待审批
    /// 列表 allow/deny。映射：allow→"accept"，deny / 超时→"decline"（安全侧 fail-safe）。
    /// 超时取 `McpPermissionHook` 一样保守的兜底（v1 不设硬等人上限，但 in-process
    /// 异步等设一个长上限防 turn 永久挂死）。
    nonisolated static func makeApprovalProvider(
        crewId: String,
        sessionId: String,
        directory: URL = LocalWhiteboardStore.defaultDirectory,
        pollIntervalNanoseconds: UInt64 = 500_000_000,
        maxWaits: Int = 3600
    ) -> (_ summary: String, _ decisions: [String]) async -> String {
        CodexManualApprovalBridge.provider(
            crewId: crewId,
            sessionId: sessionId,
            directory: directory,
            pollIntervalNanoseconds: pollIntervalNanoseconds,
            maxWaits: maxWaits)
    }

    /// codex「要一个我们给不出的回答」→ 发群通知（Todo #6）。
    ///
    /// 与 claude PTY 那条的区别：codex 这类请求我们**当场就答了**（代拒），session
    /// 不会挂在那里 —— 所以不建驻留状态、不挂升级计时，只发一条说清「它要什么、
    /// 我们替它拒了、结果是这件事它干不成」。选靶仍走同一份 `SessionDecisionNotice`
    /// （worker→@机长，机长→@人），口径统一。
    nonisolated static func makeUnanswerableNotifier(
        crewId: String, sessionId: String, isCaptain: Bool
    ) -> (_ summary: String) -> Void {
        let dir = LocalWhiteboardStore.defaultDirectory
        return { summary in
            let post = SessionDecisionNotice.post(
                stage: .first, sessionName: sessionId, sessionId: sessionId,
                isCaptain: isCaptain,
                question: "\(summary)\n（没有填这种表单的界面，已代它回绝，这件事多半干不成）",
                options: [], waitedMinutes: 0)
            LocalWhiteboardStore(directory: dir).appendSessionMessage(
                crewId: crewId, sessionId: sessionId, text: post.text,
                category: "question",
                mentions: post.mentionKinds.map {
                    LocalWhiteboardMention(kind: $0, targetId: nil)
                })
        }
    }

    /// codex 的「一轮收尾停在没 @ 到人的问句上就系统代发」（人类 Todo #25 层 1）。
    ///
    /// 与 claude 的 Stop hook 同一份判定（`SessionTurnTrace`）、同一份记账文件
    /// （`SessionTurnMarker`），只是宿主不同：claude 走 helper 子进程（settings.hooks），
    /// codex 走 app-server 的 `turn/completed` in-process。两边共用一份逻辑，
    /// 「同一轮只发一次 / 静默不发 / 已 @ 到人不发」的口径不分叉。
    nonisolated static func makeTurnEndedNotifier(
        crewId: String, sessionId: String, sessionName: String, isCaptain: Bool
    ) -> (_ lastAgentText: String) -> Void {
        let dir = LocalWhiteboardStore.defaultDirectory
        return { text in
            let board = LocalWhiteboardStore(directory: dir)
            let marker = SessionTurnMarker(directory: dir, crewId: crewId, sessionId: sessionId)
            let prev = marker.read()
            let post = SessionTurnTrace.decide(.init(
                messages: board.list(crewId: crewId),
                sessionId: sessionId,
                sessionName: sessionName,
                isCaptain: isCaptain,
                sinceMessageId: prev.lastMessageId,
                lastHandledTurnId: prev.lastTurnId,
                // codex 侧不按 turn id 去重：每次 turn/completed 恰好一次调用，
                // 天然一轮一次；痕迹判据本身也拦得住重复（代发那条即痕迹）。
                turnId: nil,
                lastAssistantMessage: text))
            if let post {
                board.appendSessionMessage(
                    crewId: crewId, sessionId: sessionId, text: post.text, category: "question",
                    senderName: sessionName,
                    mentions: post.mentionKinds.map {
                        LocalWhiteboardMention(kind: $0, targetId: nil)
                    })
            }
            // `awaitingQuestion` 每轮重写（层 2）——不是停在问句上就写 nil，红点自然熄。
            marker.write(.init(lastMessageId: board.list(crewId: crewId).last?.id ?? prev.lastMessageId,
                               lastTurnId: prev.lastTurnId,
                               awaitingQuestion: SessionTurnTrace.trailingQuestion(from: text)))
        }
    }

    enum RunnerError: LocalizedError {
        case toolNotInstalled(kind: LocalCodingAgentKind)
        case defaultShellUnavailable
        case terminalCannotBeAgent
        case captainNoWorkingDirectory

        var errorDescription: String? {
            switch self {
            case .toolNotInstalled(let kind):
                return "未在 PATH 中找到 \(kind.binaryName)，请先安装 \(kind.displayName) CLI。"
            case .defaultShellUnavailable:
                return "找不到可执行的用户默认 shell（$SHELL 与 /bin/zsh 均不可用）。"
            case .terminalCannotBeAgent:
                return "纯终端不是 agent，不能作为机长或被编排启动。"
            case .captainNoWorkingDirectory:
                return "这个 crew 没有工作目录 —— 在建 crew 时设置后再启动 Captain。"
            }
        }
    }

    /// 启动机长 session —— 手动按钮、建 crew 自动起共用这一份（单一事实源）。
    /// 已有在跑的 captain 直接返回（每 crew 同时只允许一个，防重）。无工作目录抛错。
    ///
    /// captain 与 worker 的差异只有两点：世界观后追加 captain persona
    /// （`crew-captain.zh.md`），MCP helper 带 `--captain` 解锁 answer_decision
    /// （chunk2 §2）。机长是调度者不改代码 —— 直接用 crew 目录，不开 isolation worktree。
    /// `initialPrompt` 只让它用 `post_to_crew` 报到一句 ——「建 crew 即在群里发消息」
    /// 走这条（不另写代码发系统消息，保持白板由 agent 自己写的一致语义）。报到之外
    /// 不预先指派任何动作：世界观/白板已随 system prompt 注入，待决策会另行通知，无需
    /// 在开场 prompt 里堆"先读白板再待命"那套（用户定调：报到就行，别搞乱七八糟的）。
    /// 渲染世界观要 roster → 用传入的 `backend` 拉（best-effort，失败照样启动）。
    /// `wakeText` 非 nil = 这次拉起是因为有人 @ 机长而机长没在跑（@ 唤醒语义：
    /// 不在跑就真拉起来,不只留白板）——开场 prompt 带上那条消息,报到后直接处理。
    func startCaptain(detail: CrewDetail, backend: PendingCrewBackend?,
                      wakeText: String? = nil, openingBrief: String? = nil) async throws {
        let crewId = detail.crew.id
        guard !runs.contains(where: {
            $0.crewId == crewId && $0.role == .captain && $0.status == .running
        }) else { return }
        // await 窗口内的互斥（见 `launchesInFlight`）：两个投递者同时拉同一个休眠
        // 机长时，上面那道守卫拦不住 —— run 要等下面一串 await 走完才进 runs。
        let inFlightKey = "captain:\(crewId)"
        guard !launchesInFlight.contains(inFlightKey) else { return }
        launchesInFlight.insert(inFlightKey)
        defer { launchesInFlight.remove(inFlightKey) }
        // Keep single captain slot: remove any finished captain runs before starting fresh.
        runs.removeAll { $0.crewId == crewId && $0.role == .captain }
        guard let wd = detail.crew.workingDirectory, !wd.isEmpty else {
            throw RunnerError.captainNoWorkingDirectory
        }
        let workdir = URL(fileURLWithPath: (wd as NSString).expandingTildeInPath)
        let localSessionId = "captain-" + String(UUID().uuidString.lowercased().prefix(8))
        let initialPrompt = CaptainBriefDelivery.openingPrompt(
            brief: openingBrief, wakeText: wakeText)
        // persona / members best-effort：加载失败不挡启动（只是少人设/世界观，与 worker 同策略）。
        let persona = try? LocalPromptLoader().rawTemplate(name: "crew-captain", locale: "zh")
        let members = (try? await backend?.listCrewMembers(crewId: crewId))?.members ?? []
        // 机长用建 crew 时选定的 coding agent（crew 没记 → 默认 Codex）。
        let captainKind = LocalCodingAgentKind.captainDefault(detail.crew.captainAgentKind)
        // Todo #68 第 2 件：**机长也要能续跑**。此前它是全机唯一没有续跑路的角色 ——
        // 每次新造 `captain-<uuid8>`、从不查账本，盘上一百多条历史机长对话一条都没被
        // 接回来过。
        //
        // 账本的键是 `crewId + sessionId`，而机长的 sessionId 每次都新造，所以按键查
        // 是查不到上一任的。但**每个 crew 同时只允许一个机长**（上面那道 guard +
        // `runs.removeAll { role == .captain }`），于是「本 crew 最近一条 `captain-*`
        // 记录」无歧义 —— 用它，就**不必**去动 id 的生成方式。
        //
        // 不动 id 还顺手躲开一个硬伤：复用旧 localSessionId = 复用旧的白板读游标
        // （`<crewId>.<sessionId>.cursor`），机长隔几天醒来会被一次性灌进几百条未读
        // （实测父群白板 993 条、每天 60~80 条）。新 id → 游标 `.absent` → 只投最近
        // 一批。世界观那边也不用担心：`--append-system-prompt-file` 是**每次调用现给、
        // 不进会话**的（2026-08-26 实测：resume 时不带就完全丢、带上只有一份），
        // 所以续跑不会叠加两份世界观。
        //
        // `kind` 必须过滤：crew 换过 runner 时，拿 codex 的 threadId 去喂 claude 的
        // `--resume` 是纯粹的错。续不上由 agent 自己说了算（claude 走
        // `retryWithoutResumeIfClaudeRefused`，codex 走 backend 的 resume→start 降级）。
        let previousCaptain = LocalAgentSessionStore.shared.latestCaptainRecord(
            crewId: crewId, kind: captainKind.rawValue)
        var resumeCaptainId: String? = nil
        if case .resume(let id) = AgentSessionResume.decide(
            recordedId: previousCaptain?.agentSessionId) {
            resumeCaptainId = id
        }
        var cfg = SessionConfig(kind: captainKind, initialPrompt: initialPrompt,
                                resumeSessionId: resumeCaptainId)
        // 世界观 + crew 工具按 kind 分流：claude 走文件 flag（appendSystemPromptFile +
        // settings/mcp-config），codex 走 app-server 通道（developerInstructions 字符串 +
        // mcpServers dict）。captain 两边都带（persona 追加 + helper `--captain` 解锁
        // answer_decision）。**默认 captain = codex**（captainDefault），所以 codex 这支必须接对，
        // 否则机长无世界观、无 crew 工具。
        var developerInstructions: String? = nil
        var codexMcpServers: [String: Any]? = nil
        switch captainKind {
        case .claudeCode:
            cfg.appendSystemPromptFile = LocalSessionLaunch.renderWorldModelFile(
                detail: detail, members: members, taskBrief: initialPrompt, workdir: workdir,
                sessionId: localSessionId, runnerKind: captainKind, appendPersona: persona)
            let comms = LocalSessionLaunch.prepareLocalCommsConfig(
                crewId: crewId, sessionId: localSessionId, captain: true, label: "机长")
            cfg.settingsFile = comms.settings
            cfg.mcpConfigFile = comms.mcp
        case .codex:
            developerInstructions = LocalSessionLaunch.renderWorldModelString(
                detail: detail, members: members, taskBrief: initialPrompt, workdir: workdir,
                sessionId: localSessionId, appendPersona: persona)
            codexMcpServers = LocalSessionLaunch.codexMcpServers(
                crewId: crewId, sessionId: localSessionId, captain: true, label: "机长")
        case .terminal:
            throw RunnerError.terminalCannotBeAgent
        }
        try await start(
            crewId: crewId,
            sessionId: localSessionId,
            config: cfg,
            workingDirectory: workdir,
            taskBrief: initialPrompt,
            developerInstructions: developerInstructions,
            codexMcpServers: codexMcpServers,
            role: .captain
        )
    }

    /// 起一个 worker session 执行机长派发的 brief。两条来路共用：驾驶舱控制半边（cockpit.md，
    /// 看差 → 真起 worker 补差）和机长 `start_session` 工具。复用 captain 同一套世界观 + comms
    /// 接线，区别：无 persona 追加、`role: .worker`、helper 不带 `--captain`（captain:false，不解锁
    /// answer_decision）。kind / workdir 可被机长的命令参数覆盖：
    /// - `runnerOverride` 非 nil → 强制用它（机长命令里的 `runner: "claude"/"codex"`），
    ///   否则跟 crew 的 `captainAgentKind` 默认一致（`captainDefault`）。
    /// - `isolation` → 经 `SessionWorkspace.resolve` 决定共享 crew 目录还是开独立 worktree
    ///   （worker 改代码可能要隔离，机长调度不改代码所以 `startCaptain` 恒不隔离）。
    func startForBrief(
        detail: CrewDetail, backend: PendingCrewBackend?, brief: String,
        runnerOverride: LocalCodingAgentKind? = nil, isolation: Bool = false,
        model: String? = nil, effort: String? = nil, title: String? = nil,
        /// 见 `start(userInitiated:)`。默认 false —— 这条路的主要来客是机长
        /// `start_session` 排队，背后起的不该抢前台（#42）。
        userInitiated: Bool = false
    ) async throws {
        guard let wd = detail.crew.workingDirectory, !wd.isEmpty else {
            throw RunnerError.captainNoWorkingDirectory
        }
        let baseDir = URL(fileURLWithPath: (wd as NSString).expandingTildeInPath)
        let workdir = try SessionWorkspace.resolve(
            crewDirectory: baseDir, isolation: isolation, hint: brief)
        let localSessionId = "worker-" + String(UUID().uuidString.lowercased().prefix(8))
        let kind = runnerOverride ?? LocalCodingAgentKind.captainDefault(detail.crew.captainAgentKind)
        guard kind.isAgent else { throw RunnerError.terminalCannotBeAgent }
        try await launchWorker(detail: detail, backend: backend, sessionId: localSessionId,
                               brief: brief, kind: kind, workdir: workdir,
                               model: model, effort: effort, title: title,
                               userInitiated: userInitiated)
    }

    /// @ 唤醒一个**已退出**的持久成员：复用原 sessionId 重启（白板读游标 / 成员
    /// 登记 / 审批归档全延续 —— 新进程第一轮就把停摆期间的未读白板接上，群记忆
    /// 不断片）。**agent 自己的对话上下文也接**（Todo #28）：起 session 时记下的
    /// agent 侧会话号（claude `--session-id` 指定的 uuid / codex 的 threadId）灌进
    /// `resumeSessionId`，claude 走 `--resume`、codex 走 `thread/resume`。接不回来
    /// （没记过）就如实新起一轮，并在首轮 brief 和群里明说是新开的，不装死。
    /// kind 从成员显示名反推（登记时没单存 kind），推不出落回 crew 默认。
    ///
    /// **Todo #68 两处变化：**
    /// 1. **回它当初那个目录跑**。账本现在记着起 session 时的真实 cwd（isolation
    ///    worktree 的就是 worktree 路径）——目录还在就用它，不在了才回落 crew 共享
    ///    目录。旧行为是一律拉回共享目录（上面这段注释自己写着「不恢复」），worker
    ///    醒来会在别人的目录里干活。**「不在了」是常态不是例外**：本机 62 个记过的
    ///    worktree 有 52 个已经被删，所以必须回落，不能硬用。
    /// 2. **不再预判 claude 能不能续**。记了会话号就直接带 `--resume` 去起，claude
    ///    自己拒了再由 `retryWithoutResumeIfClaudeRefused` 降级重起 + 如实进群。
    ///
    /// 已在跑 → no-op（在跑的归注入路径管）。
    func restartMember(detail: CrewDetail, backend: PendingCrewBackend?,
                       member: LocalSessionMember, wakeText: String) async throws {
        guard !runs.contains(where: {
            $0.sessionId == member.sessionId && $0.status == .running
        }) else { return }
        // 同 `startCaptain`：await 窗口内的互斥，防两个投递者把同一个已退成员
        // 拉成两个进程（两者会共用同一个 sessionId，白板游标/成员登记全乱）。
        let inFlightKey = "member:\(member.sessionId)"
        guard !launchesInFlight.contains(inFlightKey) else { return }
        launchesInFlight.insert(inFlightKey)
        defer { launchesInFlight.remove(inFlightKey) }
        // 清掉同 id 的旧已退 run —— 切换条 / 点名快照不留重影。
        runs.removeAll { $0.sessionId == member.sessionId }
        guard let wd = detail.crew.workingDirectory, !wd.isEmpty else {
            throw RunnerError.captainNoWorkingDirectory
        }
        let crewWorkdir = URL(fileURLWithPath: (wd as NSString).expandingTildeInPath)
        let kind = LocalCodingAgentKind.inferred(fromDisplayName: member.displayName)
            ?? LocalCodingAgentKind.captainDefault(detail.crew.captainAgentKind)
        // Todo #28/#68：查账本拿会话号 + 当初跑在哪儿。**记了就带着 `--resume` 去起**，
        // 不再事先猜「日志在不在我们以为的目录里」——那道门今天在本机把 69/339 条
        // （20%）本来续得回来的会话挡在了门外（见 `AgentSessionResume` 的实测）。
        // 真续不上由 claude 自己说了算：`retryWithoutResumeIfClaudeRefused`（claude）
        // / backend 的 resume→start 降级（codex），两边都 fail-loud。
        let recorded = LocalAgentSessionStore.shared.record(
            crewId: detail.crew.id, sessionId: member.sessionId)
        let workdir = AgentSessionResume.restartDirectory(
            recorded: recorded?.workingDirectory, crewDirectory: crewWorkdir)
        let decision = AgentSessionResume.decide(recordedId: recorded?.agentSessionId)
        var brief = "有人在群里 @ 你：「\(wakeText)」。你是本 crew 的既有成员"
            + "「\(member.displayName)」,此前群里的上下文在白板里(每轮自动注入),接着处理这条。"
        if let notice = AgentSessionResume.briefNotice(for: decision) {
            brief = notice + "\n\n" + brief
        }
        if let groupNotice = AgentSessionResume.whiteboardNotice(
            memberName: member.displayName, decision: decision) {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: detail.crew.id, sessionId: "system",
                text: groupNotice, category: "progress", senderName: "系统")
        }
        var resumeId: String? = nil
        if case .resume(let id) = decision { resumeId = id }
        try await launchWorker(detail: detail, backend: backend, sessionId: member.sessionId,
                               brief: brief, kind: kind, workdir: workdir,
                               model: nil, effort: nil, title: member.displayName,
                               resumeAgentSessionId: resumeId)
    }

    /// worker 启动共用体（startForBrief 新起 / restartMember 复用原 id 两条来路）。
    /// `title` = 精简标题（单一真值）：既作 helper `--label`（post_to_crew 白板署名），
    /// 又落到 run.title（群聊/成员列表主名）—— 三处同一值。nil = 从 brief 兜底 derive；
    /// 重启成员传原成员名以延续身份。
    private func launchWorker(
        detail: CrewDetail, backend: PendingCrewBackend?, sessionId localSessionId: String,
        brief: String, kind: LocalCodingAgentKind, workdir: URL,
        model: String?, effort: String?, title: String? = nil,
        /// agent 侧会话号 —— 非 nil 时续跑原对话（Todo #28，claude `--resume` /
        /// codex `thread/resume`）。nil = 新起一轮。
        resumeAgentSessionId: String? = nil,
        /// 见 `start(userInitiated:)`。`restartMember`（@ 唤醒拉起）恒 false。
        userInitiated: Bool = false
    ) async throws {
        let crewId = detail.crew.id
        // 精简标题单一真值：显式 title 优先，否则从 brief 兜底。既当 --label 也当 run.title。
        let resolvedTitle = CrewSessionTitle.resolve(explicit: title, brief: brief)
        // members best-effort：加载失败不挡启动（只是少世界观里的成员名单，与 startCaptain 同策略）。
        let members = (try? await backend?.listCrewMembers(crewId: crewId))?.members ?? []
        var cfg = SessionConfig(kind: kind, model: model, effort: effort, initialPrompt: brief,
                                resumeSessionId: resumeAgentSessionId)
        var developerInstructions: String? = nil
        var codexMcpServers: [String: Any]? = nil
        switch kind {
        case .claudeCode:
            cfg.appendSystemPromptFile = LocalSessionLaunch.renderWorldModelFile(
                detail: detail, members: members, taskBrief: brief, workdir: workdir,
                sessionId: localSessionId, runnerKind: kind, appendPersona: nil)
            let comms = LocalSessionLaunch.prepareLocalCommsConfig(
                crewId: crewId, sessionId: localSessionId, captain: false, label: resolvedTitle)
            cfg.settingsFile = comms.settings
            cfg.mcpConfigFile = comms.mcp
        case .codex:
            developerInstructions = LocalSessionLaunch.renderWorldModelString(
                detail: detail, members: members, taskBrief: brief, workdir: workdir,
                sessionId: localSessionId, appendPersona: nil)
            codexMcpServers = LocalSessionLaunch.codexMcpServers(
                crewId: crewId, sessionId: localSessionId, captain: false, label: resolvedTitle)
        case .terminal:
            throw RunnerError.terminalCannotBeAgent
        }
        try await start(
            crewId: crewId,
            sessionId: localSessionId,
            config: cfg,
            workingDirectory: workdir,
            taskBrief: brief,
            title: resolvedTitle,
            developerInstructions: developerInstructions,
            codexMcpServers: codexMcpServers,
            role: .worker,
            userInitiated: userInitiated
        )
    }

    /// Todo #68：带着 `--resume` 起的 claude，**claude 自己**拒了这个会话号时，
    /// 不带 `--resume` 重起一次，并把它的原话如实带进白板与首轮 brief。
    ///
    /// 这是「不预判、真去试」的后半段。前半段是 `AgentSessionResume.decide` —— 它现在
    /// 只看账本记没记，不再去猜「日志在不在我们以为的目录里」。那道旧门在本机把
    /// 69/339 条（20%）claude 本来续得回来的会话挡在了门外。形状与 codex 那侧
    /// `thread/resume` 失败 → 降级 `thread/start` + `notifyResumeFallback` 完全相同。
    ///
    /// **判据的顺序不能反：**
    /// 1. 这次起本来就带了 `--resume`，且 kind 是 claude；
    /// 2. 人主动停的不算（`exitReason == .userStopped`）；
    /// 3. **屏上有 claude 的那句原话**（`No conversation found with session ID: <id>`）
    ///    —— 这是**唯一**的决策依据。话不在就绝不降级：CLI 没装 / 参数写错 / 额度用尽
    ///    同样是秒退，把它们吞成「会话没了」会把真故障藏起来，而且长得跟 Todo #68 的
    ///    病一模一样（悄悄换一个新脑子，谁也不知道）；
    /// 4. 5 秒窗口只是**廉价护栏**：resume 失败是瞬时的（起来就死、exit 1，实测）。
    ///    窗口开宽了，一个跑到第 55 秒才因别的原因死掉的 session 会被判成 resume 失败
    ///    → 悄悄重起 → 那才是真把记忆弄丢。
    /// 5. 只重试一次 —— 重起那次 `resumeSessionId` 已经是 nil，进不来这条路。
    ///
    /// 读画面走 P2 的可选 `screen-text` 能力：协议模式发 `control.screenText` 到
    /// daemon 读权威画面；一行回退到 P1 时仍由同一个 capability 读本地 core。
    private func retryWithoutResumeIfClaudeRefused(
        run: CrewSessionRun, config: SessionConfig, launchedAt: Date,
        crewId: String, sessionId: String, workingDirectory: URL, taskBrief: String,
        title: String?, additionalEnv: [String: String], role: CrewSessionRun.Role
    ) {
        guard config.kind == .claudeCode,
              let resumedId = config.resumeSessionId, !resumedId.isEmpty,
              run.exitReason != .userStopped,
              Date().timeIntervalSince(launchedAt)
                  <= AgentSessionResume.claudeResumeRefusalWindow,
              let screenText = SessionAuthoritativeScreenText.read(
                  from: run.backend, maxLines: 40),
              let said = AgentSessionResume.claudeResumeRejection(
                inScreenText: screenText, resumedId: resumedId)
        else { return }

        let decision = AgentSessionResume.Decision.fresh(reason: .agentRejectedResume(
            id: resumedId, agentSaid: said,
            diagnosis: AgentSessionResume.resumeRejectionDiagnosis(
                sessionId: resumedId,
                lookup: AgentSessionResume.diskLookup(
                    home: FileManager.default.homeDirectoryForCurrentUser))))
        if let notice = AgentSessionResume.whiteboardNotice(
            memberName: run.displayName, decision: decision) {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system",
                text: notice, category: "progress", senderName: "系统")
        }
        var retry = config
        retry.resumeSessionId = nil
        retry.newSessionId = nil          // start() 会现造一个并记账（覆盖旧值，这是对的）
        var brief = taskBrief
        if let notice = AgentSessionResume.briefNotice(for: decision) {
            brief = notice + "\n\n" + taskBrief
        }
        retry.initialPrompt = brief
        // 刚退出的那条 run 留在列表里只会变重影 —— 同 id 的旧 run 清掉再起。
        runs.removeAll { $0.runID == run.runID }
        // `developerInstructions` / `codexMcpServers` 不带 —— 这条路只服务 claude
        // （上面 guard 了 kind），那两样是 codex 的通道。
        //
        // （原来这里还写着「不带 `serverLink` 重起」的理由。`serverLink` 是跨端遥控
        // 里「本机 run ↔ 服务端 session 行」的绑定，随 #63 第二期整层删除了。）
        Task { [weak self] in
            try? await self?.start(
                crewId: crewId, sessionId: sessionId, config: retry,
                workingDirectory: workingDirectory, taskBrief: brief, title: title,
                additionalEnv: additionalEnv, role: role)
        }
    }


}

/// 单次 session 的 UI 视图模型。
///
/// agent 跑在后端里（claude = 内嵌真终端；codex = app-server），UI 通过
/// `agentTerminalSession` 有条件地嵌入终端视图。这里只追踪后端进程的
/// running/exited 生命周期，并在结束时关掉 server row。
@MainActor
final class CrewSessionRun: ObservableObject, Identifiable {
    enum Status: Equatable {
        case running
        case completed
        case cancelled
        case failed
    }

    /// session 在 crew 里的角色 —— captain（调度者）/ worker（干活的）。
    /// 切换条展示用，captain 置顶。
    enum Role: Equatable {
        case captain
        case worker
    }

    let runID = UUID()
    let crewId: String
    /// 本 session 的合成 id（BYOK = startSession 生成的 localSessionId；logged =
    /// 服务端 session id）。与 MCP `--session` / 世界观 / `LocalApprovalStore` 条目
    /// 的 sessionId 三处一致 —— 右栏内联待办卡片靠它过滤出本 session 的待决策/待审批。
    let sessionId: String
    let kind: LocalCodingAgentKind
    let taskBrief: String
    /// 精简标题（≤18 字、无项目名）——**单一真值**：群聊气泡名 = 成员列表主名 =
    /// helper `--label` 白板署名，都取它。机长起 session 可经 start_session 传；
    /// 没传就从 `taskBrief` 兜底 derive（`CrewSessionTitle`）。空 = 用 `displayName`
    /// 的 kind+id6 兜底（旧行为）。
    let title: String
    let workingDirectory: URL
    /// 当前生效的 model slug（起 session 时选的，或运行态 `/model` 切换后回写的）。
    /// 运行态切换要能被人面成员卡观察到 → `@Published var`（切换后 `applyProfileChange`
    /// 回写这里，MCP 自切与人面切换共用同一回写路径）。nil = 对应 runner 默认。
    @Published var model: String?
    /// 当前生效的 thinking effort 档位（同 `model`，运行态切换后回写）。nil = runner 默认。
    @Published var effort: String?
    /// Codex native reviewer currently acknowledged by app-server. nil for Claude.
    @Published var approvalsReviewer: CodexProtocol.ApprovalsReviewer?
    /// 在途切换的目标（如「opus」）——**切换真生效前 `model`/`effort` 不动**，
    /// 人面药丸靠这个显示「→opus…」，别让 UI 抢先谎报（#544）。nil = 无在途切换。
    @Published var pendingProfile: String?
    /// Spec v2 §10 — session-level override for permission_mode（informational）。
    let permissionModeOverride: String?
    /// captain / worker 角色（切换条展示 + captain 置顶）。
    let role: Role
    let startedAt = Date()

    /// 切换条 / 群聊 / 成员列表统一的显示名：captain 直接叫「机长」；worker 优先用
    /// 精简 `title`（单一真值），没 title 才退回 agent 名 + session id 前缀。
    var displayName: String {
        if role == .captain { return "机长" }
        return title.isEmpty ? kind.displayName + " · " + String(sessionId.prefix(6)) : title
    }

    /// 后端实现（claude = AgentTerminalSession；codex = app-server；终端 = PlainTerminalSession）。
    let backend: any SessionBackend

    /// 类型转换访问：仅当后端是终端（claude）时非 nil，供 `AgentTerminalView` 使用。
    var agentTerminalSession: AgentTerminalSession? { backend as? AgentTerminalSession }
    var remoteSessionBackend: RemoteSessionBackend? { backend as? RemoteSessionBackend }
    /// claude 与纯终端都提供 PTY 视图；codex 没有。
    var terminalView: TerminalMirrorView? {
        if let agent = backend as? AgentTerminalSession { return agent.terminalView }
        if let plain = backend as? PlainTerminalSession { return plain.terminalView }
        if let remote = backend as? RemoteSessionBackend { return remote.terminalView }
        return nil
    }

    @Published private(set) var status: Status = .running
    @Published private(set) var exitCode: Int32?
    /// 终止原因（Todo #10 ①）：finalize 时由 `SessionExitReason.classify` 算出。
    /// `.hitLimit` = 因额度上限被打断（区别于正常结束/手动停）→ 触发自动续跑挂钩。
    @Published private(set) var exitReason: SessionExitReason?
    /// 干活中(跑回合) vs 空闲(存活等指令) —— 镜像后端 `isWorking`，驱动头像/切换条状态点。
    /// (与 backend.isBusy 区分:那个是唤醒注入门禁;这个是 UI 活跃信号,见 SessionBackend。)
    @Published private(set) var isWorking = false
    /// 群聊「正在输入」气泡用的显示态 —— 镜像后端 `displayIsTyping`（Todo #24）。
    /// 与 `isWorking` 分家的理由见 `SessionBackend.displayIsTyping`：那条是原始
    /// 活跃信号（回执/上报/状态点/限额恢复都吃），这条只驱动一个气泡，做了
    /// 心跳重绘过滤 + 不对称迟滞。
    @Published private(set) var displayIsTyping = false
    /// runner 健康异常(未登录/额度到顶/限额中) —— 镜像后端 `health`。非 nil 时成员
    /// 状态点转红、成员行副行显示 detail,并往本 crew 白板 fail-loud 一条(每 Kind 一次)。
    @Published private(set) var health: CrewSessionHealth?
    /// 最近一次 health 首报时刻 —— 终止原因分类的新鲜度依据（sticky 首报不能
    /// 无限期背书 hit-limit 判定，见 `SessionExitReason.classify`）。
    private(set) var healthAt: Date?
    /// 正卡在「等人拍板」上（Todo #6）—— 镜像后端 `pendingDecision`。非 nil 时
    /// 点名状态落 `awaitingDecision`（不再谎报「空闲」），并已往群里 @ 过能处理的人。
    @Published private(set) var pendingDecision: PendingTerminalDecision?
    /// 正在等人回话（人类 Todo #25 层 2）—— 非 nil 时状态点转红并呼吸。
    ///
    /// **每拍重算，不是被点亮后等人来熄**（`refreshAwaitingReply`，跟着 2 秒的点名快照
    /// 定时器走）。判定归 `SessionAwaitingReply` 那个纯函数，这里只负责把三样输入喂给它：
    /// 待办 store 里本 session 的 pending 条目、终端菜单、上一轮落在 marker 里的收尾问句。
    /// 重算而非驻留是**故意的** —— 「限额中」当初进得去出不来（#545）就是驻留态只留了
    /// 一条清除路径；重算的话拿到回复 / 被 nudge / 被 stop / 进程退出，下一拍自己就没了。
    @Published private(set) var awaitingReply: SessionAwaitingReply.Reason?
    /// 撞额度类异常（usageLimit 撞墙 / rateLimited 卡菜单）或因之终止时回调 ——
    /// runner 挂上 `autoScheduleQuotaWakeup`,实现「hit limit → 自动约额度重置唤醒」。
    /// 多次触发安全（挂唤醒侧按 session 的待触发 [auto] 钩子去重）。
    var onUsageLimit: ((CrewSessionRun) -> Void)?
    /// 拉起失败时回调（#541）—— runner 挂上后把原因落进 `lastStartError`
    /// （UI 横幅，与「人类发言自动拉起机长失败」同一条留痕通道）。
    var onLaunchFailure: ((CrewSessionRun, CrewSessionHealth) -> Void)?
    /// 后端从 working/busy 翻到 idle 的边沿。runner 用它冲刷本 session 的待投消息。
    var onBecameIdle: ((CrewSessionRun) -> Void)?
    /// 进程自然退出时让 runner 清掉尚未投出的正文与回调，避免同 id 重启后吃旧队列。
    var onEnded: ((CrewSessionRun) -> Void)?

    /// 用户主动停 → 终端退出时落 `.cancelled` 而非 `.failed`。
    private var cancelled = false
    /// Guards double-finalize.
    private var finalized = false
    /// 已经往白板 fail-loud 过的健康异常 Kind（每 Kind 只喊一次，TUI 重绘不刷屏）。
    private var announcedHealthKinds: Set<CrewSessionHealth.Kind> = []
    private var statusObservation: Task<Void, Never>?
    private var workingObservation: Task<Void, Never>?
    private var typingObservation: Task<Void, Never>?
    private var healthObservation: Task<Void, Never>?
    private var decisionObservation: Task<Void, Never>?
    /// 启动参数没生效的观察（Todo #36）—— 与 health 分开，见 `observeLaunchParameterProblems`。
    private var launchParameterObservation: Task<Void, Never>?
    /// 待决策的「等太久就升级找人」计时（一个菜单一条，清掉即取消）。
    private var decisionEscalation: Task<Void, Never>?

    nonisolated var id: UUID { runID }

    init(
        crewId: String,
        sessionId: String,
        kind: LocalCodingAgentKind,
        taskBrief: String,
        title: String = "",
        workingDirectory: URL,
        model: String? = nil,
        effort: String? = nil,
        approvalsReviewer: CodexProtocol.ApprovalsReviewer? = nil,
        permissionModeOverride: String? = nil,
        backend: any SessionBackend,
        role: Role = .worker
    ) {
        self.crewId = crewId
        self.sessionId = sessionId
        self.kind = kind
        self.taskBrief = taskBrief
        // clamp 兜底：无论从哪条来路传进来，落到 run 上的 title 都 ≤18 字、单行。
        self.title = CrewSessionTitle.clamp(title)
        self.workingDirectory = workingDirectory
        self.model = model
        self.effort = effort
        self.approvalsReviewer = kind == .codex ? (approvalsReviewer ?? .autoReview) : nil
        self.permissionModeOverride = permissionModeOverride
        self.backend = backend
        self.role = role
        self.status = Self.map(backend.status, cancelled: false)
        self.isWorking = backend.isWorking
        self.displayIsTyping = backend.displayIsTyping
        observeBackendStatus()
        // 纯终端只接进程生命周期与 PTY 视图；健康、typing、额度、待决策等观察链
        // 都是 agent 编排的一部分，不能为了复用 run 容器而给人的 shell 假装接上。
        if kind.isAgent {
            observeBackendWorking()
            observeBackendHealth()
            observeLaunchParameterProblems()
            observePendingDecision()
        }
    }

    /// 「session 卡在等人拍板」→ 发群 @ 能处理的人 + 挂升级计时（Todo #6）。
    ///
    /// 为什么不复用 health 那条通道：health 是 sticky 首报、每 Kind 一次；待决策是
    /// **来去自如**的瞬时态（人一答就该清、答完再弹一个还要能再报）。混在一起会让
    /// 第二个菜单永远报不出来。见 `SessionBackend.pendingDecisionUpdates`。
    private func observePendingDecision() {
        decisionObservation = Task { [weak self] in
            guard let self else { return }
            for await d in self.backend.pendingDecisionUpdates.values {
                if Task.isCancelled { return }
                self.pendingDecision = d
                guard let d else {
                    // 答完了 —— 状态自然回正，升级计时一起收掉（别再去 @人）。
                    self.decisionEscalation?.cancel()
                    self.decisionEscalation = nil
                    continue
                }
                self.announceDecision(d, stage: .first)
                self.armDecisionEscalation(for: d)
            }
        }
    }

    /// 发一条待决策通知。说什么/@谁全归 `SessionDecisionNotice`（纯逻辑，单测钉住）。
    private func announceDecision(
        _ d: PendingTerminalDecision, stage: SessionDecisionNotice.Stage, waitedMinutes: Int = 0
    ) {
        let post = SessionDecisionNotice.post(
            stage: stage, sessionName: displayName, sessionId: sessionId,
            isCaptain: role == .captain, question: d.prompt, options: d.options,
            waitedMinutes: waitedMinutes)
        LocalWhiteboardStore.shared.appendSessionMessage(
            crewId: crewId, sessionId: sessionId, text: post.text,
            category: "question", senderName: displayName,
            mentions: post.mentionKinds.map { LocalWhiteboardMention(kind: $0, targetId: nil) })
    }

    /// 首报若干分钟后仍是同一个菜单 → 升级 @人（机长拍不了 / 没在跑时的兜底）。
    /// 只升级一次：再没人管就该人自己去看了，继续刷群只会让通知更不值钱。
    private func armDecisionEscalation(for d: PendingTerminalDecision) {
        decisionEscalation?.cancel()
        let wait = SessionDecisionNotice.escalateAfter
        decisionEscalation = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000_000))
            guard let self, !Task.isCancelled,
                  self.pendingDecision?.fingerprint == d.fingerprint else { return }
            self.announceDecision(d, stage: .escalate, waitedMinutes: Int(wait / 60))
        }
    }

    /// 镜像后端健康异常到 run 上,并往白板 fail-loud（每 Kind 一次）——用户不用
    /// 进终端就知道「claude 没登录 / 额度到顶 / codex token 失效」（分诊第 7 点）。
    private func observeBackendHealth() {
        healthObservation = Task { [weak self] in
            guard let self else { return }
            for await h in self.backend.healthPublisher.values {
                if Task.isCancelled { return }
                self.health = h
                if h != nil { self.healthAt = Date() }
                // 后端宣布恢复（health 归 nil）→ 把额度类首报重新武装：下次真撞墙
                // 还要能再喊一次，否则「每 Kind 只喊一次」会让恢复后的再撞墙静音。
                if h == nil { self.announcedHealthKinds.subtract([.usageLimit, .rateLimited]) }
                guard let h else { continue }
                self.announce(h)
            }
        }
    }

    /// 启动参数没被 CLI 接受 → 白板 fail-loud（Todo #36）。
    ///
    /// 与 `announce(_:)` 那条**刻意分开**：那条管「这个 session 活没活」，这条管
    /// 「它活是活了，但没按你要的配置活」。后者进程健康、也在吐字，`SessionLaunchProbe`
    /// 判 `.alive`，整条健康链路一个字都不会说 —— 只有这里会。
    ///
    /// 不翻 health、不翻 status：session 确实能用（claude 用默认档继续跑），把它标成
    /// 异常会让机长以为得改派，反而是新的谎报。定向 @ 机长：起这个 session 的是它，
    /// 只有它能决定「将就用」还是「用对的参数重起」。
    private func observeLaunchParameterProblems() {
        let problems: AnyPublisher<SessionLaunchParameterProblem, Never>
        if let terminal = backend as? AgentTerminalSession {
            problems = terminal.launchParameterProblems
        } else if let remote = backend as? RemoteSessionBackend, kind == .claudeCode {
            problems = remote.launchParameterProblems
        } else {
            return
        }
        launchParameterObservation = Task { [weak self] in
            for await problem in problems.values {
                guard let self, !Task.isCancelled else { return }
                LocalWhiteboardStore.shared.appendSessionMessage(
                    crewId: self.crewId, sessionId: "system",
                    text: "⚠️ \(self.displayName)（\(self.sessionId)）起来了，但**启动参数没生效**：\n"
                        + problem.detail
                        + "\n它照常在跑（没拦、也没替你改），要按原意跑请用对的值重起，"
                        + "或用 set_session_profile 切过来。",
                    category: "error", senderName: "系统",
                    mentions: self.role == .captain
                        ? nil : [LocalWhiteboardMention(kind: "captain", targetId: nil)])
            }
        }
    }

    /// 健康异常 → 白板 fail-loud（每 Kind 一次，TUI 重绘不刷屏）+ 相应挂钩。
    /// 观察循环与 `finalize` 兜底都调这里，`announcedHealthKinds` 保证只喊一次。
    private func announce(_ h: CrewSessionHealth) {
        guard !announcedHealthKinds.contains(h.kind) else { return }
        announcedHealthKinds.insert(h.kind)
        if h.kind == .launchFailed {
            // 拉起失败 = 派出去的活没人干（#541）。**定向 @ 机长**：机长看到才能
            // 立刻改派，广播一条谁都不认领等于白喊。走与「唤醒没回执」告警同一套
            // 机制（system 身份 + captain mention + question 类别），不另起炉灶。
            // 例外：**挂掉的就是机长自己**时不 @ —— @机长会触发「目标缺席拉起」，
            // 起不来又发一条，就此成环；那条广播给人看。
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system",
                text: "\(displayName)（\(sessionId)）拉起失败：\(h.detail)"
                    + "\n派给它的活无人接手，请改派或重起。任务：\(taskBrief.prefix(80))",
                category: "question", senderName: "系统",
                mentions: role == .captain
                    ? nil : [LocalWhiteboardMention(kind: "captain", targetId: nil)])
            onLaunchFailure?(self, h)
            return
        }
        LocalWhiteboardStore.shared.appendSessionMessage(
            crewId: crewId, sessionId: sessionId,
            text: "\(displayName) 出问题了：\(h.detail)",
            category: "error", senderName: displayName)
        if h.isQuotaRelated { onUsageLimit?(self) }
    }

    /// 镜像后端 `isWorking`(claude=输出活跃度;codex=turn 进行中)到 run 上,供 UI 观察。
    private func observeBackendWorking() {
        workingObservation = Task { [weak self] in
            guard let self else { return }
            for await working in self.backend.isWorkingPublisher.values {
                if Task.isCancelled { return }
                let wasWorking = self.isWorking
                self.isWorking = working
                if wasWorking && !working {
                    self.onBecameIdle?(self)
                }
            }
        }
        typingObservation = Task { [weak self] in
            guard let self else { return }
            for await typing in self.backend.displayIsTypingUpdates.values {
                if Task.isCancelled { return }
                self.displayIsTyping = typing
            }
        }
    }

    /// 跟踪后端的 running/exited，映射到 run 的生命周期 status，并在退出时
    /// finalize（关 server row）。用 `backend.statusPublisher` 的 Combine stream 驱动。
    private func observeBackendStatus() {
        statusObservation = Task { [weak self] in
            guard let self else { return }
            for await backendStatus in self.backend.statusPublisher.values {
                if Task.isCancelled { return }
                switch backendStatus {
                case .running:
                    self.status = .running
                case let .exited(code):
                    self.exitCode = code
                    self.finalize(exitCode: code)
                }
            }
        }
    }

    /// 发文本给 agent（首条指令 / 续聊 / steer）。终端原样接收。
    /// 顺手熄掉「在等回复」——有人回话了就不该继续红着（下一轮结束时 marker 会重写）。
    func send(_ text: String) {
        clearAwaitingQuestionMarker()
        backend.send(text)
    }

    /// 本 session 的回合记账文件（层 1 写、层 2 读）。
    private var turnMarker: SessionTurnMarker {
        SessionTurnMarker(directory: LocalWhiteboardStore.defaultDirectory,
                          crewId: crewId, sessionId: sessionId)
    }

    /// 有输入进去了 → 熄掉 marker 里的收尾问句。`nudge_session`（机长代答）与
    /// `send`（注入/续聊）两条路都调 —— 「被 nudge 也要能退出待回复」（Todo #25 层 2）。
    /// 人直接在终端里打字这条路管不着，靠「还在吐字就不算在等」那道门兜住。
    func clearAwaitingQuestionMarker() {
        turnMarker.clearAwaitingQuestion()
        if case .question = awaitingReply { awaitingReply = nil }
    }

    /// 重算「在等谁回话」（Todo #25 层 2）。跟着点名快照的 2 秒定时器走。
    ///
    /// 两样磁盘输入都由 runner 在**后台**取好分发下来（`SessionAwaitingReplyInputsCache`，
    /// 2026-08-18）—— 这个方法里一次磁盘 IO 都没有，纯内存判定 + 赋值。
    /// - Parameters:
    ///   - pendingApprovalSummary: 审批账本里本 session 的 pending 条目摘要（每 crew 取一次）。
    ///   - trailingQuestion: 上一轮收尾那句问句（`SessionTurnMarker`，每 run 取一次）。
    func refreshAwaitingReply(pendingApprovalSummary: String?, trailingQuestion: String?) {
        let next = SessionAwaitingReply.reason(.init(
            isRunning: status == .running,
            pendingApprovalSummary: pendingApprovalSummary,
            pendingMenuPrompt: pendingDecision?.prompt,
            trailingQuestion: trailingQuestion,
            isProducingOutput: displayIsTyping))
        if next != awaitingReply { awaitingReply = next }
    }

    /// 切一个配置档位并等真实结果（claude=等空闲注入斜杠命令+核对回显；codex=不支持）。
    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        await backend.applyProfileSwitch(cmd)
    }

    /// 打断进行中的回合（Esc）。
    func interrupt() { backend.interrupt() }

    /// 主动停掉 session（终止子进程）。后端退出后落 `.cancelled`。
    func stop() {
        guard status == .running else { return }
        cancelled = true
        backend.stop()
    }

    deinit {
        statusObservation?.cancel()
        workingObservation?.cancel()
        typingObservation?.cancel()
        healthObservation?.cancel()
        decisionObservation?.cancel()
        launchParameterObservation?.cancel()
        decisionEscalation?.cancel()
    }

    /// 额度重置唤醒到点后清限额态（runner 的 `fire` 调）：health 红点熄灭、
    /// 扫描器与白板首报重新武装 —— 下个限额窗再撞墙能再次报警 + 再挂唤醒。
    func rearmQuotaHealth() {
        announcedHealthKinds.subtract([.usageLimit, .rateLimited])
        backend.clearQuotaHealth()
    }

    /// 终端进程退出 —— 落 terminal status + 关 server row。Idempotent。
    private func finalize(exitCode: Int32?) {
        guard !finalized else { return }
        finalized = true
        // 兜底补报（#541）：下面要收掉 health 观察，而后端翻 health 与翻 status
        // 是两次独立发布 —— 拉起失败那条若还没被观察循环取到就会连同观察一起被
        // 收走，白板永远等不到告警。这里直读后端当前 health 补一次（announce 去重）。
        if let h = backend.health {
            health = h
            if healthAt == nil { healthAt = Date() }
            announce(h)
        }
        // 终止原因分类（Todo #10 ①）：因 hit limit 被打断 → 触发自动续跑挂钩
        // （挂唤醒侧按待触发 [auto] 钩子去重,health 首报路已挂过就 no-op）。
        let reason = SessionExitReason.classify(
            cancelled: cancelled, exitCode: exitCode,
            lastHealthKind: health?.kind, healthAt: healthAt)
        exitReason = reason
        if reason == .hitLimit { onUsageLimit?(self) }
        status = Self.map(.exited(exitCode), cancelled: cancelled)
        isWorking = false
        displayIsTyping = false
        onEnded?(self)
        statusObservation?.cancel()
        workingObservation?.cancel()
        typingObservation?.cancel()
        // healthObservation 的 for-await 持着 self 强引用 —— 不 cancel 的话退出的
        // run（连带终端视图）整体泄漏,deinit 永远到不了。四条观察一起收。
        healthObservation?.cancel()
        decisionObservation?.cancel()
        launchParameterObservation?.cancel()
        // 退出的 session 不可能还「在等人选」——状态清干净 + 别再升级 @人（#545）。
        decisionEscalation?.cancel()
        decisionEscalation = nil
        pendingDecision = nil
    }

    /// Map the terminal's running/exited state to the run lifecycle.
    private static func map(_ status: SessionStatus, cancelled: Bool) -> Status {
        switch status {
        case .running:
            return .running
        case let .exited(code):
            if cancelled { return .cancelled }
            return (code ?? 0) == 0 ? .completed : .failed
        }
    }
}
#endif
