import Foundation
#if os(iOS)
import UIKit
#endif

/// 稳定的安装级设备身份。pendingcrew 的 device-grant 登录没有自带稳定
/// device_id（每次 challenge 都可以是新的），所以这里自存一个安装级 UUID
/// 到 UserDefaults —— 同一安装恒定，重装后换一个（机器在表里就当新机）。
///
/// machine 注册（`POST /v1/machines/register-self`）用它做 upsert key，
/// crew 列表展示用它判断某台 machine 是否「本机」。
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
