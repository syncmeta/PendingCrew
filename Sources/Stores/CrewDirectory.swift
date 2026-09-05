// 纯 Foundation、无平台门 —— app 侧发号，helper 子进程（MCP `directory` /
// `contact` 工具）只读。两边解的是同一份 `local-crews.json` + `crew-sessions.json`，
// **不另开号码注册表**（这个项目吃过「两本账漂移」的亏）。
import Foundation

/// 一个通讯录号码：`7`（整个 crew）或 `7-3`（某个成员分机）。
///
/// 编号规则（已由人类拍板，别自行改）：
/// * crew 号全机唯一、从 1 自增、**层级完全不参与编号**（不是 `1.2.3`）；
/// * 分机 `-1` **恒定是该 crew 的机长**，worker 按加入顺序拿 2、3、4…；
/// * 号码终身不变（crew 换爹不重发）、**永不回收**（session 退出、crew 删除后
///   号码不再分配给别人，旧记录里的号码永远解析得出当初是谁）；
/// * 人类不编号 —— 找人仍走 `ask`。
struct CrewPhoneNumber: Equatable, Hashable {
    /// 分机 1 恒归机长。
    static let captainExtension = 1

    let crew: Int
    /// nil = 整个 crew（`7`，群里广播发言）。
    let ext: Int?

    var isCaptain: Bool { ext == Self.captainExtension }

    var text: String { ext.map { "\(crew)-\($0)" } ?? "\(crew)" }

    /// 解析 `"7"` / `"7-3"`。宽容之处只有前后空白与全角连字符；其余一律拒绝
    /// （空、`7-`、`7-0`、`7-3-2`、非数字、负数）—— 查无此号必须报错，不能猜。
    static func parse(_ raw: String) -> CrewPhoneNumber? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "－", with: "-")   // 全角连字符
            .replacingOccurrences(of: "—", with: "-")   // 破折号（中文输入法常见）
        guard !trimmed.isEmpty else { return nil }
        let parts = trimmed.split(separator: "-", omittingEmptySubsequences: false)
        func positive(_ s: Substring) -> Int? {
            guard !s.isEmpty, s.allSatisfy(\.isASCII), s.allSatisfy(\.isNumber),
                  let v = Int(s), v >= 1 else { return nil }
            return v
        }
        switch parts.count {
        case 1:
            guard let crew = positive(parts[0]) else { return nil }
            return CrewPhoneNumber(crew: crew, ext: nil)
        case 2:
            guard let crew = positive(parts[0]), let ext = positive(parts[1]) else { return nil }
            return CrewPhoneNumber(crew: crew, ext: ext)
        default:
            return nil
        }
    }
}

/// 通讯录里的一行。crew 与 session 同表 —— 一个号码一行。
struct CrewDirectoryEntry: Equatable {
    enum Kind: String, Equatable { case crew, session }

    let number: CrewPhoneNumber
    let name: String
    let kind: Kind
    /// 挂在哪个部门下（组织路径，根在最左；crew 行是它父辈的路径，session 行是
    /// 它所在 crew 的完整路径）。根 crew → 空串。
    let orgPath: String
    /// 在干什么（session 的任务简述 / crew 的规模概览）。没有 → 空串。
    let activity: String
    /// 在不在线（含已退出）。
    let status: String
    /// 归属 crew id（渲染分组 / contact 寻址用）。
    let crewId: String
}

/// `contact` 的投递目标 —— 号码解析出来的三种寻址方式。
enum CrewContactTarget: Equatable {
    /// `7` —— 在该 crew 群里广播发言（等同人类在群里无 @ 说话：机长会被唤醒）。
    case broadcast(crewId: String, crewTitle: String)
    /// `7-1` —— 定向 @ 该 crew 的机长。
    case captain(crewId: String, crewTitle: String, name: String)
    /// `7-3` —— 定向 @ 那个 session。
    case session(crewId: String, crewTitle: String, sessionId: String, name: String)

    var crewId: String {
        switch self {
        case .broadcast(let id, _), .captain(let id, _, _), .session(let id, _, _, _): return id
        }
    }

    var crewTitle: String {
        switch self {
        case .broadcast(_, let t), .captain(_, let t, _), .session(_, let t, _, _): return t
        }
    }

    /// 目标的人话描述（回执里用）。
    var displayName: String {
        switch self {
        case .broadcast(_, let t): return "\(t) 全群"
        case .captain(_, let t, let n): return "\(t) · \(n)"
        case .session(_, let t, _, let n): return "\(t) · \(n)"
        }
    }
}

/// 全机通讯录：把 `local-crews.json`（号码 + 组织边 + 持久成员）与
/// `crew-sessions.json`（成员实时状态快照）汇总成一张可查、可寻址的表。
///
/// 数据源本来就都有，此前只是没人汇总。两份文件都是共享文件层 —— MCP 工具跑在
/// **helper 子进程**里，碰不到 app 内存态，只能这么读（跨进程只读的既有范式见
/// `LocalCrewStore.orgTreeLines`）。
struct CrewDirectory {
    let crews: [LocalCrew]
    let sessions: CrewSessionsSnapshot

    init(crews: [LocalCrew], sessions: CrewSessionsSnapshot = CrewSessionsSnapshot()) {
        self.crews = crews
        self.sessions = sessions
    }

    /// 通讯录**读不出来**（区别于「本机还没建过 crew」）。
    ///
    /// 立这个类型是因为老实现把三种结局压成了两种：拿到了 / 文件不在 / 读不动·解不开，
    /// 后两种都变成 `[]`。**于是「我读不到」和「它不存在」在那一行之后就再也分不开。**
    /// 2026-09-05 那次断线里 `contact` 因此回了「查无此号 1-1」—— 人去查号码，
    /// 而号码是对的；同一个 `load()` 还喂着 `directory`，整张表显示成
    /// 「本机还没有登记在案的 crew」，那句更像真话、也更危险。
    struct Unavailable: Error {
        /// 哪份文件读不出来（人要据此去查是权限还是别的）。
        let file: String
        /// 系统给的原文，别加工。
        let reason: String

        /// 给工具回执用的一句话。**必须同时说清三件事**：读不出来、哪一份、
        /// 以及「所以我现在答不了」—— 最后那半是防止读的人把它当成业务结论。
        var message: String {
            "通讯录读不出来（\(file)）：\(reason.trimmedTrailingPeriods)。"
                + "所以我现在答不了这个号码存不存在 —— 不是那个号没登记过，是我读不到那份名单。"
        }
    }

    /// 从共享文件层加载。`whiteboardDirectory` = helper 的 `--dir`（白板目录）——
    /// `local-crews.json` 在其父目录，`crew-sessions.json` 与白板同级。
    ///
    /// **文件不在 = 真的空**（全新机器还没建过 crew）；**读不动 / 解不开 = 抛**。
    /// 这里刻意做成 `throws` 而不是「失败时回空 + 另给一个查询失败的方法」——
    /// 后者调用方仍然**可以**忽略失败拿到空表，而这正是要根除的那种写法。
    nonisolated static func load(whiteboardDirectory: URL) throws -> CrewDirectory {
        let crewFile = whiteboardDirectory.deletingLastPathComponent()
            .appendingPathComponent("local-crews.json")
        let snapshotFile = whiteboardDirectory
            .appendingPathComponent(CrewSessionsSnapshot.fileName)
        // 快照也得算：分机号（`7-3`）要靠 crew-sessions.json 才解得开，
        // 只管 local-crews.json 会漏掉一半。
        return CrewDirectory(
            crews: try decodeIfPresent(LocalCrewFile.self, at: crewFile)?.crews ?? [],
            sessions: try decodeIfPresent(CrewSessionsSnapshot.self, at: snapshotFile)
                ?? CrewSessionsSnapshot())
    }

    /// 读一份可选的 JSON。**只有「文件不在」才回 nil**，其余一律抛。
    /// 不先 `fileExists` 再读 —— 那中间有一道 TOCTOU，而且对权限为 0 的文件
    /// `fileExists` 照样为真，判据会含糊。直接读，按错误码分流。
    private nonisolated static func decodeIfPresent<T: Decodable>(
        _ type: T.Type, at url: URL
    ) throws -> T? {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch let error as NSError {
            let missing = error.domain == NSCocoaErrorDomain
                && error.code == NSFileReadNoSuchFileError
            let missingPosix = error.domain == NSPOSIXErrorDomain && error.code == Int(ENOENT)
            if missing || missingPosix { return nil }
            throw Unavailable(file: url.lastPathComponent, reason: error.localizedDescription)
        }
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            // 解不开 ≠ 空：那是「这份数据我信不过」，同样不许答成业务结论。
            throw Unavailable(file: url.lastPathComponent,
                              reason: "内容解不开（\(error.localizedDescription)）")
        }
    }

    // MARK: - 表

    /// 全表（crew 按号码升序；每个 crew 后面紧跟它的成员分机，机长恒在最前）。
    /// 没号的 crew（回填前的脏数据）跳过 —— 通讯录里不放叫不出的号。
    func entries() -> [CrewDirectoryEntry] {
        let byId = Dictionary(uniqueKeysWithValues: crews.map { ($0.id, $0) })
        var out: [CrewDirectoryEntry] = []
        for crew in crews.filter({ $0.crewNumber != nil })
            .sorted(by: { ($0.crewNumber ?? 0) < ($1.crewNumber ?? 0) }) {
            guard let n = crew.crewNumber else { continue }
            let path = Self.orgPath(of: crew, in: byId)
            let live = sessions.crews[crew.id] ?? []
            let running = live.filter { $0.state != "exited" }.count
            out.append(CrewDirectoryEntry(
                number: CrewPhoneNumber(crew: n, ext: nil),
                name: crew.title,
                kind: .crew,
                orgPath: path,
                activity: "",
                status: running > 0 ? "\(running) 个 session 在" : "没人在跑",
                crewId: crew.id))
            let selfPath = path.isEmpty ? crew.title : path + " / " + crew.title
            // 机长不在 sessionMembers 里（它是 crew 记录上的 captain），分机恒 1。
            let captainEntry = live.first { $0.role == "captain" }
            out.append(CrewDirectoryEntry(
                number: CrewPhoneNumber(crew: n, ext: CrewPhoneNumber.captainExtension),
                name: captainEntry?.name ?? crew.captainName ?? "机长",
                kind: .session,
                orgPath: selfPath,
                activity: captainEntry?.brief ?? "",
                status: Self.statusLabel(captainEntry),
                crewId: crew.id))
            for m in (crew.sessionMembers ?? []).sorted(by: {
                ($0.extensionNumber ?? .max) < ($1.extensionNumber ?? .max)
            }) {
                guard let ext = m.extensionNumber else { continue }
                let entry = live.first { $0.sessionId == m.sessionId }
                out.append(CrewDirectoryEntry(
                    number: CrewPhoneNumber(crew: n, ext: ext),
                    name: entry?.name ?? m.displayName,
                    kind: .session,
                    orgPath: selfPath,
                    activity: entry?.brief ?? "",
                    status: Self.statusLabel(entry),
                    crewId: crew.id))
            }
        }
        return out
    }

    /// 按 `query` 过滤：号码前缀（`7` 命中 7 与 7-x）、名字、在干什么，任一命中即可。
    /// query 为空/纯空白 → 全表。命中一个 session 时它所在 crew 的表头也一并带上
    /// （不然只看到 `7-3` 却不知道 7 是哪个部门）。
    func entries(query: String?) -> [CrewDirectoryEntry] {
        let all = entries()
        let q = (query ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else { return all }
        let hits = all.filter { e in
            e.number.text.hasPrefix(q)
                || e.name.lowercased().contains(q)
                || e.activity.lowercased().contains(q)
                || e.orgPath.lowercased().contains(q)
        }
        let hitCrews = Set(hits.map(\.crewId))
        return all.filter { e in
            hits.contains(where: { $0.number == e.number })
                || (e.kind == .crew && hitCrews.contains(e.crewId))
        }
    }

    /// 渲染成给 agent 看的文本（crew 一行，成员缩进一行）。
    func render(query: String? = nil) -> String {
        let rows = entries(query: query)
        guard !rows.isEmpty else {
            let q = (query ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            return q.isEmpty
                ? "通讯录是空的（本机还没有登记在案的 crew）。"
                : "没有匹配「\(q)」的号码。去掉 query 可以看全表。"
        }
        var lines: [String] = []
        for e in rows {
            switch e.kind {
            case .crew:
                let dept = e.orgPath.isEmpty ? "根部门" : "挂在 \(e.orgPath)"
                lines.append("\(e.number.text) · \(e.name)（crew）｜ \(dept) ｜ \(e.status)")
            case .session:
                var line = "  \(e.number.text) · \(e.name) ｜ \(e.status)"
                if !e.activity.isEmpty { line += " — \(e.activity)" }
                lines.append(line)
            }
        }
        lines.append("（用 contact(to:\"号码\", message:\"…\") 联系；号码 = 整个 crew 就是在那个群里广播发言，"
                     + "`-1` 是那个 crew 的机长。人类不编号，找人仍用 ask。）")
        return lines.joined(separator: "\n")
    }

    // MARK: - 寻址

    /// 号码 → 投递目标。查无此号 → nil（调用方必须明确报错，别静默丢）。
    func resolve(_ number: CrewPhoneNumber) -> CrewContactTarget? {
        guard let crew = crews.first(where: { $0.crewNumber == number.crew }) else { return nil }
        guard let ext = number.ext else {
            return .broadcast(crewId: crew.id, crewTitle: crew.title)
        }
        if ext == CrewPhoneNumber.captainExtension {
            let live = (sessions.crews[crew.id] ?? []).first { $0.role == "captain" }
            return .captain(crewId: crew.id, crewTitle: crew.title,
                            name: live?.name ?? crew.captainName ?? "机长")
        }
        guard let m = (crew.sessionMembers ?? []).first(where: { $0.extensionNumber == ext })
        else { return nil }
        return .session(crewId: crew.id, crewTitle: crew.title,
                        sessionId: m.sessionId, name: m.displayName)
    }

    func resolve(_ raw: String) -> CrewContactTarget? {
        CrewPhoneNumber.parse(raw).flatMap { resolve($0) }
    }

    /// 某个 session 自己的号码（署名用）。`isCaptain` 走机长分机，不查成员表。
    func phoneNumber(crewId: String, sessionId: String, isCaptain: Bool) -> CrewPhoneNumber? {
        guard let crew = crews.first(where: { $0.id == crewId }), let n = crew.crewNumber
        else { return nil }
        if isCaptain { return CrewPhoneNumber(crew: n, ext: CrewPhoneNumber.captainExtension) }
        guard let ext = (crew.sessionMembers ?? [])
            .first(where: { $0.sessionId == sessionId })?.extensionNumber else { return nil }
        return CrewPhoneNumber(crew: n, ext: ext)
    }

    func title(ofCrew crewId: String) -> String? {
        crews.first { $0.id == crewId }?.title
    }

    // MARK: - 内部

    /// 组织路径（根在最左，不含自己）。多父 DAG 取第一条父边；环自保。
    private static func orgPath(of crew: LocalCrew, in byId: [String: LocalCrew]) -> String {
        var chain: [String] = []
        var seen: Set<String> = [crew.id]
        var cursor = crew
        while let parentId = cursor.parentCrewIds.first,
              let parent = byId[parentId], !seen.contains(parentId) {
            chain.append(parent.title)
            seen.insert(parentId)
            cursor = parent
        }
        return chain.reversed().joined(separator: " / ")
    }

    /// 在不在线（含已退出）。快照里没有这条 = app 没在跑 / 这个成员这轮没起过。
    private static func statusLabel(_ entry: CrewSessionsSnapshot.Entry?) -> String {
        guard let entry else { return "不在线" }
        switch entry.state {
        case "working": return "🟢 干活中"
        case "idle": return "🟡 空闲"
        case "awaitingDecision": return "⌛ 等人拍板"
        case "awaitingReply": return "⌛ 等回复"
        case "rateLimited": return "⏳ 限额中"
        case "error": return "🔴 异常"
        case "launchFailed": return "💀 拉起失败"
        case "exited": return "⚪ 已退出"
        default: return entry.state
        }
    }
}

/// 系统报错原文常自带句尾句号；拼进我们自己的句子里会出现「。。」。
private extension String {
    var trimmedTrailingPeriods: String {
        var s = self
        while s.hasSuffix("。") || s.hasSuffix(".") { s.removeLast() }
        return s
    }
}
