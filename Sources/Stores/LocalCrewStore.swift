import Foundation

/// 本地 crew 标题是谁定的。它是「是否该提醒机长改占位名」的事实源，不能再靠
/// 每轮看起来像不像地名来猜。
enum LocalCrewTitleSource: String, Codable, Equatable {
    case placeholder
    case human
    case captain

    /// 仅供旧 JSON 一次性迁移。把地名集合做成参数，判定本身保持纯函数、可单测。
    static func inferLegacy(title: String, placeNames: Set<String>) -> Self {
        placeNames.contains(title) ? .placeholder : .human
    }
}

/// BYOK 模式下本地 crew 的持久化 store。
///
/// **设计取舍**:
/// - 不引入 GRDB / SQLite,简单 JSON 文件够 MVP(crew 数量预期 < 100,
///   全量读写不到 1ms)
/// - 写策略:每次 mutation 后 atomic write,避免崩溃留半截 JSON
/// - 文件位置:`~/Library/Application Support/PendingCrew/local-crews.json`
///   (Mac;iOS BYOK 路径本次 scope 不开)
/// - id 用 UUID v4(`UUID().uuidString.lowercased()`),跟 edge 端 crew id
///   形状一致,后续真要往 edge 上传时可平滑迁移
///
/// **shape**:复用 `CrewSummary` / `CrewDetail`,这样 backend protocol 两条
/// 路返回同一 model,UI 层不分叉。
/// - 因为本地 crew 没有真正的 parents/children/shares,这些字段空填(`[]`)
/// - captain 留存"用户起的名字 + 自生成 botId"(纯字符串,后续 Phase 才接
///   真本机 captain template + 模型调用)
///
/// **MainActor**:跟 `CrewStore` / `AppModel` 一致,所有 mutation 都在 UI
/// 线程跑;文件 I/O 量极小不会卡 frame。
@MainActor
final class LocalCrewStore {
    static let shared = LocalCrewStore()

    private let fileURL: URL
    private var crews: [String: LocalCrew] = [:]
    /// 全机「下一个可发的 crew 号」（通讯录，2026-08-11）。**单调只增** —— 删了
    /// crew 也不回收它的号，旧记录里的号码永远解析得出当初是谁。所以绝不能改成
    /// 「现存最大号 + 1」：那会在删除后复用号码。与 crews 同落 `local-crews.json`
    /// （不另开号码注册表，避免两本账漂移）。
    private var nextCrewNumber: Int = 1

    /// `nil` baseDirectory = 默认 `Application Support/PendingCrew/`;
    /// 测试时传 tmp 目录便于隔离。
    init(baseDirectory: URL? = nil) {
        let base: URL
        if let baseDirectory {
            base = baseDirectory
        } else {
            // 缺省 Application Support/PendingCrew/。Application Support
            // 目录在 Mac 上没有 sandbox 限制,直接 mkdir。
            let support = FileManager.default
                .urls(for: .applicationSupportDirectory, in: .userDomainMask)
                .first ?? URL(fileURLWithPath: NSTemporaryDirectory())
            base = support.appendingPathComponent("PendingCrew", isDirectory: true)
        }
        do {
            try FileManager.default.createDirectory(
                at: base,
                withIntermediateDirectories: true
            )
        } catch {
            // mkdir 失败基本不可能(权限问题除外);记一行 stderr 即可,
            // 后续 read/write 会再报。
            NSLog("[LocalCrewStore] mkdir failed: \(error)")
        }
        self.fileURL = base.appendingPathComponent("local-crews.json")
        loadFromDisk()
    }

    // MARK: - Public API

    /// 全量 crew 列表(顺序按 createdAt DESC,跟 edge 端一致)。
    func listCrews() -> [CrewSummary] {
        crews.values
            .sorted { $0.createdAt > $1.createdAt }
            .map(\.summary)
    }

    /// 单条 crew detail。本地 crew 没有 parents/children/shares,空数组兜底。
    func getCrew(_ id: String) -> CrewDetail? {
        guard let crew = crews[id] else { return nil }
        return crew.detail
    }

    /// 改 crew 标题（captain `rename_crew` 经控制通道落地，由 `CrewStore` 调）。
    /// 空标题 / crew 不存在 / 同名且同来源 → 忽略（幂等，避免无谓重写）。
    /// 同名但来源变了仍要落盘：机长显式确认占位名后也不该继续收到提醒。
    func setTitle(_ id: String, _ title: String, source: LocalCrewTitleSource) {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, var crew = crews[id],
              crew.title != trimmed || crew.titleSource != source else { return }
        crew.title = trimmed
        crew.titleSource = source
        crew.updatedAt = ISO8601DateFormatter().string(from: Date())
        crews[id] = crew
        persistToDisk()
    }

    /// 改 crew 的工作目录（仓库搬家 / 目录改名）。**内存里立刻生效**，不要求重启 app ——
    /// `loadFromDisk` 只在启动跑一次，手改 JSON 会被 `persistToDisk` 整份覆写掉，
    /// 这就是必须有这个入口的原因。
    ///
    /// 只改字段。agent 侧上下文（claude 的会话日志 / 项目记忆、两家的目录信任与权限）
    /// 按路径分家，要一起搬 —— 那套规划与执行在 `WorkdirMigrationPlan` /
    /// `WorkdirMigrationExecutor`，由 UI 编排（先预览再执行），这里不代劳。
    ///
    /// 空路径 / crew 不存在 / 值没变 → 忽略（幂等，避免无谓重写）。
    func setWorkingDirectory(_ id: String, _ path: String) {
        let trimmed = path.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, var crew = crews[id], crew.workingDirectory != trimmed else { return }
        // 记下旧路径。**当前没有任何消费者** —— 它原本是迁移那侧「清扫模式」的唯一
        // 线索（回旧目录补搬会话），会话不搬了之后那条线整段删了（2026-08-26）。
        // 字段留着是纯留痕：删它会把已经写在 `local-crews.json` 里的历史一次性丢掉，
        // 而留着不花什么。只在真换路径时更新，别被无谓写入冲掉。
        if let old = crew.workingDirectory, !old.isEmpty {
            crew.previousWorkingDirectory = old
        }
        crew.workingDirectory = trimmed
        crew.updatedAt = ISO8601DateFormatter().string(from: Date())
        crews[id] = crew
        persistToDisk()
    }

    /// 改这个 crew 以后由哪一种本机 coding agent 承担机长。
    ///
    /// 重新指定机长时必须先把这个事实落盘，再停旧机长、拉新机长：这样即使 app
    /// 恰好在两步之间退出，下一次自动唤醒也不会又按旧 runner 起回来。只接受真
    /// agent（`claude_code` / `codex`）；空值、未知值、纯终端与幂等写入都忽略。
    func setCaptainAgentKind(_ id: String, _ rawKind: String) {
        guard rawKind == "claude_code" || rawKind == "codex",
              var crew = crews[id], crew.captainAgentKind != rawKind else { return }
        crew.captainAgentKind = rawKind
        crew.updatedAt = ISO8601DateFormatter().string(from: Date())
        crews[id] = crew
        persistToDisk()
    }

    /// 迁移规划层要的全部 crew 字段（id / 名 / 工作目录 / 父边）。返回元组而非专用类型 ——
    /// store 不必反过来依赖 `WorkdirMigrationPlan`。
    func workdirRows() -> [(id: String, title: String, workingDirectory: String?,
                            parentCrewIds: [String])] {
        crews.values
            .sorted { $0.createdAt < $1.createdAt }
            .map { ($0.id, $0.title, $0.workingDirectory, $0.parentCrewIds) }
    }

    /// 迁移规划层要的 crew 行（直接给 `WorkdirMigrationPlan.CrewInput`）。
    /// 界面与机长工具两条路共用同一份构造，别各拼各的。
    /// macOS-only —— `WorkdirMigrationPlan` 整个在 `#if os(macOS)` 后面（迁移只在 Mac 端做）。
    #if os(macOS)
    func workdirCrewInputs() -> [WorkdirMigrationPlan.CrewInput] {
        workdirRows().map {
            WorkdirMigrationPlan.CrewInput(
                id: $0.id, title: $0.title, workingDirectory: $0.workingDirectory,
                parentCrewIds: $0.parentCrewIds)
        }
    }
    #endif

    /// 记录/清除旧 attention 文案（`raise_attention` / `clear_attention` 经控制
    /// 通道落地，由 `CrewStore` 调）。Todo #71 起不再控制状态点。
    /// crew 不存在 / 值未变 → 忽略（幂等，避免无谓重写 + 变更信号）。
    func setAttention(_ id: String, reason: String?) {
        let trimmed = reason?.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalized = (trimmed?.isEmpty == false) ? trimmed : nil
        guard var crew = crews[id], crew.attentionReason != normalized else { return }
        crew.attentionReason = normalized
        crews[id] = crew
        persistToDisk()
    }

    /// 人手动把 crew 从侧栏藏起来 / 取回来（侧栏行右键与「已隐藏的群」列表两个入口）。
    ///
    /// `hidden == true` 时盖上**当下**的时间戳（重复藏一个已经藏着的不刷新时间戳 ——
    /// 那个时刻是未读的参照点之一，刷新它等于把已有的未读抹掉）；`false` 清空。
    /// crew 不存在 / 值未变 → 忽略（幂等，同 `setAttention`）。
    ///
    /// **只改人类界面的可见性**：不动父子边、不停 session、不碰白板。藏了的 crew
    /// 里的 session 照常干活。
    func setManuallyHidden(_ id: String, hidden: Bool) {
        guard var crew = crews[id] else { return }
        if hidden {
            guard crew.manuallyHiddenAt == nil else { return }
            crew.manuallyHiddenAt = ISO8601DateFormatter().string(from: Date())
        } else {
            guard crew.manuallyHiddenAt != nil else { return }
            crew.manuallyHiddenAt = nil
        }
        crews[id] = crew
        persistToDisk()
    }

    /// 新建本地 crew。返回包含自生成的 crewId + captainBotId(本地都用 UUID)。
    ///
    /// captain 当前只接 `systemGenerated(templateName:)` —— reuseBot 走的是
    /// edge 端 `/v1/me/bots` 列表;BYOK 本地暂不实现 reuse(留下次)。
    @discardableResult
    func createCrew(_ request: CreateCrewRequest) -> CreateCrewResponse {
        let now = ISO8601DateFormatter().string(from: Date())
        let id = "local-" + UUID().uuidString.lowercased()
        // captain bot id 也本地生成。后续 Phase 接真 captain template 时,
        // 这个 botId 会成为本地存活的 BotProfile 的 id。
        let captainBotId = "local-bot-" + UUID().uuidString.lowercased()
        let captainName: String
        switch request.captain {
        case .systemGenerated(let templateName):
            captainName = templateName?.isEmpty == false ? templateName! : "机长"
        case .reuseBot(let botId):
            // BYOK 本地 store 不支持 reuse_bot 路径(没有 bot 列表)。
            // 走兜底:把传进来的 botId 当 captainBotId 直接用,name 留空
            // ——保留 shape 完整,UI 层就当一个普通 captain 看待。
            captainName = "Bot \(botId.prefix(8))"
        }

        let providedTitle = request.title?.trimmingCharacters(in: .whitespaces)
        let hasProvidedTitle = providedTitle?.isEmpty == false
        // 通讯录号码：建 crew 时发一次，终身不变（adopt/release/换爹都不重发）。
        let number = nextCrewNumber
        nextCrewNumber += 1
        let crew = LocalCrew(
            id: id,
            // title 缺省/空 = 自动 → 兜底一个随机地名（CreateCrewSheet 正常路径会把
            // 已挑好的 crewGroundName 传进来，这里只是空标题的防御兜底）。不再出现
            // 「未命名 crew」；captain 搞清楚 crew 做什么后用 rename_crew 改成短标签。
            title: hasProvidedTitle
                ? providedTitle!
                : (PlaceNames.all.randomElement() ?? "Crew"),
            titleSource: request.initialTitleSource
                ?? (hasProvidedTitle ? .human : .placeholder),
            responsibleSubjectId: request.responsibleSubjectId,
            // 本地 crew 恒「本机」 —— runtime_location 永远 local_host。
            runtimeLocation: "local_host",
            workingDirectory: request.workingDirectory,
            machineId: request.machineId,
            captainBotId: captainBotId,
            captainName: captainName,
            captainAgentKind: request.captainAgentKind,
            createdAt: now,
            updatedAt: now,
            crewNumber: number,
            // 分机 1 归机长（crew 一建起来第一个成员就是它），worker 从 2 起。
            nextExtension: LocalCrew.firstWorkerExtension
        )
        crews[id] = crew
        persistToDisk()
        return CreateCrewResponse(crewId: id, captainBotId: captainBotId)
    }

    // MARK: - 本地 DAG 父边

    /// 把 `parentCrewId` 加进 `crewId` 的父边集合(去重)。
    ///
    /// **禁环**:若 `parentCrewId` 已经是 `crewId` 的后代(沿 children 向下能
    /// 到达),挂上去会成环 —— 拒绝并抛 `LocalCrewStoreError.wouldCreateCycle`。
    /// children 由「谁的 parentCrewIds 含本 crew」反推,所以"后代"判定走
    /// `descendants(of:)`。自挂自(crewId == parentCrewId)也算环,拒绝。
    func attachParent(crewId: String, parentCrewId: String) throws {
        guard crewId != parentCrewId else {
            throw LocalCrewStoreError.wouldCreateCycle
        }
        guard crews[crewId] != nil else {
            throw LocalCrewStoreError.crewNotFound(crewId)
        }
        guard crews[parentCrewId] != nil else {
            throw LocalCrewStoreError.crewNotFound(parentCrewId)
        }
        // parentCrewId 是 crewId 的后代 → 反向连边会成环。
        if descendants(of: crewId).contains(parentCrewId) {
            throw LocalCrewStoreError.wouldCreateCycle
        }
        guard var crew = crews[crewId] else { return }
        guard !crew.parentCrewIds.contains(parentCrewId) else { return } // 已挂,幂等
        crew.parentCrewIds.append(parentCrewId)
        crews[crewId] = crew
        persistToDisk()
    }

    /// 移除一条父边。父边不存在则无操作。
    func detachParent(crewId: String, parentCrewId: String) {
        guard var crew = crews[crewId],
              let idx = crew.parentCrewIds.firstIndex(of: parentCrewId) else { return }
        crew.parentCrewIds.remove(at: idx)
        crews[crewId] = crew
        persistToDisk()
    }

    /// `crewId` 的全部后代 id(沿 children 向下 BFS)。child = 其
    /// parentCrewIds 含某 crew 的那些 crew。带 visited 自保防脏数据成环。
    func descendants(of crewId: String) -> Set<String> {
        var result: Set<String> = []
        var queue: [String] = [crewId]
        while let current = queue.popLast() {
            for (id, crew) in crews where crew.parentCrewIds.contains(current) {
                if result.insert(id).inserted {
                    queue.append(id)
                }
            }
        }
        return result
    }

    /// `crewId` 的父 crew id 列表（直系,不递归）。
    func parentIds(of crewId: String) -> [String] {
        crews[crewId]?.parentCrewIds ?? []
    }

    /// `crewId` 挂在哪些**根 crew**（组织树最顶层的祖先）之下 —— 名字后面那行黄字
    /// 标注的数据源。DAG 允许多父,所以是集合;自己就是根 → 空数组。判定全在
    /// `CrewRootLineage`（纯函数,脏数据/环/多层都有单测），这里只喂父边表。
    func rootCrewIds(of crewId: String) -> [String] {
        CrewRootLineage.rootIds(of: crewId, parents: crews.mapValues(\.parentCrewIds))
    }

    /// `crewId` 的直接子 crew `(id, title)` 列表（反推 parentCrewIds,不递归）。
    func children(of crewId: String) -> [(id: String, title: String)] {
        crews.values
            .filter { $0.parentCrewIds.contains(crewId) }
            .sorted { $0.createdAt < $1.createdAt }
            .map { ($0.id, $0.title) }
    }

    func title(of crewId: String) -> String? { crews[crewId]?.title }
    func titleSource(of crewId: String) -> LocalCrewTitleSource? { crews[crewId]?.titleSource }

    /// 机长 `message_child_crew` 的目标解析：`hint` 依次按 子 crew 的 id 精确 →
    /// title 精确（忽略大小写）→ **唯一** title 前缀匹配。歧义/无匹配 → nil
    /// （调用方回执可选清单,别猜错部门投错群）。
    func resolveChild(of parentId: String, hint: String) -> String? {
        let kids = children(of: parentId)
        let h = hint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !h.isEmpty else { return nil }
        if let exact = kids.first(where: { $0.id == h }) { return exact.id }
        let lower = h.lowercased()
        if let exact = kids.first(where: { $0.title.lowercased() == lower }) { return exact.id }
        let prefix = kids.filter { $0.title.lowercased().hasPrefix(lower) }
        return prefix.count == 1 ? prefix[0].id : nil
    }

    /// 沿父边算深度：无父 = 0（根），否则 1 + max(父深度)。带 visited 防脏数据成环
    /// 无限递归。组织树展示/诊断用；**层数本身不设上限**。
    ///
    /// **成环时的保证**：`attachParent` 正常路径禁环，但脏数据(如手改 JSON)
    /// 仍可能出现环。visited 守卫只保证*会返回*，不保证返回值有意义 ——
    /// 环上算出来的是有限整数，但不是真实深度,调用方只能依赖"会终止"这一条,
    /// 不能依赖"数值正确"。
    func depth(of crewId: String, visited: Set<String> = []) -> Int {
        guard let crew = crews[crewId], !crew.parentCrewIds.isEmpty else { return 0 }
        guard !visited.contains(crewId) else { return 0 } // 环自保
        let next = visited.union([crewId])
        return 1 + (crew.parentCrewIds.map { depth(of: $0, visited: next) }.max() ?? 0)
    }

    // MARK: - 组织架构调整（#22/#25 机长 adopt/release/认父）

    /// `crewId` 子树的**高度**：无子 = 0，否则 1 + max(子高度)。带 visited 防脏数据
    /// 成环（同 `depth(of:)` 的自保语义：保证会返回，环上数值无意义）。
    /// 组织树展示/诊断用。
    func height(of crewId: String, visited: Set<String> = []) -> Int {
        guard !visited.contains(crewId) else { return 0 } // 环自保
        let kids = crews.values.filter { $0.parentCrewIds.contains(crewId) }
        guard !kids.isEmpty else { return 0 }
        let next = visited.union([crewId])
        return 1 + (kids.map { height(of: $0.id, visited: next) }.max() ?? 0)
    }

    /// 把 `crewId`（整棵子树）挂到 `parentId` 名下 —— 收编（adopt_crew）/ 认父
    /// （adopt_parent / create_parent_crew）共用的落地操作。父边 additive（DAG
    /// 多父保留），已是其子 → 幂等 no-op。
    ///
    /// 校验只有**禁环**（沿用 `attachParent`）：不能把 crew 挂进自己的子树，
    /// 否则汇报线/注入会无限递归。层数不限 —— PendingCrew 就是给大规模 agent
    /// 组织用的，组织树想多深就多深。
    func adopt(crewId: String, underParent parentId: String) throws {
        guard crews[crewId] != nil else { throw LocalCrewStoreError.crewNotFound(crewId) }
        guard crews[parentId] != nil else { throw LocalCrewStoreError.crewNotFound(parentId) }
        guard crewId != parentId else { throw LocalCrewStoreError.wouldCreateCycle }
        if descendants(of: crewId).contains(parentId) {
            throw LocalCrewStoreError.wouldCreateCycle
        }
        try attachParent(crewId: crewId, parentCrewId: parentId)
    }

    /// 把 `parentId` 的**直系子** `crewId` 摘出：`newParentId == nil` → 只摘这条
    /// 父边（无其它父则回到顶层）；非 nil → 转挂到新父（须先通过 `adopt` 的
    /// 环校验，校验不过原边不动 —— 先挂新边再摘旧边，失败无副作用）。
    /// `crewId` 不是 `parentId` 的直系子 → 抛 `notDirectChild`（上级只能动直系子）。
    func release(crewId: String, from parentId: String, to newParentId: String?) throws {
        guard let crew = crews[crewId] else { throw LocalCrewStoreError.crewNotFound(crewId) }
        guard crew.parentCrewIds.contains(parentId) else {
            throw LocalCrewStoreError.notDirectChild(crewId)
        }
        guard newParentId != parentId else { return } // 转挂给原父 = no-op
        if let newParentId {
            try adopt(crewId: crewId, underParent: newParentId)
        }
        detachParent(crewId: crewId, parentCrewId: parentId)
    }

    /// **全局** crew 解析（收编/认父的目标可以是本机任意 crew，不限直系子）：
    /// id 精确 → title 精确（忽略大小写，须唯一）→ 唯一 title 前缀。
    /// 歧义/无匹配 → nil（调用方回执候选清单，别猜错收编错部门）。
    func resolveAnyCrew(hint: String) -> String? {
        let h = hint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !h.isEmpty else { return nil }
        if crews[h] != nil { return h }
        let lower = h.lowercased()
        let exacts = crews.values.filter { $0.title.lowercased() == lower }
        if exacts.count == 1 { return exacts[0].id }
        if !exacts.isEmpty { return nil } // 同名歧义
        let prefix = crews.values.filter { $0.title.lowercased().hasPrefix(lower) }
        return prefix.count == 1 ? prefix[0].id : nil
    }

    /// 全部本地 crew `(id, title)`（按 createdAt 升序）—— 解析失败时回执候选清单用。
    func allCrewTitles() -> [(id: String, title: String)] {
        crews.values.sorted { $0.createdAt < $1.createdAt }.map { ($0.id, $0.title) }
    }

    /// 全机 crew **组织树扁平行**（DFS 前序;`depth` = 缩进层级）—— 机长每轮注入的
    /// 树概览数据源。nonisolated 静态：与 `title(ofCrew:)` 同款轻量跨进程读
    /// `local-crews.json`（claude 路走 helper 子进程,拿不到 MainActor 单例）。
    /// 根/子都按 createdAt 升序;多父 crew 在每个父下各出现一次;环自保（路径上
    /// 已出现的不再下钻）。缺文件/解码失败 → 空数组（注入端省略概览块）。
    nonisolated static func orgTreeLines(
        whiteboardDirectory: URL
    ) -> [(id: String, title: String, depth: Int, titleSource: LocalCrewTitleSource?)] {
        let file = whiteboardDirectory.deletingLastPathComponent()
            .appendingPathComponent("local-crews.json")
        guard let data = try? Data(contentsOf: file),
              let payload = try? JSONDecoder().decode(LocalCrewFile.self, from: data)
        else { return [] }
        let all = payload.crews.sorted { $0.createdAt < $1.createdAt }
        var childMap: [String: [LocalCrew]] = [:]
        for c in all {
            for p in c.parentCrewIds { childMap[p, default: []].append(c) }
        }
        let ids = Set(all.map(\.id))
        // 根 = 无父,或父不在文件里（脏引用兜底当根,别整棵丢失）。
        let roots = all.filter { c in
            c.parentCrewIds.isEmpty || !c.parentCrewIds.contains(where: ids.contains)
        }
        var out: [(id: String, title: String, depth: Int, titleSource: LocalCrewTitleSource?)] = []
        func walk(_ c: LocalCrew, _ depth: Int, _ path: Set<String>) {
            out.append((c.id, c.title, depth, c.titleSource))
            guard !path.contains(c.id) else { return }
            for kid in childMap[c.id] ?? [] where !path.contains(kid.id) {
                walk(kid, depth + 1, path.union([c.id]))
            }
        }
        for r in roots { walk(r, 0, []) }
        return out
    }

    // MARK: - Session 成员（chunk 4 补口）

    /// 把一个 session 登记成 crew 的持久成员。session 一经拉起就算入伙,
    /// 退出/重启 app 后仍留在成员列表(与 IM「成员离线仍是成员」同语义)——
    /// 此前成员列表只含 captain + 本机人类,本地 session 全靠各视图临时合成
    /// 在跑的 run,退出即消失(用户点名的「新建 session 不出现在成员列表」)。
    /// crew 不存在(如登录态 edge crew,membership 归 edge 管)或已登记 → 忽略。
    /// **已登记的 sessionId 原样返回（幂等）** —— `restartMember` 复用原 sessionId
    /// 重启时会再走一遍这里，绝不能重新发分机号（号码终身绑 session）。
    func recordSessionMember(crewId: String, sessionId: String, displayName: String) {
        guard var crew = crews[crewId] else { return }
        var sessions = crew.sessionMembers ?? []
        guard !sessions.contains(where: { $0.sessionId == sessionId }) else { return }
        // 分机号：从 crew 自己的计数器发，只增不减（退出的成员不腾号）。
        let ext = crew.nextExtension ?? LocalCrew.firstWorkerExtension
        crew.nextExtension = ext + 1
        sessions.append(LocalSessionMember(
            sessionId: sessionId,
            displayName: displayName,
            createdAt: ISO8601DateFormatter().string(from: Date()),
            extensionNumber: ext))
        crew.sessionMembers = sessions
        crews[crewId] = crew
        persistToDisk()
    }

    /// crew 的持久 session 成员列表（按登记顺序）。
    func sessionMembers(crewId: String) -> [LocalSessionMember] {
        crews[crewId]?.sessionMembers ?? []
    }

    // MARK: - 通讯录号码（2026-08-11）

    /// 这个 crew 的通讯录号码（`7`）。查无此 crew / 尚未回填 → nil。
    func crewNumber(of crewId: String) -> Int? { crews[crewId]?.crewNumber }

    /// 某个 session 的分机号（`7-3` 里的 3）。机长不在 `sessionMembers` 里 ——
    /// 它恒是 `CrewPhoneNumber.captainExtension`，由调用方自己认。
    func extensionNumber(crewId: String, sessionId: String) -> Int? {
        crews[crewId]?.sessionMembers?.first { $0.sessionId == sessionId }?.extensionNumber
    }

    /// 某个 session 的完整号码（`7-3`）。crew 没号 / 该 session 不是本 crew 持久
    /// 成员 → nil。`isCaptain` 走机长分机（`7-1`），不查成员表。
    func phoneNumber(crewId: String, sessionId: String, isCaptain: Bool) -> CrewPhoneNumber? {
        guard let crew = crews[crewId]?.crewNumber else { return nil }
        if isCaptain { return CrewPhoneNumber(crew: crew, ext: CrewPhoneNumber.captainExtension) }
        guard let ext = extensionNumber(crewId: crewId, sessionId: sessionId) else { return nil }
        return CrewPhoneNumber(crew: crew, ext: ext)
    }

    /// 删除一条 crew。
    func deleteCrew(_ id: String) {
        guard crews.removeValue(forKey: id) != nil else { return }
        persistToDisk()
    }

    /// 全清(调试 / 本地数据重置用)。
    func clearAll() {
        crews.removeAll()
        persistToDisk()
    }

    // MARK: - 轻量跨进程只读

    /// 按 crewId 读当前 title —— 不建 `@MainActor` store 实例，直接解码
    /// `local-crews.json`。给 `HookEmitter`（每轮注入白板时带「本 crew 当前名」）
    /// 用：它既跑在 app 进程（codex whiteboardProvider）也跑在 helper 子进程
    /// （claude PostToolUse hook），后者拿不到 MainActor 单例，只能读共享文件。
    /// `whiteboardDirectory` = 白板目录（helper 的 `--dir`），`local-crews.json`
    /// 固定在其父目录（`Application Support/PendingCrew/`，见
    /// `LocalWhiteboardStore.defaultDirectory` 布局）。缺文件/解码失败/查无此
    /// crew → nil。
    nonisolated static func titleMetadata(
        ofCrew crewId: String, whiteboardDirectory: URL
    ) -> (title: String, source: LocalCrewTitleSource?)? {
        let file = whiteboardDirectory.deletingLastPathComponent()
            .appendingPathComponent("local-crews.json")
        guard let data = try? Data(contentsOf: file),
              let payload = try? JSONDecoder().decode(LocalCrewFile.self, from: data)
        else { return nil }
        guard let crew = payload.crews.first(where: { $0.id == crewId }) else { return nil }
        return (crew.title, crew.titleSource)
    }

    nonisolated static func title(ofCrew crewId: String, whiteboardDirectory: URL) -> String? {
        titleMetadata(ofCrew: crewId, whiteboardDirectory: whiteboardDirectory)?.title
    }

    // MARK: - Persistence

    private func loadFromDisk() {
        guard FileManager.default.fileExists(atPath: fileURL.path) else {
            return
        }
        do {
            let data = try Data(contentsOf: fileURL)
            let payload = try JSONDecoder().decode(LocalCrewFile.self, from: data)
            crews = Dictionary(uniqueKeysWithValues: payload.crews.map { ($0.id, $0) })
            // 计数器同样落在这个文件里（不另开号码注册表）。缺键 = 通讯录之前的旧
            // 文件，下面回填时算出来。
            nextCrewNumber = max(1, payload.nextCrewNumber ?? 1)
            // 旧记录没有 titleSource：只在首次成功加载时按地名池启发式回填，随后立即
            // 持久化；以后启动直接读字段，不会因地名池变化反复重算。
            let placeNames = Set(PlaceNames.all)
            var didBackfill = false
            for id in crews.keys {
                guard var crew = crews[id], crew.titleSource == nil else { continue }
                crew.titleSource = .inferLegacy(title: crew.title, placeNames: placeNames)
                crews[id] = crew
                didBackfill = true
            }
            if backfillNumbers() { didBackfill = true }
            if didBackfill { persistToDisk() }
        } catch {
            // JSON 损坏 → 不直接清掉,留备份让用户 / 调试时手动救;内存里
            // 留空 store 当从 0 开始。
            let backup = fileURL.appendingPathExtension("corrupt-\(Int(Date().timeIntervalSince1970))")
            try? FileManager.default.moveItem(at: fileURL, to: backup)
            NSLog("[LocalCrewStore] decode failed (\(error)), backed up to \(backup.lastPathComponent)")
        }
    }

    /// 存量回填（通讯录上线的一次性迁移）：没号的 crew 按 createdAt 升序补号、
    /// 没分机的成员按现有登记顺序补分机（机长恒占 1，worker 从 2 起）。返回
    /// 「有没有改动」，由 `loadFromDisk` 顺手落盘 —— 只跑一次，之后字段都在。
    ///
    /// 计数器保护：文件里已有的最大号 + 1 只用来**修复缺失的计数器**（旧文件没这个
    /// 键），不是发号规则本身。计数器一旦落盘就只增不减，删 crew 也不回收号码。
    private func backfillNumbers() -> Bool {
        var changed = false
        let highest = crews.values.compactMap(\.crewNumber).max() ?? 0
        if nextCrewNumber <= highest {
            nextCrewNumber = highest + 1
            changed = true
        }
        for id in crews.keys.sorted(by: { (crews[$0]?.createdAt ?? "") < (crews[$1]?.createdAt ?? "") }) {
            guard var crew = crews[id] else { continue }
            var touched = false
            if crew.crewNumber == nil {
                crew.crewNumber = nextCrewNumber
                nextCrewNumber += 1
                touched = true
            }
            var members = crew.sessionMembers ?? []
            if members.contains(where: { $0.extensionNumber == nil }) {
                var next = crew.nextExtension ?? LocalCrew.firstWorkerExtension
                for i in members.indices where members[i].extensionNumber == nil {
                    members[i].extensionNumber = next
                    next += 1
                }
                crew.sessionMembers = members
                crew.nextExtension = next
                touched = true
            } else if crew.nextExtension == nil {
                // 成员都有号（或没成员）却缺计数器：从已发出的最大分机接着走。
                crew.nextExtension = max(
                    LocalCrew.firstWorkerExtension,
                    (members.compactMap(\.extensionNumber).max() ?? 1) + 1)
                touched = true
            }
            if touched {
                crews[id] = crew
                changed = true
            }
        }
        return changed
    }

    private func persistToDisk() {
        let payload = LocalCrewFile(
            version: 1, crews: Array(crews.values), nextCrewNumber: nextCrewNumber)
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            let data = try encoder.encode(payload)
            try data.write(to: fileURL, options: [.atomic])
        } catch {
            NSLog("[LocalCrewStore] persist failed: \(error)")
        }
    }
}

// MARK: - Wire types

/// 本地 crew 完整内存表示。比 CrewSummary 多 captain name + working dir,
/// 比 CrewDetail 少 parents/children/shares。
///
/// **internal 而非 private**：`CrewDirectory` 要在 helper 子进程里解同一份
/// `local-crews.json`（通讯录不另开注册表，号码就长在这些记录上）。
struct LocalCrew: Codable, Equatable {
    /// 第一个 worker 分机号。1 恒归机长（crew 建起来第一个成员就是它）。
    static let firstWorkerExtension = 2

    let id: String
    /// `var` —— captain `rename_crew` 经 `LocalCrewStore.setTitle` 改名。
    var title: String
    /// optional 只为解码旧 JSON；loadFromDisk 会一次性补齐并持久化。
    var titleSource: LocalCrewTitleSource?
    let responsibleSubjectId: String
    let runtimeLocation: String
    /// `var` —— 仓库搬家后经 `LocalCrewStore.setWorkingDirectory` 改（含 agent 上下文
    /// 迁移，见 `WorkdirMigrationPlan`）。建 crew 时定下、之后无从更改是原来的病。
    var workingDirectory: String?
    /// 改工作目录之前的那个路径。**只写不读：当前没有任何消费者。**
    /// 它原本是迁移「清扫模式」的唯一线索，随会话搬运一起删了（2026-08-26）；
    /// 而它本来也只记一层，够不着更早那次搬家。留着是纯留痕，别当它还在被用。
    /// optional → 旧 JSON 缺键向后兼容。
    var previousWorkingDirectory: String? = nil
    /// 所选 machine id（nil = 本机）。登录态多机时记录 crew 运行在哪台机；
    /// 本机/BYOK 路径恒 nil。
    let machineId: String?
    let captainBotId: String?
    let captainName: String?
    /// 机长跑哪个本机 coding agent（`LocalCodingAgentKind.rawValue` ——
    /// "claude_code" / "codex"）。建 crew 时表单选定并持久化;机长 session 据此
    /// 起对应 agent。旧 JSON 缺此键 → decodeIfPresent 兜底 nil（下游回落 `.codex`）。
    var captainAgentKind: String? = nil
    let createdAt: String
    /// `var` —— `setTitle` 改名时一并刷新。
    var updatedAt: String
    /// 本地 crew DAG 的**父边**:本 crew 挂在哪些父 crew 之下(crew id 列表)。
    /// 「家」在本地 —— DAG 组织模型不走 edge,纯本地持久化。children 不存,渲染
    /// 时从「谁的 parentCrewIds 含本 crew」反推。旧 JSON 缺此键 → 默认空数组
    /// (= 根 crew),不破坏 decode。
    var parentCrewIds: [String] = []
    /// 持久 session 成员（chunk 4 补口）。optional → 旧 JSON 缺键向后兼容。
    var sessionMembers: [LocalSessionMember]? = nil
    /// 旧 attention 文案。Todo #71 起不再控制状态点；optional → 旧 JSON 缺键向后兼容。
    var attentionReason: String? = nil
    /// 通讯录 crew 号（`7`）。全机唯一、终身不变 —— 被 adopt/release/换爹都不重发，
    /// **层级完全不参与编号**。optional 只为解码旧 JSON；首次加载一次性回填。
    var crewNumber: Int? = nil
    /// 下一个可发的 worker 分机号。只增不减 —— 成员退出后号不腾给别人，旧记录里的
    /// `7-3` 永远解析得出当初是谁。optional 同上（旧 JSON 回填）。
    var nextExtension: Int? = nil

    /// **人手动**把这个 crew 从侧栏藏起来的时刻（ISO8601）。nil = 没藏。
    ///
    /// 字段名写死了「人手动藏的」而不是笼统的 `hidden`：将来若真加了「自动判定该藏
    /// 谁」，两种语义挤在同一个布尔值里就再也分不开了 —— 而那时已经有存量数据，
    /// 分不开就永远分不开。自动那条路当前**没做**（判据与实测见
    /// `docs/internal/2026-08-26-crew-hide-manual.md`），这个名字是给它留的位置。
    ///
    /// 时间戳本身也有用：它是「藏了之后这个群有没有新动静」的参照点之一
    /// （另一个是 crew 级 lastViewed，见 `CrewViewedStore`）。
    ///
    /// **只是人类界面的概念**，不是组织结构的改变 —— 藏了的 crew 里 session 照常
    /// 干活、照常收发消息，`directory` / `contact` / 组织树一律不受影响。
    /// optional → 旧 JSON 缺键向后兼容。
    var manuallyHiddenAt: String? = nil

    /// memberwise init —— `createCrew` 等内部构造点用。父边默认空(新建 = 根)。
    init(
        id: String,
        title: String,
        titleSource: LocalCrewTitleSource,
        responsibleSubjectId: String,
        runtimeLocation: String,
        workingDirectory: String?,
        machineId: String?,
        captainBotId: String?,
        captainName: String?,
        captainAgentKind: String? = nil,
        createdAt: String,
        updatedAt: String,
        parentCrewIds: [String] = [],
        crewNumber: Int? = nil,
        nextExtension: Int? = nil
    ) {
        self.id = id
        self.title = title
        self.titleSource = titleSource
        self.responsibleSubjectId = responsibleSubjectId
        self.runtimeLocation = runtimeLocation
        self.workingDirectory = workingDirectory
        self.machineId = machineId
        self.captainBotId = captainBotId
        self.captainName = captainName
        self.captainAgentKind = captainAgentKind
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.parentCrewIds = parentCrewIds
        self.crewNumber = crewNumber
        self.nextExtension = nextExtension
    }

    /// 显式 decoder —— 非 optional 的 `parentCrewIds` 在旧 JSON(无此键)里
    /// 合成 Codable 会抛 keyNotFound,这里用 decodeIfPresent ?? [] 兜底。
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decode(String.self, forKey: .title)
        titleSource = try c.decodeIfPresent(LocalCrewTitleSource.self, forKey: .titleSource)
        responsibleSubjectId = try c.decode(String.self, forKey: .responsibleSubjectId)
        runtimeLocation = try c.decode(String.self, forKey: .runtimeLocation)
        workingDirectory = try c.decodeIfPresent(String.self, forKey: .workingDirectory)
        previousWorkingDirectory = try c.decodeIfPresent(
            String.self, forKey: .previousWorkingDirectory)
        machineId = try c.decodeIfPresent(String.self, forKey: .machineId)
        captainBotId = try c.decodeIfPresent(String.self, forKey: .captainBotId)
        captainName = try c.decodeIfPresent(String.self, forKey: .captainName)
        captainAgentKind = try c.decodeIfPresent(String.self, forKey: .captainAgentKind)
        createdAt = try c.decode(String.self, forKey: .createdAt)
        updatedAt = try c.decode(String.self, forKey: .updatedAt)
        parentCrewIds = try c.decodeIfPresent([String].self, forKey: .parentCrewIds) ?? []
        sessionMembers = try c.decodeIfPresent([LocalSessionMember].self, forKey: .sessionMembers)
        attentionReason = try c.decodeIfPresent(String.self, forKey: .attentionReason)
        crewNumber = try c.decodeIfPresent(Int.self, forKey: .crewNumber)
        nextExtension = try c.decodeIfPresent(Int.self, forKey: .nextExtension)
        manuallyHiddenAt = try c.decodeIfPresent(String.self, forKey: .manuallyHiddenAt)
    }

    var summary: CrewSummary {
        CrewSummary(
            id: id,
            title: title,
            responsibleSubjectId: responsibleSubjectId,
            runtimeLocation: runtimeLocation,
            captainBotId: captainBotId,
            status: "active",
            createdAt: createdAt,
            updatedAt: updatedAt,
            parentCrewIds: parentCrewIds,
            captainAgentKind: captainAgentKind,
            machineId: machineId,
            attentionReason: attentionReason,
            manuallyHiddenAt: manuallyHiddenAt
        )
    }

    var detail: CrewDetail {
        CrewDetail(
            crew: CrewDetail.CrewBody(
                id: id,
                title: title,
                responsibleSubjectId: responsibleSubjectId,
                runtimeLocation: runtimeLocation,
                workingDirectory: workingDirectory,
                captainBotId: captainBotId,
                status: "active",
                createdAt: createdAt,
                updatedAt: updatedAt,
                captainAgentKind: captainAgentKind
            ),
            // 本地 crew 没有 DAG / 责任分账 —— 全空数组。后续 Phase 接本机
            // captain template 时也只动 captain section,parents/children
            // 这些跨 crew 关系本地路径不开。
            parents: [],
            children: [],
            shares: [
                CrewDetail.ResponsibilityShare(
                    subjectId: responsibleSubjectId,
                    shareBps: 10000,
                    isTiebreaker: true,
                    // 面向人的名字统一叫「人」—— 与成员列表里那一项、侧栏身份区
                    // 逐字一致(用户定调:不写"本机"、不带括号后缀)。
                    displayName: "人",
                    kind: "byok"
                )
            ],
            captain: captainBotId.map {
                // 旧 JSON 里默认名存的是英文 "Captain" —— 归一成「机长」，
                // 与群聊 @ 候选 / 气泡 / 成员列表的显示名统一（用户点名的
                // 「成员列表叫 Captain、群聊叫机长」分裂就来自这里）。
                CrewDetail.Captain(
                    botId: $0,
                    displayName: (captainName == "Captain" ? nil : captainName) ?? "机长")
            }
        )
    }
}

/// 本地 crew 的一个持久 session 成员。session 拉起时登记,退出后仍保留
/// (成员资格 ≠ 存活状态;live 状态由各视图 merge 在跑的 run 补上)。
struct LocalSessionMember: Codable, Equatable {
    let sessionId: String
    let displayName: String
    let createdAt: String
    /// 通讯录分机号（`7-3` 里的 3）。登记时发一次，终身绑这个 sessionId ——
    /// `restartMember` 复用原 sessionId 重启时不重发。optional 只为解码旧 JSON。
    var extensionNumber: Int? = nil
}

/// `local-crews.json` 的文件形状。**internal**：`CrewDirectory` 在 helper 子进程
/// 解同一份（跨进程只读的既有范式，见 `LocalCrewStore.orgTreeLines`）。
struct LocalCrewFile: Codable {
    /// schema 版本号,以后 shape 改了走 migration。
    let version: Int
    let crews: [LocalCrew]
    /// 全机「下一个可发的 crew 号」（通讯录）。缺键 = 通讯录之前的旧文件，
    /// 加载时按存量回填算出来（decodeIfPresent —— 旧文件照样解得开）。
    var nextCrewNumber: Int? = nil
}

/// 本地 crew store 的错误。目前只覆盖 DAG 父边操作。
enum LocalCrewStoreError: LocalizedError {
    /// 挂父边会让本地 DAG 成环(parentCrewId 是 crewId 的后代,或自挂自)。
    case wouldCreateCycle
    /// 引用了不存在的本地 crew。
    case crewNotFound(String)
    /// release 的操作对象不是发起 crew 的直系子（上级只能动直系子）。
    case notDirectChild(String)

    var errorDescription: String? {
        switch self {
        case .wouldCreateCycle:
            return "挂到这个父 crew 会形成环(它已经是当前 crew 的子孙)"
        case .crewNotFound(let id):
            return "本地 crew \(id) 不存在"
        case .notDirectChild(let id):
            return "crew \(id) 不是本 crew 的直系子,不能操作"
        }
    }
}
