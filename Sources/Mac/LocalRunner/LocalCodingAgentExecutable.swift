#if os(macOS)
import Foundation

/// 解析本机 coding agent 的可执行路径。
///
/// **关键约束**：从 Finder / Dock / Xcode 启动的 GUI app 拿到的是 launchd 注入的
/// 短 PATH（通常只有 `/usr/bin:/bin:/usr/sbin:/sbin`），**不含**用户在
/// `.zshrc` / `.zprofile` 里扩出来的 PATH —— 而 `claude` / `codex` 这类 CLI 常装在
/// `~/.local/bin`、npm / bun / cargo 等全局 bin 处。早期版本只查进程 PATH +
/// Homebrew 两条路径，于是装在 `~/.local/bin` 的 captain 在 GUI 里永远「找不到」
/// → 静默起不来。
///
/// 现在先获取登录交互 shell 的 PATH（进程级缓存），再按 PATH 顺序取第一个命中。
/// 若 shell 探测失败，仍保留 GUI 进程 PATH 和系统目录。
///
/// 找不到时返回 `nil`，**不抛** —— 调用方负责把「未安装」做成用户可见提示。
public enum LocalCodingAgentExecutable {

    /// 解析登录 shell PATH 中第一个可执行的同名文件。
    public static func resolve(_ kind: LocalCodingAgentKind) -> URL? {
        if let discovered = resolve(kind, path: childProcessPath) { return discovered }
        // #188：受管安装的 fallback 只给 Codex，且只接受固定应用数据目录里已
        // 复验过的 npm target。Desktop app bundle 永远不是候选。
        return kind == .codex ? CodexCLIProvisioningService.managedExecutable() : nil
    }

    /// 与 agent discovery 同一条 PATH 的通用可执行文件查找；受控安装仅以此解析 npm，
    /// 不执行 shell、alias 或消息文本。
    static func resolveNamed(_ name: String, path: String = childProcessPath) -> URL? {
        guard !name.isEmpty, !name.contains("/"), !name.contains("\\0") else { return nil }
        for dir in path.split(separator: ":").map(String.init) {
            let candidate = URL(fileURLWithPath: dir).appendingPathComponent(name)
            if FileManager.default.isExecutableFile(atPath: candidate.path) { return candidate }
        }
        return nil
    }

    /// 可注入 PATH 的纯搜索边界；测试用临时目录验证首命中顺序。
    static func resolve(_ kind: LocalCodingAgentKind, path: String) -> URL? {
        guard kind.isAgent else { return nil }
        for dir in path.split(separator: ":").map(String.init) {
            let candidate = URL(fileURLWithPath: dir).appendingPathComponent(kind.binaryName)
            if FileManager.default.isExecutableFile(atPath: candidate.path) {
                return candidate
            }
        }
        return nil
    }

    /// Discovery 入口：只返回当前机器上能找到的 agent kind。普通终端的 shell
    /// 由 `PlainTerminalSession` 独立解析，不属于 agent CLI discovery。
    public static func discoverAvailable() -> [LocalCodingAgentKind] {
        LocalCodingAgentKind.allCases.filter { $0.isAgent && resolve($0) != nil }
    }

    /// 子进程 env 里该用的 `PATH` —— **和 `resolve` 用的是同一条搜索路径**。
    ///
    /// 「定位用富 PATH、运行用短 PATH」这个不对称是个真的坑：我们特意起登录 shell
    /// 把用户完整 PATH 抓回来定位 CLI，却把 GUI app 自己那条短 PATH 传给子进程。
    /// 于是「找得到 `codex`，但 `codex` 找不到 `node`」——npm/nvm 装的 codex 是个
    /// `#!/usr/bin/env node` 脚本，PATH 里没 node，内核执行 shebang 当场失败，
    /// 进程秒退，现象正好是「启动后立刻退出」。子进程再 spawn 的孙进程（git、
    /// 各种 node 工具）同样吃这条 PATH，一起受害。
    ///
    /// 复用 `searchDirectories()`（登录 shell PATH 已进程级缓存，不会再起一次 shell）。
    public static var childProcessPath: String {
        composeChildPath(
            searchDirs: searchDirectories(),
            parentPath: ProcessInfo.processInfo.environment["PATH"])
    }

    /// `childProcessPath` 的纯逻辑内核（可单测）：搜索目录 → 父进程 PATH → 系统兜底，
    /// 按此优先级保序去重拼接。
    ///
    /// - 搜索目录排前面：CLI 在哪被找到，它的同伴运行时（node/bun/python）多半也在那。
    /// - 父进程 PATH 仍并进来：只增不减，绝不因为这次改动弄丢原本能用的目录。
    /// - 系统目录兜底：登录 shell 抓失败时 `searchDirs` 为空，
    ///   连 `/usr/bin` 都没有的 PATH 会让子进程连 `env` / `git` 都跑不了。
    public static func composeChildPath(
        searchDirs: [String],
        parentPath: String?,
        systemDirs: [String] = defaultSystemDirs
    ) -> String {
        var seen = Set<String>()
        var out: [String] = []
        func add(_ raw: String) {
            let trimmed = raw.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, seen.insert(trimmed).inserted else { return }
            out.append(trimmed)
        }
        for d in searchDirs { add(d) }
        for d in (parentPath ?? "").split(separator: ":") { add(String(d)) }
        for d in systemDirs { add(d) }
        return out.joined(separator: ":")
    }

    /// 任何 PATH 都必须包含的系统目录（`env` / `sh` / `git` 都在这里）。
    public static let defaultSystemDirs: [String] = [
        "/usr/bin", "/bin", "/usr/sbin", "/sbin",
    ]

    // MARK: - internals

    /// 候选目录：登录 shell 的 PATH。其它目录只有在进程 PATH 中才参与搜索。
    private static func searchDirectories() -> [String] {
        var seen = Set<String>()
        var dirs: [String] = []
        func add(_ raw: String) {
            let trimmed = raw.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, seen.insert(trimmed).inserted else { return }
            dirs.append(trimmed)
        }
        for d in cachedLoginShellPathDirs { add(d) }
        return dirs
    }

    /// 进程级缓存：登录 shell 的 PATH 解析一次即可。app 生命周期内 PATH 基本不变，
    /// 而起一次登录 shell 有 ~百毫秒开销（要 source rc 文件），不值得每次 `resolve`
    /// 都付（`discoverAvailable` 一轮就 N 次）。
    private static let cachedLoginShellPathDirs: [String] = loginShellPathDirs()

    /// Shared numeric parser for installed CLI versions.
    /// Release tooling compares these numeric segments with missing segments padded with 0
    /// (scripts/release/build-macos-update.sh version_gt); reject unknown suffixes here.
    static func versionComponents(_ raw: String) -> [Int]? {
        let value = raw.hasPrefix("v") ? String(raw.dropFirst()) : raw
        let parts = value.split(separator: ".", omittingEmptySubsequences: false)
        guard !parts.isEmpty, parts.allSatisfy({ !$0.isEmpty && $0.utf8.allSatisfy { $0 >= 48 && $0 <= 57 } }) else { return nil }
        let numbers = parts.compactMap { Int($0) }
        return numbers.count == parts.count ? numbers : nil
    }

    /// Strip the CLI's presentation envelope; numeric parsing has one owner above.
    static func cliVersion(_ output: String) -> String? {
        let value = output.trimmingCharacters(in: .whitespacesAndNewlines)
        let raw: String
        if value.hasPrefix("codex-cli ") { raw = String(value.dropFirst("codex-cli ".count)) }
        else if value.hasSuffix(" (Claude Code)") { raw = String(value.dropLast(" (Claude Code)".count)) }
        else { raw = value }
        guard !raw.hasPrefix("v"), let parts = versionComponents(raw), parts.count == 3 else { return nil }
        return raw
    }

    /// 起一次用户登录+交互 shell，把它解析好的 `$PATH` 抓回来切成目录列表。
    /// 失败（无 SHELL / 起不来 / 解析不出）→ 空数组，再用进程 PATH。
    private static func loginShellPathDirs() -> [String] {
        guard let raw = loginShellPath() else { return [] }
        return raw.split(separator: ":").map(String.init)
    }

    /// 哨兵把真正的 `$PATH` 从交互式 rc 文件可能往 stdout 吐的噪声里摘出来。
    private static let sentinelOpen = "<<<PCREW_PATH:"
    private static let sentinelClose = ":PCREW_PATH>>>"

    private static func loginShellPath() -> String? {
        loginShellPath(shell: ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh", timeout: 5)
    }

    /// A shell rc file may block or fill stderr. Use the bounded command runner,
    /// with the inherited environment to avoid recursing into childProcessPath.
    static func loginShellPath(shell: String, timeout: TimeInterval) -> String? {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("pcrew-path-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        guard let result = try? AgentCLICommand.run(URL(fileURLWithPath: shell), [
            "-lic", "/usr/bin/printf '\(sentinelOpen)%s\(sentinelClose)' \"$PATH\"",
        ], timeout, directory: directory, environment: ProcessInfo.processInfo.environment),
              result.status == 0 else { return nil }
        let out = result.output
        guard let lo = out.range(of: sentinelOpen)?.upperBound,
              let hi = out.range(of: sentinelClose, range: lo..<out.endIndex)?.lowerBound
        else { return nil }
        let path = String(out[lo..<hi])
        return path.isEmpty ? nil : path
    }
}
#endif
