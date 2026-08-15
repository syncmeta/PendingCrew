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
    public static func resolve(_ kind: LocalCodingAgentKind) -> URL? {
        guard kind.isAgent else { return nil }
        for dir in searchDirectories() {
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
    /// 版本号按数值分段比较，不能用字符串序 —— 字符串序会把 "v9.0.0" 排在
    /// "v22.14.0" 前面，于是老版本 node 抢在新版前面进 PATH。
    static func versionedNodeBinDirs(
        root: String, versions: [String], binSubpath: String
    ) -> [String] {
        versions
            .sorted { versionComponents($0).lexicographicallyPrecedes(versionComponents($1)) }
            .reversed()
            .map { "\(root)/\($0)/\(binSubpath)" }
    }

    /// "v22.14.0" → [22, 14, 0]。解不出的段当 0（宁可排前后错一点，也别崩）。
    private static func versionComponents(_ raw: String) -> [Int] {
        raw.drop(while: { !$0.isNumber })
            .split(separator: ".")
            .map { Int($0.prefix(while: { $0.isNumber })) ?? 0 }
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
        let shell = ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh"
        let process = Process()
        process.executableURL = URL(fileURLWithPath: shell)
        // `-l` 登录（source .zprofile / .profile）+ `-i` 交互（source .zshrc）——
        // PATH 扩展两处都可能在。`-c` 传命令后 shell 跑完即退，不会阻塞等 tty 输入。
        // 用哨兵包住 $PATH，再从 stdout 里摘 —— 交互式 rc 偶尔往 stdout 吐东西。
        // `printf` 用绝对路径，避免依赖刚要解析的那个 PATH。
        process.arguments = [
            "-lic",
            "/usr/bin/printf '\(sentinelOpen)%s\(sentinelClose)' \"$PATH\"",
        ]
        let stdoutPipe = Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = Pipe()  // 丢弃 rc 噪声
        // 不覆盖 environment：继承一个 sane 基底（TERM/USER/HOME 等），让登录 shell
        // 在其上按用户 rc 重建 PATH（rc 里的 `export PATH="$HOME/.local/bin:$PATH"`
        // 这种前置式扩展无论基底 PATH 长短都会把 ~/.local/bin 带进来）。

        do {
            try process.run()
        } catch {
            return nil
        }
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }

        let data = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
        let out = String(data: data, encoding: .utf8) ?? ""
        guard let lo = out.range(of: sentinelOpen)?.upperBound,
              let hi = out.range(of: sentinelClose, range: lo..<out.endIndex)?.lowerBound
        else { return nil }
        let path = String(out[lo..<hi])
        return path.isEmpty ? nil : path
    }
}
#endif
