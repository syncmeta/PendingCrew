import Foundation

/// 一台「机器」—— 本机 / peer 电脑 / Fly machine 统一进 `pendingbot.machine`
/// 表（RLS 限本账号）。`GET /v1/machines` 下发这个形状（camelCase，edge 已
/// 手动 map）。
///
/// `kind` 是字符串而不是 enum —— 跟 `CrewSummary.runtimeLocation` 一样留
/// forward-compat 余地，遇到没见过的值回落原始字符串显示，不 decode 失败。
struct Machine: Decodable, Identifiable, Equatable, Hashable {
    let id: String
    let kind: String              // "computer" | "fly"
    let deviceId: String?
    let displayName: String
    let flyMachineId: String?
    let status: String?
    let lastSeenAt: String?

    enum Kind: String { case computer, fly }
    var kindEnum: Kind? { Kind(rawValue: kind) }

    /// `isLocal` = 本机（deviceId == DeviceIdentity.current）。computer 机器
    /// 在本机时显桌面图标，远端 peer 显个人热点；fly 显云。
    func displayIcon(isLocal: Bool) -> String {
        if kindEnum == .fly { return "cloud" }
        return isLocal ? "desktopcomputer" : "personalhotspot"
    }
}
