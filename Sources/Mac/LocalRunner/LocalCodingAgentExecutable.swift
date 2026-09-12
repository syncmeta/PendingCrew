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
/// 现在的策略（不依赖进程自身 PATH）：
/// 1. 起一次用户**登录 + 交互** shell（`$SHELL -lic`）把它解析好的 `$PATH` 抓回来
///    （`.zprofile` / `.zshrc` 里的 PATH 扩展两处都吃得到）。进程级缓存一次。
/// 2. 叠加一组常见安装目录兜底（`~/.local/bin`、Homebrew、各运行时全局 bin）。
/// 3. 在这些目录里找可执行文件，返回第一个命中。
///
/// 找不到时返回 `nil`，**不抛** —— 调用方负责把「未安装」做成用户可见提示。
public enum LocalCodingAgentExecutable {

    /// 解析 `kind` 对应的可执行文件 URL。命中目录列表里第一个可执行的同名文件。
    /// 人在设置里指定过目录的话，那个目录排最前 —— 它的语义就是「别再自动猜了，
    /// 就用这个」。
    public static func resolve(_ kind: LocalCodingAgentKind) -> URL? {
        guard kind.isAgent else { return nil }
        for dir in (overrideDirectory(kind).map { [$0] } ?? []) + searchDirectories() {
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
            // 人工指定的目录也要进子进程 PATH：人指一个目录是为了让那里的 CLI 被用上，
            // 而 CLI 的同伴运行时（node/bun）多半就在它旁边 —— 只让定位看得见、
            // 运行看不见，正是本文件开头那段「定位用富 PATH、运行用短 PATH」的老坑。
            searchDirs: allOverrideDirectories() + searchDirectories(),
            parentPath: ProcessInfo.processInfo.environment["PATH"])
    }

    // MARK: - 人工指定的目录（人类 Todo #11）

    /// 人在设置里为某个 harness 指定的 CLI 目录。
    ///
    /// **存 UserDefaults，不进 app 数据目录**，这是有意的：这块设置存在的意义就是
    /// 「自动找不到时人来指一下」，而那时候机器多半正出着别的毛病（本机数据目录
    /// 周期性读不动就是其中一种）。把「救场用的设置」押在另一处可能同时坏掉的存储上，
    /// 等于在它最该管用的时候不管用。
    public static func overrideDirectoryKey(_ kind: LocalCodingAgentKind) -> String {
        "cli.directoryOverride.\(kind.rawValue)"
    }

    /// 展开 `~` 之后的目录；没设过 / 设了空串 = nil。
    public static func overrideDirectory(
        _ kind: LocalCodingAgentKind, defaults: UserDefaults = .standard
    ) -> String? {
        let raw = (defaults.string(forKey: overrideDirectoryKey(kind)) ?? "")
            .trimmingCharacters(in: .whitespaces)
        return raw.isEmpty ? nil : (raw as NSString).expandingTildeInPath
    }

    /// 设为空 / nil = 取消指定，回到自动搜索。
    public static func setOverrideDirectory(
        _ path: String?, for kind: LocalCodingAgentKind, defaults: UserDefaults = .standard
    ) {
        let trimmed = (path ?? "").trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty {
            defaults.removeObject(forKey: overrideDirectoryKey(kind))
        } else {
            defaults.set(trimmed, forKey: overrideDirectoryKey(kind))
        }
    }

    /// 这个指定目录有没有问题 —— 供设置界面**当场**告诉人，而不是让他保存完去猜
    /// 为什么 session 还是起不来。nil = 没问题（或压根没设）。
    public static func overrideProblem(
        _ kind: LocalCodingAgentKind, defaults: UserDefaults = .standard
    ) -> String? {
        guard let dir = overrideDirectory(kind, defaults: defaults) else { return nil }
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: dir, isDirectory: &isDir) else {
            return "这个目录不存在。"
        }
        guard isDir.boolValue else {
            return "这是一个文件，不是目录 —— 这里要填 \(kind.binaryName) **所在的目录**。"
        }
        let candidate = URL(fileURLWithPath: dir).appendingPathComponent(kind.binaryName)
        guard FileManager.default.isExecutableFile(atPath: candidate.path) else {
            return "这个目录里没有可执行的 \(kind.binaryName)。会继续按自动搜索找。"
        }
        return nil
    }

    /// 全部已设定的目录（给子进程 PATH 用）。
    public static func allOverrideDirectories(
        defaults: UserDefaults = .standard
    ) -> [String] {
        LocalCodingAgentKind.allCases
            .filter(\.isAgent)
            .compactMap { overrideDirectory($0, defaults: defaults) }
    }

    /// `childProcessPath` 的纯逻辑内核（可单测）：搜索目录 → 父进程 PATH → 系统兜底，
    /// 按此优先级保序去重拼接。
    ///
    /// - 搜索目录排前面：CLI 在哪被找到，它的同伴运行时（node/bun/python）多半也在那。
    /// - 父进程 PATH 仍并进来：只增不减，绝不因为这次改动弄丢原本能用的目录。
    /// - 系统目录兜底：登录 shell 抓失败时 `searchDirs` 只剩用户级 bin，
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
        "/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin",
    ]

    // MARK: - internals

    /// 候选目录：登录 shell 的 PATH（缓存）+ 常见 CLI 安装位兜底。保序去重。
    private static func searchDirectories() -> [String] {
        var seen = Set<String>()
        var dirs: [String] = []
        func add(_ raw: String) {
            let trimmed = raw.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, seen.insert(trimmed).inserted else { return }
            dirs.append(trimmed)
        }
        for d in cachedLoginShellPathDirs { add(d) }
        for d in fallbackPrefixes { add((d as NSString).expandingTildeInPath) }
        for d in cachedNodeToolchainDirs { add(d) }
        return dirs
    }

    /// 进程级缓存：登录 shell 的 PATH 解析一次即可。app 生命周期内 PATH 基本不变，
    /// 而起一次登录 shell 有 ~百毫秒开销（要 source rc 文件），不值得每次 `resolve`
    /// 都付（`discoverAvailable` 一轮就 N 次）。
    private static let cachedLoginShellPathDirs: [String] = loginShellPathDirs()

    /// 常见 CLI 安装目录 —— 登录 shell 抓不到时的兜底（用户级 bin、Homebrew、
    /// 各语言运行时全局 bin）。`~` 由调用处展开。
    private static let fallbackPrefixes: [String] = [
        "~/.local/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "~/.npm-global/bin",
        "~/.bun/bin",
        "~/.deno/bin",
        "~/.cargo/bin",
        "~/bin",
    ]

    // MARK: - node 工具链兜底（nvm / fnm / volta）

    /// 版本管理器装的 node 的 bin 目录。**光有 `codex` / `claude` 不够** —— npm 装的
    /// codex 是 node 脚本，运行时得在 PATH 里找得到 `node`；而 nvm/fnm 的 node 目录
    /// 带版本号，不是固定路径，登录 shell 没 `nvm use` 时它压根不在 `$PATH` 里。
    /// 进程级缓存：要枚举目录，别每次 `resolve` 都扫盘。
    private static let cachedNodeToolchainDirs: [String] = nodeToolchainDirs()

    /// volta 的 shim 目录是固定的；nvm / fnm 的要按版本枚举。
    private static func nodeToolchainDirs() -> [String] {
        let home = NSHomeDirectory()
        var dirs: [String] = [home + "/.volta/bin"]
        // nvm: ~/.nvm/versions/node/<version>/bin
        dirs += versionedNodeBinDirs(
            root: home + "/.nvm/versions/node",
            versions: subdirectories(of: home + "/.nvm/versions/node"),
            binSubpath: "bin")
        // fnm: <root>/<version>/installation/bin（两处可能的 root）
        for root in [home + "/.fnm/node-versions",
                     home + "/Library/Application Support/fnm/node-versions"] {
            dirs += versionedNodeBinDirs(
                root: root, versions: subdirectories(of: root), binSubpath: "installation/bin")
        }
        return dirs.filter { FileManager.default.fileExists(atPath: $0) }
    }

    /// 纯逻辑（可单测）：版本目录名 → bin 目录列表，**新版在前**。
    static func versionedNodeBinDirs(root: String, versions: [String], binSubpath: String) -> [String] {
        versions.sorted {
            (versionComponents($0) ?? []).lexicographicallyPrecedes(versionComponents($1) ?? [])
        }.reversed().map { "\(root)/\($0)/\(binSubpath)" }
    }

    /// Shared numeric parser for installed CLI versions and versioned node directories.
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

    private static func subdirectories(of path: String) -> [String] {
        (try? FileManager.default.contentsOfDirectory(atPath: path)) ?? []
    }

    /// 起一次用户登录+交互 shell，把它解析好的 `$PATH` 抓回来切成目录列表。
    /// 失败（无 SHELL / 起不来 / 解析不出）→ 空数组，靠 `fallbackPrefixes` 兜底。
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
