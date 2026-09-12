import Foundation

/// agent 侧会话号账本（人类 Todo #28：session 关掉再点恢复要真的接上原对话）。
///
/// 我们自己的 `localSessionId` 一直是稳定的（身份/成员登记/白板游标都挂在它上面），
/// 但 agent 那侧另有一套会话号 —— claude 的 session uuid、codex 的 threadId —— 此前
/// 从没落过盘，所以重启时无从传给 `--resume` / `thread/resume`。这个 store 就是那本账：
/// 按 `crewId + sessionId` 记一条，重启时查出来带上。
///
/// 走 `MultiProcessJSONStore` 基座三件套（flock / 逐条 lenient / corrupt 归档 fail-loud），
/// 与 `LocalWakeupStore` 等同一套写法与硬化策略。单文件跨 crew 共用（行内带 crewId）。
/// **自包含 Foundation**（编进 PendingCrewTests bundle 单测）。
final class LocalAgentSessionStore: @unchecked Sendable {
    /// 一条会话号记录。`kind` 是 runner 名 —— 写进来的是
    /// `LocalCodingAgentKind.rawValue`（`claude_code` / `codex`），不是 `claude` ——
    /// `latestCaptainRecord(crewId:kind:)` 按它**严格过滤**（crew 换过 runner 时，拿
    /// codex 的 threadId 去喂 claude 的 `--resume` 是纯粹的错），所以它**不只是留痕**，
    /// 写入方别改成别的字面量。
    /// （它原本还有第二个消费者 `WorkdirMigrationPlan`「这条会话要不要搬」——
    /// 会话搬运于 2026-08-26 整段删除，那个消费者没了，这条约束仍然承重。）
    ///
    /// `updatedAt` = ISO8601 最近一次写入时刻。**Todo #68 之后它是承重数据**：机长
    /// 续跑要靠它在同 crew 的多条 `captain-*` 记录里挑最新的一条。所以比大小一律走
    /// `CrewTimestamp.parse` 解析后的 `Date`，不许按字典序比 —— 现在写出去的都是
    /// `Z` 结尾的 UTC，但只要有一天混进带时区偏移的串，字典序就会排错。
    ///
    /// `workingDirectory` = 起这个 session 时进程**真正跑在**哪个目录（isolation
    /// worktree 的成员记的就是 worktree 路径）。**它只回答「当初在哪儿跑」，不回答
    /// 「日志在哪儿」**（Todo #68 实测：claude `--resume` 按会话号找全盘，跟目录无关）。
    /// 用途只有一个：@ 唤醒一个退出的成员时，让新进程回到它自己的那个目录，而不是
    /// 一律拉回 crew 共享目录 —— 后者是 `restartMember` 一直以来的行为，注释里自己
    /// 也承认「不恢复」。**旧记录没有这个字段 → 解成 nil，按老行为回落**（Optional
    /// 让整份账本照常解码，不会有一条被 lenient 逐条解码丢掉）。
    struct Record: Codable, Equatable {
        let crewId: String
        let sessionId: String
        var kind: String
        var agentSessionId: String
        var updatedAt: String
        /// 本特性上线前写的记录没有这个字段 —— 解成 nil，调用方按「不知道」处理。
        var workingDirectory: String?
        /// **这个 session 被明确指定过的模型 / effort**（2026-09-12，Todo #146）。
        ///
        /// 在这之前，切模型只改内存里的 `run.model`，一个字都不落盘。于是
        /// `restartMember` 用 `model: nil` 重起、`startCaptain` 也不带，
        /// **session 被 @ 唤醒拉起来的那一刻就悄悄回到默认模型** —— codex 的默认
        /// 恰好是最贵的那个（`~/.codex/config.toml` 顶层 model）。人看到的是
        /// 「切了，回执说成功了，过一会儿又变回去了」，而没有任何地方说过它退回去。
        ///
        /// 只记**明确指定过的**值：没指定过就是 nil，重启时照旧走默认解析 ——
        /// 把一次默认解析的结果记成「这个 session 的选择」会让以后改默认对老 session
        /// 无效，那是另一种静默。
        var model: String?
        var effort: String?
    }

    static let shared = LocalAgentSessionStore()

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// 记下（或更新）某个 session 的 agent 侧会话号。同 crew+session 覆盖旧值 ——
    /// 一个 localSessionId 同时只可能有一个活着的 agent 会话。
    ///
    /// `workingDirectory` 传 nil 时**保留旧值不清空**：调用方「这次不知道」不等于
    /// 「这个成员没有工作目录」，把已知的信息覆盖成未知是净损失。
    func record(crewId: String, sessionId: String, kind: String, agentSessionId: String,
                workingDirectory: String? = nil,
                model: String? = nil, effort: String? = nil,
                now: Date = Date(), onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) {
        let trimmed = agentSessionId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        let workdir = workingDirectory?.trimmingCharacters(in: .whitespacesAndNewlines)
        withFileLock {
            var rows = loadLocked(onIncident: onIncident)
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(
                rows, at: fileURL) else { return }
            let stamp = ISO8601DateFormatter().string(from: now)
            if let i = rows.firstIndex(where: { $0.crewId == crewId && $0.sessionId == sessionId }) {
                rows[i].kind = kind
                rows[i].agentSessionId = trimmed
                rows[i].updatedAt = stamp
                if let workdir, !workdir.isEmpty { rows[i].workingDirectory = workdir }
                // 同 workingDirectory 的规矩：传 nil = 这次不知道，**不清空已知值**。
                if let model, !model.isEmpty { rows[i].model = model }
                if let effort, !effort.isEmpty { rows[i].effort = effort }
            } else {
                rows.append(Record(crewId: crewId, sessionId: sessionId, kind: kind,
                                   agentSessionId: trimmed, updatedAt: stamp,
                                   workingDirectory: (workdir?.isEmpty == false) ? workdir : nil,
                                   model: (model?.isEmpty == false) ? model : nil,
                                   effort: (effort?.isEmpty == false) ? effort : nil))
            }
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
        }
    }

    /// 切换成功之后把档位记下来（Todo #146）。**只更新已存在的那一行** ——
    /// 切换成功的前提是 session 正在跑，而正在跑就意味着启动时那条记录已经写过了
    /// （claude 在拿到会话号那一刻写、codex 在握手拿到 threadId 那一刻写）。
    /// 查不到行就什么都不做：与其凭空造一条没有会话号的记录，不如让这次不记 ——
    /// 一条会话号为空的记录会把「续跑哪一轮」这个判断带偏。
    ///
    /// 两个档位各自独立：只切了模型就只传模型，另一个传 nil（不清空）。
    func recordProfile(crewId: String, sessionId: String,
                       model: String? = nil, effort: String? = nil,
                       now: Date = Date(),
                       onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) {
        guard model?.isEmpty == false || effort?.isEmpty == false else { return }
        withFileLock {
            var rows = loadLocked(onIncident: onIncident)
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(
                rows, at: fileURL) else { return }
            guard let i = rows.firstIndex(where: {
                $0.crewId == crewId && $0.sessionId == sessionId }) else { return }
            if let model, !model.isEmpty { rows[i].model = model }
            if let effort, !effort.isEmpty { rows[i].effort = effort }
            rows[i].updatedAt = ISO8601DateFormatter().string(from: now)
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
        }
    }

    /// 某个 crew 的全部记录，按 sessionId 索引。**一次取完**：机长点名要给每一行
    /// 附产出证据（Todo #107），逐行现查等于逐行上一次文件锁。
    /// 查不到的 sessionId 在这张表里就是**不存在**，调用方必须自己决定那叫什么 ——
    /// 别在这里替它编一个默认值。
    func records(crewId: String,
                 onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> [String: Record] {
        withFileLock {
            var out: [String: Record] = [:]
            for r in loadLocked(onIncident: onIncident) where r.crewId == crewId {
                out[r.sessionId] = r
            }
            return out
        }
    }

    /// 查某个 session 记着的 agent 侧会话号（没有 → nil，调用方按「新开一轮」处理）。
    func agentSessionId(crewId: String, sessionId: String,
                        onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> String? {
        withFileLock {
            loadLocked(onIncident: onIncident)
                .first { $0.crewId == crewId && $0.sessionId == sessionId }?
                .agentSessionId
        }
    }

    /// 查某个 session 的整条记录（会话号 + 当初跑在哪儿）。没有 → nil。
    func record(crewId: String, sessionId: String,
                onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> Record? {
        withFileLock {
            loadLocked(onIncident: onIncident)
                .first { $0.crewId == crewId && $0.sessionId == sessionId }
        }
    }

    /// 某个 crew **最近一条机长记录**（Todo #68 第 2 件：机长也要能续跑）。
    ///
    /// 机长的 localSessionId 每次启动都新造（`CrewSessionRunner.startCaptain` 里
    /// `"captain-" + uuid8`，全仓只有那一处铸这种 id），所以按 `crewId + sessionId`
    /// 是查不到上一任机长的。但**每个 crew 同时只允许一个机长**（`startCaptain` 开头
    /// 那道 guard + `runs.removeAll { role == .captain }`），于是「本 crew 最近一条
    /// `captain-*` 记录」是无歧义的 —— 不必去动 id 的生成方式。
    ///
    /// **不动 id 还顺手躲开一个硬伤**：复用旧 localSessionId 就等于复用旧的白板读游标
    /// （`<crewId>.<sessionId>.cursor`），机长隔几天醒来会被一次性灌进几百条未读
    /// （实测父群白板 993 条、每天 60~80 条）。新 id → 游标是 `.absent` → 只投最近
    /// 一批，问题自己就没了。
    ///
    /// `kind` 必须传并且严格过滤：crew 换过 runner（claude ↔ codex）时，拿 codex 的
    /// threadId 去喂 claude 的 `--resume` 是纯粹的错。
    ///
    /// **这本账按 crew 只增不减**（每起一次机长追加一行，本机已 139 条）。今天无害，
    /// 但从 Todo #68 起它是承重数据 —— 别以为它是有界的；真要清理是另一件事。
    func latestCaptainRecord(crewId: String, kind: String,
                             onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> Record? {
        withFileLock {
            loadLocked(onIncident: onIncident)
                .filter { $0.crewId == crewId && $0.kind == kind
                    && $0.sessionId.hasPrefix(Self.captainSessionIdPrefix) }
                .max { lhs, rhs in
                    (CrewTimestamp.parse(lhs.updatedAt) ?? .distantPast)
                        < (CrewTimestamp.parse(rhs.updatedAt) ?? .distantPast)
                }
        }
    }

    /// 机长 localSessionId 的前缀 —— 与 `CrewSessionRunner.startCaptain` 那一处
    /// （全仓唯一的铸造点）同一个字面量。改那边就得改这边。
    static let captainSessionIdPrefix = "captain-"

    /// 全部记录（排查/测试用）。
    func list(onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> [Record] {
        withFileLock { loadLocked(onIncident: onIncident) }
    }

    // MARK: - Persistence（基座三件套）

    private var fileURL: URL { directory.appendingPathComponent("agent-sessions.json") }

    private func withFileLock<T>(_ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("agent-sessions.lock"), body)
    }

    private func loadLocked(onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void) -> [Record] {
        MultiProcessJSONStore.loadRowsLocked(Record.self, at: fileURL, onIncident: onIncident)
    }
}
