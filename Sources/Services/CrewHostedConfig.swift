import Foundation

/// PendingCrew 直接登录所需配置。
///
/// **占位值** —— 本仓库不携带任何真实后端坐标。要用云端 crew / 直接登录，
/// 把下面四个常量换成你自己的 Supabase 项目与 Turnstile 站点密钥；
/// 只跑本地 crew 的话这些值用不到。
enum CrewHostedConfig {
    static let supabaseURL = URL(string: "https://YOUR-PROJECT.supabase.co")!
    static let supabasePublishableKey = "sb_publishable_REPLACE_ME"
    static let turnstileSiteKey = "0x0000000000000000000000"
    static let turnstileHost = URL(string: "https://example.com")!
}
