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
            try data.write(to: url, options: .atomic)
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
