import Foundation

/// 白板 / crew 时间戳的 ISO-8601 解析 —— **全 app 一份口径**。
///
/// 时间戳带不带小数秒取决于谁写的：本机直接写的不带（`ISO8601DateFormatter()` 默认），
/// relay 从 edge 搬进来的带。两种都得认，少认一种就是"有的行没时间/排不上序"。
///
/// 从 `CrewTimeSeparator.parse`（SwiftUI 视图文件）里摘出来放这儿，是为了让**纯逻辑
/// 能单测**：侧栏时间流的排序键要用它，而视图文件带 Theme/SwiftUI 依赖，进不了
/// test bundle。`CrewTimeSeparator.parse` 现在只是这里的转发。
enum CrewTimestamp {
    static func parse(_ iso: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: iso) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: iso)
    }
}

/// crew 的「最新活动时间」—— **单一真值**。
///
/// 侧栏两种视图都从这里取：层级视图行尾那颗相对时间 pill、时间流视图的排序键。
/// 两处对不上就是新 bug，所以口径只许在这个函数里定义一次（`CrewTimelineOrderingTests`
/// 钉死）：
/// - 首选**该 crew 白板最后一条消息的 createdAt**（`LocalWhiteboardStore.list(...).last`）；
/// - 白板还空着才退回 `crew.updatedAt` —— 它只在创建/改名时写，单独拿来显示会一直
///   像"创建时间"，所以只当兜底不当首选。
///
/// 时间戳可能带/不带小数秒（本机写不带，relay 从 edge 搬进来的带），统一走
/// `CrewTimestamp.parse` 双格式解析 —— 群聊分隔条（`CrewTimeSeparator.parse`）转发的
/// 也是它，解析口径全 app 一份。
enum CrewActivityTime {
    /// - Parameters:
    ///   - lastMessageCreatedAt: 白板最后一条消息的 ISO 时间（无消息 → nil）。
    ///   - crewUpdatedAt: `CrewSummary.updatedAt`，兜底用。
    /// - Returns: 解析出来的时间；两个都解析不出（脏数据/空串）→ nil（不显示、排最后）。
    static func resolve(lastMessageCreatedAt: String?, crewUpdatedAt: String?) -> Date? {
        if let iso = lastMessageCreatedAt, let date = CrewTimestamp.parse(iso) { return date }
        if let iso = crewUpdatedAt, let date = CrewTimestamp.parse(iso) { return date }
        return nil
    }
}
