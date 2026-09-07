import Foundation

/// 机长「命名 / 拆组」硬信号的纯判定层。阈值集中在这里，既方便审阅依据，也让
/// store / hook 的文件 IO 不混进单测。
enum CaptainAwarenessLogic {
    /// 4 条白板消息通常已经覆盖「需求 + 至少一轮澄清/拆解」，足以从随机地名提炼
    /// 一个短标签；再早容易凭第一句话误命名，再晚则占位名已在侧栏停留太久。
    static let namingMessageThreshold = 4

    /// 机长 + 3 个仍存活的 worker（共 4 session）开始形成多个并行沟通面，值得
    /// 提醒是否把独立主题升成子部门；只是建议线，不自动拆。
    static let parallelSessionThreshold = 4

    /// 最近 15 分钟 12 条消息约等于持续每 75 秒一条，已不是偶发对话，主群主题
    /// 很容易互相穿插；此时提示把高频往来迁到子 crew 降噪。
    static let densityWindow: TimeInterval = 15 * 60
    static let densityMessageThreshold = 12

    /// 即使实际数字每轮微变，也至少 30 分钟不再提示；完全相同的压力快照要等 2 小时
    /// 才允许重提，兼顾「不刷屏」与长期高压时仍可温和复查。
    static let splitCooldown: TimeInterval = 30 * 60
    static let identicalSplitReminderInterval: TimeInterval = 2 * 60 * 60

    struct SplitSignal: Equatable {
        let activeSessionCount: Int
        let recentMessageCount: Int

        var signature: String { "sessions:\(activeSessionCount)|messages:\(recentMessageCount)" }
    }

    static func shouldRemindToRename(
        titleSource: LocalCrewTitleSource?, whiteboardMessageCount: Int
    ) -> Bool {
        titleSource == .placeholder && whiteboardMessageCount >= namingMessageThreshold
    }

    static func splitSignal(activeSessionCount: Int, recentMessageCount: Int) -> SplitSignal? {
        guard activeSessionCount >= parallelSessionThreshold
                || recentMessageCount >= densityMessageThreshold else { return nil }
        return SplitSignal(
            activeSessionCount: activeSessionCount,
            recentMessageCount: recentMessageCount)
    }

    static func shouldEmitSplitHint(
        signal: SplitSignal,
        previousSignature: String?,
        previousDate: Date?,
        now: Date
    ) -> Bool {
        shouldEmitHint(signature: signal.signature,
                       previousSignature: previousSignature,
                       previousDate: previousDate, now: now)
    }

    /// 机长注入面上**每一条**软提示共用的冷却判定：至少隔 `splitCooldown` 才可能
    /// 再说；完全相同的快照要隔 `identicalSplitReminderInterval` 才允许重提。
    ///
    /// 这个函数是从拆组信号那条**原地抽出来的同一份实现**，不是照着又写了一份 ——
    /// 陈旧度提示（Todo #108）要的就是同一套去重/冷却姿态，仓库里已有正确的孪生，
    /// 共用它；再发明第二种冷却，两条提示迟早会有一条被人改跑偏。
    static func shouldEmitHint(
        signature: String,
        previousSignature: String?,
        previousDate: Date?,
        now: Date
    ) -> Bool {
        guard let previousDate else { return true }
        let elapsed = now.timeIntervalSince(previousDate)
        guard elapsed >= splitCooldown else { return false }
        if previousSignature == signature {
            return elapsed >= identicalSplitReminderInterval
        }
        return true
    }

    // MARK: - 板子陈旧度（人类 Todo #108：我自己的账停了我不知道）

    /// 一条「进行中」的计划多久没动就算陈旧。**依据写在这里，不是随手取的数**：
    ///
    /// - **下界由噪音定**：机长更板的三个时刻是派活 / 收活 / 翻牌。一条刚派出去的
    ///   活半小时、一小时没动是正常的（#107 讲的「半小时没人管」说的是没人过问，
    ///   不是板没更新），阈值必须明显高于「派出去到第一份回报」那个间隔，否则这行
    ///   字每轮都在，等于没有。
    /// - **上界由伤害定**：#108 的现场是一条「等人类点头发版」在**版已经发出 4 小时
    ///   之后**仍挂在板上误导人。阈值必须小于 4 小时，这个信号才来得及在造成误导
    ///   *之前*出现。
    /// - **取 2 小时**：落在两者之间；放回 #108 那次事故里，它会在那条变成谎话之前
    ///   先响两次。
    ///
    /// 它**只陈述事实**（几条、最旧的是哪条、多久没动），不下命令 —— 一条计划该不该
    /// 动、值不值得现在动，只有机长知道。真正带强制力的是 ① 督办租约那条路。
    static let staleThreshold: TimeInterval = 2 * 60 * 60

    struct StaleSignal: Equatable {
        let count: Int
        let oldestNumber: Int
        let oldestTitle: String
        let oldestIdle: TimeInterval

        /// **签名里故意不含时长**：含了的话每多过一小时签名就变一次，冷却会退化成
        /// 「每 30 分钟必提一次」。与拆组信号同一套口径 —— 签名认的是「哪几条陈旧」，
        /// 时长只进文案。
        var signature: String { "stale:\(count)|oldest:\(oldestNumber)" }
    }

    /// 只看「进行中」那一档。
    /// - 「完成」不是陈旧，是做完了；
    /// - 「卡住」已经明说在等人，板上那行本身就是最新的现状；
    /// - 「没做」压根还没开始，久没动是它的正常形态。
    ///
    /// 也就是说：这条信号只对**声称自己正在推进、却久无动静**的条目发声，那正是
    /// #108 里会变成谎话的那一类。
    static func staleSignal(plans: [CockpitPlanItem], now: Date) -> StaleSignal? {
        let stale = plans.compactMap { item -> (CockpitPlanItem, TimeInterval)? in
            guard CockpitPlan.status(item.status) == .inProgress else { return nil }
            guard let updated = HookEmitter.parseISO8601(item.updatedAt) else { return nil }
            let idle = now.timeIntervalSince(updated)
            guard idle >= staleThreshold else { return nil }
            return (item, idle)
        }
        guard let oldest = stale.max(by: { $0.1 < $1.1 }) else { return nil }
        return StaleSignal(count: stale.count, oldestNumber: oldest.0.number,
                           oldestTitle: oldest.0.title, oldestIdle: oldest.1)
    }

    static func renderStaleHint(_ s: StaleSignal) -> String {
        "📋 板子陈旧：你有 \(s.count) 条「进行中」的计划超过 "
            + CockpitPlan.elapsedLabel(staleThreshold)
            + "没动，最旧的是 #\(s.oldestNumber)「\(s.oldestTitle)」——已 "
            + CockpitPlan.elapsedLabel(s.oldestIdle)
            + "没动。板上那几行是给人看的现状，久没动的那条现在多半已经不是真的了。"
    }

    static func recentMessageCount(timestamps: [Date], now: Date) -> Int {
        timestamps.filter { timestamp in
            let age = now.timeIntervalSince(timestamp)
            return age >= 0 && age <= densityWindow
        }.count
    }

    static func renderSplitHint(_ signal: SplitSignal) -> String {
        let facts = [
            signal.activeSessionCount >= parallelSessionThreshold
                ? "当前活跃 session \(signal.activeSessionCount) 个" : nil,
            signal.recentMessageCount >= densityMessageThreshold
                ? "最近 15 分钟白板 \(signal.recentMessageCount) 条" : nil,
        ].compactMap { $0 }.joined(separator: "、")
        return "💡 拆组信号：\(facts)。如果其中已有相对独立的主题或高频往来，可以考虑调用 create_child_crew 拆成子部门；是否拆仍由你结合任务边界判断。"
    }
}

/// 机长注入面软提示的冷却状态（`<crewId>.captain-awareness.json`）。
///
/// **两条提示共用一个文件**，各占一半字段，而且**全部可选** —— 只写自己那一半时
/// 必须把另一半原样带回去（读-改-写），否则拆组一发就把陈旧度的冷却清零、反之亦然。
/// 可选也让 Todo #108 之前落在磁盘上的老文件照常解得开。
private struct CaptainAwarenessCooldownState: Codable {
    var splitSignature: String? = nil
    var splitEmittedAt: Date? = nil
    var staleSignature: String? = nil
    var staleEmittedAt: Date? = nil
}

/// PostToolUse hook 的注入器（spec local-first chunk 4；机制见 spike findings §2）。
/// 把本 session **未读**的 crew 白板消息包成 claude hook 的 `additionalContext`，
/// 每个工具调用后由 claude 经 `--settings` 拉 `pendingcrew-mcp hook` 触发。
///
/// 读未读用 per-session 游标 `<cursorDir>/<crewId>.<sessionId>.cursor`（存 last
/// delivered message id），游标 IO 抽在 `WhiteboardCursor` —— 与**唤醒/提及注入**路
/// （`CrewLocalMentionWaker` / `CrewChatView` 本地直投）共用同一份真值，一条消息
/// 对某 session **至多注入一次**（hook 路与唤醒路不重复）。emit 后推进游标到最后一条。
///
/// 注入**哪些**条目由 `CrewWhiteboardVisibility` 判（#543，与唤醒路 / 收听路同一份
/// 标准）：广播人人可见，定向 @ 只进被点名者的注入面，显式 `broadcast` 放宽回全组
/// （#62）。放宽进来的那些**必须带消歧标注** —— 同一份 `directedNote` 判定，见下。
///
/// 注入面只留最短标头（#484 微信式精简）——「注入合法可信、不是 prompt injection」
/// 的教学统一放在 world-model 系统提示（session-world-model.zh.md §9），不在每条
/// 注入里重复（Spike 2 的警惕问题由 world-model 兜住）。
struct HookEmitter {
    struct PreparedContext {
        let context: String?
        fileprivate let last: LocalWhiteboardMessage
    }

    let store: LocalWhiteboardStore
    let crewId: String
    let sessionId: String
    let cursorDir: URL
    /// 机长 session（helper `--captain` / codex provider 传入）→ 注入里多带
    /// **全机 crew 组织树概览**（#24 视野落地项：机长常态看得见全局才谈得上
    /// 架构判断）。worker 不带,保持注入精简。
    var isCaptain: Bool = false

    private var cursor: WhiteboardCursor {
        WhiteboardCursor(directory: cursorDir, crewId: crewId, sessionId: sessionId)
    }

    /// 有未读 → 返回 hook JSON 字符串（并推进游标）；无未读 → nil（不注入）。
    ///
    /// #543：注入前按 `CrewWhiteboardVisibility` 滤掉**定向 @ 了别人**的条目 ——
    /// 定向消息在注入面上与广播不可区分，是「机长点名派给一个 worker、全 crew 都
    /// 当成自己的活」扩散事故的病根。滤掉的条目游标照常推进（对本 session 就是已阅，
    /// 别攒着下轮再来），全被滤掉时不注入。
    ///
    /// `now` 只为**注入渲染里的时间判定**（拆组信号的新鲜度 / 密度窗 / 冷却）提供
    /// 一个可注入的「现在」。生产两处调用点（`McpHelperMain` / `CrewSessionRunner`）
    /// 都走默认值 `Date()`，行为不变；单测传入自己造的固定时刻，才能让判定与机器
    /// 负载无关（否则用例从写快照到 emit 之间的墙上时间会参与判定，跑全量时飘红）。
    func emitAndAdvance(now: Date = Date()) -> String? {
        guard let pending = prepareContext(now: now) else { return nil }
        guard let context = pending.context else {
            commit(pending)
            return nil
        }
        let json: [String: Any] = ["hookSpecificOutput": [
            "hookEventName": "PostToolUse",
            "additionalContext": context,
        ]]
        guard let data = try? JSONSerialization.data(withJSONObject: json),
              let s = String(data: data, encoding: .utf8) else { return nil }
        commit(pending)
        return s
    }

    /// 与 hook 同一套未读 / 可见性 / 游标语义，但直接返回纯上下文。
    /// Claude 新 session 的第一轮没有 PostToolUse 事件，启动 prompt 走这条；Codex 的
    /// `turn/start.additionalContext` 也可直接复用，不必先包 JSON 再拆 JSON。
    func emitContextAndAdvance(excluding excludedId: String? = nil,
                               now: Date = Date()) -> String? {
        guard let pending = prepareContext(excluding: excludedId, now: now) else { return nil }
        commit(pending)
        return pending.context
    }

    /// Codex `turn/start` 的两阶段读取：准备只读，不推进游标；RPC 确认受理后调用
    /// `commit`。拒绝/断线时保留未读，下一次提交仍能带上原消息。
    /// `excluding`：这一条的正文已经由别的通道送到了（#105 ②：`wakeText` 被烤进
    /// 开场 prompt），别再渲染一遍。游标照常推进到未读末尾 —— 它确实已经送到了。
    func prepareContext(excluding excludedId: String? = nil,
                        now: Date = Date()) -> PreparedContext? {
        let unread = cursor.unread(in: store)
        guard let last = unread.messages.last else { return nil }
        // 要的是「**看得见吗**」，不是「该叫醒吗」—— 这条路每轮都跑，本身就不唤醒
        // 任何人，只决定渲染什么进上下文。只 @ 了人类的消息在这里必须可见（2026-08-23
        // 修的正主：过去它对所有 agent 隐身）。
        var mine = CrewWhiteboardVisibility.visible(
            unread.messages, to: sessionId, isCaptain: isCaptain)
        if let excludedId { mine.removeAll { $0.id == excludedId } }
        return PreparedContext(
            context: mine.isEmpty ? nil : render(mine, omitted: unread.omitted, now: now),
            last: last)
    }

    func commit(_ prepared: PreparedContext) {
        cursor.advance(to: prepared.last, in: store)
    }

    private func render(_ msgs: [LocalWhiteboardMessage], omitted: Int, now: Date) -> String {
        var lines: [String] = []
        let allMessages = store.list(crewId: crewId)
        // 注入面消歧（#62）用的花名册：sessionId → 显示名。判定本身是纯函数
        // （`CrewWhiteboardVisibility.directedNote`），这里只负责取名字 + 拼字符串。
        let roster = CrewSessionsSnapshot.displayNames(ofCrew: crewId, directory: cursorDir)
        // 每轮注入当前 crew 名（crew-sidebar-status spec §1）：机长在长 session 中 /
        // 改名后也随时知道当前名字，才能判断名字是否仍贴切（rename_crew 的前提）。
        // title 从共享 local-crews.json 轻量读（app 进程与 helper 子进程同一路径，
        // 见 LocalCrewStore.title(ofCrew:whiteboardDirectory:)）；读不到则省略此行。
        let titleMetadata = LocalCrewStore.titleMetadata(
            ofCrew: crewId, whiteboardDirectory: cursorDir)
        if let titleMetadata {
            lines.append("本 crew 当前名：\(titleMetadata.title)")
            if isCaptain && CaptainAwarenessLogic.shouldRemindToRename(
                titleSource: titleMetadata.source,
                whiteboardMessageCount: allMessages.count
            ) {
                lines.append("⚠️ 命名待办：当前名是系统占位名，白板已有 \(allMessages.count) 条消息，主题应已明朗。现在请调用 rename_crew 改成贴切的短标签。")
            }
        }
        if isCaptain {
            // 两条软提示共读共写同一份冷却状态 —— 读一次、各自判、最后写一次，
            // 免得后写的那条把先写的那条的冷却抹掉。
            var state = loadAwarenessState()
            var stateChanged = false
            if let hint = splitHint(allMessages: allMessages, state: &state, now: now) {
                lines.append(hint)
                stateChanged = true
            }
            if let hint = staleHint(state: &state, now: now) {
                lines.append(hint)
                stateChanged = true
            }
            if stateChanged { saveAwarenessState(state) }
        }
        // 机长视野（#24）：全机 crew 组织树 + 各 crew 最近一句动静。轻量跨进程读
        // local-crews.json + 各 crew 白板尾行;本机只有本 crew 一个时省略（无全局可看）。
        if isCaptain {
            lines.append(contentsOf: renderOrgTree())
        }
        // #105 ①：有上限就必须自报。**静默截断是我们要防的第三道** ——
        // 收的人看不到这一行，就会把手里这 30 条当成全部。
        if omitted > 0 {
            lines.append("群聊白板·未读（较早的 \(omitted) 条已省略，只给最近 \(msgs.count) 条；要看全的用 read_whiteboard）：")
        } else {
            lines.append("群聊白板·未读：")
        }
        for m in msgs {
            // 有显示名（senderName）→ 直接用名字，让 agent 看得见是谁发的；
            // 无名才退回旧格式（session:<id> / 人类），保持兼容。
            let who: String
            if let name = m.senderName, !name.isEmpty {
                who = name
            } else {
                switch m.senderKind {
                case "session": who = "session:\(m.senderSessionId ?? "?")"
                case "user": who = "人类"
                default: who = m.senderKind
                }
            }
            // #62：这条对我可见、但收窄型 mention 里没有我 → 前置「（发给 XX 的）」。
            // 治 #543 的病根（当年是把它藏起来绕过去的）：看得见，而且看得出不是我的活。
            let note = CrewWhiteboardVisibility.directedNote(
                m, to: sessionId, isCaptain: isCaptain, displayName: { roster[$0] }) ?? ""
            // agentText = 正文 + 附件绝对路径提示行（Todo #3 群聊图片）。
            lines.append("- \(who): \(note)\(m.agentText)")
        }
        return lines.joined(separator: "\n")
    }

    /// 全机 crew 组织树概览行（机长注入用）。缩进 = 父子层级,标注本 crew,每行
    /// 尾带该 crew 白板最近一句（截 30 字,给「一句现状」的体感）。本机只有一个
    /// crew → 返回空（没有全局可言,别添噪音）。
    private func renderOrgTree() -> [String] {
        let rows = LocalCrewStore.orgTreeLines(whiteboardDirectory: cursorDir)
        guard rows.count > 1 else { return [] }
        var lines = ["本机 crew 组织树（机长视野;缩进=父子。架构该调就调：adopt_crew 收编 / release_crew 摘出转挂 / create_parent_crew 建父 / adopt_parent 认父）："]
        for r in rows {
            let indent = String(repeating: "  ", count: r.depth)
            let marker = r.id == crewId ? "（本 crew）" : ""
            let placeholder = r.depth > 0 && r.titleSource == .placeholder
                ? "〔占位名·待子机长改名〕" : ""
            let last = (store.list(crewId: r.id).last?.text ?? "")
                .replacingOccurrences(of: "\n", with: " ")
            let preview = last.isEmpty ? "" : "：\(last.prefix(30))\(last.count > 30 ? "…" : "")"
            lines.append("\(indent)- \(r.title)\(marker)\(placeholder)\(preview)")
        }
        return lines
    }

    private func splitHint(allMessages: [LocalWhiteboardMessage],
                           state: inout CaptainAwarenessCooldownState, now: Date) -> String? {
        let recentCount = CaptainAwarenessLogic.recentMessageCount(
            timestamps: allMessages.compactMap { Self.parseISO8601($0.createdAt) },
            now: now)
        let activeCount = activeSessionCount(now: now)
        guard let signal = CaptainAwarenessLogic.splitSignal(
            activeSessionCount: activeCount, recentMessageCount: recentCount)
        else { return nil }
        guard CaptainAwarenessLogic.shouldEmitSplitHint(
            signal: signal,
            previousSignature: state.splitSignature,
            previousDate: state.splitEmittedAt,
            now: now)
        else { return nil }
        state.splitSignature = signal.signature
        state.splitEmittedAt = now
        return CaptainAwarenessLogic.renderSplitHint(signal)
    }

    /// 板子陈旧度（人类 Todo #108）。数据现成 —— 计划条目本来就记着最后更新时间，
    /// 界面上也一直显示「进行中 · 最后更新 3 天前」；缺的只是把它送进注入面，
    /// 让机长不必点开驾驶舱也知道自己那块板停在哪儿。
    ///
    /// ⚠️ **这条只治得了 #108，治不了 #107**：整个注入面挂在 `prepareContext` 的
    /// 「有未读白板消息」那道 guard 后面，群里安静时它一个字也送不出去。#107 那种
    /// 「交出去的活没人管、群里又没人说话」只有主动唤醒（① 督办租约）能救。这句话
    /// 有一条测试钉着：`testStaleBoardCannotReachACaptainWhoHasNoUnreadMessages`。
    private func staleHint(state: inout CaptainAwarenessCooldownState, now: Date) -> String? {
        let plans = CockpitPlanStore(directory: cursorDir).list(crewId: crewId)
        guard let signal = CaptainAwarenessLogic.staleSignal(plans: plans, now: now) else { return nil }
        guard CaptainAwarenessLogic.shouldEmitHint(
            signature: signal.signature,
            previousSignature: state.staleSignature,
            previousDate: state.staleEmittedAt,
            now: now)
        else { return nil }
        state.staleSignature = signal.signature
        state.staleEmittedAt = now
        return CaptainAwarenessLogic.renderStaleHint(signal)
    }

    private var awarenessStateURL: URL {
        cursorDir.appendingPathComponent("\(crewId).captain-awareness.json")
    }

    private func loadAwarenessState() -> CaptainAwarenessCooldownState {
        (try? Data(contentsOf: awarenessStateURL))
            .flatMap { try? JSONDecoder().decode(CaptainAwarenessCooldownState.self, from: $0) }
            ?? CaptainAwarenessCooldownState()
    }

    private func saveAwarenessState(_ state: CaptainAwarenessCooldownState) {
        guard let data = try? JSONEncoder().encode(state) else { return }
        try? data.write(to: awarenessStateURL, options: .atomic)
    }

    /// 快照每 2 秒刷新；超过 15 秒说明 app 已停或数据链异常，不拿陈旧 roster 制造
    /// “当前并行”假信号。存活 = 除 exited / launchFailed 外的状态（含等待决策/限额）。
    private func activeSessionCount(now: Date) -> Int {
        let url = cursorDir.appendingPathComponent(CrewSessionsSnapshot.fileName)
        guard let data = try? Data(contentsOf: url),
              let snapshot = try? JSONDecoder().decode(CrewSessionsSnapshot.self, from: data),
              let updatedAt = Self.parseISO8601(snapshot.updatedAt),
              now.timeIntervalSince(updatedAt) >= 0,
              now.timeIntervalSince(updatedAt) <= 15
        else { return 0 }
        return (snapshot.crews[crewId] ?? []).filter {
            $0.state != "exited" && $0.state != "launchFailed"
        }.count
    }

    /// 非 private：`CaptainAwarenessLogic.staleSignal` 解计划条目的 `updatedAt`
    /// 也走这一个解析器（带/不带小数秒两种形状），别在同一个文件里养第二份。
    static func parseISO8601(_ value: String) -> Date? {
        if let date = ISO8601DateFormatter().date(from: value) { return date }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: value)
    }
}
