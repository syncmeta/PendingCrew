import Foundation

/// PendingCrew 直接登录所需配置。
///
/// **本仓库只带占位值** —— 不携带任何真实后端坐标。要用云端 crew / 直接登录，
/// 把下面四个常量换成你自己的 Supabase 项目与 Turnstile 站点密钥；只跑本地
/// crew 的话这些值一次都用不到（启动路径不碰 `CrewSupabaseStack`）。
///
/// `isConfigured` 是「云端这条线通不通」的**唯一真值**。登录入口据它决定是
/// 正常走还是当场说清，README 的能力清单也引用它 —— 这样填上真坐标那天，
/// 代码自己就不拦了，不需要有人记得回去改文档。
enum CrewHostedConfig {
    /// 占位常量的字面量。`isConfigured` 拿它们做判据，所以**换坐标的时候
    /// 只改下面 public 的那四个，别动这里** —— 动了会让判据永远为真。
    private enum Placeholder {
        static let supabaseURL = "https://YOUR-PROJECT.supabase.co"
        static let supabasePublishableKey = "sb_publishable_REPLACE_ME"
        static let turnstileSiteKey = "0x0000000000000000000000"
        static let turnstileHost = "https://example.com"
    }

    static let supabaseURL = URL(string: Placeholder.supabaseURL)!
    static let supabasePublishableKey = Placeholder.supabasePublishableKey
    static let turnstileSiteKey = Placeholder.turnstileSiteKey
    static let turnstileHost = URL(string: Placeholder.turnstileHost)!

    /// 四个坐标是否都已从占位值换成真的。
    ///
    /// 必须**四个全换**才算配好：登录流程是「Turnstile 人机验证 → Supabase
    /// 发码/验码」串起来的，缺任何一段都走不完，只换一半反而会走到半路才炸，
    /// 那比一开始就说清更难查。
    static var isConfigured: Bool {
        supabaseURL.absoluteString != Placeholder.supabaseURL
            && supabasePublishableKey != Placeholder.supabasePublishableKey
            && turnstileSiteKey != Placeholder.turnstileSiteKey
            && turnstileHost.absoluteString != Placeholder.turnstileHost
    }

    /// 用户主动去走登录时给他看的说明。
    ///
    /// 措辞的两个要点：① 先说「本地 crew 不需要它」—— 大多数人到这儿是误入，
    /// 得让他知道自己没错过什么；② 再说要接自己的后端该看哪儿。**不说
    /// 「失败」「错误」**，因为这不是故障，是这份开源仓库本来就不带的东西。
    static let unconfiguredNotice = """
        云端登录不可用：本仓库只带占位后端坐标。\
        跑本地 crew（起 Claude Code / Codex session、群聊、终端）不需要登录。\
        要接自己的 Supabase + Turnstile，见 README 的「状态」一节。
        """
}
