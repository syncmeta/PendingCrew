#if os(macOS)
import Foundation

/// 唤醒投递**出队那一刻的重新决定**（人类 Todo #105 ③）。
///
/// ## 病根
///
/// `CrewLocalMentionWaker` 从前在**扫描那一刻**就把正文 + 最近上下文渲染成一个
/// 字符串塞进 `CrewDeferredWakeQueue`；目标忙就压着，空闲再发，**中间从不重读白板**。
/// 同一段时间里 hook 路每次工具调用都在把同一条渲染进「未读」并推进游标 ——
/// 于是队列弹出的是一份**已经被消费掉的旧快照**。
///
/// 「重放送的是消息的最初形态」不是缓存了旧数据：**队列里存的本来就是一份写死的串**。
///
/// 两个独立现场都只有这一条解释得了：人类那次（90 秒、发生在被回复之后）、
/// 4-1 那次（一个回合内两条，全程在跑、没重启、不是被拉起来的）。
///
/// ## 改法
///
/// 队列里存**消息身份**，不存渲染结果；真要发之前再问两句：
/// **这条还该发吗**（目标游标是不是已经过去了）、**该发什么**（现取，不是快照）。
enum CrewWakeDispatch {

    /// 队列里携带的东西。**白板来的唤醒必须是 `.whiteboardEntry`** ——
    /// 一旦它以 `.literal` 形态入队，出队时就没有任何东西可以重新决定。
    enum Payload: Equatable {
        /// 与白板无关的唤醒（机长交接期间攒下的补投等）：没有「现取」可言。
        case literal(String)
        /// 白板上的某一条。真要发时按 id 现取。
        case whiteboardEntry(crewId: String, entryId: String)
    }

    /// 出队要发之前重新决定。返回 `nil` = **别发了**。
    ///
    /// - `hasDelivered`: 目标的**盘上**游标是不是已经越过这条了（#105 ④ 那本唯一判据）。
    /// - `renderNow`: **在这一刻**按白板当前内容渲染；返回 nil = 白板上已经找不到它。
    ///
    /// 两条 fail-safe 的方向是相反的、都是刻意的：
    /// - 已投过 → 不发（重复比沉默烦人，且账本已经记着它投过了）。
    /// - 找不到 → 不发（宁可不发，也不发一句无法追溯的话）。
    static func resolve(
        _ payload: Payload,
        hasDelivered: (String, String) -> Bool,
        renderNow: (String, String) -> String?
    ) -> String? {
        switch payload {
        case .literal(let text):
            return text
        case .whiteboardEntry(let crewId, let entryId):
            guard !hasDelivered(crewId, entryId) else { return nil }
            return renderNow(crewId, entryId)
        }
    }
}
#endif
