#if os(macOS)
import Foundation

/// 「更改 crew 工作目录（含 agent 上下文迁移）」的**规划层**。
///
/// 背景：crew 的 `workingDirectory` 此前只在建 crew 那一刻写死，仓库一搬家（本项目
/// 2026-08-17 把 PendingCrew 从 PendingBot 的 monorepo 里拆出去）就没有任何入口能改。
/// 而 agent 侧有三样东西**按工作目录路径分家**，光改字段等于把它们丢在旧路径上：
///
/// - `~/.claude.json` 的 `projects["<绝对路径>"]` 记着「这个目录信任过 / 这些工具允许过」。
///   新路径没有条目 → 新目录下第一个 session **挂在**信任提示上（不是弹个框就过去，是
///   进程起来了、不吐第一个字、也不报错，而点名显示为「空闲」，见
///   `CrewSessionsSnapshot.state` 的注释）。这是这整套东西今天存在的头号理由。
/// - `~/.codex/config.toml` 的 `[projects."<绝对路径>"] trust_level` 同理。
/// - claude 的项目记忆在 `~/.claude/projects/<workdir slug>/memory/`。旧工作目录是
///   **多个 crew 共用**的（本机 16 个 crew 都指着同一个 dev 目录），所以只能**复制**
///   不能搬 —— 搬走等于把留守 crew 的记忆偷走。
///
/// **会话日志（`<会话号>.jsonl`）不在上面这张单子里，而且刻意不搬**：2026-08-26 实测
/// （claude 2.1.246）`claude --resume <id>` **不按目录找会话**，它扫整个
/// `~/.claude/projects` 树、只认文件名；把 jsonl 挪到一个跟任何真实路径都对不上的目录，
/// 再换第三个目录 resume 照样接上。官方 `--help` 划的是同一条界：`--continue` 写明
/// *in the current directory*，`--resume` 一个字都没提目录。**所以搬它零功能收益** ——
/// 曾经那套搬运（含「活着的成员留待清扫、停了再调一次补搬」的整个幂等清扫模式）
/// 已于 2026-08-26 整段删除。查实见
/// `docs/internal/2026-08-26-session-resume-workdir-evaluation.md`。
/// 代价只有一条、且是整洁问题不是正确性问题：旧 slug 下会留着一堆不再对应任何真实
/// 目录的文件夹。
///
/// **一条为「机长自己调」而立的规矩**（人面走界面时不成立，机长走 MCP 工具时必成立）：
/// **调用者自己不算拦路**。机长本身就是本 crew 里一个在跑的 session，若沿用
/// 「有 session 在跑就拒绝」，它永远调不动这个工具。所以拦的只是**别人家还在
/// 干活的 worker**；空闲的 worker 和调用者自己都不拦。
///
/// 也因此这个工具**一次做完，没有第二趟** —— 不再有「等谁停了再调一次」这回事。
///
/// 这一层**只算「要做哪些动作」**，不碰文件系统（存在性判定由调用方以闭包喂进来），
/// 照 `AgentSessionResume` 的路子写，供单测直接跑。真正落地在
/// `WorkdirMigrationExecutor`（薄薄一层，照单执行 + 备份 + fail-loud）。
enum WorkdirMigrationPlan {

    // MARK: - 输入

    /// 一条 crew 的迁移相关字段（`LocalCrewStore` 喂进来）。
    struct CrewInput: Equatable {
        let id: String
        let title: String
        let workingDirectory: String?
        let parentCrewIds: [String]

        init(id: String, title: String, workingDirectory: String?,
             parentCrewIds: [String] = []) {
            self.id = id
            self.title = title
            self.workingDirectory = workingDirectory
            self.parentCrewIds = parentCrewIds
        }
    }

    /// 一个还活着的 session。`isWorking` = 正在跑回合（区别于「存活但空闲」）。
    struct RunningSessionInput: Equatable {
        let crewId: String
        let sessionId: String
        let displayName: String
        /// 正在干活 → 拦路（除非它就是调用者）。空闲 → 不拦。
        let isWorking: Bool

        init(crewId: String, sessionId: String, displayName: String, isWorking: Bool = false) {
            self.crewId = crewId
            self.sessionId = sessionId
            self.displayName = displayName
            self.isWorking = isWorking
        }
    }

    /// crew 的最小引用（回执/预览列表用）。
    struct CrewRef: Equatable {
        let id: String
        let title: String
    }

    /// 规划的全部输入。
    struct Inputs {
        /// 全机 crew（算子树 + 查各自旧目录都要它）。
        var crews: [CrewInput]
        /// 从哪个 crew 发起（信任/权限/记忆的来源目录取它的旧工作目录）。
        var rootCrewId: String
        /// 勾了哪些 crew 一起迁（必须含 rootCrewId）。
        var selectedCrewIds: Set<String>
        /// 目标工作目录（用户选的 / 机长填的）。
        var newWorkdir: String
        var runningSessions: [RunningSessionInput]
        /// 谁在调（机长自己的 localSessionId）。它不算拦路；nil = 人面走界面。
        var callerSessionId: String?
        var home: URL

        init(crews: [CrewInput], rootCrewId: String, selectedCrewIds: Set<String>,
             newWorkdir: String,
             runningSessions: [RunningSessionInput] = [],
             callerSessionId: String? = nil, home: URL) {
            self.crews = crews
            self.rootCrewId = rootCrewId
            self.selectedCrewIds = selectedCrewIds
            self.newWorkdir = newWorkdir
            self.runningSessions = runningSessions
            self.callerSessionId = callerSessionId
            self.home = home
        }
    }

    /// `~/.claude.json` 里一个路径条目的快照。**只看「哪几个键有实质值」**，不搬整条 ——
    /// 实测新路径可能已经有条目、但 `hasTrustDialogAccepted` 是 `false`（人没进去过就
    /// 被别的路径写出来了）。整条「已存在就跳过」会把信任弹框留在那儿等着卡人，所以
    /// 按键合并：目标缺的 / 目标是空值的才补，目标已有实质值的一律不动。
    struct ClaudeProjectSettings: Equatable {
        var exists: Bool = false
        /// `claudeSettingsKeys` 里**有实质值**的那些（true / 非空数组 / 非空字典）。
        var meaningfulKeys: Set<String> = []

        init(exists: Bool = false, meaningfulKeys: Set<String> = []) {
            self.exists = exists
            self.meaningfulKeys = meaningfulKeys
        }
    }

    /// 迁移只关心这几个键 —— 「信任过 / 允许过哪些工具 / 挂了哪些 mcp」。
    /// 其余全是统计与缓存（lastCost / lastSessionId / exampleFiles…），跟着搬只会误导。
    static let claudeSettingsKeys: [String] = [
        "hasTrustDialogAccepted",
        "hasCompletedProjectOnboarding",
        "hasClaudeMdExternalIncludesApproved",
        "allowedTools",
        "mcpContextUris",
        "mcpServers",
        "enabledMcpjsonServers",
        "disabledMcpjsonServers",
    ]

    /// 「有实质值」= true / 非空数组 / 非空字典 / 非空字符串。`false`、空数组、空字典、
    /// 缺键都算「没值」 —— 这些正是该被源覆盖的。执行层用同一个判定读真 JSON。
    static func isMeaningful(_ value: Any?) -> Bool {
        switch value {
        case nil, is NSNull: return false
        case let b as Bool: return b
        case let n as NSNumber:
            // JSONSerialization 把 true/false 解成 NSNumber，先按布尔看。
            if CFGetTypeID(n) == CFBooleanGetTypeID() { return n.boolValue }
            return n != 0
        case let s as String: return !s.isEmpty
        case let a as [Any]: return !a.isEmpty
        case let d as [String: Any]: return !d.isEmpty
        default: return true
        }
    }

    /// 对外界的只读探针 —— 规划层唯一接触现实的方式，全是闭包，单测直接喂假数据。
    struct Probe {
        var pathExists: (String) -> Bool
        var isDirectory: (String) -> Bool
        var isWritable: (String) -> Bool
        /// 目录下**递归**的普通文件相对路径（目录不存在 → 空）。
        var listFiles: (String) -> [String]
        /// `~/.claude.json` 的 `projects["<绝对路径>"]` 快照（存在性 + 哪几个键有实质值）。
        var claudeProjectSettings: (String) -> ClaudeProjectSettings
        /// `~/.codex/config.toml` 的 `[projects."<绝对路径>"] trust_level`（无 → nil）。
        var codexTrustLevel: (String) -> String?

        init(pathExists: @escaping (String) -> Bool,
             isDirectory: @escaping (String) -> Bool,
             isWritable: @escaping (String) -> Bool,
             listFiles: @escaping (String) -> [String] = { _ in [] },
             claudeProjectSettings: @escaping (String) -> ClaudeProjectSettings
                 = { _ in ClaudeProjectSettings() },
             codexTrustLevel: @escaping (String) -> String? = { _ in nil }) {
            self.pathExists = pathExists
            self.isDirectory = isDirectory
            self.isWritable = isWritable
            self.listFiles = listFiles
            self.claudeProjectSettings = claudeProjectSettings
            self.codexTrustLevel = codexTrustLevel
        }
    }

    // MARK: - 输出

    /// 拦路的硬条件 —— 有任意一条就不给执行（预览里原样列出来给人看）。
    enum Blocker: Equatable {
        case emptyNewWorkdir
        case newWorkdirMissing(String)
        case newWorkdirNotADirectory(String)
        case newWorkdirNotWritable(String)
        case rootCrewNotFound(String)
        case noCrewSelected
        /// 被迁 crew 里还有**别人**正在干活的 session —— 先停了再迁。
        /// 调用者自己不进这个列表（否则机长永远调不动），空闲的 worker 也不进。
        ///
        /// **这条以前有两层意思，现在只剩一层。** 原来它还兼着数据完整性
        /// （别搬一份 claude 正在写的 `.jsonl`）；会话搬运删掉之后那半没有了，
        /// 剩下的纯粹是常识：别把目录从正在干活的人脚下抽走。**它不再保护任何文件** ——
        /// 从硬约束降级成一条判断，别照着以为绕过它会损坏什么。
        case sessionsBusy([RunningSessionInput])
    }

    /// 要做的动作，**按这个顺序执行**：先补新路径的信任/权限（补错了不损坏旧路径），
    /// 再复制共享记忆，最后才改 crew 自己的字段 —— 这样中途炸了，
    /// crew 还指着旧目录，成员照旧能接回上下文。
    enum Action: Equatable {
        /// `~/.claude.json`：把旧路径 `projects` 条目里的**这几个键**补给新路径（旧的留着，
        /// 目标已有实质值的键不动）。
        case copyClaudeProjectSettings(fromPath: String, toPath: String, keys: [String])
        /// `~/.codex/config.toml`：给新路径补一条 `[projects."<新路径>"] trust_level`。
        case copyCodexTrust(fromPath: String, toPath: String, trustLevel: String)
        /// claude 项目记忆：整个项目共享，**复制不移动**（旧路径还有别的 crew 在用）。
        case copyClaudeMemoryFile(relativePath: String, from: String, to: String)
        /// 改 crew 自己的 `workingDirectory`（内存 + 落盘，不要求重启 app）。
        case setCrewWorkingDirectory(crewId: String, title: String, from: String?, to: String)
    }

    /// 没做的事 —— 每一条都要能对人说清楚（撞名跳过 / 源不存在 / 本来就不用做）。
    enum Skip: Equatable {
        case memoryDirectoryMissing(path: String)
        case memoryTargetExists(relativePath: String, path: String)
        /// 旧路径压根没有条目、或那几个键都没实质值 → 没什么可搬的。
        case claudeProjectSettingsSourceEmpty(path: String)
        /// 新路径那几个键都已经有实质值 → 不覆盖。
        case claudeProjectSettingsAlreadyComplete(path: String)
        case codexTrustSourceMissing(path: String)
        case codexTrustTargetExists(path: String)
        case crewHasNoWorkingDirectory(crewId: String, title: String)
        case crewAlreadyAtNewWorkdir(crewId: String, title: String)
    }

    /// 一份完整计划 = dry-run 的全部内容（预览就是把它渲染出来）。
    struct Plan: Equatable {
        var blockers: [Blocker] = []
        var actions: [Action] = []
        var skips: [Skip] = []
        /// 真正会改字段的 crew。
        var crews: [CrewRef] = []

        /// 能不能按「确认执行」。没有动作也算不能 —— 别让人点一个什么都不干的按钮。
        var isExecutable: Bool { blockers.isEmpty && !actions.isEmpty }

        var memoryCopyCount: Int {
            actions.filter { if case .copyClaudeMemoryFile = $0 { return true }; return false }.count
        }
    }

    // MARK: - 规划

    /// 算出一份计划。**纯函数**：只读 `inputs`，只经 `probe` 的闭包看现实。
    static func make(_ inputs: Inputs, probe: Probe) -> Plan {
        var plan = Plan()
        let newDir = normalize(inputs.newWorkdir)

        // ── 1. 目标目录：存在、是目录、可写。不存在就拒绝，绝不偷偷创建。
        if newDir.isEmpty {
            plan.blockers.append(.emptyNewWorkdir)
        } else if !probe.pathExists(newDir) {
            plan.blockers.append(.newWorkdirMissing(newDir))
        } else if !probe.isDirectory(newDir) {
            plan.blockers.append(.newWorkdirNotADirectory(newDir))
        } else if !probe.isWritable(newDir) {
            plan.blockers.append(.newWorkdirNotWritable(newDir))
        }

        let byId = Dictionary(uniqueKeysWithValues: inputs.crews.map { ($0.id, $0) })
        guard let root = byId[inputs.rootCrewId] else {
            plan.blockers.append(.rootCrewNotFound(inputs.rootCrewId))
            return plan
        }
        // ── 2. 选中的 crew（root 恒在内，即使调用方没勾）。
        let selected = inputs.crews
            .filter { inputs.selectedCrewIds.contains($0.id) || $0.id == inputs.rootCrewId }
        if selected.isEmpty {
            plan.blockers.append(.noCrewSelected)
            return plan
        }
        let selectedIds = Set(selected.map(\.id))

        // ── 3. 拦路的只有「**别人**正在干活」。调用者自己（机长）不拦，空闲 worker 不拦。
        let busy = inputs.runningSessions.filter {
            selectedIds.contains($0.crewId) && $0.isWorking
                && $0.sessionId != inputs.callerSessionId
        }
        if !busy.isEmpty {
            plan.blockers.append(.sessionsBusy(busy))
        }

        let projects = inputs.home.appendingPathComponent(".claude/projects", isDirectory: true).path
        let rootSource = sourceDirectory(for: root, newDir: newDir)

        // ── 4/5/6. 信任 / 权限 / 记忆：以 root 的旧目录为源。
        if let rootSource, !newDir.isEmpty {
            let src = probe.claudeProjectSettings(rootSource)
            let dst = probe.claudeProjectSettings(newDir)
            let keys = claudeSettingsKeys.filter {
                src.meaningfulKeys.contains($0) && !dst.meaningfulKeys.contains($0)
            }
            if !src.exists || src.meaningfulKeys.isEmpty {
                plan.skips.append(.claudeProjectSettingsSourceEmpty(path: rootSource))
            } else if keys.isEmpty {
                plan.skips.append(.claudeProjectSettingsAlreadyComplete(path: newDir))
            } else {
                plan.actions.append(.copyClaudeProjectSettings(
                    fromPath: rootSource, toPath: newDir, keys: keys))
            }

            if let trust = probe.codexTrustLevel(rootSource) {
                if probe.codexTrustLevel(newDir) != nil {
                    plan.skips.append(.codexTrustTargetExists(path: newDir))
                } else {
                    plan.actions.append(
                        .copyCodexTrust(fromPath: rootSource, toPath: newDir, trustLevel: trust))
                }
            } else {
                plan.skips.append(.codexTrustSourceMissing(path: rootSource))
            }

            let memFrom = projects + "/" + projectSlug(forWorkdir: rootSource) + "/memory"
            let memTo = projects + "/" + projectSlug(forWorkdir: newDir) + "/memory"
            if !probe.isDirectory(memFrom) {
                plan.skips.append(.memoryDirectoryMissing(path: memFrom))
            } else {
                for rel in probe.listFiles(memFrom).sorted() {
                    let dst = memTo + "/" + rel
                    if probe.pathExists(dst) {
                        plan.skips.append(.memoryTargetExists(relativePath: rel, path: dst))
                    } else {
                        plan.actions.append(.copyClaudeMemoryFile(
                            relativePath: rel, from: memFrom + "/" + rel, to: dst))
                    }
                }
            }
        }

        // ── 7. 最后才改 crew 自己的字段（前面炸了就还指着旧目录，信任/记忆不丢）。
        for crew in selected.sorted(by: { $0.id < $1.id }) {
            let old = crew.workingDirectory.map(normalize)
            if old == nil || old?.isEmpty == true {
                plan.skips.append(.crewHasNoWorkingDirectory(crewId: crew.id, title: crew.title))
            }
            if old == newDir {
                plan.skips.append(.crewAlreadyAtNewWorkdir(crewId: crew.id, title: crew.title))
                continue
            }
            guard !newDir.isEmpty else { continue }
            plan.crews.append(CrewRef(id: crew.id, title: crew.title))
            plan.actions.append(.setCrewWorkingDirectory(
                crewId: crew.id, title: crew.title, from: old, to: newDir))
        }

        return plan
    }

    /// 信任 / 权限 / 记忆该从**哪个目录**取。就是这个 crew 当前的工作目录；
    /// 已经在目标上（或压根没有目录）→ nil，没什么可取的，只改字段。
    ///
    /// **这里原本还有第二支**：当前目录 == 目标时回落 `previousWorkingDirectory`，
    /// 那是为「补搬上一轮留下的会话」立的清扫模式。会话不搬了，那支一起删了 ——
    /// 而它本来也够不着更早那次搬家（`previousWorkingDirectory` 只记一层）。
    static func sourceDirectory(for crew: CrewInput, newDir: String) -> String? {
        guard let current = crew.workingDirectory.map(normalize),
              !current.isEmpty, current != newDir else { return nil }
        return current
    }

    // MARK: - 子树（勾选框 / 机长指定目标的数据源）

    /// `rootId` 自己 + 全部后代（沿 `parentCrewIds` 反推 children，BFS，带 visited 防脏数据成环）。
    /// 顺序：root 在前，其余按 title 稳定排序 —— 预览列表每次打开都一样。
    static func subtree(rootId: String, crews: [CrewInput]) -> [CrewInput] {
        guard let root = crews.first(where: { $0.id == rootId }) else { return [] }
        var result: [CrewInput] = [root]
        var visited: Set<String> = [rootId]
        var queue: [String] = [rootId]
        while let current = queue.popLast() {
            let kids = crews
                .filter { $0.parentCrewIds.contains(current) }
                .sorted { $0.title == $1.title ? $0.id < $1.id : $0.title < $1.title }
            for kid in kids where visited.insert(kid.id).inserted {
                result.append(kid)
                queue.append(kid.id)
            }
        }
        return result
    }

    /// 机长 `change_workdir` 的 `crew` 参数解析：**只在本 crew 的子树里找**（不能拿它去
    /// 改别的部门）。id 精确 → 标签名精确（忽略大小写）→ **唯一**前缀。歧义/无匹配 → nil。
    /// 口径与 `LocalCrewStore.resolveChild` 一致，只是范围换成整棵子树（含 root 自己）。
    static func resolveTarget(hint: String, rootId: String, crews: [CrewInput]) -> String? {
        let h = hint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !h.isEmpty else { return rootId }
        let scope = subtree(rootId: rootId, crews: crews)
        if let exact = scope.first(where: { $0.id == h }) { return exact.id }
        let lower = h.lowercased()
        let byTitle = scope.filter { $0.title.lowercased() == lower }
        if byTitle.count == 1 { return byTitle[0].id }
        if !byTitle.isEmpty { return nil }
        let prefix = scope.filter { $0.title.lowercased().hasPrefix(lower) }
        return prefix.count == 1 ? prefix[0].id : nil
    }

    // MARK: - 路径

    /// 绝对路径归一：展开 `~`、去掉 `..`/`.`、去掉尾部 `/`。规划与执行都只认归一后的串
    /// —— slug 是从路径字面量算的，`/a/b` 和 `/a/b/` 会算出**两个不同目录**。
    static func normalize(_ path: String) -> String {
        let trimmed = path.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return "" }
        var p = (trimmed as NSString).expandingTildeInPath
        p = (p as NSString).standardizingPath
        while p.count > 1 && p.hasSuffix("/") { p.removeLast() }
        return p
    }

    /// 工作目录 → claude 项目目录名。与 `AgentSessionResume.projectSlug(forWorkdir:)`
    /// 同一口径（转发过去，别留第二份实现）。
    static func projectSlug(forWorkdir workdir: String) -> String {
        AgentSessionResume.projectSlug(forWorkdir: workdir)
    }
}
#endif
