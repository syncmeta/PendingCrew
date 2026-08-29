import Foundation
#if os(iOS)
import UIKit
#endif

/// 稳定的安装级设备身份：自存一个安装级 UUID 到 UserDefaults —— 同一安装恒定，
/// 重装后换一个（就当新机）。
///
/// **它原本还是云端 machine 注册（`POST /v1/machines/register-self`）的 upsert
/// key；那条注册路随 #63 第二期（2026-08-26）删除，现在不存在了。** 今天唯一的
/// 活读者是 `CrewStore.refreshMachines()` / `MachineGrouping`：判断某台 machine
/// 是否「本机」。
///
/// **它不是机器身份凭证。** 这是一个可被任何人读写的 UserDefaults UUID，不能拿来
/// 向远程主机证明「我是这台设备」。接 Fly 远程主机时需要的是设备密钥对（私钥进
/// Keychain，`synchronizable=false`），见
/// `docs/internal/2026-08-29-fly-remote-host-review.md` §3。
enum DeviceIdentity {
    private static let key = "pendingcrew.deviceId"

    /// 安装级稳定 UUID（小写）。首次访问惰性生成并持久化。
    static var current: String {
        if let v = UserDefaults.standard.string(forKey: key) { return v }
        let v = UUID().uuidString.lowercased()
        UserDefaults.standard.set(v, forKey: key)
        return v
    }

    /// 人类可读机器名（注册时作 display_name）。
    static var displayName: String {
        #if os(macOS)
        return Host.current().localizedName ?? "Mac"
        #elseif os(iOS)
        return UIDevice.current.name
        #else
        return "PendingCrew Device"
        #endif
    }
}
