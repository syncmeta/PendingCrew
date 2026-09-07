#if os(macOS)
import Foundation
import ServiceManagement

/// 开机自启开关的**执行层**（P5b·A，人类 Todo #7）。判断在 `DaemonAutostartPlan`。
///
/// ## 默认关，而且只有人点了才会开
///
/// 注册登录项是**改用户机器上的设置**：装上之后它自己会起，系统设置里会多出一条
/// 「PendingCrew」。所以这一层不在任何地方自动调 `enable()` —— 它只在人明确
/// 打开开关时被调用。`refreshAfterUpdate()` 也只会在**已经开着**的时候重新注册，
/// 关着时一步都不走（见 `DaemonAutostartPlan.refresh` 里那条 guard）。
///
/// ## 关掉之前先让它自己收尾
///
/// SDK 头文件说 `unregister` 会把正在跑的服务 **kill 掉**，而且不等它被回收。
/// 对我们这个 daemon 而言，被一刀砍下去意味着它底下所有 agent 子进程变成孤儿 ——
/// 而避免这件事正是优雅退出存在的全部理由。所以 `disable()` 的顺序是
/// **先 SIGTERM 让它自己停光 session，再 unregister**。
///
/// ## 回执必须说清「现在会发生什么」
///
/// 装和移除都不是"点一下就好"：注册可能落在「等人去系统设置里点头」这一档，
/// 移除之后正在跑的后台已经停了。这两件事不写进回执，人就只能靠猜。
@MainActor
enum DaemonAutostart {
    private static let registeredBuildKey = "daemonAutostart.registeredBuild"

    private static var service: SMAppService {
        .agent(plistName: PendingCrewLaunchAgent.plistName)
    }

    static var state: DaemonAutostartState {
        switch service.status {
        case .enabled: return .on
        case .requiresApproval: return .waitingForApproval
        case .notRegistered: return .off
        case .notFound: return .missing
        @unknown default: return .missing
        }
    }

    /// 回执。`ok` 之外一律带上原文的系统错误描述 —— 「注册失败」四个字帮不了任何人。
    struct Receipt {
        var succeeded: Bool
        var state: DaemonAutostartState
        var text: String
    }

    /// 打开开机自启。**只该由人点开关时调用。**
    @discardableResult
    static func enable() -> Receipt {
        do {
            try service.register()
        } catch {
            // 已经注册过 = 期望状态成立，不是失败。
            if (error as NSError).code != kSMErrorAlreadyRegistered {
                return Receipt(succeeded: false, state: state,
                               text: "打开开机自启失败：\(error.localizedDescription)")
            }
        }
        rememberBuild()
        let now = state
        return Receipt(succeeded: now != .off && now != .missing, state: now, text: now.text)
    }

    /// 关掉开机自启。**先让后台自己收尾，再摘登录项** —— 见类型注释。
    @discardableResult
    static func disable() -> Receipt {
        let stopped = DaemonStopper(dataRoot: dataRoot).stop()
        do {
            try service.unregister()
        } catch {
            // 本来就没注册 = 期望状态成立。
            if (error as NSError).code != kSMErrorJobNotFound {
                return Receipt(succeeded: false, state: state,
                               text: "关闭开机自启失败：\(error.localizedDescription)")
            }
        }
        UserDefaults.standard.removeObject(forKey: registeredBuildKey)
        // **复核**，不拿"调用没抛"当结果：摘干净了没有是问得出来的事实。
        let now = state
        guard now == .off || now == .missing else {
            return Receipt(succeeded: false, state: now,
                           text: "登录项没有摘干净，现在是「\(now.text)」。"
                               + "请在「系统设置 → 通用 → 登录项」里手动关掉 PendingCrew。")
        }
        return Receipt(succeeded: true, state: now,
                       text: "开机自启已关闭。" + stopped.text)
    }

    /// app 更新之后重新注册。**关着的时候一步都不走。**
    ///
    /// 不做这件事的后果不是报错，是**安静地不再自启**，而开关看起来还是"已开启"。
    @discardableResult
    static func refreshAfterUpdate() -> Receipt? {
        let current = SessionDaemonHost.currentBuild
        let decision = DaemonAutostartPlan.refresh(
            state: state,
            registeredBuild: UserDefaults.standard.string(forKey: registeredBuildKey),
            currentBuild: current)
        guard case let .reregister(reason) = decision else { return nil }
        // 头文件建议：可执行文件变了时，先 unregister 再 register。
        try? service.unregister()
        do {
            try service.register()
        } catch {
            if (error as NSError).code != kSMErrorAlreadyRegistered {
                return Receipt(succeeded: false, state: state,
                               text: "更新后重新注册登录项失败：\(error.localizedDescription)"
                                   + "（\(reason)）")
            }
        }
        rememberBuild()
        return Receipt(succeeded: true, state: state, text: "已重新注册登录项（\(reason)）。")
    }

    private static func rememberBuild() {
        UserDefaults.standard.set(SessionDaemonHost.currentBuild, forKey: registeredBuildKey)
    }

    private static var dataRoot: URL {
        PendingCrewDaemonPaths.standard().lock.deletingLastPathComponent()
    }
}
#endif
