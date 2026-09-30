#if os(macOS)
import Foundation
import Darwin

struct AgentCLIFailure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// Shared across the viewer, daemon and every crew. A launch keeps a shared lease
/// until its process exits; maintenance needs an exclusive lease before inspecting ps.
final class AgentCLIMaintenanceLease {
    private let fd: Int32
    static func acquire(_ kind: LocalCodingAgentKind, exclusive: Bool,
                        directory: URL = LocalWhiteboardStore.defaultDirectory) throws -> AgentCLIMaintenanceLease {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent("cli-\(kind.rawValue).lock").path
        let fd = open(path, O_CREAT | O_RDWR | O_CLOEXEC | O_NOFOLLOW, S_IRUSR | S_IWUSR)
        guard fd >= 0 else { throw AgentCLIFailure(message: "无法打开 CLI 维护锁：\(String(cString: strerror(errno)))") }
        guard flock(fd, (exclusive ? LOCK_EX : LOCK_SH) | LOCK_NB) == 0 else {
            close(fd)
            throw AgentCLIFailure(message: "\(kind.displayName) 仍有 PendingCrew session 或检测探针存活（包括空闲/启动中），或版本维护正在进行；请停止 session / 等待探针结束后重试，本次操作未执行。")
        }
        return AgentCLIMaintenanceLease(fd: fd)
    }
    private init(fd: Int32) { self.fd = fd }
    deinit { flock(fd, LOCK_UN); close(fd) }
}

struct AgentCLICommandResult {
    let status: Int32
    let output: String
    let logURL: URL?
}

/// No shell interpolation. A separate process group bounds updater children too.
/// stdout/stderr go to a file, so pipe capacity cannot deadlock the timeout.
enum AgentCLICommand {
    static func run(_ executable: URL, _ arguments: [String], _ timeout: TimeInterval) throws -> AgentCLICommandResult {
        try run(executable, arguments, timeout,
                directory: LocalWhiteboardStore.defaultDirectory.appendingPathComponent("cli-maintenance-logs"))
    }

    static func run(_ executable: URL, _ arguments: [String], _ timeout: TimeInterval, directory: URL,
                    environment: [String: String]? = nil) throws -> AgentCLICommandResult {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let log = directory.appendingPathComponent(UUID().uuidString + ".log")
        let fd = open(log.path, O_CREAT | O_EXCL | O_RDWR | O_CLOEXEC, S_IRUSR | S_IWUSR)
        guard fd >= 0 else { throw AgentCLIFailure(message: "无法创建 CLI 命令日志，未执行命令。") }
        defer { close(fd) }
        var actions: posix_spawn_file_actions_t?
        var attrs: posix_spawnattr_t?
        posix_spawn_file_actions_init(&actions)
        posix_spawnattr_init(&attrs)
        defer { posix_spawn_file_actions_destroy(&actions); posix_spawnattr_destroy(&attrs) }
        posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_adddup2(&actions, fd, STDOUT_FILENO)
        posix_spawn_file_actions_adddup2(&actions, fd, STDERR_FILENO)
        posix_spawnattr_setflags(&attrs, Int16(POSIX_SPAWN_SETPGROUP | POSIX_SPAWN_CLOEXEC_DEFAULT))
        posix_spawnattr_setpgroup(&attrs, 0)
        var env = environment ?? ProcessInfo.processInfo.environment
        if environment == nil { env["PATH"] = LocalCodingAgentExecutable.childProcessPath }
        env["NO_COLOR"] = "1"
        env["TERM"] = "dumb"
        let argv = ([executable.path] + arguments).map { strdup($0) } + [nil]
        let envp = env.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer { argv.forEach { free($0) }; envp.forEach { free($0) } }
        var pid: pid_t = 0
        let rc = argv.withUnsafeBufferPointer { ap in
            envp.withUnsafeBufferPointer { ep in
                posix_spawn(&pid, executable.path, &actions, &attrs,
                            UnsafeMutablePointer(mutating: ap.baseAddress!), UnsafeMutablePointer(mutating: ep.baseAddress!))
            }
        }
        guard rc == 0 else { throw AgentCLIFailure(message: "无法运行 \(executable.path)：\(String(cString: strerror(rc)))；日志：\(log.path)") }
        let deadline = Date().addingTimeInterval(timeout)
        var status: Int32 = 0
        var reaped = false
        while true {
            let waited = reaped ? pid : waitpid(pid, &status, WNOHANG)
            if waited == pid { reaped = true }
            // An updater may exit while its installer child still runs. Keep the
            // maintenance lock until the entire group exits, not just the wrapper.
            if reaped && kill(-pid, 0) < 0 && errno == ESRCH { break }
            if waited < 0 && errno != EINTR {
                kill(-pid, SIGKILL)
                throw AgentCLIFailure(message: "无法确认 CLI 命令退出状态；请检查安装状态。日志：\(log.path)")
            }
            if Date() >= deadline {
                kill(-pid, SIGTERM)
                Thread.sleep(forTimeInterval: 0.2)
                kill(-pid, SIGKILL)
                if !reaped { while waitpid(pid, &status, 0) < 0 && errno == EINTR {} }
                throw AgentCLIFailure(message: "CLI 命令超过 \(Int(timeout)) 秒，已终止命令进程组；安装可能只完成一部分，请重新检测。日志：\(log.path)")
            }
            Thread.sleep(forTimeInterval: 0.05)
        }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: false)
        // Keep a bounded UI tail and retain the complete on-disk log.
        let length = try handle.seekToEnd()
        try handle.seek(toOffset: length > 32_768 ? length - 32_768 : 0)
        let output = String(decoding: try handle.readToEnd() ?? Data(), as: UTF8.self)
        let exitCode = status & 0x7f == 0 ? (status >> 8) & 0xff : 128 + (status & 0x7f)
        return AgentCLICommandResult(status: exitCode, output: output, logURL: log)
    }
}

enum AgentCLIProcessScan {
    /// Any live runner process counts, regardless of CPU use / turn status / crew.
    /// Match executable positions, not arbitrary prompt text containing "codex".
    static func blockers(_ output: String, kind: LocalCodingAgentKind) throws -> [Int32] {
        let lines = output.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        guard !lines.isEmpty else { throw AgentCLIFailure(message: "进程列表为空，无法确认没有 session；拒绝维护。") }
        return try lines.compactMap { line in
            let fields = line.split(maxSplits: 1, whereSeparator: { $0.isWhitespace })
            guard fields.count == 2, let pid = Int32(fields[0]), pid > 0 else {
                throw AgentCLIFailure(message: "进程列表无法解析，拒绝维护。")
            }
            let tokens = fields[1].split(whereSeparator: { $0.isWhitespace }).map(String.init)
            guard let first = tokens.first else { throw AgentCLIFailure(message: "缺少进程命令，拒绝维护。") }
            let name = URL(fileURLWithPath: first).lastPathComponent
            let nativeClaude = kind == .claudeCode && first.contains("/.local/share/claude/versions/")
            let direct = name == kind.binaryName || nativeClaude
            let node = ["node", "bun"].contains(name) && tokens.dropFirst().prefix(2).contains {
                kind == .codex ? $0.contains("/@openai/codex/") : $0.contains("/@anthropic-ai/claude-code/")
            }
            return direct || node ? pid : nil
        }
    }
}

struct AgentCLIInstallation: Equatable {
    let kind: LocalCodingAgentKind
    let executable: URL
    let resolved: URL
    let version: String
    let native: Bool
    let rollbackReleases: [String]
    let checkedAt: Date
}

/// 人看得见的「Codex CLI 缺失」指引。这里的来源/版本是产品代码钉死的，绝不
/// 从白板消息、URL 参数或任意文本拼出将要执行的命令。
struct CodexCLIInstallGuide: Equatable {
    static let officialRegistry = "https://registry.npmjs.org/"
    static let officialPackage = "@openai/codex"
    static let officialPackageVersion = "0.159.1"
    static let documentationURL = URL(string: "https://learn.chatgpt.com/docs/codex/cli")!

    let registry = officialRegistry
    let package = officialPackage
    let version = officialPackageVersion
    let documentation = documentationURL
}

/// 唯一能交给安装器的动作。构造器仅限本文件，调用者无法把白板/聊天文本替换成
/// package、registry、target 或 argv。
struct CodexCLIInstallAction: Equatable {
    let packageManager: URL
    let target: URL
    let cache: URL
    let registry: String
    let package: String
    let arguments: [String]
    let requiresSecondConfirmation = true

    fileprivate init(packageManager: URL, target: URL, cache: URL) {
        self.packageManager = packageManager
        self.target = target
        self.cache = cache
        self.registry = CodexCLIInstallGuide.officialRegistry
        self.package = CodexCLIInstallGuide.officialPackage + "@" + CodexCLIInstallGuide.officialPackageVersion
        self.arguments = [
            "install",
            "--prefix", target.path,
            "--cache", cache.path,
            "--userconfig", "/dev/null",
            "--globalconfig", "/dev/null",
            "--registry", CodexCLIInstallGuide.officialRegistry,
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            self.package,
        ]
    }
}

struct ManagedCodexCLIInstallation: Equatable {
    let executable: URL
    let version: String
}

/// 这是 UI 的权威状态，而不是对 Desktop bundle、auth.json 或某条错误字符串的猜测。
/// `installedNeedsManualSignIn` 不读取认证缓存；它只是明确告诉人：本轮只验证了二进制。
enum CodexCLIProvisioningState: Equatable {
    case ready
    case missing(CodexCLIInstallGuide)
    case awaitingSecondConfirmation(CodexCLIInstallAction)
    case installing(CodexCLIInstallAction)
    case installedNeedsManualSignIn(ManagedCodexCLIInstallation)
    case failed(String)
}

/// #188：Codex CLI 缺失时的固定安装链。
///
/// - P1：`currentState`/`prepareOfficialInstall` 给 UI 一个可见、可解释的状态；准备
///   动作本身不启动网络/包管理器。
/// - P2：`installConfirmed` 只接受上面的受限动作，并复用跨 crew 的 CLI 独占锁。
///   它不读认证缓存、不启动 `codex login`、不启动 app-server/agent，也不自动重试。
struct CodexCLIProvisioningService {
    typealias Locate = () -> URL?
    typealias Execute = (URL, [String], TimeInterval, [String: String]) throws -> AgentCLICommandResult

    var home = FileManager.default.homeDirectoryForCurrentUser
    var managedRoot = PendingCrewDataRoot.subdirectory("managed-codex-cli")
    var lockDirectory = LocalWhiteboardStore.defaultDirectory
    var resolveCodex: () -> URL? = { LocalCodingAgentExecutable.resolve(.codex) }
    var locateNpm: Locate = { LocalCodingAgentExecutable.resolveNamed("npm") }
    var processExecute: AgentCLIMaintenanceService.Execute = AgentCLICommand.run
    var execute: Execute = { executable, arguments, timeout, environment in
        try AgentCLICommand.run(
            executable, arguments, timeout,
            directory: LocalWhiteboardStore.defaultDirectory.appendingPathComponent("cli-maintenance-logs"),
            environment: environment)
    }

    func currentState() -> CodexCLIProvisioningState {
        resolveCodex() == nil ? .missing(.init()) : .ready
    }

    /// 第一次点击只产生固定计划；绝不联网、绝不执行 npm。
    func prepareOfficialInstall() throws -> CodexCLIInstallAction {
        guard case .missing = currentState() else {
            throw AgentCLIFailure(message: "已找到 Codex CLI；不会覆盖已有安装。")
        }
        guard let npm = locateNpm(), npm.lastPathComponent == "npm",
              FileManager.default.isExecutableFile(atPath: npm.path) else {
            throw AgentCLIFailure(message: "未找到可执行 npm；无法准备官方 Codex CLI 安装。")
        }
        let target = managedTarget
        guard Self.isDescendant(target, of: managedRoot) else {
            throw AgentCLIFailure(message: "受管 Codex CLI 目标目录无效；拒绝安装。")
        }
        guard !FileManager.default.fileExists(atPath: target.path) else {
            throw AgentCLIFailure(message: "受管 Codex CLI 目标已存在；不会覆盖或自动重试，请重新检测后人工处理。")
        }
        return .init(packageManager: npm, target: target, cache: managedCache)
    }

    /// 只能由 UI 的第二次确认调用。动作字段逐项回验，避免未来调用点把它退化成
    /// `String -> shell` 的自由命令入口。
    func installConfirmed(_ action: CodexCLIInstallAction) throws -> ManagedCodexCLIInstallation {
        guard action.requiresSecondConfirmation,
              action.registry == CodexCLIInstallGuide.officialRegistry,
              action.package == CodexCLIInstallGuide.officialPackage + "@" + CodexCLIInstallGuide.officialPackageVersion,
              action.target == managedTarget,
              action.cache == managedCache,
              action.arguments == CodexCLIInstallAction(
                  packageManager: action.packageManager,
                  target: managedTarget,
                  cache: managedCache).arguments,
              action.packageManager.lastPathComponent == "npm",
              FileManager.default.isExecutableFile(atPath: action.packageManager.path)
        else {
            throw AgentCLIFailure(message: "安装计划不是 PendingCrew 生成的固定官方动作；拒绝执行。")
        }

        let lease = try AgentCLIMaintenanceLease.acquire(.codex, exclusive: true, directory: lockDirectory)
        defer { withExtendedLifetime(lease) {} }
        let blockers = try AgentCLIMaintenanceService.processBlockers(.codex, execute: processExecute)
        guard blockers.isEmpty else {
            throw AgentCLIFailure(message: "Codex 仍有存活进程，PID：\(blockers.map(String.init).joined(separator: ", "))；不会安装。")
        }
        guard case .missing = currentState() else {
            throw AgentCLIFailure(message: "确认前已出现可用 Codex CLI；不会覆盖或自动重试。")
        }
        guard !FileManager.default.fileExists(atPath: action.target.path) else {
            throw AgentCLIFailure(message: "确认前受管目标已出现；不会覆盖或自动重试。")
        }

        // This is deliberately after the second confirmation and every launch/process
        // gate. Creating these private directories is part of the confirmed action;
        // the first click remains offline and leaves no filesystem artifacts.
        //
        // If npm itself fails, root/target/cache and the 0600 command log are retained
        // for human inspection. A later prepare refuses the existing version target,
        // so this path never silently cleans up, retries, or falls back to HOME's cache.
        try Self.preparePrivateDirectory(managedRoot)
        try Self.preparePrivateDirectory(managedCache)
        try Self.preparePrivateDirectory(action.target)

        let install = try execute(action.packageManager, action.arguments, 300, environment)
        guard install.status == 0 else {
            throw AgentCLIFailure(message: "官方 Codex CLI 安装失败（退出码 \(install.status)）：\n\(install.output)\n日志：\(install.logURL?.path ?? "无")")
        }
        guard let executable = Self.managedExecutable(root: managedRoot) else {
            throw AgentCLIFailure(message: "安装命令成功退出，但固定目标未出现可执行 Codex CLI；不会标成可用。")
        }
        let version = try execute(executable, ["--version"], 15, environment)
        guard version.status == 0,
              LocalCodingAgentExecutable.cliVersion(version.output) == CodexCLIInstallGuide.officialPackageVersion else {
            throw AgentCLIFailure(message: "安装后 Codex CLI 版本复验失败；不会标成可用。日志：\(version.logURL?.path ?? "无")")
        }
        return .init(executable: executable, version: CodexCLIInstallGuide.officialPackageVersion)
    }

    /// 仅在固定受管目录下接受 npm 的 `.bin/codex`；不探测或借用 Desktop app bundle。
    static func managedExecutable(root: URL = PendingCrewDataRoot.subdirectory("managed-codex-cli")) -> URL? {
        let target = managedTarget(root: root)
        let executable = target.appendingPathComponent("node_modules/.bin/codex")
        let resolved = executable.resolvingSymlinksInPath()
        guard isPrivateDirectory(root),
              isPrivateDirectory(target),
              isDescendant(executable, of: target),
              isDescendant(resolved, of: target),
              FileManager.default.isExecutableFile(atPath: executable.path)
        else { return nil }
        return executable
    }

    /// 普通 refresh 只能从固定安装目标和已复验的二进制版本判断“这是我们受管的
    /// CLI”。它特意不读取任何认证状态，因此调用方必须继续显示认证未知，而不是
    /// 把一次成功的 `--version` 探测误报为可直接启动。
    static func managedInstallation(
        for installation: AgentCLIInstallation,
        root: URL = PendingCrewDataRoot.subdirectory("managed-codex-cli")
    ) -> ManagedCodexCLIInstallation? {
        guard installation.kind == .codex,
              installation.version == CodexCLIInstallGuide.officialPackageVersion,
              let executable = managedExecutable(root: root),
              installation.executable.standardizedFileURL == executable.standardizedFileURL
        else { return nil }
        return .init(executable: executable, version: installation.version)
    }

    private var managedTarget: URL { Self.managedTarget(root: managedRoot) }
    private var managedCache: URL { managedRoot.appendingPathComponent("npm-cache", isDirectory: true) }

    private static func managedTarget(root: URL) -> URL {
        root.appendingPathComponent(CodexCLIInstallGuide.officialPackageVersion, isDirectory: true)
    }

    private var environment: [String: String] {
        var values = [
            "HOME": home.path,
            "PATH": LocalCodingAgentExecutable.childProcessPath,
            "npm_config_registry": CodexCLIInstallGuide.officialRegistry,
            "npm_config_cache": managedCache.path,
            "npm_config_ignore_scripts": "true",
            "npm_config_audit": "false",
            "npm_config_fund": "false",
        ]
        if let tmp = ProcessInfo.processInfo.environment["TMPDIR"], !tmp.isEmpty { values["TMPDIR"] = tmp }
        return values
    }

    private static func isDescendant(_ child: URL, of parent: URL) -> Bool {
        let childPath = child.standardizedFileURL.path
        let parentPath = parent.standardizedFileURL.path
        return childPath.hasPrefix(parentPath + "/")
    }

    /// Make a directory private only after proving an existing directory is safe to
    /// touch. In particular, never chmod through a symlink or repair a directory that
    /// another local user/group could already have populated.
    private static func preparePrivateDirectory(_ url: URL) throws {
        var metadata = stat()
        if lstat(url.path, &metadata) != 0 {
            guard errno == ENOENT else {
                throw AgentCLIFailure(message: "无法检查受管 Codex CLI 目录；拒绝安装。")
            }
            try FileManager.default.createDirectory(
                at: url,
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700])
            guard lstat(url.path, &metadata) == 0 else {
                throw AgentCLIFailure(message: "受管 Codex CLI 目录创建后无法检查；拒绝安装。")
            }
        }

        guard isOwnedDirectory(metadata) else {
            throw AgentCLIFailure(message: "受管 Codex CLI 目录不是当前用户私有目录（符号链接、owner 或 group/others 写入不安全）；拒绝安装。")
        }
        guard chmod(url.path, 0o700) == 0, isPrivateDirectory(url) else {
            throw AgentCLIFailure(message: "无法把受管 Codex CLI 目录固定为当前用户 0700；拒绝安装。")
        }
    }

    private static func isPrivateDirectory(_ url: URL) -> Bool {
        var metadata = stat()
        guard lstat(url.path, &metadata) == 0,
              isOwnedDirectory(metadata),
              metadata.st_mode & mode_t(0o777) == mode_t(0o700)
        else { return false }
        return true
    }

    private static func isOwnedDirectory(_ metadata: stat) -> Bool {
        guard metadata.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR),
              metadata.st_uid == geteuid(),
              metadata.st_mode & mode_t(0o022) == 0
        else { return false }
        return true
    }
}

/// Injectable command/locator boundary: tests never call a real updater.
struct AgentCLIMaintenanceService {
    typealias Execute = (URL, [String], TimeInterval) throws -> AgentCLICommandResult
    var home = FileManager.default.homeDirectoryForCurrentUser
    var lockDirectory = LocalWhiteboardStore.defaultDirectory
    var resolve: (LocalCodingAgentKind) -> URL? = LocalCodingAgentExecutable.resolve
    var execute: Execute = AgentCLICommand.run

    func inspect(_ kind: LocalCodingAgentKind) throws -> AgentCLIInstallation {
        guard let exe = resolve(kind) else { throw AgentCLIFailure(message: "未找到 \(kind.displayName)；请先安装 CLI。") }
        let resolved = exe.resolvingSymlinksInPath()
        let result = try checked(exe, ["--version"], 15)
        guard let version = LocalCodingAgentExecutable.cliVersion(result.output) else {
            throw AgentCLIFailure(message: "\(kind.displayName) 版本输出无法解析：\(result.output)\n日志：\(result.logURL?.path ?? "无")")
        }
        guard exe.resolvingSymlinksInPath() == resolved else { throw AgentCLIFailure(message: "检测期间 CLI 路径发生变化，请重新检测。") }
        let native: Bool
        var releases: [String] = []
        if kind == .claudeCode {
            native = resolved.deletingLastPathComponent() == home.appendingPathComponent(".local/share/claude/versions")
                && resolved.lastPathComponent == version
        } else {
            let current = home.appendingPathComponent(".codex/packages/standalone/current")
            let release = resolved.deletingLastPathComponent().deletingLastPathComponent()
            native = release.deletingLastPathComponent() == releasesDirectory
                && resolved == current.appendingPathComponent("bin/codex").resolvingSymlinksInPath()
                && release.lastPathComponent.hasPrefix(version + "-")
            if native {
                let suffix = String(release.lastPathComponent.dropFirst(version.count))
                releases = try FileManager.default.contentsOfDirectory(atPath: releasesDirectory.path).filter {
                    guard $0.hasSuffix(suffix), $0 != release.lastPathComponent else { return false }
                    let raw = String($0.dropLast(suffix.count))
                    return LocalCodingAgentExecutable.cliVersion(raw) != nil
                        && FileManager.default.isExecutableFile(atPath: releasesDirectory.appendingPathComponent($0 + "/bin/codex").path)
                }.sorted {
                    (LocalCodingAgentExecutable.versionComponents(String($1.dropLast(suffix.count))) ?? [])
                        .lexicographicallyPrecedes(LocalCodingAgentExecutable.versionComponents(String($0.dropLast(suffix.count))) ?? [])
                }
            }
        }
        return .init(kind: kind, executable: exe, resolved: resolved, version: version,
                     native: native, rollbackReleases: releases, checkedAt: Date())
    }

    private var releasesDirectory: URL { home.appendingPathComponent(".codex/packages/standalone/releases") }

    func update(_ expected: AgentCLIInstallation, target: String = "") throws -> String {
        let lease = try AgentCLIMaintenanceLease.acquire(expected.kind, exclusive: true, directory: lockDirectory)
        defer { withExtendedLifetime(lease) {} }
        try requireNoProcesses(expected.kind)
        let before = try verified(expected)
        guard target.isEmpty || (expected.kind == .claudeCode && (["stable", "latest"].contains(target) || LocalCodingAgentExecutable.cliVersion(target) == target)) else {
            throw AgentCLIFailure(message: "不支持的升级目标；仅 Claude 可选 stable / latest / 具体版本。")
        }
        let result = try checked(before.executable, ["update"] + (target.isEmpty ? [] : [target]), 300)
        let after = try inspect(expected.kind)
        return "命令已完成：\(before.version) → \(after.version)。\(before.version == after.version ? "版本未变化；这不证明已是最新。" : "新 session 将使用检测到的版本。")\n\(result.output)\n完整日志：\(result.logURL?.path ?? "测试输出")"
    }

    func doctor(_ kind: LocalCodingAgentKind) throws -> String {
        guard kind == .claudeCode, let exe = resolve(kind) else { throw AgentCLIFailure(message: "仅支持已安装 Claude Code 的 doctor。") }
        let result = try checked(exe, ["doctor"], 30)
        return result.output + "\n完整日志：" + (result.logURL?.path ?? "测试输出")
    }

    func rollback(_ expected: AgentCLIInstallation, release: String) throws -> String {
        let lease = try AgentCLIMaintenanceLease.acquire(.codex, exclusive: true, directory: lockDirectory)
        defer { withExtendedLifetime(lease) {} }
        try requireNoProcesses(.codex)
        let before = try verified(expected)
        guard before.kind == .codex, before.rollbackReleases.contains(release) else { throw AgentCLIFailure(message: "回滚目标不在本机同平台保留版本里。") }
        let target = releasesDirectory.appendingPathComponent(release)
        guard target.resolvingSymlinksInPath() == target else { throw AgentCLIFailure(message: "回滚目录包含外部符号链接，拒绝切换。") }
        let binary = target.appendingPathComponent("bin/codex")
        guard binary.resolvingSymlinksInPath() == binary else { throw AgentCLIFailure(message: "回滚二进制是符号链接，拒绝切换。") }
        let probe = try checked(binary, ["--version"], 15)
        guard let version = LocalCodingAgentExecutable.cliVersion(probe.output), release.hasPrefix(version + "-") else { throw AgentCLIFailure(message: "回滚候选的真实版本与目录不符，未切换。") }
        let current = home.appendingPathComponent(".codex/packages/standalone/current")
        _ = try FileManager.default.destinationOfSymbolicLink(atPath: current.path)
        let temporary = current.deletingLastPathComponent().appendingPathComponent(".current-" + UUID().uuidString)
        try FileManager.default.createSymbolicLink(at: temporary, withDestinationURL: target)
        defer { try? FileManager.default.removeItem(at: temporary) }
        guard rename(temporary.path, current.path) == 0 else { throw AgentCLIFailure(message: "回滚切换失败：\(String(cString: strerror(errno)))") }
        let after = try inspect(.codex)
        guard after.version == version, after.resolved == binary else { throw AgentCLIFailure(message: "已切换 current，但复验不一致；请检查安装状态，不能宣称回滚成功。") }
        return "已回滚：\(before.version) → \(after.version)。新 session 将使用 \(after.version)。"
    }

    private func verified(_ expected: AgentCLIInstallation) throws -> AgentCLIInstallation {
        let actual = try inspect(expected.kind)
        guard actual.native else { throw AgentCLIFailure(message: "此安装不是受支持的原生目录；brew/npm/自定义安装仅检测，请通过原安装渠道管理。") }
        guard actual.resolved == expected.resolved, actual.executable == expected.executable, actual.version == expected.version else {
            throw AgentCLIFailure(message: "确认后 CLI 安装已变化，请重新检测并确认。")
        }
        return actual
    }

    static func processBlockers(_ kind: LocalCodingAgentKind, execute: Execute) throws -> [Int32] {
        let result = try execute(URL(fileURLWithPath: "/bin/ps"), ["-axo", "pid=,command=", "-ww"], 10)
        guard result.status == 0 else {
            throw AgentCLIFailure(message: "ps -axo pid=,command= -ww 失败（退出码 \(result.status)）：\n\(result.output)\n日志：\(result.logURL?.path ?? "无")")
        }
        return try AgentCLIProcessScan.blockers(result.output, kind: kind)
    }

    private func requireNoProcesses(_ kind: LocalCodingAgentKind) throws {
        let pids = try Self.processBlockers(kind, execute: execute)
        guard pids.isEmpty else { throw AgentCLIFailure(message: "\(kind.displayName) 仍有存活进程（含空闲 session / 外部终端 / 探针），PID：\(pids.map(String.init).joined(separator: ", "))。请全部停止后再试。") }
    }

    private func checked(_ exe: URL, _ args: [String], _ timeout: TimeInterval) throws -> AgentCLICommandResult {
        let result = try execute(exe, args, timeout)
        guard result.status == 0 else { throw AgentCLIFailure(message: "\(exe.lastPathComponent) \(args.joined(separator: " ")) 失败（退出码 \(result.status)）：\n\(result.output)\n日志：\(result.logURL?.path ?? "无")") }
        return result
    }
}
#endif
