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
        guard let previousDate else { return true }
        let elapsed = now.timeIntervalSince(previousDate)
        guard elapsed >= splitCooldown else { return false }
        if previousSignature == signal.signature {
            return elapsed >= identicalSplitReminderInterval
        }
        return true
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

private struct CaptainAwarenessCooldownState: Codable {
    let splitSignature: String
    let splitEmittedAt: Date
}

/// PostToolUse hook 的注入器（spec local-first chunk 4；机制见 spike findings §2）。
/// 把本 session **未读**的 crew 白板消息包成 claude hook 的 `additionalContext`，
/// 每个工具调用后由 claude 经 `--settings` 拉 `pendingcrew-mcp hook` 触发。
///
/// 读未读用 per-session 游标 `<cursorDir>/<crewId>.<sessionId>.cursor`（存 last
/// delivered message id），游标 IO 抽在 `WhiteboardCursor` —— 与**唤醒/提及注入**路
/// （`CrewMailboxWaker` 登录态、`CrewChatView` 本地直投）共用同一份真值，一条消息
/// 对某 session **至多注入一次**（hook 路与唤醒路不重复）。emit 后推进游标到最后一条。
///
/// 注入**哪些**条目由 `CrewWhiteboardVisibility` 判（#543，与唤醒路 / 收听路同一份
/// 标准）：广播人人可见，定向 @ 只进被点名者的注入面。
///
/// 注入面只留最短标头（#484 微信式精简）——「注入合法可信、不是 prompt injection」
/// 的教学统一放在 world-model 系统提示（session-world-model.zh.md §9），不在每条
/// 注入里重复（Spike 2 的警惕问题由 world-model 兜住）。
struct HookEmitter {
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
    func emitAndAdvance() -> String? {
        guard let pending = pendingContext() else { return nil }
        guard let context = pending.context else {
            cursor.advance(to: pending.last, in: store)
            return nil
        }
        let json: [String: Any] = ["hookSpecificOutput": [
            "hookEventName": "PostToolUse",
            "additionalContext": context,
        ]]
        guard let data = try? JSONSerialization.data(withJSONObject: json),
              let s = String(data: data, encoding: .utf8) else { return nil }
        cursor.advance(to: pending.last, in: store)
        return s
    }

    /// 与 hook 同一套未读 / 可见性 / 游标语义，但直接返回纯上下文。
    /// Claude 新 session 的第一轮没有 PostToolUse 事件，启动 prompt 走这条；Codex 的
    /// `turn/start.additionalContext` 也可直接复用，不必先包 JSON 再拆 JSON。
    func emitContextAndAdvance() -> String? {
        guard let pending = pendingContext() else { return nil }
        cursor.advance(to: pending.last, in: store)
        return pending.context
    }

    private func pendingContext() -> (context: String?, last: LocalWhiteboardMessage)? {
        let unread = cursor.unread(in: store)
        guard let last = unread.last else { return nil }
        // 要的是「**看得见吗**」，不是「该叫醒吗」—— 这条路每轮都跑，本身就不唤醒
        // 任何人，只决定渲染什么进上下文。只 @ 了人类的消息在这里必须可见（2026-08-23
        // 修的正主：过去它对所有 agent 隐身）。
        let mine = CrewWhiteboardVisibility.visible(unread, to: sessionId, isCaptain: isCaptain)
        return (mine.isEmpty ? nil : render(mine), last)
    }

    private func render(_ msgs: [LocalWhiteboardMessage], now: Date = Date()) -> String {
        var lines: [String] = []
        let allMessages = store.list(crewId: crewId)
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
        if isCaptain, let splitHint = splitHint(allMessages: allMessages, now: now) {
            lines.append(splitHint)
        }
        // 机长视野（#24）：全机 crew 组织树 + 各 crew 最近一句动静。轻量跨进程读
        // local-crews.json + 各 crew 白板尾行;本机只有本 crew 一个时省略（无全局可看）。
        if isCaptain {
            lines.append(contentsOf: renderOrgTree())
        }
        lines.append("群聊白板·未读：")
        for m in msgs {
            // 有显示名（本地 senderName 或 relay senderDisplayName）→ 直接用名字，
            // 让 agent 看得见是谁发的；无名才退回旧格式（session:<id> / 人类），保持兼容。
            let who: String
            if let name = m.senderName ?? m.senderDisplayName, !name.isEmpty {
                who = name
            } else {
                switch m.senderKind {
                case "session": who = "session:\(m.senderSessionId ?? "?")"
                case "user": who = "人类"
                default: who = m.senderKind
                }
            }
            // agentText = 正文 + 附件绝对路径提示行（Todo #3 群聊图片）。
            lines.append("- \(who): \(m.agentText)")
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

    private func splitHint(allMessages: [LocalWhiteboardMessage], now: Date) -> String? {
        let recentCount = CaptainAwarenessLogic.recentMessageCount(
            timestamps: allMessages.compactMap { Self.parseISO8601($0.createdAt) },
            now: now)
        let activeCount = activeSessionCount(now: now)
        guard let signal = CaptainAwarenessLogic.splitSignal(
            activeSessionCount: activeCount, recentMessageCount: recentCount)
        else { return nil }

        let stateURL = cursorDir.appendingPathComponent("\(crewId).captain-awareness.json")
        let previous = (try? Data(contentsOf: stateURL))
            .flatMap { try? JSONDecoder().decode(CaptainAwarenessCooldownState.self, from: $0) }
        guard CaptainAwarenessLogic.shouldEmitSplitHint(
            signal: signal,
            previousSignature: previous?.splitSignature,
            previousDate: previous?.splitEmittedAt,
            now: now)
        else { return nil }

        let next = CaptainAwarenessCooldownState(
            splitSignature: signal.signature, splitEmittedAt: now)
        if let data = try? JSONEncoder().encode(next) {
            try? data.write(to: stateURL, options: .atomic)
        }
        return CaptainAwarenessLogic.renderSplitHint(signal)
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

    private static func parseISO8601(_ value: String) -> Date? {
        if let date = ISO8601DateFormatter().date(from: value) { return date }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: value)
    }
}
