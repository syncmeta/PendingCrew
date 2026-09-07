#if os(macOS)
import Foundation

/// 开机自启 + 异常退出自拉的那份 LaunchAgent（P5b·A，人类 Todo #7「都做」）。
///
/// 走 `SMAppService.agent(plistName:)`：plist 放在 app 包的
/// `Contents/Library/LaunchAgents/`，注册/注销都由 app 自己调 API，不往
/// `~/Library/LaunchAgents` 里写文件。**这一层只管「那份 plist 该长什么样」**，
/// 真正的注册动作在 `DaemonAutostart`（那一层要 import ServiceManagement，
/// 进不了 test bundle）。
///
/// ## 三条 SDK 头文件里白纸黑字的事实（2026-09-07 读 `SMAppService.h` / `man 5 launchd.plist`）
///
/// - `BundleProgram` 是 SMAppService 专有的 launchd 键：**app 包内相对路径**。
///   用它而不是绝对路径，人把 app 挪个地方就不会失效。
/// - `KeepAlive = { SuccessfulExit = false }` 的语义是「**退出状态非 0 才重启**」，
///   而且 `SuccessfulExit` 这个键**隐含 `RunAtLoad = true`**（man page 原话：
///   "This key implies that RunAtLoad is set to true"）—— 所以开机自启这半也一起有了。
/// - 「app 更新了 plist 或可执行文件之后**必须重新注册**，否则可能起不来」。
///   我们有自动更新，所以这条不是理论问题，见 `DaemonAutostart` 里的处理。
///
/// ## 为什么 argv 里多一个 `--from-launchd`
///
/// 这是**这一笔里最容易被漏掉的一处**。`--daemon` 起不来时的退出码按
/// 「拉起方要的东西拿到没有」给：锁被 app 窗口占着时 `DaemonExitCode.forDaemonStart`
/// 返回**非 0**（对 app 里的 `ViewerSessionClient` 而言那确实是失败 —— 它要的是
/// 一个 daemon，而它没拿到）。
///
/// 但 launchd 眼里非 0 = 异常退出 = **立刻重启**。于是「人开着 GUI」这个再正常不过
/// 的状态会变成：起 → 发现 app 占着锁 → 退 1 → launchd 拉回来 → 起 → …
/// **一个只在别人开着窗口时才发作的重启循环**（launchd 会限流到 10 秒一次，所以它
/// 不会烧 CPU，只会在日志里无声地滚，更难被发现）。
///
/// 病根是同一个退出码在回答**两个不同的问题**：
/// - app 问的是「我要的那个 daemon 起来了吗」→ 没有，非 0。
/// - launchd 问的是「这次运行算正常收场吗」→ **算** —— 编排者已经有了，
///   这个进程该退，退得对。
///
/// 一个信号当两件事用，跟 `try?` 既当「缺席」又当「读不动」是同一族。所以让
/// launchd 那条路自报身份，两个问题各答各的。
enum PendingCrewLaunchAgent {
    /// launchd 的 job label，也是 plist 的文件名（`.plist` 后缀）。
    static let label = "com.pendingname.pendingcrew.daemon"
    static var plistName: String { label + ".plist" }

    /// app 包内相对路径 —— 同一个二进制的第三副身份，不 embed 第二个可执行文件。
    static let bundleProgram = "Contents/MacOS/PendingCrew"

    /// 「这一趟是 launchd 拉起来的」。见类型注释。
    static let launchdFlag = "--from-launchd"

    /// 只在**异常退出**时重启。绝不能是 `.always` —— 那一档会把用户的正常停用
    /// 也拉回来，等于把「我想关掉它」这个能力从人手里拿走。
    static let restartPolicy = LaunchAgentRestartPolicy.onlyWhenExitWasUnsuccessful

    /// 该长什么样。**shipping 的那份 plist 由测试逐键对着它核**，所以这个函数是
    /// 事实源，plist 文件只是它的一份拷贝 —— 有人手改了那份文件，测试会红。
    static var plist: [String: Any] {
        [
            "Label": label,
            "BundleProgram": bundleProgram,
            "ProgramArguments": [bundleProgram, SessionDaemonMainFlag.daemon, launchdFlag],
            "KeepAlive": restartPolicy.keepAlivePlistValue,
            // `SuccessfulExit` 已经隐含 RunAtLoad，写出来是为了让读 plist 的人
            // 不必去翻 man page 才知道它开机会自己起。
            "RunAtLoad": true,
            // 崩溃循环时的最小间隔。launchd 本来就限流（默认 10 秒），显式写出来
            // 是为了让「自拉」这件事有一个看得见的节流值，而不是靠系统默认。
            "ThrottleInterval": 10,
        ]
    }

    /// 这一趟是不是 launchd 拉起来的。
    static func launchedByLaunchd(_ argv: [String]) -> Bool { argv.contains(launchdFlag) }
}

/// `--daemon` 那个 flag 的字面值。
///
/// 单独拎出来是因为 `SessionDaemonMain` 在 app target 里、进不了 test bundle，
/// 而上面那份 plist 必须带上同一个 flag —— 两边各写一份字面量的话，改了一边
/// 另一边不会有任何反应，plist 里那个 flag 会安静地变成一个没人认识的参数。
enum SessionDaemonMainFlag {
    static let daemon = "--daemon"
}
#endif
