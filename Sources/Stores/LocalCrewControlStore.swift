import Foundation

/// 本地 crew 元数据控制通道（spec 2026-06-17-pendingcrew-crew-naming）。
///
/// captain 的 MCP helper 是**离线子进程**，碰不到 `LocalCrewStore`（crew 标题真源）/
/// 网络，只能读写 `--dir` 下的共享文件。`rename_crew` 工具把一条待改名写进这里，
/// app 侧（`CrewStore`）靠目录监听感知、排空、落地到 `LocalCrewStore.setTitle` 并刷新侧栏。
///
/// 每 crew 一个 JSON：`<dir>/<crewId>.crewmeta.json` = `CrewMetaChange`（last-write-wins ——
/// 只保留最新一次改名，captain 反复改也只落最后那个）。与 `LocalWhiteboardStore`
/// 的 `<crewId>.json` / `LocalApprovalStore` 的 `<crewId>.approvals.json` 后缀不同，不冲突；
/// 共用 `LocalWhiteboardStore.defaultDirectory`，所以写盘即触发 app 的 `directoryChanged`。
///
/// **自包含 Foundation**（编进 `pendingcrew-mcp` re-exec helper + PendingCrewTests bundle）。
/// `@unchecked Sendable`：实例状态全 `let`。并发模型与批量 store 不同,不需要 flock：
/// rename/attention 是单文件整写、last-write-wins 即语义（只留最新一次）；命令队列
/// 每条独立文件原子写、drain 读完即删,无 read-modify-write。坏文件（解码失败）
/// drain 时归档 + 白板回执（#528）,不静默丢也不滞留重复解码。
final class LocalCrewControlStore: @unchecked Sendable {
    static let shared = LocalCrewControlStore()

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    // MARK: - Write（helper 侧：rename_crew 工具）

    /// 写一条待改名（覆盖既有 → 只留最新）。空名（trim 后）忽略，不落盘。
    /// **返回 nil = 真的写进去了**（下同，本 store 全部写入口一个口径）。
    @discardableResult
    func requestRename(crewId: String, name: String) -> Error? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return ControlChannelWriteError.refusedHere("名字为空") }
        let change = CrewMetaChange(title: trimmed, ts: ISO8601DateFormatter().string(from: Date()))
        guard let data = try? JSONEncoder().encode(change) else {
            return ControlChannelWriteError.refusedHere("编码失败")
        }
        do {
            try data.write(to: fileURL(crewId), options: .atomic)
            return nil
        } catch {
            return error
        }
    }

    // MARK: - Read / drain（app 侧：CrewStore 落地）

    /// 偷看某 crew 的待改名（不删）。无 → nil。
    func pendingRename(crewId: String) -> String? {
        load(crewId)?.title
    }

    /// 排空全部待改名（目录监听 tick 调）：返回 `(crewId, title)` 列表并删除对应文件。
    /// 落地由调用方做（`LocalCrewStore.setTitle` + 刷新）。
    func drainRenames() -> [(crewId: String, title: String)] {
        guard let files = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil) else { return [] }
        var out: [(crewId: String, title: String)] = []
        for url in files where url.lastPathComponent.hasSuffix(Self.suffix) {
            let file = url.lastPathComponent
            let crewId = String(file.dropLast(Self.suffix.count))
            guard !crewId.isEmpty, let data = try? Data(contentsOf: url) else { continue }
            guard let change = try? JSONDecoder().decode(CrewMetaChange.self, from: data) else {
                quarantineBadFile(url, what: "crew 改名命令")
                continue
            }
            out.append((crewId, change.title))
            try? FileManager.default.removeItem(at: url)
        }
        return out
    }

    // MARK: - Attention（旧会话兼容文案，不再控制侧栏状态点）
    //
    // 与 rename 同款 last-write-wins 单文件：`<crewId>.crewattention.json` =
    // `CrewAttentionChange`（reason 非 nil = 记录，nil = 清除）。同 tick 先 raise
    // 后 clear 只落最后一次 —— app 侧 drain 时直接应用最终态即可。

    /// 记录 attention（helper 侧：`raise_attention` 工具）。reason 一句话说明为什么
    /// 需要人类注意；空（trim 后）忽略，不落盘。
    @discardableResult
    func requestAttention(crewId: String, reason: String) -> Error? {
        let trimmed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return ControlChannelWriteError.refusedHere("reason 为空") }
        return writeAttention(crewId: crewId, reason: trimmed)
    }

    /// 清除 attention（helper 侧：`clear_attention` 工具）。
    @discardableResult
    func requestClearAttention(crewId: String) -> Error? {
        writeAttention(crewId: crewId, reason: nil)
    }

    /// 偷看某 crew 的待应用 attention 变更（不删）。无 → nil。
    func pendingAttention(crewId: String) -> CrewAttentionChange? {
        guard let data = try? Data(contentsOf: attentionFileURL(crewId)) else { return nil }
        return try? JSONDecoder().decode(CrewAttentionChange.self, from: data)
    }

    /// 排空全部待应用 attention 变更（目录监听 tick 调）：返回 `(crewId, reason)`
    /// 列表并删除对应文件。`reason == nil` = 熄灭。落地由调用方做
    /// （`LocalCrewStore.setAttention` + 刷新）。
    func drainAttentions() -> [(crewId: String, reason: String?)] {
        guard let files = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil) else { return [] }
        var out: [(crewId: String, reason: String?)] = []
        for url in files where url.lastPathComponent.hasSuffix(Self.attentionSuffix) {
            let file = url.lastPathComponent
            let crewId = String(file.dropLast(Self.attentionSuffix.count))
            guard !crewId.isEmpty, let data = try? Data(contentsOf: url) else { continue }
            guard let change = try? JSONDecoder().decode(CrewAttentionChange.self, from: data) else {
                quarantineBadFile(url, what: "attention 变更")
                continue
            }
            out.append((crewId, change.reason))
            try? FileManager.default.removeItem(at: url)
        }
        return out
    }

    private func writeAttention(crewId: String, reason: String?) -> Error? {
        let change = CrewAttentionChange(reason: reason, ts: ISO8601DateFormatter().string(from: Date()))
        guard let data = try? JSONEncoder().encode(change) else {
            return ControlChannelWriteError.refusedHere("编码失败")
        }
        do {
            try data.write(to: attentionFileURL(crewId), options: .atomic)
            return nil
        } catch {
            return error
        }
    }

    // MARK: - Command queue（helper 侧：start_session / create_child_crew）
    //
    // rename 是 last-write-wins 单文件；起 session / 建子 crew 可重复，用队列：
    // 每条一个独立文件 <crewId>.<uuid>.crewcmd.json，drain 时全读+删。

    /// 机长起 worker session：brief 必填（空则忽略）；runner/isolation/model/effort 可选
    /// （model/effort nil = 用对应 runner 的默认）。
    @discardableResult
    func enqueueStartSession(crewId: String, brief: String, runner: String?, isolation: Bool?,
                             model: String? = nil, effort: String? = nil, title: String? = nil) -> Error? {
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "start_session",
            brief: brief, runner: runner, isolation: isolation, title: title,
            model: model, effort: effort,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 机长主动交接。两种模式严格互斥：
    /// - existing：`targetSessionId` 非 nil、`runner` nil；
    /// - create-new：`targetSessionId` nil、`runner` 必为 claude/codex。
    /// helper 只负责可靠入队，live runner 的停旧/起新/回滚由 app 侧同一服务执行。
    @discardableResult
    func enqueueCaptainHandoff(
        crewId: String, requesterSessionId: String,
        targetCrewId: String? = nil,
        targetSessionId: String?, runner: String?, model: String?, effort: String?,
        title: String?, openingBrief: String?
    ) -> Error? {
        let existing = targetSessionId?.isEmpty == false && runner == nil
        let fresh = targetSessionId == nil && (runner == "claude" || runner == "codex")
        guard existing || fresh else {
            return ControlChannelWriteError.refusedHere("两种交接模式的参数组合不合法")
        }
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "handoff_captain",
            brief: "-", runner: runner, isolation: nil, title: title,
            model: model, effort: effort, sessionId: targetSessionId,
            note: openingBrief, requesterSessionId: requesterSessionId,
            targetCrewId: targetCrewId,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 机长以当前 crew 为父建子 crew：brief 必填；title 可选（不给 → app 侧地名占位）。
    @discardableResult
    func enqueueCreateChildCrew(crewId: String, sessionId: String,
                                brief: String, title: String?) -> Error? {
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "create_child_crew",
            brief: brief, runner: nil, isolation: nil, title: title,
            model: nil, effort: nil, sessionId: sessionId,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// session 自切模型/effort（至少一个非空;两个都空则忽略不落盘）。
    /// `sessionId` = 发起方自己 —— app 侧据此找到对应 run。
    @discardableResult
    func enqueueSetProfile(crewId: String, sessionId: String, model: String?, effort: String?) -> Error? {
        guard model?.isEmpty == false || effort?.isEmpty == false else {
            return ControlChannelWriteError.refusedHere("model / effort 都是空的")
        }
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "set_profile",
            brief: "-", runner: nil, isolation: nil, title: nil,
            model: model, effort: effort, sessionId: sessionId,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 机长跨 crew 消息（汇报线）：`direction` = "to_parent"（发所有直系父）|
    /// "to_child"（发 `targetHint` 指定的直系子）。消息文本放 `brief`（复用非空
    /// 校验）。DAG 解析/投递/唤醒归 app 侧（helper 读不到 crew store）。
    @discardableResult
    func enqueueCrewMessage(crewId: String, sessionId: String, direction: String,
                            targetHint: String?, message: String) -> Error? {
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "crew_message",
            brief: message, runner: nil, isolation: nil, title: targetHint,
            sessionId: sessionId, direction: direction,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// session 设定时唤醒：到点 app 把 `note` 注入回该 session（不在跑 → 白板落一条）。
    @discardableResult
    func enqueueScheduleWakeup(crewId: String, sessionId: String, fireAt: String, note: String) -> Error? {
        guard !fireAt.isEmpty, !note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return ControlChannelWriteError.refusedHere("fireAt / note 缺一")
        }
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "schedule_wakeup",
            brief: "-", runner: nil, isolation: nil, title: nil,
            sessionId: sessionId, fireAt: fireAt, note: note,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 挂一笔**督办租约**（`plan_add` / `plan_update` 的 `supervise_after_minutes`，
    /// 人类 Todo #107）。走的就是上面那条 `schedule_wakeup` 命令通道 —— 多带的只有
    /// 「替哪条计划盯着」和「基础间隔」，app 侧据此换成确定性 id 并走督办分支。
    ///
    /// **解除条件不在这里**：唯一能让它消音的是把那条计划翻到 done / blocked
    /// （见 `SupervisionLease`）。这里**故意没有**「取消督办」这个方法。
    @discardableResult
    func enqueueSupervisionLease(crewId: String, sessionId: String, planNumber: Int,
                                 fireAt: String, baseSeconds: Double) -> Error? {
        guard !fireAt.isEmpty, planNumber >= 1 else {
            return ControlChannelWriteError.refusedHere("fireAt / planNumber 不合法")
        }
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "schedule_wakeup",
            brief: "-", runner: nil, isolation: nil, title: nil,
            sessionId: sessionId, fireAt: fireAt,
            note: "督办：计划 #\(planNumber)",
            planNumber: planNumber, leaseBaseSeconds: baseSeconds,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// session 开/关群聊收听（`listen` 工具;#465）。开 = `until`（ISO8601）必给，
    /// `senders` 可选（nil = 听全部）；关 = `off: true`（until/senders 忽略）。
    /// 同一 session 重复开 = 覆盖（app 侧 last-write-wins）。
    @discardableResult
    func enqueueListen(crewId: String, sessionId: String, until: String?,
                       senders: [String]?, off: Bool) -> Error? {
        guard off || until?.isEmpty == false else {
            return ControlChannelWriteError.refusedHere("开收听必须给 until")
        }
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "listen",
            brief: off ? "off" : "on", runner: nil, isolation: nil, title: nil,
            sessionId: sessionId, fireAt: until, senders: senders, off: off,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 机长自检某 session：inspect_session（wake-resilience 机长自愈）。返回命令 id ——
    /// helper 用它 long-poll `takeCommandResponse` 拿终端快照。
    @discardableResult
    func enqueueInspectSession(crewId: String, targetSessionId: String) -> EnqueuedCommand {
        let id = UUID().uuidString.lowercased()
        let failure = enqueue(CrewCommand(
            id: id, crewId: crewId, kind: "inspect_session",
            brief: "-", runner: nil, isolation: nil, title: nil,
            sessionId: targetSessionId,
            ts: ISO8601DateFormatter().string(from: Date())))
        return EnqueuedCommand(id: id, failure: failure)
    }

    /// 机长解卡某 session：nudge_session —— 向其 PTY 发文本/按键（`input` 放 note 字段）。
    /// 返回命令 id（helper 同样 long-poll 应答确认送达）。
    @discardableResult
    func enqueueNudgeSession(crewId: String, targetSessionId: String, input: String) -> EnqueuedCommand {
        let id = UUID().uuidString.lowercased()
        let failure = enqueue(CrewCommand(
            id: id, crewId: crewId, kind: "nudge_session",
            brief: "-", runner: nil, isolation: nil, title: nil,
            sessionId: targetSessionId, note: input,
            ts: ISO8601DateFormatter().string(from: Date())))
        return EnqueuedCommand(id: id, failure: failure)
    }

    /// 机长终止本 crew 某 session：`reason` 放 note，发起机长 session id 单独保留，
    /// app 侧据此先落白板回执，再走 runner 的既有 `run.stop()` 路径真停进程。
    /// 返回命令 id（helper long-poll 明确拿到成功或拒绝原因）。
    @discardableResult
    func enqueueStopSession(crewId: String, requesterSessionId: String,
                            targetSessionId: String, reason: String) -> EnqueuedCommand {
        let id = UUID().uuidString.lowercased()
        let failure = enqueue(CrewCommand(
            id: id, crewId: crewId, kind: "stop_session",
            brief: "-", runner: nil, isolation: nil, title: nil,
            sessionId: targetSessionId, note: reason,
            requesterSessionId: requesterSessionId,
            ts: ISO8601DateFormatter().string(from: Date())))
        return EnqueuedCommand(id: id, failure: failure)
    }

    /// 机长改工作目录（含 agent 上下文迁移）：`path` 目标目录；`targetHint` 指定本 crew
    /// 子树里的哪一个（空 = 本 crew）；`includeChildren` 连子 crew 一起迁；`confirm=false`
    /// 只出预览。目标解析 / 规划 / 执行全归 app 侧（helper 读不到 crew store 与在跑的 run）。
    /// 返回命令 id —— helper long-poll 拿预览或回执。
    @discardableResult
    func enqueueChangeWorkdir(crewId: String, sessionId: String, targetHint: String?,
                              path: String, includeChildren: Bool, confirm: Bool) -> EnqueuedCommand {
        let id = UUID().uuidString.lowercased()
        let hint = targetHint?.trimmingCharacters(in: .whitespacesAndNewlines)
        let failure = enqueue(CrewCommand(
            id: id, crewId: crewId, kind: "change_workdir",
            brief: "-", runner: nil, isolation: nil,
            title: (hint?.isEmpty == false) ? hint : nil,
            sessionId: sessionId, path: path,
            includeChildren: includeChildren, confirm: confirm,
            ts: ISO8601DateFormatter().string(from: Date())))
        return EnqueuedCommand(id: id, failure: failure)
    }

    // MARK: - Command responses（inspect/nudge 的应答半边）
    //
    // 有些机长命令要**带结果回来**（inspect 的终端快照）：app 侧执行完写
    // `<crewId>.<commandId>.crewresp.json`，helper 侧 long-poll `takeCommandResponse`
    // （读到即删，一次性消费）。与 approvals 的 raise→answer long-poll 同思路，
    // 但不进人类待办列表 —— 这是机长↔app 的机器通道。

    /// app 侧：写一条命令应答。
    @discardableResult
    func writeCommandResponse(crewId: String, commandId: String, text: String) -> Error? {
        let resp = CrewCommandResponse(text: text, ts: ISO8601DateFormatter().string(from: Date()))
        guard let data = try? JSONEncoder().encode(resp) else {
            return ControlChannelWriteError.refusedHere("编码失败")
        }
        do {
            try data.write(to: responseURL(crewId: crewId, commandId: commandId), options: .atomic)
            return nil
        } catch {
            return error
        }
    }

    /// helper 侧：取一条命令应答（读到即删）。无 → nil（继续 poll）。
    func takeCommandResponse(crewId: String, commandId: String) -> String? {
        let url = responseURL(crewId: crewId, commandId: commandId)
        guard let data = try? Data(contentsOf: url),
              let resp = try? JSONDecoder().decode(CrewCommandResponse.self, from: data) else { return nil }
        try? FileManager.default.removeItem(at: url)
        return resp.text
    }

    private func responseURL(crewId: String, commandId: String) -> URL {
        directory.appendingPathComponent("\(crewId).\(commandId)\(Self.respSuffix)")
    }

    // MARK: - 组织架构调整（#22/#25：收编/摘出/建父/认父）
    //
    // 目标 crew 的解析（标签名/id/唯一前缀 → crew id）归 app 侧 —— helper 是离线
    // 子进程,读不到 crew store。hint 一律放 `title` 字段（同 crew_message 的
    // targetHint 复用约定）;brief 用 "-" 占位过非空守卫。

    /// 机长收编：把 `target`（顶层/无关 crew 的标签名或 id）挂到当前 crew 名下当直系子。
    @discardableResult
    func enqueueAdoptCrew(crewId: String, sessionId: String, target: String) -> Error? {
        let t = target.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return ControlChannelWriteError.refusedHere("目标 crew 为空") }
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "adopt_crew",
            brief: "-", runner: nil, isolation: nil, title: t, sessionId: sessionId,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 机长摘出/转挂**直系子** `child`：`to == nil` → 摘回顶层；非 nil → 转挂到
    /// 自己的另一个直系子（hint 放 `note` 字段）。
    @discardableResult
    func enqueueReleaseCrew(crewId: String, sessionId: String, child: String, to: String?) -> Error? {
        let c = child.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !c.isEmpty else { return ControlChannelWriteError.refusedHere("子 crew 为空") }
        let dest = to?.trimmingCharacters(in: .whitespacesAndNewlines)
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "release_crew",
            brief: "-", runner: nil, isolation: nil, title: c, sessionId: sessionId,
            note: (dest?.isEmpty == false) ? dest : nil,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 机长建父：在自己头上新建一个父 crew（自动起父机长）。`title` nil = 地名占位。
    @discardableResult
    func enqueueCreateParentCrew(crewId: String, sessionId: String, title: String?) -> Error? {
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "create_parent_crew",
            brief: "-", runner: nil, isolation: nil, title: title, sessionId: sessionId,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 机长认父：把现有 crew `target`（标签名或 id）认作当前 crew 的父。
    @discardableResult
    func enqueueAdoptParent(crewId: String, sessionId: String, target: String) -> Error? {
        let t = target.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return ControlChannelWriteError.refusedHere("目标 crew 为空") }
        return enqueue(CrewCommand(
            id: UUID().uuidString.lowercased(), crewId: crewId, kind: "adopt_parent",
            brief: "-", runner: nil, isolation: nil, title: t, sessionId: sessionId,
            ts: ISO8601DateFormatter().string(from: Date())))
    }

    /// 真正落盘的那一下。**返回 nil = 这条命令真的写到磁盘上了。**
    ///
    /// 2026-09-09 之前这里是 `try? data.write(...)` + `Void`：写不进去和写进去了
    /// 在调用方那儿一模一样，于是**二十个机长/成员工具**（`report_to_parent` 打头）
    /// 在数据目录写失败时照样回一句「已提交」。`report_to_parent` 尤其阴 ——
    /// 它是子 crew 唯一的向上通道，它一撒谎，上级就以为下面没动静。
    private func enqueue(_ cmd: CrewCommand) -> Error? {
        guard !cmd.brief.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return ControlChannelWriteError.refusedHere("命令正文为空")
        }
        guard let data = try? JSONEncoder().encode(cmd) else {
            return ControlChannelWriteError.refusedHere("命令编码失败")
        }
        let url = directory.appendingPathComponent("\(cmd.crewId).\(cmd.id)\(Self.cmdSuffix)")
        do {
            try data.write(to: url, options: .atomic)
            return nil
        } catch {
            return error
        }
    }

    /// 排空全部待执行命令（目录监听 tick 调）：返回列表并删除对应文件。
    /// 执行由调用方做（CrewStore）。跨 crew 一并返回，调用方按 crewId 分派。
    /// 解码不了的命令文件不再 continue 滞留（每 tick 重复解码 + 静默丢命令）——
    /// 归档 + 白板回执，见 `quarantineBadFile`。
    func drainCommands() -> [CrewCommand] {
        guard let files = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil) else { return [] }
        var out: [CrewCommand] = []
        // 排空即删 —— 所以「这一批到底是哪几个文件」只有此刻拿得到。见
        // `CrewCommandDrainLog`：它是用来划「通道 / 消费」的界的。
        var drained: [CrewCommandDrainLog.Entry] = []
        for url in files where url.lastPathComponent.hasSuffix(Self.cmdSuffix) {
            // 读不出来（fd 打满 / 权限抖动 / IO）→ 原地留着，下个 tick 再来 ——
            // **绝不**当成损坏去归档（2026-08-12 P0 的不变式，见 MultiProcessJSONStore ④）。
            guard let bytes = try? MultiProcessJSONStore.readDataIfExists(at: url) else { continue }
            guard let cmd = try? JSONDecoder().decode(CrewCommand.self, from: bytes) else {
                // 归档前复验一次：重读 + 重解。复验读不出来 → 也留着不动。
                guard let rebytes = try? MultiProcessJSONStore.readDataIfExists(at: url) else {
                    continue
                }
                guard let cmd = try? JSONDecoder().decode(CrewCommand.self, from: rebytes) else {
                    quarantineBadFile(url, what: "机长命令")
                    continue
                }
                out.append(cmd)
                drained.append(.init(url: url, command: cmd))
                try? FileManager.default.removeItem(at: url)
                continue
            }
            out.append(cmd)
            drained.append(.init(url: url, command: cmd))
            try? FileManager.default.removeItem(at: url)
        }
        // **空排空不写** —— 目录监听每个 tick 都会调一次，每次都写会把日志淹掉，
        // 而淹掉的日志和没有日志是同一个东西。
        if !drained.isEmpty { CrewCommandDrainLog.sink(CrewCommandDrainLog.line(entries: drained)) }
        return out.sorted { $0.ts < $1.ts }
    }

    /// drain 遇到解码不了的文件：归档成 `<file>.corrupt-<unix毫秒>`（归档挪不动就
    /// 删除 —— 两条路都不让它滞留、每 tick 重复解码）+ 往该 crew 白板落一条系统
    /// 回执 —— 发起方（机长/session）以为命令已发出,静默丢必须有人看见。
    /// crewId 取文件名首段（本目录所有控制文件都是 `<crewId>.…` 命名,crewId 无点）。
    private func quarantineBadFile(_ url: URL, what: String) {
        let archive = MultiProcessJSONStore.quarantine(url)
        if archive == nil { try? FileManager.default.removeItem(at: url) }
        let crewId = url.lastPathComponent.components(separatedBy: ".").first ?? ""
        guard !crewId.isEmpty else { return }
        LocalWhiteboardStore(directory: directory).appendSessionMessage(
            crewId: crewId, sessionId: "system",
            text: "一条\(what)文件损坏，已归档为 "
                + "\(archive?.lastPathComponent ?? "（归档失败，已删除）")（whiteboards 目录）。"
                + "这条命令不会被执行，发起方请重发。",
            senderName: "系统")
    }

    // MARK: - Persistence

    private static let cmdSuffix = ".crewcmd.json"

    private static let respSuffix = ".crewresp.json"

    private static let suffix = ".crewmeta.json"

    private static let attentionSuffix = ".crewattention.json"

    private func fileURL(_ crewId: String) -> URL {
        // crewId 是受控本地 id（"local-"+uuid，无路径分隔符/点），直接当文件名安全。
        directory.appendingPathComponent("\(crewId)\(Self.suffix)")
    }

    private func attentionFileURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId)\(Self.attentionSuffix)")
    }

    private func load(_ crewId: String) -> CrewMetaChange? {
        guard let data = try? Data(contentsOf: fileURL(crewId)) else { return nil }
        return try? JSONDecoder().decode(CrewMetaChange.self, from: data)
    }
}

/// 控制通道自己判定「这次没写」的原因（不是磁盘错误，是参数在这一层就不合法）。
/// 跟真正的 IO 错误分开，是因为调用方该做的事不同：一个改参数重来，一个换条路。
enum ControlChannelWriteError: LocalizedError {
    case refusedHere(String)

    var errorDescription: String? {
        switch self {
        case .refusedHere(let why): return "控制通道拒绝了这条命令：\(why)"
        }
    }
}

/// 一条入队命令的**双份结果**：命令 id（用来 long-poll 应答）+ 这次到底写没写进去。
///
/// 只返回 id 的老形状是这一族缺陷的另一个入口：拿到一个 id 就以为命令排上了，
/// 而它可能压根没落盘 —— 然后 long-poll 会对着一条不存在的命令干等到超时，
/// 超时那句话说的是「app 可能没在跑」，把病因指到完全错的地方。
struct EnqueuedCommand {
    let id: String
    /// nil = 真的写进去了。
    let failure: Error?
}

/// 一条待落地的 crew 元数据变更（目前只有改名）。`ts` = 写入时间（ISO8601），调试用。
struct CrewMetaChange: Codable, Equatable {
    let title: String
    let ts: String
}

/// 一条待落地的旧 attention 文案变更。`reason` 非 nil = 记录，nil = 清除。
struct CrewAttentionChange: Codable, Equatable {
    let reason: String?
    let ts: String
}

/// 一条命令应答（app → helper；inspect/nudge/stop 的结果回传）。
struct CrewCommandResponse: Codable, Equatable {
    let text: String
    let ts: String
}

/// 一条待执行的机长命令（起 session / 建子 crew / 组织架构调整等）。`ts` 用于
/// drain 后按写入序执行。
struct CrewCommand: Codable, Equatable {
    let id: String
    let crewId: String
    /// "start_session" | "handoff_captain" | "create_child_crew" | "set_profile" | "schedule_wakeup" |
    /// "listen" | "crew_message" | "inspect_session" | "nudge_session" | "stop_session" |
    /// "adopt_crew" | "release_crew" | "create_parent_crew" | "adopt_parent" | "change_workdir"
    let kind: String
    let brief: String
    let runner: String?     // start_session：nil=随 crew captainAgentKind；"claude"/"codex" 覆盖
    let isolation: Bool?    // start_session：nil/false=共享 crew 目录；true=独立 worktree
    let title: String?      // create_child_crew：nil=地名占位
    /// start_session / set_profile：模型别名/slug（claude 如 "opus"/"sonnet"，codex 如
    /// "gpt-5-codex"）。nil=对应 runner 默认。optional → 旧队列文件缺键向后兼容。
    var model: String? = nil
    /// start_session / set_profile：thinking effort（claude: low/medium/high/xhigh/max；
    /// codex: minimal/low/medium/high）。nil=runner 默认。
    var effort: String? = nil
    /// set_profile / schedule_wakeup：目标（=发起方自己的）session id。
    var sessionId: String? = nil
    /// schedule_wakeup：唤醒时刻（ISO8601）。
    var fireAt: String? = nil
    /// schedule_wakeup：唤醒时带回的备注。
    var note: String? = nil
    /// stop_session：发起终止的机长 session id（目标仍放 sessionId）。
    var requesterSessionId: String? = nil
    /// crew_message：投递方向 "to_parent" | "to_child"（targetHint 复用 `title` 字段）。
    var direction: String? = nil
    /// listen：只听这些发送者（"human"/"captain"/session id 或其前缀/显示名）。nil=全部。
    var senders: [String]? = nil
    /// listen：true=停止收听（此时 fireAt/senders 忽略）。
    var off: Bool? = nil
    /// schedule_wakeup 的**督办租约**变体（#107）：这条唤醒替哪条计划盯着。
    /// nil = 普通定时唤醒。两者共用 `kind: "schedule_wakeup"` 这一条命令通道。
    var planNumber: Int? = nil
    /// 督办的基础间隔（秒），退避在它上面翻倍。
    var leaseBaseSeconds: Double? = nil
    /// change_workdir：目标工作目录的绝对路径。
    var path: String? = nil
    /// change_workdir：连同子 crew 一起迁（nil = true）。
    var includeChildren: Bool? = nil
    /// change_workdir：true=真执行；nil/false=只出预览（dry-run）。
    var confirm: Bool? = nil
    /// handoff_captain：显式目标 crew；nil = 保持历史语义，只操作 `crewId` 本身。
    var targetCrewId: String? = nil
    let ts: String
}

/// **一次排空一行日志** —— 用来把「命令通道」和「消费方」这两段分开。
///
/// ## 它不是用来抓某个 bug 的
///
/// 2026-09-04 有一次「投 2 条命令、见到 3 个 session」的异常，判定它花了**三趟受控
/// 实验**：命令文件**排空即删**、daemon 日志只记启动 / 连接 / 退出、**不记排空** ——
/// 等人发现异常时，能判定它的东西已经不存在了。当时唯一站得住的结论是「查不了」。
///
/// 真相后来查明是**消费方**把第一条处理了两遍（`@Published` 数组当队列用）。
/// **有这行日志的话，它会显示「一次排空、两个不同文件名」——当场把嫌疑从通道摘
/// 出去、直接指向消费方**，第一趟实验就能收尾。
///
/// ## 为什么必须带文件名（少了它这行就白记）
///
/// 两个候选解释在日志里长得不一样：**「多写了一个文件」是两个不同文件名，
/// 「同一个文件被排空两次」是同一个文件名出现两次**。只记命令 id 的话，重复写入
/// 若恰好用了不同 uuid，两种解释**仍然分不开**。
enum CrewCommandDrainLog {

    /// 这一批里的一条。
    struct Entry {
        let fileName: String
        let id: String
        let kind: String
        let crewId: String

        init(fileName: String, id: String, kind: String, crewId: String) {
            self.fileName = fileName
            self.id = id
            self.kind = kind
            self.crewId = crewId
        }

        init(url: URL, command: CrewCommand) {
            self.init(fileName: url.lastPathComponent, id: command.id,
                      kind: command.kind, crewId: command.crewId)
        }
    }

    /// 一次排空一行的文案。纯函数。
    static func line(entries: [Entry]) -> String {
        let detail = entries
            .map { "[\($0.kind) \($0.crewId) \($0.fileName)]" }
            .joined(separator: " ")
        return "排空机长命令 \(entries.count) 条：\(detail)"
    }

    /// 日志去处。**daemon 在启动时把它接到自己那份日志文件上**（`~/Library/Logs/
    /// PendingCrew/daemon-*.log` 才是人真的会去翻的观察窗；`NSLog` 进系统日志，
    /// 落在那儿等于写了没人看）。默认值给 GUI / 单测用。
    nonisolated(unsafe) static var sink: (String) -> Void = defaultSink

    static let defaultSink: (String) -> Void = { NSLog("[PendingCrew] %@", $0) }
}
