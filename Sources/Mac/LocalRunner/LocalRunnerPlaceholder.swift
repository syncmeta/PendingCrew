#if os(macOS)
import Foundation

/// 本地 coding agent runner 模块入口（保留 module-level 注释；真实现见下面文件）。
///
/// agent 跑在内嵌真终端（`AgentTerminalSession` + SwiftTerm 的 PTY）里，交互式
/// 跑 claude/codex（自动审批）。旧的一次性 `--print` 子进程 + 结构化事件解析
/// 路径已退役（从未验证、无用户）—— Phase 2 的跨端遥控会基于终端的 PTY 字节流
/// 重建，而不是解析结构化事件。
///
/// **相关文件**：
///   - `LocalCodingAgentKind.swift`       —— Claude Code / Codex 枚举
///   - `LocalCodingAgentExecutable.swift` —— `which` + Homebrew 路径解析
///   - `LocalCodingAgentSpec.swift`       —— env 白名单构造（`LocalCodingAgentEnv`）
///   - `AgentTerminalSession.swift`       —— 内嵌 PTY 终端 + argv + send/interrupt/stop
///
/// **安全模型**（spec v2 §8.4）：
///   - 完全信任 + 最小 env 卫生（白名单 + 用户自配 API key）
///   - **不**继承 PendingCrew 父进程的全部 env
enum LocalRunnerModule {
    /// 留一个 marker 让 `import` 文件外面好查找。无运行时意义。
    static let moduleLoaded: Bool = true
}
#endif
