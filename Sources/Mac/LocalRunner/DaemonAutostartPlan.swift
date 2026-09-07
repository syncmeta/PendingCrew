#if os(macOS)
import Foundation

/// 开机自启这个开关的**状态与决策**（P5b·A）。真正调 `SMAppService` 的那一层在
/// `DaemonAutostart`（要 import ServiceManagement，进不了 test bundle）——
/// 凡是能判断对错的部分都在这里。
enum DaemonAutostartState: Equatable {
    /// 没注册过。**这是默认**。
    case off
    /// 注册了，会开机自启、异常退出自拉。
    case on
    /// 注册成功了，但要人去「系统设置 → 通用 → 登录项」点头（或者人在那里撤销过）。
    /// **不许把它当成 `on`**：那样界面会显示"已开启"而它其实一次都不会起。
    case waitingForApproval
    /// 系统找不到这个服务（多半是 app 包里那份 plist 没进对位置）。
    case missing

    var isRunningAtLogin: Bool { self == .on }

    /// 给人看的一句话。**每一档都要说清「现在会发生什么」**，不是只说状态名。
    var text: String {
        switch self {
        case .off: return "开机不自启。退出 PendingCrew 后台就停了。"
        case .on: return "开机自启已开启，异常退出后会自动拉起。"
        case .waitingForApproval:
            return "已注册，但还没生效 —— 需要在「系统设置 → 通用 → 登录项」里允许 PendingCrew。"
        case .missing:
            return "系统里找不到这个登录项（app 包里的 LaunchAgent 可能没装对位置）。"
        }
    }
}

/// app 更新之后要不要重新注册。
///
/// SDK 头文件原话：「If an app updates either the plist or the executable for a
/// LaunchAgent, the SMAppService **must be re-registered** or it may not launch.」
/// 我们有自动更新（Sparkle），所以这不是理论问题 —— 不处理的话，用户开着自启，
/// 某次更新之后它**安静地不再自启**，而开关看起来还是"已开启"。
enum DaemonAutostartRefresh: Equatable {
    case notNeeded
    case reregister(reason: String)
}

enum DaemonAutostartPlan {
    /// - Parameters:
    ///   - state: 当前系统里的状态。
    ///   - registeredBuild: 上一次注册时那个 build 戳（没注册过则 nil）。
    ///   - currentBuild: 现在这个 build 戳。
    static func refresh(state: DaemonAutostartState,
                        registeredBuild: String?,
                        currentBuild: String) -> DaemonAutostartRefresh {
        // **关掉的时候什么都别做。** 一次更新绝不许顺手把自启打开 —— 那是在用户
        // 没点头的情况下改他机器上的登录项，比"更新后不自启"严重得多。
        guard state != .off, state != .missing else { return .notNeeded }
        guard registeredBuild != currentBuild else { return .notNeeded }
        return .reregister(
            reason: "app 从 \(registeredBuild ?? "未知") 更新到 \(currentBuild)，"
                + "LaunchAgent 必须重新注册，否则下次开机不会自启")
    }
}
#endif
