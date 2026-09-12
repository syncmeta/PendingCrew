import Combine
import Foundation

/// **crew 级**「最后看过」时间 —— 侧栏「已隐藏的群」那行算未读用。
///
/// 为什么另起一个而不是往 `LocalCrew` 里加字段：这是「**这台机器上这个人**的阅读
/// 状态」，不是 crew 自己的属性。crew 的磁盘 JSON（`local-crews.json`）是组织事实，
/// 谁看没看过不该混进去。仓库已有先例 —— `SessionUnreadStore` 是 session 级的同一
/// 件事，同样存 UserDefaults、同样不进 session 的持久数据。
///
/// 未读的口径（单一真值在 `CrewHiding.hiddenEntries`）：
/// **末条白板消息时间 > max(manuallyHiddenAt, lastViewed)**。
/// 藏起来 ≠ 不再关心 —— 人会点进去看看有没有动静，看完那个提示就该消失；
/// **一个不可信的计数比没有计数糟**。所以从「已隐藏的群」列表点进去看那一下就
/// `markViewed`，未读清掉，群**不取回**（取回是另一个显式动作）。
///
/// **别在这里读白板。** `SessionUnreadStore.unreadCount` 里那句
/// `whiteboard.list(crewId:)` 是解整板 JSON，它活得下去是因为只作用于当前打开的
/// 那一个 session；侧栏是几十个 crew × 每次重绘，照抄它就是 2026-08-17「开久了
/// 卡」的完整复现。侧栏那条路只比时间戳，消息快照从
/// `CrewStore.lastWhiteboardMessages` 拿（后台按指纹门控算好的）。
@MainActor
final class CrewViewedStore: ObservableObject {
    static let shared = CrewViewedStore()

    private static let defaultsKey = "PendingCrew.crewLastViewed"

    /// crewId → 最后查看时间。
    private var lastViewed: [String: Date]

    private let defaults: UserDefaults

    /// `defaults` 可注入 —— 单测用自己的 suite，别污染真 UserDefaults。
    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let raw = defaults.dictionary(forKey: Self.defaultsKey) as? [String: TimeInterval] ?? [:]
        lastViewed = raw.mapValues { Date(timeIntervalSince1970: $0) }
    }

    /// 标记某 crew 已被查看（从「已隐藏的群」列表点进去时调）。
    func markViewed(_ crewId: String, at date: Date = Date()) {
        objectWillChange.send()
        lastViewed[crewId] = date
        persist()
    }

    /// 某 crew 最后查看时间；从没看过 → nil（未读只跟 `manuallyHiddenAt` 比）。
    func lastViewedAt(_ crewId: String) -> Date? {
        lastViewed[crewId]
    }

    /// 整张表（喂给 `CrewHiding.hiddenEntries` 的纯函数入参）。
    var snapshot: [String: Date] { lastViewed }

    private func persist() {
        let plain = lastViewed.mapValues { $0.timeIntervalSince1970 }
        defaults.set(plain, forKey: Self.defaultsKey)
        Self.mirrorToDisk(plain)
    }

    // MARK: - 跨进程镜像（Todo #102）

    /// 磁盘镜像的位置。UserDefaults 只有 app 进程读得到，而 MCP helper 是**另一个
    /// 进程** —— 它要把「他最后一次打开哪个 crew」当排序原料交给总机长 session。
    ///
    /// 为什么是镜像而不是搬家：UserDefaults 那份是 app 内所有既有读点在用的
    /// （侧栏未读判定等），搬家会牵动一片；而这里只需要**跨进程读得到**。
    /// 两份都由 `persist()` 一处写，不存在第二个写点。
    nonisolated static func mirrorURL(dataRoot: URL = PendingCrewDataRoot.url) -> URL {
        dataRoot.appendingPathComponent("crew-last-viewed.json")
    }

    nonisolated private static func mirrorToDisk(_ plain: [String: TimeInterval]) {
        guard let data = try? JSONEncoder().encode(plain) else { return }
        try? MultiProcessJSONStore.writeStaged(data, to: mirrorURL())
    }

    /// 跨进程读那份镜像。
    ///
    /// **返回 nil 和返回空字典是两件事**，调用方必须分得开：
    /// `nil` = 这份镜像**读不出来**（文件不在 / 坏了）→「我看不出来」；
    /// `[:]` = 读出来了、里面确实一条都没有 →「他确实还没打开过任何 crew」。
    /// 把前者当成后者，就是把「看不出来」报成「确实没有」—— 这台机器上栽过。
    nonisolated static func loadMirror(dataRoot: URL = PendingCrewDataRoot.url) -> [String: Date]? {
        guard let data = try? Data(contentsOf: mirrorURL(dataRoot: dataRoot)),
              let plain = try? JSONDecoder().decode([String: TimeInterval].self, from: data)
        else { return nil }
        return plain.mapValues { Date(timeIntervalSince1970: $0) }
    }
}
