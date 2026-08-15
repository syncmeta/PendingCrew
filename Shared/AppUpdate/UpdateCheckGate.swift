import Foundation

/// Sparkle 更新检查的忙时闸门（纯逻辑，无 Sparkle 依赖，可单测）。
///
/// 规则只有一条：**用户手点的检查永远放行；后台定时检查在忙时拦下**
/// （拦下后 Sparkle 会按自己的重试节奏稍后再来，不丢检查）。
/// PendingCrew 的「忙」= 有 session 在跑——后台弹更新会打断人正在干的活；
/// PendingBot 不注入忙判定，等价于永不忙。
enum UpdateCheckGate {
    static func allows(userInitiated: Bool, busy: Bool) -> Bool {
        userInitiated || !busy
    }
}
