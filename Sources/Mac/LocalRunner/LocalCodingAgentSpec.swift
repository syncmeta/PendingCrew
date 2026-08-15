#if os(macOS)
import Foundation

/// 构造本机 coding agent 子进程的 env（spec v2 §8.4.2 白名单）。
///
/// 内嵌终端（`AgentTerminalSession`）拿一个 plain `[String: String]` env 起 PTY；
/// 这里是那个 env 的**唯一**来源，集中收口白名单逻辑。不继承 PendingCrew 父进程
/// 的全部 env —— 只显式放行一组基础 shell vars + 用户自配的 LLM API key。
public enum LocalCodingAgentEnv {

    /// 构造子进程的完整 env。
    ///
    /// **白名单**：
    /// - 透传基础 shell vars：`PATH` / `HOME` / `SHELL` / `LANG` / `TERM`
    /// - 透传 `LC_*`（locale）：避免 CLI 报 locale 不一致 warning
    /// - 透传 `USER` / `LOGNAME`：CLI 可能用它做 `~` 解析或日志
    /// - 透传 `TMPDIR`：macOS launchd 给每个用户一个 sandbox tmpdir，子进程也得用同一个
    /// - `additionalEnv` 里用户自配的 LLM API key
    ///
    /// **不透传**：PendingBot device grant token、Keychain 凭据、任何 `PENDING*` 前缀的
    /// PendingCrew 自己 env。所有非白名单 var 默认就不在返回 dict 里。
    public static func build(
        additionalEnv: [String: String] = [:],
        kind: LocalCodingAgentKind = .claudeCode
    ) -> [String: String] {
        var result: [String: String] = [:]

        let parent = ProcessInfo.processInfo.environment
        for key in passthroughKeys {
            if let value = parent[key] {
                result[key] = value
            }
        }
        // LC_* 是一组 var（LC_ALL / LC_CTYPE / LC_COLLATE / …）—— 不枚举全部，
        // 一律放行 LC_ 前缀。
        for (key, value) in parent where key.hasPrefix("LC_") {
            result[key] = value
        }
        // PATH：**必须和定位可执行文件时用的是同一条**。此前这里只透传 GUI app 自己
        // 那条短 PATH（launchd 注入的 `/usr/bin:/bin:/usr/sbin:/sbin`），于是出现
        // 「找得到 codex，但 codex 找不到 node」——npm/nvm 装的 codex 是
        // `#!/usr/bin/env node` 脚本，PATH 里没 node 时内核执行 shebang 当场失败，
        // 进程秒退，看起来就是「启动后立刻退出」。子进程再起的孙进程（git、node
        // 工具链）也吃这条 PATH。`childProcessPath` 复用已缓存的登录 shell PATH，
        // 并把父进程 PATH 与系统目录并进来（只增不减）。
        result["PATH"] = LocalCodingAgentExecutable.childProcessPath

        // 用户自配的 API key 等 —— 覆盖 / 补充，不与上面的白名单冲突。
        for (key, value) in additionalEnv {
            // 防御：明确拒绝 PendingCrew 自己的 secret 误进来。Caller 不应该传
            // 这种 key，但 defensive 一下方便排查。
            if isForbidden(key: key) { continue }
            result[key] = value
        }

        // 计费守卫：codex app-server 靠 ~/.codex/auth.json 走 ChatGPT 订阅计费；
        // 若父进程导出了 OPENAI_API_KEY，子进程会翻到按量 API 计费。
        // 我们控制子进程 env，所以对 codex 一律剥掉这个 key。
        // claude 路径不受影响（它用 ANTHROPIC_API_KEY）。
        if kind == .codex {
            result.removeValue(forKey: "OPENAI_API_KEY")
        }

        return result
    }

    /// 透传到子进程的 env var 白名单。详细原因见 `build(additionalEnv:)`。
    public static let passthroughKeys: [String] = [
        "PATH",
        "HOME",
        "SHELL",
        "LANG",
        "TERM",
        "USER",
        "LOGNAME",
        "TMPDIR",
    ]

    /// `additionalEnv` 里**绝不**允许的 key（PendingCrew 自己的 secret 命名规约）。
    /// 这是 defensive check，调用方本来就不该传。
    public static func isForbidden(key: String) -> Bool {
        // 别让 PendingCrew 自己的 device-grant / supabase 凭据混进去。
        // SUPABASE_ 整个前缀封死（SECRET_KEY/PUBLISHABLE_KEY/JWT_SECRET/
        // DB_PASSWORD…）——按名枚举在 key 改名时会漏（service_role →
        // sb_secret 迁移差点就漏了），sub-agent 也没有任何合法理由拿到
        // 我们的 supabase 凭据。
        let upper = key.uppercased()
        return upper.hasPrefix("PENDINGBOT_")
            || upper.hasPrefix("PENDINGCREW_")
            || upper.hasPrefix("SUPABASE_")
    }
}
#endif
