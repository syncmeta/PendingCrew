import Foundation

/// 刷新闸（#443 第二道闸）。
///
/// 群聊中栏的每个白板 tick 都会重拉一次整板。**重拉是廉价的（一次 JSON 读），
/// 重排不是** —— 往 `@State` 上重新赋一个数组，哪怕内容一模一样，SwiftUI 也会
/// 让整个 body 失效，`LazyVStack` 于是把整条消息列表重新测量一遍（现场 sample：
/// `LazyStack.measureEstimates → LazyHVStack.lengthAndSpacing → sizeThatFits`
/// 占满主线程）。
///
/// 所以：先比内容，真变了才赋值。上游的相关性闸（`FileChangeGate`）负责少给
/// tick，这一道负责「就算漏进来一个 tick，也不该引发整表重排」。
///
/// 依赖 `CrewWhiteboardEntry` / `CrewMember` 的 `Equatable` 是**逐字段内容比较**
/// （二者都是全 `let` 存储属性的合成实现）——若哪天有人往里加了个每次重建都不同
/// 的字段（时间戳、随机 id），这道闸会静默失效，`CrewChatRefreshGateTests` 钉的
/// 就是这一点。
enum CrewChatRefreshGate {

    /// 内容一样 → nil（调用方什么都别做）；不一样 → 新值。
    ///
    /// 用法固定成这一种形状，免得调用点写成 `if a != b { a = b }` 时漏掉某一处：
    /// ```
    /// if let fresh = CrewChatRefreshGate.changed(current: entries, fresh: loaded) {
    ///     entries = fresh
    /// }
    /// ```
    static func changed<T: Equatable>(current: T, fresh: T) -> T? {
        current == fresh ? nil : fresh
    }
}
