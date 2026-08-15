import Foundation

/// 驾驶舱(cockpit)的数据模型 + 轻量解析。
///
/// cockpit.md:驾驶舱是[对账](../../../docs/handbook/pendingcrew/concepts/reconciliation.md)的**纯呈现面**——
/// 读三本账(期望 docs/handbook / 现状 docs/state / task docs/tasks)摆出来,**差靠人扫两栏读**,
/// 不替人判 drift、不做一致性检查。状态走 frontmatter `status:` 渲成色块,正文交给 MarkdownText 原样渲、
/// **不另解析**(不把文档解析成 AST)。本文件只做「抽 status / 现状一句话 / task 链」这点轻解析。

// MARK: - Types

/// 一个 topic = 一条现状条(docs/state 的 `## <key>`),并知道它对应的期望页路径。
struct CockpitTopic: Identifiable, Equatable {
    let area: String        // "pendingcrew" / "pendingbot" / "shared"
    let key: String         // "concepts/cockpit" / "vision" / "features/three-pane-ui"
    let status: String      // 原始 status 值,如 "partial, pending-qa"
    let summary: String      // 现状 一句话
    let evidence: String     // 证据
    let taskIds: [String]    // task: 行里抽出的新账本 id(yyyymmdd-xx),用于链 task 卡
    let taskRaw: String      // task: 行原文(展示兜底)

    var id: String { "\(area)/\(key)" }
    /// 期望页相对 docs/handbook 的路径(无扩展名),如 "pendingcrew/concepts/cockpit"。
    var expectationRelpath: String { "\(area)/\(key)" }
}

/// 一个 task = docs/tasks/<id>.md 的 frontmatter。
struct CockpitTask: Identifiable, Equatable {
    let id: String
    let title: String
    let status: String      // todo / doing / pending-qa / done / dropped
    let owner: String
    let crew: String        // 归属 crew(驾驶舱沿 DAG 子树过滤);~ = 无
    let expects: [String]
    let state: [String]
    let deps: [String]
    let updated: String     // 时间戳(dashboard 排「最近」用)
}

/// handbook 目录树节点(期望页以目录形式浏览)。
struct HandbookNode: Identifiable, Equatable {
    let id: String          // 相对 docs/handbook 的路径,如 "pendingcrew/concepts" / "pendingcrew/concepts/crew"
    let name: String        // 显示名(文件夹名 / 页末段)
    let isPage: Bool        // true = .md 页;false = 文件夹
    var children: [HandbookNode]
    var pageRelpath: String? { isPage ? id : nil }   // 渲染:docs/handbook/<id>.md
    /// OutlineGroup 用:叶子返 nil(才不画展开箭头),文件夹返非空。
    var childrenOrNil: [HandbookNode]? { children.isEmpty ? nil : children }
}

/// status → 桶(纯展示分组,**不判 drift**)。现状 planned/partial/done(+pending-qa);task todo/doing/…
func cockpitStatusBucket(_ raw: String) -> String {
    let s = raw.lowercased()
    if s.contains("pending-qa") { return "pending-qa" }
    if s.hasPrefix("done") { return "done" }
    if s.hasPrefix("partial") || s == "doing" { return "partial" }
    if s.hasPrefix("planned") || s == "todo" { return "planned" }
    if s == "dropped" { return "dropped" }
    return "other"
}

/// 路线账(docs/roadmap.md,第四本账)——期望的时间切片。阶段顺序 = 文件顺序;
/// 进度不落盘 —— 阶段归「在做 / 接下来 / 做过」三段直接读人拍板的 status(#542 起不再
/// 从现状账聚合出 done/total 计数,那是被砍掉的密集统计)。
struct CockpitPhaseEntry: Equatable {
    let relpath: String   // 期望页 relpath,同 CockpitTopic.expectationRelpath 坐标系
    let note: String      // " — " 后的备注,可空
}

/// 阶段之下的一层**分组**（路线账里的 `### 组名`）—— 地图的比例尺:阶段太粗、条目太细,
/// 中间这层让人先看「哪块在动」再决定放大看哪几条。没写 `###` 的老格式归一个隐式默认组
/// (`name` 空),渲染时直接摊条目、不占一行。
struct CockpitPhaseGroup: Identifiable, Equatable {
    let name: String
    let entries: [CockpitPhaseEntry]
    var id: String { name }
    /// 隐式默认组(阶段下直接写条目,没有 `###`)。
    var isImplicit: Bool { name.isEmpty }
}

struct CockpitPhase: Identifiable, Equatable {
    let name: String      // "## " 后的阶段名
    let status: String    // planned / doing / done / dropped(人拍板,captain 只提议)
    let target: String    // 可空,粗粒度时间锚,不做到期检测
    let goal: String      // 目标: 一句话
    let groups: [CockpitPhaseGroup]
    var id: String { name }
    /// 全阶段条目拍平(阶段级聚合进度、跨账定位都按它算)。
    var entries: [CockpitPhaseEntry] { groups.flatMap(\.entries) }
}

/// 一批条目的就绪度 —— 地图上的刻度。**两档**(人类 Todo #31 拍板):
/// `verified` = 现状 status 落 `done` 桶(已验证);`awaitingQA` = 落 `pending-qa` 桶
/// (代码做完、卡在真机/观感 QA)。
///
/// 此前只数 `done`、pending-qa 一律不计 —— 而这个项目几乎所有活都停在 QA 那一步,于是
/// 做完的全被算成没做(v1 收口阶段长期显示 `0/15`,与事实完全不符,人类因此问「驾驶舱还
/// 有什么用」)。两档**分开可见**:既不合并成一个数把待验说成已验,也不再把它当零。
/// **纯展示聚合,不判 drift**(cockpit.md 边界)。
struct CockpitProgress: Equatable {
    let verified: Int
    let awaitingQA: Int
    let total: Int
    /// 实心那截 —— 已验证。
    var fraction: Double { total == 0 ? 0 : Double(verified) / Double(total) }
    /// 浅色那截的右缘 —— 已验证 + 做完待验(画在实心之下,两者的差就是"待验"那段)。
    var settledFraction: Double { total == 0 ? 0 : Double(verified + awaitingQA) / Double(total) }
    var isEmpty: Bool { total == 0 }
    /// 紧凑标签:有待验的写成 `3+9/15`,没有就退回 `3/15`。
    var label: String { awaitingQA == 0 ? "\(verified)/\(total)" : "\(verified)+\(awaitingQA)/\(total)" }
    /// 完整读法(tooltip 用,别让 `+` 号靠猜)。
    var longLabel: String { "已验证 \(verified) · 做完待验 \(awaitingQA) · 共 \(total)" }
}

enum CockpitRoadmapProgress {
    /// 纯函数(不碰磁盘):条目 + 「relpath → 现状 status」表 → 两档就绪度。
    /// 没入现状账的条目两档都不计(只进 total)。
    static func of(_ entries: [CockpitPhaseEntry], statusByRelpath: [String: String]) -> CockpitProgress {
        var verified = 0, awaiting = 0
        for e in entries {
            switch cockpitStatusBucket(statusByRelpath[e.relpath] ?? "") {
            case "done": verified += 1
            case "pending-qa": awaiting += 1
            default: break
            }
        }
        return CockpitProgress(verified: verified, awaitingQA: awaiting, total: entries.count)
    }
}

struct CockpitRoadmap: Equatable {
    let preamble: String  // 第一个 "## " 之前的 markdown 原文(主线声明)
    let phases: [CockpitPhase]
}

/// 一次加载出的三本账数据。
struct CockpitData {
    var topics: [CockpitTopic]
    /// 仓库 markdown task 账(docs/tasks/*.md)—— 现状条 / roadmap 的跨账链还挂在它上面。
    var tasks: [CockpitTask]
    /// 任务段实际读的那本账（#542：活跃账优先,读不到才回落这里的 `tasks`）。
    var taskSource: CockpitTaskLedger.Source = .repoLedger(reason: "")
    /// 任务段的统一条目（来自 `taskSource` 那本账）。
    var taskItems: [CockpitTaskItem] = []
    /// 期望 handbook 目录树(期望页以目录形式浏览)。
    var handbookTree: [HandbookNode]
    /// docs/handbook 绝对目录,用于详情页按需读期望页 markdown。
    var handbookDir: URL
    /// 路线账(docs/roadmap.md);文件不存在为 nil(路线段渲空态引导)。
    var roadmap: CockpitRoadmap?
    /// 期望页 relpath → 现状 status。**加载期一次算好** —— 地图上每行都要按 relpath 找现状,
    /// 逐行 `topics.first {}` 是 O(n²)。
    var statusByRelpath: [String: String] = [:]
    /// 路线账里真实存在的期望页 relpath。同样**加载期一次 stat 好** —— 原来每行 body 里
    /// 做一次同步 `FileManager.fileExists`,滚动时每帧重跑。
    var roadmapPagesPresent: Set<String> = []

    func task(_ id: String) -> CockpitTask? { tasks.first { $0.id == id } }

    /// 一批路线条目的就绪度(地图刻度)。
    func progress(_ entries: [CockpitPhaseEntry]) -> CockpitProgress {
        CockpitRoadmapProgress.of(entries, statusByRelpath: statusByRelpath)
    }
}

// MARK: - Pure parsing (Foundation only — swiftc 可单测)

enum CockpitParser {
    /// 解析 docs/state/<area>.md 成 topics。
    static func parseStateFile(area: String, content: String) -> [CockpitTopic] {
        var topics: [CockpitTopic] = []
        var key: String?
        var status = "", summary = "", evidence = "", taskRaw = ""
        func flush() {
            guard let k = key else { return }
            topics.append(CockpitTopic(
                area: area, key: k, status: status, summary: summary,
                evidence: evidence, taskIds: extractTaskIds(taskRaw), taskRaw: taskRaw))
        }
        for line in content.components(separatedBy: "\n") {
            if let h = headingKey(line) {
                flush()
                key = h; status = ""; summary = ""; evidence = ""; taskRaw = ""
                continue
            }
            if key == nil { continue }
            if let v = field(line, "status") { status = v }
            else if let v = field(line, "现状") { summary = v }
            else if let v = field(line, "证据") { evidence = v }
            else if let v = field(line, "task") { taskRaw = v }
        }
        flush()
        return topics
    }

    /// 解析 docs/tasks/<id>.md 的 frontmatter 成 CockpitTask。
    static func parseTaskFile(_ content: String) -> CockpitTask? {
        guard let fm = frontmatter(content) else { return nil }
        let id = fmScalar(fm, "id")
        guard !id.isEmpty else { return nil }
        return CockpitTask(
            id: id,
            title: fmScalar(fm, "title"),
            status: fmScalar(fm, "status"),
            owner: fmScalar(fm, "owner"),
            crew: fmScalar(fm, "crew"),
            expects: fmList(fm, "expects"),
            state: fmList(fm, "state"),
            deps: fmList(fm, "deps"),
            updated: fmScalar(fm, "updated"))
    }

    /// 从现状条 task: 行抽新账本 id(yyyymmdd-xx);#NNN / Tx.y 忽略。
    static func extractTaskIds(_ value: String) -> [String] {
        var out: [String] = []
        let scalars = Array(value)
        var i = 0
        while i < scalars.count {
            // 找 8 位数字 + '-' + 2 位 [a-z0-9]
            if i + 11 <= scalars.count,
               scalars[i...(i + 7)].allSatisfy({ $0.isNumber }),
               scalars[i + 8] == "-",
               isIdSuffix(scalars[i + 9]), isIdSuffix(scalars[i + 10]) {
                // 边界:前一个不是数字/字母,后一个不是字母数字
                let prevOK = i == 0 || !(scalars[i - 1].isNumber || scalars[i - 1].isLetter)
                let nextIdx = i + 11
                let nextOK = nextIdx >= scalars.count || !(scalars[nextIdx].isNumber || scalars[nextIdx].isLetter)
                if prevOK && nextOK {
                    out.append(String(scalars[i...(i + 10)]))
                    i += 11
                    continue
                }
            }
            i += 1
        }
        return out
    }

    /// 解析 docs/roadmap.md。第一个 "## " 前是 preamble;每节 = 一阶段,
    /// 字段裸行(status/target/目标),`### 组名` = 阶段内分组,列表项 = 条目(relpath [— 备注])。
    /// 没写 `###` 的老格式照吃 —— 条目归一个隐式默认组。
    static func parseRoadmapFile(_ content: String) -> CockpitRoadmap {
        var preamble: [String] = []
        var phases: [CockpitPhase] = []
        var name: String?
        var status = "", target = "", goal = ""
        var groups: [CockpitPhaseGroup] = []
        var groupName: String?              // nil = 还没遇到 ###(隐式默认组)
        var entries: [CockpitPhaseEntry] = []
        /// 显式组即使空也留(人写了 ### 就是要看见这块);隐式组空则不产生。
        func flushGroup() {
            if groupName != nil || !entries.isEmpty {
                groups.append(CockpitPhaseGroup(name: groupName ?? "", entries: entries))
            }
            entries = []
        }
        func flush() {
            guard let n = name else { return }
            flushGroup()
            phases.append(CockpitPhase(name: n, status: status, target: target, goal: goal, groups: groups))
            groups = []; groupName = nil
        }
        for line in content.components(separatedBy: "\n") {
            if let h = headingKey(line) {
                flush()
                name = h; status = ""; target = ""; goal = ""
                continue
            }
            if name == nil { preamble.append(line); continue }
            if let g = groupKey(line) { flushGroup(); groupName = g; continue }
            if let v = field(line, "status") { status = v }
            else if let v = field(line, "target") { target = v }
            else if let v = field(line, "目标") { goal = v }
            else if let e = entryItem(line) { entries.append(e) }
        }
        flush()
        return CockpitRoadmap(
            preamble: preamble.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines),
            phases: phases)
    }

    // MARK: helpers

    private static func headingKey(_ line: String) -> String? {
        guard line.hasPrefix("## "), !line.hasPrefix("### ") else { return nil }
        return String(line.dropFirst(3)).trimmingCharacters(in: .whitespaces)
    }

    /// 路线账阶段内的分组标题 `### 组名`。
    private static func groupKey(_ line: String) -> String? {
        guard line.hasPrefix("### "), !line.hasPrefix("#### ") else { return nil }
        return String(line.dropFirst(4)).trimmingCharacters(in: .whitespaces)
    }

    private static func field(_ line: String, _ name: String) -> String? {
        for sep in [":", "："] {
            let prefix = name + sep
            if line.hasPrefix(prefix) {
                return String(line.dropFirst(prefix.count)).trimmingCharacters(in: .whitespaces)
            }
        }
        return nil
    }

    private static func isIdSuffix(_ c: Character) -> Bool {
        c.isNumber || (c.isLetter && c.isLowercase)
    }

    /// "- <relpath>[ — <note>]" → 条目;其他行忽略。
    private static func entryItem(_ line: String) -> CockpitPhaseEntry? {
        let t = line.trimmingCharacters(in: .whitespaces)
        guard t.hasPrefix("- ") else { return nil }
        let body = String(t.dropFirst(2)).trimmingCharacters(in: .whitespaces)
        guard !body.isEmpty else { return nil }
        if let r = body.range(of: " — ") {
            return CockpitPhaseEntry(
                relpath: String(body[..<r.lowerBound]).trimmingCharacters(in: .whitespaces),
                note: String(body[r.upperBound...]).trimmingCharacters(in: .whitespaces))
        }
        return CockpitPhaseEntry(relpath: body, note: "")
    }

    /// 抽 frontmatter(首个 `---` … `---` 之间)。
    private static func frontmatter(_ content: String) -> String? {
        let lines = content.components(separatedBy: "\n")
        guard lines.first == "---" else { return nil }
        var body: [String] = []
        for line in lines.dropFirst() {
            if line == "---" { return body.joined(separator: "\n") }
            body.append(line)
        }
        return nil
    }

    private static func fmScalar(_ fm: String, _ key: String) -> String {
        for line in fm.components(separatedBy: "\n") {
            if line.hasPrefix(key + ":") {
                return String(line.dropFirst(key.count + 1)).trimmingCharacters(in: .whitespaces)
            }
        }
        return ""
    }

    private static func fmList(_ fm: String, _ key: String) -> [String] {
        let raw = fmScalar(fm, key)
        var inner = raw.trimmingCharacters(in: .whitespaces)
        if inner.hasPrefix("[") { inner.removeFirst() }
        if inner.hasSuffix("]") { inner.removeLast() }
        return inner.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }
}

// MARK: - Loader (FileManager)

enum CockpitLoader {
    /// 空账本 —— crew 没配工作目录时给不依赖账本的段（任务段的人类 Todo）当底座。
    static let empty = CockpitData(
        topics: [], tasks: [], handbookTree: [],
        handbookDir: URL(fileURLWithPath: "/"), roadmap: nil)

    /// 从 crew 的工作目录读三本账。
    static func load(crewRoot: URL) -> CockpitData {
        let stateDir = crewRoot.appendingPathComponent("docs/state", isDirectory: true)
        let tasksDir = crewRoot.appendingPathComponent("docs/tasks", isDirectory: true)
        let handbookDir = crewRoot.appendingPathComponent("docs/handbook", isDirectory: true)

        var topics: [CockpitTopic] = []
        for file in markdownFiles(in: stateDir) {
            let area = file.deletingPathExtension().lastPathComponent
            if let content = readString(file) {
                topics += CockpitParser.parseStateFile(area: area, content: content)
            }
        }

        var tasks: [CockpitTask] = []
        for file in markdownFiles(in: tasksDir) where file.lastPathComponent != "README.md" {
            if let content = readString(file), let t = CockpitParser.parseTaskFile(content) {
                tasks.append(t)
            }
        }

        let roadmapFile = crewRoot.appendingPathComponent("docs/roadmap.md")
        let roadmap = readString(roadmapFile).map(CockpitParser.parseRoadmapFile)

        // 任务段读哪本账（#542）：活跃 task 账优先,读不到才回落上面这本 markdown。
        let source = CockpitTaskLedgerLoader.resolve(crewRoot: crewRoot)
        let items: [CockpitTaskItem]
        switch source {
        case .live(_, let dir): items = CockpitTaskLedgerLoader.loadLive(dir: dir)
        case .repoLedger:       items = tasks.map(repoItem)
        }

        // 地图渲染要的两张索引一次算好(见 CockpitData 上的注释):现状表 + 期望页存在性。
        var statusByRelpath: [String: String] = [:]
        for t in topics { statusByRelpath[t.expectationRelpath] = t.status }
        var present: Set<String> = []
        let fm = FileManager.default
        for entry in roadmap?.phases.flatMap(\.entries) ?? []
        where fm.fileExists(atPath: handbookDir.appendingPathComponent("\(entry.relpath).md").path) {
            present.insert(entry.relpath)
        }

        return CockpitData(
            topics: topics, tasks: tasks,
            taskSource: source, taskItems: items,
            handbookTree: handbookTree(handbookDir: handbookDir),
            handbookDir: handbookDir,
            roadmap: roadmap,
            statusByRelpath: statusByRelpath,
            roadmapPagesPresent: present)
    }

    /// 仓库 markdown task → 统一条目。`updated` 是 `2026-06-26` 这种日期串。
    private static func repoItem(_ t: CockpitTask) -> CockpitTaskItem {
        CockpitTaskItem(
            id: t.id,
            title: t.title.isEmpty ? t.id : t.title,
            statusRaw: t.status,
            origin: .repoLedger,
            updated: isoDay.date(from: t.updated),
            badge: t.id)
    }

    private static let isoDay: DateFormatter = {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .gregorian)
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    /// 读期望页 markdown(详情页用)。relpath 形如 "pendingcrew/concepts/cockpit"。
    static func readExpectation(handbookDir: URL, relpath: String) -> String? {
        readString(handbookDir.appendingPathComponent("\(relpath).md"))
    }

    /// 期望 handbook 目录树 —— 文件夹/页递归。文件夹在前、页在后,各按名排。
    /// 仓库级一份;per-crew 沿 DAG 切片是显示层的事(cockpit.md:派生无损、零额外存储)。
    static func handbookTree(handbookDir: URL) -> [HandbookNode] {
        buildNodes(dir: handbookDir, prefix: "")
    }

    private static func buildNodes(dir: URL, prefix: String) -> [HandbookNode] {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(
            at: dir, includingPropertiesForKeys: [.isDirectoryKey], options: [.skipsHiddenFiles])
        else { return [] }
        var folders: [HandbookNode] = []
        var pages: [HandbookNode] = []
        for url in items {
            let isDir = (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false
            if isDir {
                let name = url.lastPathComponent
                let rel = prefix.isEmpty ? name : "\(prefix)/\(name)"
                let kids = buildNodes(dir: url, prefix: rel)
                if !kids.isEmpty {
                    folders.append(HandbookNode(id: rel, name: name, isPage: false, children: kids))
                }
            } else if url.pathExtension == "md" {
                let base = url.deletingPathExtension().lastPathComponent
                if base == "README" { continue }
                let rel = prefix.isEmpty ? base : "\(prefix)/\(base)"
                pages.append(HandbookNode(id: rel, name: base, isPage: true, children: []))
            }
        }
        folders.sort { $0.name < $1.name }
        pages.sort { $0.name < $1.name }
        return folders + pages
    }

    private static func markdownFiles(in dir: URL) -> [URL] {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) else { return [] }
        return items.filter { $0.pathExtension == "md" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    private static func readString(_ url: URL) -> String? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}
