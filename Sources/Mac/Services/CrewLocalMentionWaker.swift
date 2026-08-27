#if os(macOS)
import Foundation
import Combine

/// 本地 mention 唤醒器（wake-resilience 根因修复的活体半边）。
///
/// 2026-07-19 事故：session/机长经 `post_to_crew` 发的定向 @ 只落白板 JSON，
/// app 侧没有任何观察者把它转成对 idle run 的注入 —— 机长三轮派工全部躺在白板，
/// 4 个 idle worker 一个没醒。本服务补上这条路：
///
///   白板变更（进程内 `changes` + 跨进程 `directoryChanged`）
///     → 按 per-crew 扫描游标取**新增**条目
///     → `CrewLocalMentionWakeLogic.pending` 滤出 session/机长定向 @，并把人类普通
///       消息默认路由给当前机长
///     → 复用同一套决策核心 `CrewLocalMentionInjectLogic.decide`
///       （busy 不打断 / 空闲注入 / 近期上下文按目标游标现取）
///     → 命中注入 + 回执确认（`CrewSessionRunner.confirmWake`）后才推进目标游标；
///       失败不消费 + 白板告警 @captain。人类消息免延迟回执采样，避免短 turn 已完成
///       后被误报；投递本身仍走同一队列。
///     → 目标完全没在跑 → 拉起（captain=startCaptain，持久成员=restartMember），
///       与 `CrewChatView.wakeAbsentMentionTargets` 同语义。
///
/// 扫描游标内存态、启动时钉到各 run 所在 crew 白板当前尾 —— 不回放历史（app
/// 启动前积压的 @ 由 hook 路 / 机长重发兜；run 不跨 app 重启，历史 @ 的目标
/// 多半也不在了）。所有编排在 MainActor，决策全在纯函数（有单测）。
@MainActor
final class CrewLocalMentionWaker {
    private weak var runner: CrewSessionRunner?
    /// 拉起缺席目标需要 crew detail —— 从 app 态现取 backend（弱化对 AppModel 的依赖）。
    private let backendProvider: () -> PendingCrewBackend?
    private var watchers: [AnyCancellable] = []
    /// 该监听的 crew（有本地 run 的 + 本机全部）。钉不上的留在这儿等下次事件重试。
    private var watched: Set<String> = []
    /// 已钉住游标的 crew —— 只有钉成功的才投递。
    private var pinned: Set<String> = []
    /// per-crew 扫描游标 = 最后已扫白板条目的 (id, 时间戳)。钉时白板为空 → 无游标，
    /// `entries(after: nil)` 返回全部 = 钉之后的一切，语义正确。
    ///
    /// #595：位置从裸 id 换成复合位置。白板被归档重建换了一批 id 时，裸 id 游标当场
    /// 悬空，而「悬空」在旧实现里等于「全是新的」—— 全机 session 被几周前的 @ 拉起来
    /// 照过期指令返工。带上时间戳后，悬空也只切出真正更新的那批。
    private var cursors: [String: WhiteboardCursorPosition] = [:]
    /// per-crew 白板文件指纹门（Todo #59）。
    ///
    /// 目录事件不带文件名，所以 `directoryChanged` 一来就要把 `watched` 里**每个**
    /// crew 扫一遍 —— 而「扫」= 取文件锁 + 整份读 + 整份 JSON 解码，全在主线程。
    /// 原来的注释写「读增量靠游标，很廉价」，那是错的：游标只裁剪解完之后的行，
    /// 读和解一分钱没省。本机白板目录 67 个 json / 3.8 MB，全量走一遍实测 9~11 ms，
    /// 而 helper 子进程每发一条 post_to_crew 就是一个 tick。
    ///
    /// 门就是 `FileChangeGate` 本来的用途（#443 建它时的原话）：一次 `stat` 约 1 µs，
    /// 67 个文件 0.07~0.10 ms —— 比它省下的那一遍便宜两个数量级。
    private var fileGates: [String: FileChangeGate] = [:]

    init(runner: CrewSessionRunner, backendProvider: @escaping () -> PendingCrewBackend?) {
        self.runner = runner
        self.backendProvider = backendProvider
    }

    /// 订阅白板变更 + 钉现有 run 所在 crew 的游标。幂等。
    ///
    /// ⚠️ 只有编排者进程有资格起它（spec §6.2 闸门 1）。viewer 里误起 = 当场崩，
    /// 不是悄悄跑成双头 —— 双头会让同一批账被两个进程交替覆盖、唤醒发两遍，
    /// 而那种症状事后基本查不出来。
    func start() {
        precondition(
            ProcessRole.current == .orchestrator,
            "\(type(of: self)).start 只能在编排者进程里调用，当前角色=\(ProcessRole.current.rawValue)")
        guard watchers.isEmpty else { return }
        for crewId in Set(runner?.runs.map(\.crewId) ?? []) { pin(crewId) }
        // 通讯录 `contact`（2026-08-11）：来电可以打给一个此刻一个 run 都没有的 crew
        // （连机长都没起）。那种 crew 从来不会被 `notifyRunStarted` 钉上 → 扫不到 →
        // 来电只躺在白板上没人醒。所以这里把**本机所有 crew** 都钉在「当前白板末尾」：
        // 游标就在尾巴上，历史一条不回放；此后写进来的每条都扫得到。app 起来之后
        // 新建的 crew 由 `notifyRunStarted` 兜住（建 crew 必起机长）。
        for crewId in LocalCrewStore.shared.allCrewTitles().map(\.id) { pin(crewId) }
        let store = LocalWhiteboardStore.shared
        store.startWatching()
        // 进程内 append（codex in-process post / relay 搬运 / 系统消息）带 crewId 直扫。
        store.changes
            .sink { [weak self] crewId in
                Task { @MainActor in self?.scan(crewId: crewId) }
            }
            .store(in: &watchers)
        // helper 子进程 post_to_crew 写盘 → 目录事件无 crewId → 扫所有已钉 crew。
        // 每个 crew 先过一道文件指纹门（`fileGates`）——「靠游标就很廉价」是错的，
        // 游标只裁剪解完之后的行，读和解一分钱不省（Todo #59）。
        store.directoryChanged
            .sink { [weak self] in
                Task { @MainActor in
                    guard let self else { return }
                    for crewId in self.watched { self.scan(crewId: crewId, gated: true) }
                }
            }
            .store(in: &watchers)
    }

    /// run 启动时钉它所在 crew 的游标（幂等）。runner.start() 在 CLI 子进程能发出
    /// 第一条 post_to_crew 之前调它 —— 该 crew 后续所有 @ 都保证被扫到。
    func notifyRunStarted(crewId: String) { pin(crewId) }

    /// 钉一个 crew 的扫描游标（幂等）。判定在 `CrewLocalMentionWakeLogic.pinPosition`
    /// （纯函数、有单测）：白板末条是**读失败的合成警示行**时这次不钉 —— 那条只在内存里，
    /// 磁盘上没有，钉下去当场悬空、下一次扫描全量重放（#595）。留在 `watched` 里，
    /// 下一次白板事件再试。
    private func pin(_ crewId: String) {
        watched.insert(crewId)
        guard !pinned.contains(crewId) else { return }
        switch CrewLocalMentionWakeLogic.pinPosition(
            rows: LocalWhiteboardStore.shared.list(crewId: crewId)) {
        case .retryLater:
            return
        case .pin(let position):
            pinned.insert(crewId)
            cursors[crewId] = position
        }
    }

    /// 扫一个 crew 的新增条目 → 逐条投递。游标先行推进：投递触发的白板写
    /// （告警/回执）再进扫描时是新批次，不会重扫本批。
    ///
    /// `gated: true` 用于目录 tick 那条扇出路 —— 先比一次白板文件指纹，没变就
    /// 什么都不做（见 `fileGates`）。进程内 `changes` 那条带着 crewId 直投，不门控，
    /// 只把指纹同步进门里，免得随后必然到达的目录 tick 再解一遍同一份文件。
    private func scan(crewId: String, gated: Bool = false) {
        guard pinned.contains(crewId) else {
            // 上次读失败没钉上 —— 这次事件顺手补钉（仍钉在当前尾，不回放历史）。
            if watched.contains(crewId) { pin(crewId) }
            return
        }
        // 指纹必须在读**之前**取：反过来的话，取指纹与读之间落进来的那次写会被
        // 记成「已读过」，下一拍就跳过 —— 那是真丢消息。现在这个顺序最坏只是
        // 多解一遍（游标会把它变成零投递）。
        let fingerprint = LocalWhiteboardStore.shared.fingerprint(crewId: crewId)
        var gate = fileGates[crewId] ?? FileChangeGate(seed: nil)
        let changed = gate.shouldYield(fingerprint)
        fileGates[crewId] = gate
        if gated, !changed { return }

        let entries = LocalWhiteboardStore.shared.entries(crewId: crewId, after: cursors[crewId])
        guard let last = entries.last else { return }
        cursors[crewId] = WhiteboardCursorPosition(id: last.id, createdAt: last.createdAt)
        for delivery in CrewLocalMentionWakeLogic.pending(entries: entries) {
            deliver(delivery, crewId: crewId)
        }
    }

    /// 一条待唤醒投递：在跑目标走 decide 注入（busy 不打断），缺席目标拉起。
    private func deliver(_ d: CrewLocalMentionWakeLogic.PendingDelivery, crewId: String) {
        guard let runner else { return }
        let store = LocalWhiteboardStore.shared
        let cursorDir = LocalWhiteboardStore.defaultDirectory
        // 候选 = 本 crew 在跑 run，排除发送者自己（自己 @ 自己不注入）。
        let candidates = runner.runs.filter {
            $0.crewId == crewId && $0.status == .running && $0.sessionId != d.senderSessionId
        }
        // 近期上下文（仅 claude 目标）：按目标自己的未读游标现取（#490 语义），
        // 剔掉本条 @ 消息自身 —— 它已是注入正文，重复出现会读两遍。
        // #543：上下文同样只放对该 session 可见的（@ 别人的定向不当「近期群聊」灌进来）。
        // 这里要的是「**看得见吗**」：谁被唤醒由下面的 `plannedInjections` 定（只认
        // session/captain），这一段只决定那次唤醒**附带什么上下文**。
        var unreadBySession: [String: [LocalWhiteboardMessage]] = [:]
        for r in candidates where r.kind == .claudeCode {
            let unread = WhiteboardCursor(
                directory: cursorDir, crewId: crewId, sessionId: r.sessionId).unread(in: store)
            unreadBySession[r.sessionId] = CrewWhiteboardVisibility.visible(
                unread, to: r.sessionId, isCaptain: r.role == .captain)
        }
        let runStates: [CrewLocalMentionInjectLogic.RunState] = candidates.map {
            .init(sessionId: $0.sessionId, isBusy: $0.backend.isBusy,
                  isClaude: $0.kind == .claudeCode)
        }
        let captainSessionId = runner.runs
            .first { $0.crewId == crewId && $0.role == .captain && $0.status == .running }?
            .sessionId
        let injections = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: d.mentions, runs: runStates,
            messageText: d.messageText, senderName: d.senderName,
            captainSessionId: captainSessionId,
            recent: { sid in
                Array((unreadBySession[sid] ?? []).filter { $0.id != d.entryId }.suffix(15))
            })
        for inj in injections {
            guard let run = candidates.first(where: { $0.sessionId == inj.sessionId }) else { continue }
            // 目标游标推进 = 本地链路的「已消费」标记：回执确认到达才推进（失败
            // 留着未读，目标解卡后 hook 路 / 下次唤醒还能带到，消息不丢）。
            let consume: () -> Void = { [weak self] in
                guard self != nil else { return }
                if run.kind == .claudeCode, let lastUnread = unreadBySession[inj.sessionId]?.last {
                    WhiteboardCursor(directory: cursorDir, crewId: crewId, sessionId: inj.sessionId)
                        .advance(to: lastUnread, in: store)
                }
            }
            runner.deliverOrDeferWake(
                sourceKey: "whiteboard:" + d.entryId,
                to: run,
                text: inj.text
            ) {
                if d.trackReceipt {
                    runner.confirmWake(run: run, crewId: crewId, onConfirmed: consume)
                } else {
                    consume()   // system 条目免回执（告警自身不再告警，防环）
                }
            }
        }
        // @ 的目标完全没在跑 → 真拉起来（与人类路 wakeAbsentMentionTargets 同语义）。
        wakeAbsent(d, crewId: crewId)
    }

    /// 拉起缺席目标：captain → startCaptain（带唤醒文本开场）；持久成员 →
    /// restartMember（复用原 sessionId，白板游标延续）。目标既不在跑也不在本地
    /// 成员登记（登录态 edge session）→ 跳过，那是 mailbox 唤醒的辖区。
    private func wakeAbsent(_ d: CrewLocalMentionWakeLogic.PendingDelivery, crewId: String) {
        guard let runner else { return }
        let runningIds = Set(runner.runs
            .filter { $0.crewId == crewId && $0.status == .running }.map(\.sessionId))
        let captainRunning = runner.runs.contains {
            $0.crewId == crewId && $0.role == .captain && $0.status == .running
        }
        let wake = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: d.mentions, runningSessionIds: runningIds, captainRunning: captainRunning)
        guard wake.needCaptain || !wake.sessionIds.isEmpty,
              let backend = backendProvider() else { return }
        let members = LocalCrewStore.shared.sessionMembers(crewId: crewId)
        let wakeText = "\(d.senderName)：\(d.messageText)"
        Task { @MainActor in
            do {
                let detail = try await backend.getCrew(crewId)
                if wake.needCaptain {
                    try await runner.startCaptain(detail: detail, backend: backend, wakeText: wakeText)
                }
                for sid in wake.sessionIds {
                    guard let m = members.first(where: { $0.sessionId == sid }) else { continue }
                    try await runner.restartMember(
                        detail: detail, backend: backend, member: m, wakeText: wakeText)
                }
            } catch {
                // fail-loud：拉起失败落白板（system，不再 @ 防环），机长/人看得见。
                LocalWhiteboardStore.shared.appendSessionMessage(
                    crewId: crewId, sessionId: "system",
                    text: "定向 @ 的目标没在跑，自动拉起失败：\(error.localizedDescription)"
                        + "——需要手动起它，起后白板会带到这条消息。",
                    senderName: "系统")
            }
        }
    }
}
#endif
