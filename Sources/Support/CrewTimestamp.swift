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

    /// 进程级复用的两个格式器 —— **别改回每次 `ISO8601DateFormatter()`**（人类 Todo #140 ①）。
    ///
    /// 那个构造要开 ICU 的日期格式器 + locale + 数字格式器。2026-09-11 症状发生时对界面
    /// 进程采的样里，`libicucore` 吃掉主线程独占耗时的 **7.1%**，全部落在
    /// `udat_open` / `__CreateCFDateFormatter` / `_localeWithNewCalendarIdentifier`
    /// 这条路上（读数见 `docs/internal/2026-09-11-typing-lag-profile.md`）。
    ///
    /// **为什么是两个实例、而不是一个来回改 `formatOptions`**：改选项就是写共享状态，
    /// 那才是真正不能共享的那种用法。两个各自只读，构造完再没人写过它们。
    ///
    /// **「只读地共享安全」这一条是实测的，不是引文**：
    /// `CrewChatTypingLagCostTests.test_共享格式器并发解析结果与串行一致` 拿 8 条队列
    /// × 500 轮打同一对实例，与串行结果逐条对齐。边界也写在那条用例上 —— 它证明的是
    /// 结果一致，不是「没有数据竞争」（那一趟没开 TSan）。
    ///
    /// 顺序固定「先带小数秒、再不带」。两种 options 在实测里对同一个字符串**互斥**
    /// （`test_三个解析入口的结果与改动前逐条一致` 钉的就是这个），所以调用方原来各自
    /// 的尝试顺序收到这一份来，结果一个字不差。
    private static let fractional = make([.withInternetDateTime, .withFractionalSeconds])
    private static let plain = make([.withInternetDateTime])

    private static func make(_ options: ISO8601DateFormatter.Options) -> ISO8601DateFormatter {
        CrewChatCostCounters.note(.iso8601FormatterBuild)
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = options
        return formatter
    }

    static func parse(_ iso: String) -> Date? {
        if let date = fractional.date(from: iso) { return date }
        return plain.date(from: iso)
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
