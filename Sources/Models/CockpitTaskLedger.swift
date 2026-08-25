import Foundation

/// 驾驶舱「任务」段的**账本来源判定** + 三段式归并（#542）。
///
/// 病根：驾驶舱原来只读仓库里的 `docs/tasks/*.md`(对账三本账里的 task 那本)。那本账
/// 自 2026-07-07 起就没人再写(4 个文件、其中只有 1 条非终态) —— 而 crew 真正在跑的活
/// 全在 coding agent 的**活跃 task 账**里(`~/.claude/tasks/<id>/*.json`，`<id>` 由仓库
/// `.claude/settings.json` 的 `env.CLAUDE_CODE_TASK_LIST_ID` 指定)。所以任务段看到的
/// 是**一本死账**：这是数据源失养，不是渲染问题，把陈旧数据画好看没有意义。
///
/// 这里把「读哪本」收成一处**纯判定**(可单测)：活跃账读得到就读活跃账，读不到才回落
/// 仓库 markdown 账，并把回落这件事**如实标出来**。
enum CockpitTaskLedger {

    // MARK: - 来源判定

    /// 任务段实际读到的账本。
    enum Source: Equatable {
        /// coding agent 的活跃 task 账（`~/.claude/tasks/<id>/`）。
        case live(id: String, dir: URL)
        /// 回落：仓库 `docs/tasks/*.md`（活跃账没配 / 目录空 / 读不到）。
        case repoLedger(reason: String)
    }

    /// 从 `.claude/settings.json` 原文抽 `env.CLAUDE_CODE_TASK_LIST_ID`。
    /// `settings.local.json` 优先（本机覆盖 checked-in 配置）；都没有 → nil。
    static func taskListId(settings: String?, localSettings: String?) -> String? {
        for raw in [localSettings, settings] {
            if let id = envValue(raw, key: "CLAUDE_CODE_TASK_LIST_ID"), !id.isEmpty {
                return id
            }
        }
        return nil
    }

    /// 判定读哪本账。`dirHasTasks` 注入目录探测，便于单测不碰真实文件系统。
    static func resolve(taskListId: String?, tasksHome: URL,
                        dirHasTasks: (URL) -> Bool) -> Source {
        guard let id = taskListId, !id.isEmpty else {
            return .repoLedger(reason: "这个仓库的 .claude/settings.json 没配 CLAUDE_CODE_TASK_LIST_ID")
        }
        let dir = tasksHome.appendingPathComponent(id, isDirectory: true)
        guard dirHasTasks(dir) else {
            return .repoLedger(reason: "活跃 task 账目录空或读不到（\(dir.path)）")
        }
        return .live(id: id, dir: dir)
    }

    /// `~/.claude/tasks`。
    static func defaultTasksHome(home: URL = currentHome) -> URL {
        home.appendingPathComponent(".claude/tasks", isDirectory: true)
    }

    /// 用户家目录。`homeDirectoryForCurrentUser` 在 iOS 上不可用（整个 iOS
    /// target 都因此编不过），而活跃 task 账本来就只有 Mac 侧有 —— iOS 上退回
    /// 沙盒 home，读不到就按既有逻辑回落仓库账，不额外分支。
    static var currentHome: URL {
        #if os(macOS)
        FileManager.default.homeDirectoryForCurrentUser
        #else
        URL(fileURLWithPath: NSHomeDirectory())
        #endif
    }

    // MARK: - 活跃账条目解析

    /// 活跃 task 账的一个 JSON 文件。字段只取渲染要用的那几个，其余(blocks/blockedBy/
    /// description)不解析 —— 驾驶舱是 glance，不是任务详情页。
    static func parseLiveTask(_ data: Data, updated: Date?) -> CockpitTaskItem? {
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = (obj["id"] as? String) ?? (obj["id"] as? Int).map(String.init),
              !id.isEmpty
        else { return nil }
        let subject = (obj["subject"] as? String) ?? ""
        let status = (obj["status"] as? String) ?? ""
        return CockpitTaskItem(
            id: id,
            title: subject.isEmpty ? "#\(id)" : subject,
            statusRaw: status,
            origin: .live,
            updated: updated,
            badge: "#\(id)")
    }

    // MARK: - 三段式归并

    /// status → 段。人类要的是「做了什么 / 在做什么 / 接下来做什么」，所以状态**只分三档**，
    /// 不再按 5 个原始状态铺成看板列。`dropped`(作废)返 nil —— 不占版面。
    ///
    /// 各本账的状态词都吃：活跃账 `pending / in_progress / completed`；
    /// 仓库账 `todo / doing / pending-qa / done / dropped`(可能是 `partial, pending-qa` 这种复合值)；
    /// 人类 Todo `pending / in_progress / completed`；机长作战板
    /// `not_started / in_progress / blocked / done`（Todo #66）。
    static func band(_ statusRaw: String) -> CockpitBand? {
        let s = statusRaw.lowercased().trimmingCharacters(in: .whitespaces)
        if s.isEmpty { return .next }
        if s == "dropped" { return nil }
        if s == "completed" || s.hasPrefix("done") { return .done }
        if s == "pending" || s == "todo" || s == "not_started" || s.hasPrefix("planned") { return .next }
        // 「卡住」归**在做**那一段，不归「接下来」：它是开了工却推不动的活，摆进「接下来
        // 做什么」等于把一件正卡着人的事描述成还没开始。唯一产出这个词的是机长作战板
        // （`CockpitPlanStatus.blocked`）—— 另外几本账的状态词里没有 blocked（活跃账
        // pending/in_progress/completed；仓库账 todo/doing/pending-qa/done/dropped；
        // 人类 Todo pending/in_progress/completed），所以这条改动不影响它们。
        if s == "blocked" { return .doing }
        // 其余都算「在做」：in_progress / doing / partial / pending-qa / partial, pending-qa …
        return .doing
    }

    /// 归成三段。「做了什么」按最近更新截断(默认 8 条) —— 完成的活会攒到几百条，
    /// 全铺出来就又变成密集统计；只留最近几条，其余是历史、不该糊在脸上。
    static func bands(_ items: [CockpitTaskItem], doneLimit: Int = 8) -> [CockpitBandGroup] {
        var buckets: [CockpitBand: [CockpitTaskItem]] = [:]
        for item in items {
            guard let b = band(item.statusRaw) else { continue }
            buckets[b, default: []].append(item)
        }
        return CockpitBand.allCases.map { band in
            var list = (buckets[band] ?? []).sorted(by: newerFirst)
            let total = list.count
            if band == .done, list.count > doneLimit { list = Array(list.prefix(doneLimit)) }
            return CockpitBandGroup(band: band, items: list, hiddenCount: total - list.count)
        }
    }

    /// 更新时间新的在前；没有时间戳的排最后（同为 nil 时按 id 倒序，新 id 在前）。
    private static func newerFirst(_ a: CockpitTaskItem, _ b: CockpitTaskItem) -> Bool {
        switch (a.updated, b.updated) {
        case let (x?, y?): return x == y ? idNewerFirst(a, b) : x > y
        case (_?, nil): return true
        case (nil, _?): return false
        case (nil, nil): return idNewerFirst(a, b)
        }
    }

    private static func idNewerFirst(_ a: CockpitTaskItem, _ b: CockpitTaskItem) -> Bool {
        if let x = Int(a.id), let y = Int(b.id) { return x > y }
        return a.id > b.id
    }

    // MARK: - helpers

    /// 从 settings JSON 原文里取 `env.<key>`。解析不了就当没有（配置坏了不该让驾驶舱崩）。
    private static func envValue(_ raw: String?, key: String) -> String? {
        guard let raw, let data = raw.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let env = obj["env"] as? [String: Any]
        else { return nil }
        return env[key] as? String
    }
}

// MARK: - Loader（FileManager；判定逻辑全在上面的纯函数里）

enum CockpitTaskLedgerLoader {
    /// 给定 crew 工作目录，判定它的任务段该读哪本账。
    static func resolve(crewRoot: URL,
                        tasksHome: URL = CockpitTaskLedger.defaultTasksHome()) -> CockpitTaskLedger.Source {
        let dotClaude = crewRoot.appendingPathComponent(".claude", isDirectory: true)
        let id = CockpitTaskLedger.taskListId(
            settings: readString(dotClaude.appendingPathComponent("settings.json")),
            localSettings: readString(dotClaude.appendingPathComponent("settings.local.json")))
        return CockpitTaskLedger.resolve(taskListId: id, tasksHome: tasksHome) { dir in
            !jsonFiles(in: dir).isEmpty
        }
    }

    /// 读活跃账目录下的全部 task。`updated` 取文件 mtime —— 活跃账 JSON 里没有时间戳，
    /// 而「最近在做什么」必须按时间排，mtime 是这本账唯一诚实的时间来源。
    static func loadLive(dir: URL) -> [CockpitTaskItem] {
        jsonFiles(in: dir).compactMap { url in
            guard let data = try? Data(contentsOf: url) else { return nil }
            let mtime = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate
            return CockpitTaskLedger.parseLiveTask(data, updated: mtime)
        }
    }

    private static func jsonFiles(in dir: URL) -> [URL] {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(
            at: dir, includingPropertiesForKeys: [.contentModificationDateKey],
            options: [.skipsHiddenFiles])
        else { return [] }
        return items.filter { $0.pathExtension == "json" }
    }

    private static func readString(_ url: URL) -> String? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}

// MARK: - 展示模型

/// 三段式 glance 的段 —— 人类的原话：「一眼看出做了什么 / 在做什么 / 接下来做什么」。
/// 顺序按人读的顺序：先看在做的，再看接下来，最后回望做过的。
enum CockpitBand: String, CaseIterable, Identifiable {
    case doing = "在做什么"
    case next = "接下来做什么"
    case done = "做了什么"
    var id: String { rawValue }
}

struct CockpitBandGroup: Equatable, Identifiable {
    let band: CockpitBand
    let items: [CockpitTaskItem]
    /// 被截断掉的条数（只有「做了什么」会有）。
    let hiddenCount: Int
    var id: String { band.rawValue }
}

/// 任务段展示用的统一条目 —— 活跃账 / 仓库 markdown 账 / 人类 Todo 三种来源共用一个形状，
/// 这样三段式里能把「人给的活」和「机器账上的活」摆在同一段里，不再各占一个 tab。
struct CockpitTaskItem: Identifiable, Equatable {
    enum Origin: Equatable {
        case live         // coding agent 活跃 task 账
        case repoLedger   // 仓库 docs/tasks/*.md
        case humanTodo    // 人类 Todo 列表（每 crew 一份）
        /// 机长自己排的作战板（`CockpitPlanStore`，人类 Todo #66）。**它是这里唯一
        /// 由机长第一手写下的来源** —— 另外三种都是从别人的账上读来的。加进来是为了
        /// 让 glance 不漏掉机长的计划；不加它就会变成一座孤岛（自己有视图、却不进
        /// 「在做什么 / 接下来 / 做了什么」这个总摘要）。
        case captainPlan
    }

    let id: String
    let title: String
    let statusRaw: String
    let origin: Origin
    let updated: Date?
    /// 行尾的短标记（task 号 / Todo #N）。
    let badge: String
    /// 附一行从属信息（Todo 的最新回应等），可空。
    var note: String = ""
}
