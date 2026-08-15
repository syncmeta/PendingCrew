#if os(macOS)
import Foundation

/// hub 事件驱动 relay 拉取（CC-P4）的**纯决策核心**。
///
/// 背景：登录态 crew 的远端消息此前只靠 `CrewRelayAgent` 的 5s 轮询进本地白板。
/// CC-P4 给 relay 加 hub 订阅（`CrewRealtimeClient` 连 `conv:<remoteId>`），
/// `.changed` 帧 → 立即拉该 crew 的白板增量 → `appendRelayMessage` 落盘
/// （自然触发 `LocalWhiteboardStore.changes` → UI 刷新 / listen 投递）。
/// 5s 轮询保留作兜底（hub 断线期间 + 上行推送侧不经 hub）。
///
/// 这里抽两块可单测的纯逻辑；IO 编排（开 socket、调 API）留在 `CrewRelayAgent`：
/// - `reconcile`：期望连接集合（relay 绑定）vs 已开连接集合 → 该开/该关哪些。
/// - `CrewHubPullCoalescer`：hub 事件风暴下的拉取去重状态机。
enum CrewRelayHubLogic {

    /// 连接对账：`desired` = 当前 relay 绑定的 crewId 集合（登录态才非空），
    /// `connected` = 已持有 hub 连接的 crewId 集合。返回该新开与该关闭的集合。
    static func reconcile(
        desired: Set<String>, connected: Set<String>
    ) -> (open: Set<String>, close: Set<String>) {
        (open: desired.subtracting(connected), close: connected.subtracting(desired))
    }
}

/// 每 crew 一个的拉取合并去重状态机（值类型，宿主放 dict 里）。
///
/// 语义：同 crew 同时至多一个拉取在飞。飞行中到达的 hub 事件不排队、不丢 ——
/// 合并成「本次结束后立刻再拉一次」（拉取按游标取增量，一次补拉覆盖飞行期间
/// 的全部新消息）。
struct CrewHubPullCoalescer: Equatable {
    private(set) var running = false
    private(set) var pending = false

    /// hub 事件到达。返回 true = 调用方现在就起一次拉取；false = 已有拉取在飞，
    /// 事件已合并（结束时 `didFinish` 会指示补拉）。
    mutating func shouldStart() -> Bool {
        if running {
            pending = true
            return false
        }
        running = true
        return true
    }

    /// 一次拉取结束。返回 true = 飞行期间有事件被合并，调用方应立刻再拉一次
    /// （状态保持 running）；false = 无积压，回到 idle。
    mutating func didFinish() -> Bool {
        if pending {
            pending = false
            return true
        }
        running = false
        return false
    }
}
#endif
