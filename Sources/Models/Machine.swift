import Foundation

/// 一台「机器」—— 本机 / peer 电脑 / Fly machine。
///
/// **这个形状曾经是云端 `GET /v1/machines` 的下行 DTO；那个端点和它背后的
/// `pendingbot.machine` 表随 #63 第二期（2026-08-26「跨端遥控，端掉」）一起
/// 没了。** 今天唯一的产出方是 `CrewStore.refreshMachines()`，它恒返回一台本机。
/// 留着这个类型不是为了将来对接某个云 API —— 是因为 `MachineGrouping` 与侧栏
/// 分组现在真读它，而且下一台机器会从**本地**来：
///
/// > 远程主机（Fly machine）按 `docs/internal/2026-08-29-fly-remote-host-review.md`
/// > 的方案，是「前后端分离」那条 session 协议的第三种传输 —— viewer 直连机器，
/// > 不经中继、不要账号。所以第二台机器将由本地机器名册产出，不由服务端下发。
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
