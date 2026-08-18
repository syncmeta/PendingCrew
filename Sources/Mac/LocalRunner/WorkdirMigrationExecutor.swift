#if os(macOS)
import Foundation
import TOMLKit

/// `WorkdirMigrationPlan` 的**执行层** —— 薄薄一层，照单干活，全程 fail-loud。
///
/// 三条纪律：
/// 1. **先备份**：动 `~/.claude.json` / `~/.codex/config.toml` / `local-crews.json` 之前，
///    整份拷进一个带时间戳的备份目录。备份失败就一步都不走。
/// 2. **顺序**：信任/权限 → 记忆（复制）→ 会话（移动）→ 最后才改 crew 字段。中途炸了
///    crew 还指着旧目录，成员照旧接得回上下文。
/// 3. **不吞错**：任一步失败立刻停，回执里说清停在哪一步、之前已经做了什么。
enum WorkdirMigrationExecutor {

    // MARK: - 回执

    struct Failure: Equatable {
        /// 停在哪一步（人能看懂的短语，如「搬 claude 会话 <id>」）。
        var step: String
        var message: String
    }

    struct Receipt: Equatable {
        var backupDirectory: String = ""
        /// 搬成功的 claude 会话号。
        var movedTranscripts: [String] = []
        var movedSidecars: [String] = []
        var copiedMemoryFiles: [String] = []
        var claudeSettingsKeysCopied: [String] = []
        var codexTrustCopied: Bool = false
        var crewsUpdated: [WorkdirMigrationPlan.CrewRef] = []
        var skips: [WorkdirMigrationPlan.Skip] = []
        /// 做了、但**没能确认落住**的事（如 claude.json 被别的进程覆盖）。
        /// 绝不当成功report —— 人得知道第一次进新目录可能还要手点一次信任框。
        var warnings: [String] = []
        /// 这次是清扫（补搬上次留下的尾巴）而非首迁。
        var isSweep: Bool = false
        /// 非 nil = 中途停了，上面那些是「已经做了的」。
        var failure: Failure?

        var succeeded: Bool { failure == nil }
    }

    // MARK: - 探针（规划层唯一接触现实的入口）

    static func probe(home: URL, fileManager fm: FileManager = .default)
        -> WorkdirMigrationPlan.Probe {
        let claudeProjects = loadClaudeProjects(home: home)
        let codexTrust = loadCodexTrustLevels(home: home)
        return WorkdirMigrationPlan.Probe(
            pathExists: { fm.fileExists(atPath: $0) },
            isDirectory: { path in
                var isDir: ObjCBool = false
                return fm.fileExists(atPath: path, isDirectory: &isDir) && isDir.boolValue
            },
            isWritable: { fm.isWritableFile(atPath: $0) },
            listFiles: { dir in relativeFilePaths(under: dir, fileManager: fm) },
            claudeProjectSettings: { path in
                guard let entry = claudeProjects[path] else {
                    return WorkdirMigrationPlan.ClaudeProjectSettings()
                }
                let keys = WorkdirMigrationPlan.claudeSettingsKeys
                    .filter { WorkdirMigrationPlan.isMeaningful(entry[$0]) }
                return WorkdirMigrationPlan.ClaudeProjectSettings(
                    exists: true, meaningfulKeys: Set(keys))
            },
            codexTrustLevel: { codexTrust[$0] })
    }

    /// 目录下**递归**的普通文件相对路径（目录不存在 → 空）。隐藏文件也算 ——
    /// 记忆目录里什么都可能有，别自作主张漏掉。
    static func relativeFilePaths(under dir: String, fileManager fm: FileManager = .default)
        -> [String] {
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: dir, isDirectory: &isDir), isDir.boolValue else { return [] }
        let base = URL(fileURLWithPath: dir, isDirectory: true)
        guard let e = fm.enumerator(at: base, includingPropertiesForKeys: [.isRegularFileKey],
                                    options: []) else { return [] }
        var out: [String] = []
        for case let url as URL in e {
            let isFile = (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile
            guard isFile == true else { continue }
            let full = url.standardizedFileURL.path
            let prefix = base.standardizedFileURL.path + "/"
            guard full.hasPrefix(prefix) else { continue }
            out.append(String(full.dropFirst(prefix.count)))
        }
        return out
    }

    // MARK: - 读 ~/.claude.json / ~/.codex/config.toml

    static func claudeJSONURL(home: URL) -> URL { home.appendingPathComponent(".claude.json") }
    static func codexConfigURL(home: URL) -> URL {
        home.appendingPathComponent(".codex/config.toml")
    }

    /// `~/.claude.json` 的 `projects` 表（读不到 / 解不开 → 空，规划层会当「源没有条目」）。
    static func loadClaudeProjects(home: URL) -> [String: [String: Any]] {
        guard let data = try? Data(contentsOf: claudeJSONURL(home: home)),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let projects = root["projects"] as? [String: Any] else { return [:] }
        var out: [String: [String: Any]] = [:]
        for (k, v) in projects {
            if let entry = v as? [String: Any] { out[k] = entry }
        }
        return out
    }

    /// `~/.codex/config.toml` 的 `[projects."<路径>"] trust_level`。
    static func loadCodexTrustLevels(home: URL) -> [String: String] {
        guard let text = try? String(contentsOf: codexConfigURL(home: home), encoding: .utf8),
              let table = try? TOMLTable(string: text),
              let projects = table["projects"]?.table else { return [:] }
        var out: [String: String] = [:]
        for key in projects.keys {
            if let level = projects[key]?.table?["trust_level"]?.string { out[key] = level }
        }
        return out
    }

    // MARK: - 执行

    /// 照计划执行。**只在 `plan.isExecutable` 时调**（有 blocker 一律不执行）。
    ///
    /// - Parameters:
    ///   - backupDirectory: 这次迁移的备份目录（调用方带时间戳建，如
    ///     `…/PendingCrew/backups/workdir-migration-<stamp>`）。
    ///   - extraBackupFiles: 额外要备份的文件（`local-crews.json`）。
    ///   - applyCrewWorkingDirectory: 把 crew 字段改掉（`LocalCrewStore.setWorkingDirectory`）。
    static func execute(
        plan: WorkdirMigrationPlan.Plan,
        home: URL,
        backupDirectory: URL,
        extraBackupFiles: [URL] = [],
        fileManager fm: FileManager = .default,
        applyCrewWorkingDirectory: (String, String) throws -> Void
    ) -> Receipt {
        var receipt = Receipt(backupDirectory: backupDirectory.path, skips: plan.skips,
                              isSweep: plan.isSweep)

        guard plan.blockers.isEmpty else {
            receipt.failure = Failure(step: "预检",
                                      message: "计划里还有 \(plan.blockers.count) 条拦路条件，拒绝执行。")
            return receipt
        }

        // ── 0. 备份。备份不成，一步都不走。
        do {
            try fm.createDirectory(at: backupDirectory, withIntermediateDirectories: true)
            for src in [claudeJSONURL(home: home), codexConfigURL(home: home)] + extraBackupFiles
            where fm.fileExists(atPath: src.path) {
                let dst = backupDirectory.appendingPathComponent(src.lastPathComponent)
                if fm.fileExists(atPath: dst.path) { try fm.removeItem(at: dst) }
                try fm.copyItem(at: src, to: dst)
            }
        } catch {
            receipt.failure = Failure(step: "备份", message: "\(error.localizedDescription)")
            return receipt
        }

        for action in plan.actions {
            do {
                switch action {
                case .copyClaudeProjectSettings(let from, let to, let keys):
                    let (confirmed, tries) = try copyClaudeProjectSettingsVerified(
                        home: home, from: from, to: to, keys: keys)
                    receipt.claudeSettingsKeysCopied = confirmed
                    let lost = keys.filter { !confirmed.contains($0) }
                    if !lost.isEmpty {
                        receipt.warnings.append(
                            "claude 的目录信任/权限有 \(lost.joined(separator: "、")) 没落住"
                            + "（写了 \(tries) 次，读回来还是没有 —— `~/.claude.json` 被别的 claude 进程覆盖了）。"
                            + "第一次进新目录可能要人手点一次「信任这个文件夹」。")
                    }

                case .copyCodexTrust(_, let to, let trustLevel):
                    try addCodexTrust(home: home, path: to, trustLevel: trustLevel)
                    receipt.codexTrustCopied = true

                case .copyClaudeMemoryFile(let rel, let from, let to):
                    try ensureParentDirectory(of: to, fileManager: fm)
                    guard !fm.fileExists(atPath: to) else { break } // 竞态兜底：绝不覆盖
                    try fm.copyItem(atPath: from, toPath: to)
                    receipt.copiedMemoryFiles.append(rel)

                case .moveClaudeTranscript(let id, _, let from, let to):
                    try ensureParentDirectory(of: to, fileManager: fm)
                    guard !fm.fileExists(atPath: to) else { break }
                    try fm.moveItem(atPath: from, toPath: to)
                    receipt.movedTranscripts.append(id)

                case .moveClaudeTranscriptSidecar(let id, _, let from, let to):
                    try ensureParentDirectory(of: to, fileManager: fm)
                    guard !fm.fileExists(atPath: to) else { break }
                    try fm.moveItem(atPath: from, toPath: to)
                    receipt.movedSidecars.append(id)

                case .setCrewWorkingDirectory(let crewId, let title, _, let to):
                    try applyCrewWorkingDirectory(crewId, to)
                    receipt.crewsUpdated.append(
                        WorkdirMigrationPlan.CrewRef(id: crewId, title: title))
                }
            } catch {
                receipt.failure = Failure(step: stepLabel(action),
                                          message: "\(error.localizedDescription)")
                return receipt
            }
        }
        return receipt
    }

    static func stepLabel(_ action: WorkdirMigrationPlan.Action) -> String {
        switch action {
        case .copyClaudeProjectSettings: return "复制 claude 的目录信任/权限记录"
        case .copyCodexTrust: return "补 codex 的目录信任记录"
        case .copyClaudeMemoryFile(let rel, _, _): return "复制记忆文件 \(rel)"
        case .moveClaudeTranscript(let id, let name, _, _): return "搬「\(name)」的会话 \(id)"
        case .moveClaudeTranscriptSidecar(let id, let name, _, _):
            return "搬「\(name)」会话 \(id) 的附属目录"
        case .setCrewWorkingDirectory(_, let title, _, _): return "改「\(title)」的工作目录"
        }
    }

    private static func ensureParentDirectory(of path: String, fileManager fm: FileManager) throws {
        let parent = (path as NSString).deletingLastPathComponent
        guard !parent.isEmpty else { return }
        try fm.createDirectory(atPath: parent, withIntermediateDirectories: true)
    }

    // MARK: - ~/.claude.json

    /// 把 `from` 条目里的 `keys` 补进 `to` 条目（**只补，不覆盖已有实质值**，旧条目原样留着）。
    ///
    /// **已知窄口**：`~/.claude.json` 是全机 claude 共用的一份，别的 crew 里正在跑的
    /// claude 也会写它。这里是「读—改—写」，理论上能和它们的写撞成丢更新。缓解是
    /// ①迁移前会拒绝本 crew 有 session 在跑；②整份文件先备份（撞了能原样找回）。
    /// 真要根治得等 claude 那边给出带锁的写入通道 —— 我们这侧没有可用的锁。
    static func copyClaudeProjectSettings(home: URL, from: String, to: String,
                                          keys: [String]) throws {
        let url = claudeJSONURL(home: home)
        let data = try Data(contentsOf: url)
        guard var root = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw MigrationError("~/.claude.json 不是一个 JSON 对象，拒绝改写。")
        }
        guard var projects = root["projects"] as? [String: Any] else {
            throw MigrationError("~/.claude.json 里没有 projects 表，拒绝改写。")
        }
        guard let source = projects[from] as? [String: Any] else {
            throw MigrationError("~/.claude.json 里找不到旧路径的条目：\(from)")
        }
        var target = (projects[to] as? [String: Any]) ?? [:]
        for key in keys {
            guard let value = source[key], WorkdirMigrationPlan.isMeaningful(value) else { continue }
            guard !WorkdirMigrationPlan.isMeaningful(target[key]) else { continue }
            target[key] = value
        }
        projects[to] = target
        root["projects"] = projects
        let out = try JSONSerialization.data(
            withJSONObject: root, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        let perms = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.posixPermissions]
        try out.write(to: url, options: .atomic)
        // 原文件是 600；atomic 替换会带默认权限，把它按原样恢复（别把凭证类文件放宽）。
        if let perms {
            try? FileManager.default.setAttributes([.posixPermissions: perms], ofItemAtPath: url.path)
        }
    }

    /// 写 + **读回校验**，没落住就重试。返回真正确认落住的键。
    ///
    /// 为什么非要读回来：`~/.claude.json` 是 claude 自己也在写的整份文件（每个 session
    /// 退出都会覆写一遍），本机常年十几个 claude 在跑 —— 我们刚写进去的条目**真的可能
    /// 被别的进程覆盖掉**。这是「读—改—写」这套玩法的固有窄口，我们这侧没有可用的锁，
    /// 只能靠重试 + 如实报（`confirmed` 少于 `keys` 时调用方必须把话说出去，不许默默
    /// 当成功）。
    static func copyClaudeProjectSettingsVerified(
        home: URL, from: String, to: String, keys: [String],
        attempts: Int = 3, waitBetween: TimeInterval = 0.35
    ) throws -> (confirmed: [String], attemptsUsed: Int) {
        var lastError: Error?
        for attempt in 1...max(1, attempts) {
            do {
                try copyClaudeProjectSettings(home: home, from: from, to: to, keys: keys)
            } catch {
                // 源条目没了之类的硬错误，重试也没用 —— 直接抛。
                throw error
            }
            let landed = loadClaudeProjects(home: home)[to] ?? [:]
            let confirmed = keys.filter { WorkdirMigrationPlan.isMeaningful(landed[$0]) }
            if confirmed.count == keys.count { return (confirmed, attempt) }
            lastError = MigrationError("写进去的键没全部落住（第 \(attempt) 次）")
            if attempt < max(1, attempts) { Thread.sleep(forTimeInterval: waitBetween) }
        }
        let landed = loadClaudeProjects(home: home)[to] ?? [:]
        let confirmed = keys.filter { WorkdirMigrationPlan.isMeaningful(landed[$0]) }
        _ = lastError
        return (confirmed, max(1, attempts))
    }

    // MARK: - ~/.codex/config.toml

    /// 给新路径补一条 `[projects."<路径>"] trust_level`（旧的留着）。
    ///
    /// 走 TOMLKit 正经解析 → 改 → 序列化，**再回读比对**：新文件解出来必须恰好等于
    /// 「原内容 + 这一条」，多一分少一分都不写。字符串拼接改 TOML 是禁的。
    static func addCodexTrust(home: URL, path: String, trustLevel: String) throws {
        let url = codexConfigURL(home: home)
        let text = try String(contentsOf: url, encoding: .utf8)
        let table = try TOMLTable(string: text)
        guard let beforeJSON = jsonObject(table.convert(to: .json)) as? [String: Any] else {
            throw MigrationError("~/.codex/config.toml 解析后拿不到对象，拒绝改写。")
        }
        let projects = table["projects"]?.table ?? TOMLTable()
        if projects[path] != nil {
            throw MigrationError("~/.codex/config.toml 里新路径已有条目，拒绝覆盖：\(path)")
        }
        projects[path] = TOMLTable(["trust_level": trustLevel])
        table["projects"] = projects

        let rendered = table.convert(to: .toml)
        // 回读校验：解得开 + 内容恰好是「原来的 + 这一条」。
        let reparsed = try TOMLTable(string: rendered)
        guard let afterJSON = jsonObject(reparsed.convert(to: .json)) as? [String: Any] else {
            throw MigrationError("改写后的 config.toml 回读失败，已放弃改写（原文件未动）。")
        }
        var expected = beforeJSON
        var expectedProjects = (beforeJSON["projects"] as? [String: Any]) ?? [:]
        expectedProjects[path] = ["trust_level": trustLevel]
        expected["projects"] = expectedProjects
        guard NSDictionary(dictionary: afterJSON).isEqual(to: expected) else {
            throw MigrationError("改写后的 config.toml 与预期不一致，已放弃改写（原文件未动）。")
        }
        try Data(rendered.utf8).write(to: url, options: .atomic)
    }

    private static func jsonObject(_ text: String) -> Any? {
        try? JSONSerialization.jsonObject(with: Data(text.utf8))
    }

    struct MigrationError: LocalizedError, Equatable {
        let message: String
        init(_ message: String) { self.message = message }
        var errorDescription: String? { message }
    }

    // MARK: - 回执文案（纯函数，可单测）

    /// 迁完往群里发的那条。**如实**：搬了多少、什么没搬、停在哪一步、什么没落住。
    static func receiptText(_ r: Receipt, newWorkdir: String) -> String {
        var lines: [String] = []
        if let failure = r.failure {
            lines.append("**迁移工作目录：中途停了。** 停在「\(failure.step)」：\(failure.message)")
        } else if r.isSweep {
            lines.append("**清扫完成**（补搬上一轮留下的尾巴）。目录 `\(newWorkdir)`")
        } else {
            lines.append("**迁移工作目录：完成。** 新目录 `\(newWorkdir)`")
        }
        let crews = r.crewsUpdated.map { "「\($0.title)」" }.joined(separator: "、")
        lines.append("- 改了工作目录的 crew：\(r.crewsUpdated.isEmpty ? "无" : crews)")
        lines.append("- claude 会话搬过去 \(r.movedTranscripts.count) 个"
            + (r.movedSidecars.isEmpty ? "" : "（含 \(r.movedSidecars.count) 个附属目录）"))
        lines.append("- claude 项目记忆复制 \(r.copiedMemoryFiles.count) 个文件（旧目录原样留着，别的 crew 还在用）")
        lines.append("- claude 目录信任/权限："
            + (r.claudeSettingsKeysCopied.isEmpty
               ? "没补（源没有可搬的，或新路径已经有了）"
               : "补了 \(r.claudeSettingsKeysCopied.joined(separator: "、"))"))
        lines.append("- codex 目录信任：" + (r.codexTrustCopied ? "已补" : "没补（源没有，或新路径已经有了）"))

        let notable = r.skips.compactMap(skipLine)
        if !notable.isEmpty {
            lines.append("- **没搬的**：")
            lines.append(contentsOf: notable.map { "  - " + $0 })
        }
        for w in r.warnings { lines.append("- ⚠️ " + w) }
        lines.append("- 备份在 `\(r.backupDirectory)`")

        let sweep = pendingSweepMembers(r.skips)
        if !sweep.isEmpty {
            lines.append("- **留待清扫**：\(sweep.joined(separator: "、"))"
                + " —— 它们此刻还活着、正在写自己的会话日志，现在搬会搬到半截。"
                + "等它们停了**再调一次** `change_workdir`（同一个路径即可）就会把这批补搬过去。")
        }
        if r.failure != nil {
            lines.append("上面列的是**已经做完**的部分。剩下的没做，crew 字段没改完的仍指着旧目录。")
        } else {
            lines.append(effectiveScopeNote)
        }
        return lines.joined(separator: "\n")
    }

    /// 生效边界 —— 每份回执都要带，免得人以为点完当场全员换了目录。
    static let effectiveScopeNote =
        "**生效边界**：新目录只对**之后新起 / 重启**的 session 生效；此刻还在跑的（包括发起这次"
        + "迁移的机长自己）仍然工作在旧目录里，直到它重启为止。"

    /// 「留待清扫」的成员名（去重，按出现顺序）。
    static func pendingSweepMembers(_ skips: [WorkdirMigrationPlan.Skip]) -> [String] {
        var seen = Set<String>()
        return skips.compactMap { skip in
            guard case .sessionStillLive(_, let name) = skip else { return nil }
            return seen.insert(name).inserted ? name : nil
        }
    }

    // MARK: - 预览文案（dry-run，机长 `change_workdir` 不带 confirm 时返回这个）

    /// 把一份计划渲染成人/机长都能一眼看懂的预览。纯函数，可单测。
    static func previewText(_ plan: WorkdirMigrationPlan.Plan, newWorkdir: String) -> String {
        var lines: [String] = []
        lines.append(plan.isSweep
            ? "**预览（清扫模式）**：目录已经是 `\(newWorkdir)`，这次只补搬上一轮留下的尾巴。"
            : "**预览（还没动手）**：把工作目录改成 `\(newWorkdir)`。")

        if !plan.blockers.isEmpty {
            lines.append("")
            lines.append("**拦路的（不解决就不能执行）**：")
            for b in plan.blockers { lines.append("- " + blockerText(b)) }
        }

        lines.append("")
        lines.append("**会做这些**：")
        lines.append("- 改工作目录的 crew：" + (plan.crews.isEmpty
            ? "无（都已经在新目录上了）"
            : plan.crews.map { "「\($0.title)」" }.joined(separator: "、")))
        lines.append("- 搬 claude 会话：\(plan.claudeTranscriptMoveCount) 个")
        lines.append("- 复制 claude 项目记忆：\(plan.memoryCopyCount) 个文件（旧目录原样留着，别的 crew 还在用）")
        lines.append("- 目录信任 / 工具权限：" + trustSummary(plan))
        if !plan.affectedMembers.isEmpty {
            lines.append("- 涉及的成员：" + plan.affectedMembers.joined(separator: "、"))
        }

        let notable = plan.skips.compactMap(skipLine)
        if !notable.isEmpty {
            lines.append("")
            lines.append("**搬不了 / 不搬的**：")
            lines.append(contentsOf: notable.map { "- " + $0 })
        }

        lines.append("")
        if !plan.isExecutable {
            lines.append(plan.blockers.isEmpty
                ? "**没有要做的动作** —— 已经是这个目录、也没有剩下要搬的东西了。"
                : "**现在不能执行**，先把上面拦路的解决掉。")
        } else {
            lines.append("确认无误就再调一次，带上 `confirm: true`。")
        }
        lines.append(effectiveScopeNote)
        return lines.joined(separator: "\n")
    }

    /// 拦路条件的人话（预览 / UI 共用同一份文案，别写两遍）。
    static func blockerText(_ b: WorkdirMigrationPlan.Blocker) -> String {
        switch b {
        case .emptyNewWorkdir: return "还没给新目录。"
        case .newWorkdirMissing(let p): return "目录不存在：\(p)（不会替你创建）"
        case .newWorkdirNotADirectory(let p): return "这不是一个目录：\(p)"
        case .newWorkdirNotWritable(let p): return "目录不可写：\(p)"
        case .rootCrewNotFound(let id): return "找不到这个 crew：\(id)"
        case .noCrewSelected: return "一个 crew 都没选。"
        case .sessionsBusy(let busy):
            let names = busy.map { "「\($0.displayName)」" }.joined(separator: "、")
            return "这些成员**正在干活**，先让它们停下来再迁：\(names)"
        }
    }

    /// 「信任/权限这一栏」的一句话（预览 / UI 共用）。
    static func trustSummary(_ plan: WorkdirMigrationPlan.Plan) -> String {
        var parts: [String] = []
        for action in plan.actions {
            switch action {
            case .copyClaudeProjectSettings(_, _, let keys):
                parts.append("claude 补 " + keys.joined(separator: "、"))
            case .copyCodexTrust(_, _, let level):
                parts.append("codex 补 trust_level=\(level)")
            default: break
            }
        }
        return parts.isEmpty ? "无需改动（源没有，或新路径已经有了）" : parts.joined(separator: "；")
    }

    /// 只把「人需要知道」的跳过项渲染出来；纯粹「本来就不用搬」的不进回执，免得刷屏。
    static func skipLine(_ skip: WorkdirMigrationPlan.Skip) -> String? {
        switch skip {
        case .transcriptTargetExists(let id, let name, _):
            return "「\(name)」的会话 \(id)：新目录下已有同名文件，跳过没覆盖"
        case .transcriptSourceMissing(let id, let name, _):
            return "「\(name)」的会话 \(id)：旧目录里已经找不到了（它重启会是新脑子）"
        case .memoryTargetExists(let rel, _):
            return "记忆 `\(rel)`：新目录下已有同名文件，跳过没覆盖，要合请自己合"
        case .sessionStillLive(_, let name):
            return "「\(name)」此刻还活着（正在写自己的会话日志），会话记录留待清扫"
        case .memoryDirectoryMissing:
            return "旧目录下没有项目记忆，没什么可复制的"
        case .claudeProjectSettingsSourceEmpty:
            return "claude 那边旧路径没有可搬的信任/权限记录 —— 新目录第一次开 session 可能要按一次信任框"
        case .codexTrustSourceMissing:
            return "codex 那边旧路径没有信任记录 —— 新目录第一次开 codex 可能要按一次信任框"
        case .unknownAgentKind(let kind, let name):
            return "「\(name)」记的是 \(kind) 会话，不认识这种 runner，没动"
        case .crewHasNoWorkingDirectory(_, let title):
            return "「\(title)」原本就没有工作目录，只是把新目录填上"
        case .claudeProjectSettingsAlreadyComplete,
             .codexTrustTargetExists,
             .codexSessionNeedsNoMove,
             .crewAlreadyAtNewWorkdir:
            return nil
        }
    }
}
#endif
