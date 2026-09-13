import Foundation

/// 总机长排布：**哪几个 crew 该顶到侧栏最前，以及为什么**（Todo #102 / 人类 #109）。
///
/// 落 `~/Library/Application Support/PendingCrew/crew-arrangement.json`，全机一份 ——
/// 它排的是**整个侧栏**，不是某个 crew 内部的事，所以不跟着 crew 分文件。
/// app 读、MCP helper 写，跨进程，与 `local-crews.json` 同一层。
///
/// ## 它是覆盖层，不是排序本身
/// 侧栏的**基础序**（最近活动倒序）永远算得出来，这份排布只是叠在上面。文件不存在、
/// 读不出来、里面的 id 已经没了 —— 一律退回基础序，**不空白、不少行**。
/// 一个 agent 的判断可以决定「推荐你先看什么」，**但不能决定「这台机器上有什么」**。
///
/// ## 必须能看得见是谁排的、为什么、什么时候
/// 所以 `reason` / `bySenderName` / `createdAt` 都是**存下来给人看的**，不是日志。
/// 人看到一个不合意的顺序时，得分得清「这是规则算的」还是「这是谁排的」——
/// **做成黑箱，它第一次排错就会被永久关掉。**
struct CrewArrangement: Codable, Equatable, Sendable {
    /// 顶到最前的 crew id，按给定顺序。没被提到的跟在后面、保持基础序。
    var crewIds: [String]
    /// 为什么这么排。**必填** —— 见类型注释。
    var reason: String
    /// 谁排的（session id）。
    var bySessionId: String?
    /// 谁排的（显示名，给人看）。
    var bySenderName: String?
    /// 排的那一刻。视图拿它显示「这份排布是多久前排的」——
    /// 陈旧度要看得见，否则人会以为眼前这个顺序是刚算的。
    var createdAt: String
}

/// 读写那一份排布。自包含 Foundation（要跟着 `McpServer` 编进 helper）。
enum CrewArrangementStore {
    static func fileURL(dataRoot: URL) -> URL {
        dataRoot.appendingPathComponent("crew-arrangement.json")
    }

    /// helper 只拿得到白板目录（`--dir`），数据根是它的上一级 —— 与
    /// `LocalCrewStore.orgTreeLines(whiteboardDirectory:)` 同一条推导，别另写一份。
    static func fileURL(whiteboardDirectory: URL) -> URL {
        fileURL(dataRoot: whiteboardDirectory.deletingLastPathComponent())
    }

    /// 读。**任何读不出来都返回 nil = 没有排布 = 退回基础序**，不抛、不半途而废。
    static func load(at url: URL) -> CrewArrangement? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(CrewArrangement.self, from: data)
    }

    /// 写（atomic：临时文件 + rename）。写不成返回 false —— 调用方要如实回执，
    /// 别让 agent 以为自己排好了。
    @discardableResult
    static func save(_ arrangement: CrewArrangement, to url: URL) -> Bool {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(arrangement) else { return false }
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try MultiProcessJSONStore.writeStaged(data, to: url)
            return true
        } catch {
            return false
        }
    }

    /// 清掉排布（退回纯基础序）。文件本来就不在也算成功。
    @discardableResult
    static func clear(at url: URL) -> Bool {
        guard FileManager.default.fileExists(atPath: url.path) else { return true }
        return (try? FileManager.default.removeItem(at: url)) != nil
    }
}

// MARK: - 总机长给每个机组写的那句摘要（人类 Todo #145）

/// 总机长给**一个**机组写的一句摘要。显示在侧栏「总机长」视图那一行的消息位上。
///
/// ## 为什么每条自带写入时刻，而不是整份共用排布那个 `createdAt`
/// 过期是**逐个机组**判的：写完之后那个机组又有了新消息，这一句才算过期。
/// 而且写入是**合并**的 —— 总机长这次只重写了三个机组，其余机组那句仍是上次写的，
/// 共用一个时刻就会把上次那几句的年龄说成这次。
struct CrewChiefSummary: Codable, Equatable, Sendable {
    var text: String
    /// ISO8601，精确到秒（与白板消息的 `createdAt` 同一个格式器口径 —— 过期判定
    /// 就是拿这两个比，精度不一致会让「同一秒」的判断偏向说新）。
    var writtenAt: String
    var bySessionId: String?
    var bySenderName: String?
}

/// 读写那一份摘要表：`crew-chief-summaries.json`，全机一份，`crewId → 摘要`。
///
/// ## 为什么不塞进 `CrewArrangement`
/// 两件事的生命周期不一样。`arrange_crews(crew_ids: [])` 的语义是「撤掉排布、退回
/// 基础序」，实现是**删文件** —— 摘要若住在同一个文件里，撤一次顺序就连带把每个
/// 机组那句话全抹了，而人和总机长都没有说过要抹。分两个文件，入口仍是同一个工具
/// （人类要的是「点一下 = 总结 + 排序」，总机长一次调用做完两件）。
enum CrewChiefSummaryStore {
    static func fileURL(dataRoot: URL) -> URL {
        dataRoot.appendingPathComponent("crew-chief-summaries.json")
    }

    /// helper 只拿得到白板目录 —— 与 `CrewArrangementStore.fileURL(whiteboardDirectory:)` 同一条推导。
    static func fileURL(whiteboardDirectory: URL) -> URL {
        fileURL(dataRoot: whiteboardDirectory.deletingLastPathComponent())
    }

    /// 界面读：读不出来 = 没有摘要（每行退回机长自报的状态 / 「还没有」），不抛。
    static func load(at url: URL) -> [String: CrewChiefSummary] {
        (try? loadReportingFailure(at: url)) ?? [:]
    }

    /// **写之前**的读：文件不在 = 空表；文件在、但读不出来或解不开 → **抛**。
    ///
    /// 写是「读出旧表 → 合并 → 整份写回」。要是把读失败当成空表，一次瞬时 EPERM
    /// 就会拿「只有这次这几句」的表盖掉全部旧摘要 —— 与 2026-08-12 白板被扫光同一个形状。
    static func loadReportingFailure(at url: URL) throws -> [String: CrewChiefSummary] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [:] }
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode([String: CrewChiefSummary].self, from: data)
    }

    /// 写（atomic）。写不成返回 false —— 调用方要如实回执。
    @discardableResult
    static func save(_ summaries: [String: CrewChiefSummary], to url: URL) -> Bool {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(summaries) else { return false }
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try MultiProcessJSONStore.writeStaged(data, to: url)
            return true
        } catch {
            return false
        }
    }

    /// 合并：这次给了的机组换成新的一句（带这次的写入时刻），没给的**原样留着**。
    ///
    /// 没给的留着不会变成「旧话冒充新话」：它带着自己当初的写入时刻，那个机组之后
    /// 一有新消息就被判过期（`CrewStatusLine.summaryIsStale`）。
    static func merging(_ existing: [String: CrewChiefSummary],
                        incoming: [String: String],
                        writtenAt: String,
                        bySessionId: String?,
                        bySenderName: String?) -> [String: CrewChiefSummary] {
        var out = existing
        for (crewId, text) in incoming {
            out[crewId] = CrewChiefSummary(text: text, writtenAt: writtenAt,
                                           bySessionId: bySessionId, bySenderName: bySenderName)
        }
        return out
    }
}

/// `arrange_crews(summaries:)` 收下来的东西怎么判。写侧判定，纯函数。
///
/// **长度只提醒、不拦**（机长 2026-09-13 定的默认值，人类 Todo #7 还没拍）：
/// 与 `CrewStatusIntake.hintLength` 同一条提醒线 —— 侧栏那一行露得出多少字是同一件事，
/// 别在两处各写一个 40。显示宽度不该变成写入端的合法性判据。
enum CrewChiefSummaryIntake {
    enum Decision: Equatable {
        /// 没给 `summaries`。
        case none
        /// 收下；`notes` 是要写进回执的提醒（**照样写进去了**）。
        case accepted([String: String], notes: [String])
        /// 形状不对 —— **整次调用什么都不写**（摘要不写、顺序也不动）。
        /// 写一半（顺序动了、摘要没落）会让侧栏显示上一轮的旧话配这一轮的新顺序。
        case refused(String)
    }

    static func decide(_ raw: Any?) -> Decision {
        guard let raw, !(raw is NSNull) else { return .none }
        guard let dict = raw as? [String: Any] else {
            return .refused("`summaries` 要是一个对象：`{\"crewId\": \"一句话\"}`，收到的不是。"
                + "**这次什么都没写**（摘要没写，顺序也没动）。")
        }
        var accepted: [String: String] = [:]
        var notes: [String] = []
        for key in dict.keys.sorted() {
            let crewId = key.trimmingCharacters(in: .whitespacesAndNewlines)
            guard let value = dict[key] as? String else {
                return .refused("`summaries` 里「\(key)」对应的不是一句话（要字符串）。"
                    + "**这次什么都没写**（摘要没写，顺序也没动）。")
            }
            let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !crewId.isEmpty else {
                notes.append("有一条的 crewId 是空的，跳过了")
                continue
            }
            guard !text.isEmpty else {
                // 空的不当成「删掉这句」：想让一行回到机长自报，是一个要说出来的决定，
                // 不该靠传个空串悄悄发生。
                notes.append("\(crewId) 那句是空的，没写（原来那句不动）")
                continue
            }
            if text.count > CrewStatusIntake.hintLength {
                notes.append("\(crewId) 那句 \(text.count) 字，侧栏那一行大概只露得出 "
                    + "\(CrewStatusIntake.hintLength) 字左右 —— **已经照原样写进去了**，"
                    + "只是前 \(CrewStatusIntake.hintLength) 字要能自己说清")
            }
            accepted[crewId] = text
        }
        if accepted.isEmpty && notes.isEmpty { return .none }
        return .accepted(accepted, notes: notes)
    }
}
