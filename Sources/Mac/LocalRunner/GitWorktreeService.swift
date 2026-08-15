#if os(macOS)
import Foundation

/// 薄薄一层包住几个 `git` 命令,给 session 面板"新 worktree"用。
///
/// **设计**:
/// - 全部走 `/usr/bin/env git` —— 跟 `LocalCodingAgentExecutable` 一个套路,
///   遵循 GUI launchd 注入的 PATH。用户没装 git 时这里直接 throw,UI 层兜底
///   把按钮禁用。
/// - 不走 shell,argv 数组形式,不需要 escape。同样防 shell injection。
/// - **env 卫生**:跟子进程一样的白名单(PATH/HOME/SHELL/LANG/USER/LOGNAME/TMPDIR)。
///   git 需要 HOME 来读 ~/.gitconfig 和 ~/.ssh/known_hosts —— 必须透传。
/// - sync API:几十 ms 的命令,不值得 async/await 包装。actor / Task 包不包外面
///   由 caller 决定;我们在 SwiftUI button handler 里直接放 `Task.detached`
///   就行。
///
/// **不做**的事:
/// - 不做 `git pull` / `git fetch` —— PendingCrew 不替用户决定 sync 策略
/// - 不做"clean working tree 检查" —— `git worktree add` 自己会拒绝有冲突的情况
/// - 不缓存:每次问都重新跑 git,避免和外面 CLI / GUI Git 客户端状态不一致
public enum GitWorktreeService {

    public enum GitError: Error, CustomStringConvertible {
        /// `git` 不在 PATH 上(或不可执行)。
        case gitNotInstalled
        /// 目录不是 git repo。
        case notAGitRepo(URL)
        /// `git <subcommand>` 退出非 0,带 stderr 全文。
        case commandFailed(argv: [String], exitCode: Int32, stderr: String)
        /// 想拿 stdout 解析,结果不是合法 UTF-8。
        case nonUTF8Output(argv: [String])

        public var description: String {
            switch self {
            case .gitNotInstalled:
                return "git 不在 PATH 上 — 请先 `brew install git`"
            case .notAGitRepo(let url):
                return "目录不是 git repo: \(url.path)"
            case .commandFailed(let argv, let code, let stderr):
                let cmd = (["git"] + argv).joined(separator: " ")
                return "git 命令失败 (\(code)): \(cmd)\nstderr:\n\(stderr.trimmingCharacters(in: .whitespacesAndNewlines))"
            case .nonUTF8Output(let argv):
                let cmd = (["git"] + argv).joined(separator: " ")
                return "git 命令 stdout 不是 UTF-8: \(cmd)"
            }
        }
    }

    // MARK: - Public API

    /// 给定目录是不是 git work tree(含 worktree 副本)。
    /// 找不到 git 直接 throw —— UI 层据此决定整组"git 选项"灰显。
    public static func isGitRepository(at url: URL) throws -> Bool {
        guard let git = locateGit() else { throw GitError.gitNotInstalled }
        let result = try run(
            git: git,
            cwd: url,
            argv: ["rev-parse", "--is-inside-work-tree"],
            // 不在 repo 里时 git 返回 non-zero + stderr 喊"not a git repo"。
            // 我们想把这翻译成 false 而不是 throw,所以 acceptNonZero=true。
            acceptNonZero: true
        )
        guard result.exitCode == 0 else { return false }
        return result.stdout.trimmingCharacters(in: .whitespacesAndNewlines) == "true"
    }

    /// 列出 repo 里所有 local branch 名(去掉 `* ` / `+ ` 前缀)。
    /// 用 `git for-each-ref refs/heads/` 而不是 `git branch --list`:
    /// `for-each-ref` 输出更稳定(无星号 / 无 padding),容易解析。
    public static func listLocalBranches(at url: URL) throws -> [String] {
        guard let git = locateGit() else { throw GitError.gitNotInstalled }
        guard try isGitRepository(at: url) else { throw GitError.notAGitRepo(url) }
        let result = try run(
            git: git,
            cwd: url,
            argv: ["for-each-ref", "--format=%(refname:short)", "refs/heads/"]
        )
        return result.stdout
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// 当前 checkout 的分支名(detached HEAD 时为 nil)。
    public static func currentBranch(at url: URL) throws -> String? {
        guard let git = locateGit() else { throw GitError.gitNotInstalled }
        guard try isGitRepository(at: url) else { throw GitError.notAGitRepo(url) }
        let result = try run(
            git: git,
            cwd: url,
            argv: ["symbolic-ref", "--quiet", "--short", "HEAD"],
            // detached HEAD 时 symbolic-ref 返回 non-zero;我们把这个当 nil。
            acceptNonZero: true
        )
        guard result.exitCode == 0 else { return nil }
        let trimmed = result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// 拿 repo 的 top-level dir(`git rev-parse --show-toplevel`)。
    /// 用来把"worktree add 到 repo 根隔壁的 .pendingcrew/worktrees/"对齐。
    public static func repositoryRoot(at url: URL) throws -> URL {
        guard let git = locateGit() else { throw GitError.gitNotInstalled }
        guard try isGitRepository(at: url) else { throw GitError.notAGitRepo(url) }
        let result = try run(
            git: git,
            cwd: url,
            argv: ["rev-parse", "--show-toplevel"]
        )
        let path = result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        return URL(fileURLWithPath: path)
    }

    /// 在 repo 根的 `.pendingcrew/worktrees/<slug>/` 处 `git worktree add`。
    ///
    /// - `sourceRepository`: 任何在 repo 内的目录(包括 sub-worktree)
    /// - `branchName`: checkout 的 base 分支(必填,否则 worktree add 会拿 HEAD)
    /// - `newBranchName`: 非 nil = `git worktree add -b <new> <path> <branch>`(在新分支上起);
    ///   nil = `git worktree add <path> <branch>`(共享已有分支)
    /// - `worktreeNameHint`: 目录命名前缀。最终名字 `<hint>-<8 char uuid>` 避免碰撞。
    ///
    /// 返回新 worktree 的绝对 URL,可直接当 session 的 working directory 用。
    @discardableResult
    public static func addWorktree(
        sourceRepository: URL,
        branchName: String,
        newBranchName: String?,
        worktreeNameHint: String
    ) throws -> URL {
        guard let git = locateGit() else { throw GitError.gitNotInstalled }
        guard try isGitRepository(at: sourceRepository) else {
            throw GitError.notAGitRepo(sourceRepository)
        }
        let root = try repositoryRoot(at: sourceRepository)
        let container = root
            .appendingPathComponent(".pendingcrew", isDirectory: true)
            .appendingPathComponent("worktrees", isDirectory: true)
        // 容器不存在就 mkdir -p。`.pendingcrew/` 通常不在 .gitignore,所以
        // 我们顺手把目录加进 git 的 exclude(本地 only,不影响 repo)。
        try FileManager.default.createDirectory(at: container, withIntermediateDirectories: true)
        ensureLocalGitignore(repoRoot: root)

        let slug = sanitizeSlug(worktreeNameHint)
        let suffix = UUID().uuidString.lowercased().prefix(8)
        let dirName = "\(slug)-\(suffix)"
        let target = container.appendingPathComponent(dirName, isDirectory: true)

        // 构造 argv。`worktree add` 形式:
        //   git worktree add [-b <newbranch>] <path> [<commit-ish>]
        var argv: [String] = ["worktree", "add"]
        if let newBranchName, !newBranchName.isEmpty {
            argv.append(contentsOf: ["-b", newBranchName])
        }
        argv.append(target.path)
        argv.append(branchName)

        _ = try run(git: git, cwd: root, argv: argv)
        return target
    }

    // MARK: - Internals

    /// 在 repo `.git/info/exclude` 里加一条 `/.pendingcrew/` 避免把 worktree
    /// 元数据误 commit。`.git/info/exclude` 跟 `.gitignore` 行为一致但**仅本机**,
    /// 不污染 repo。已经有的话不重复加。
    private static func ensureLocalGitignore(repoRoot: URL) {
        let exclude = repoRoot
            .appendingPathComponent(".git", isDirectory: true)
            .appendingPathComponent("info", isDirectory: true)
            .appendingPathComponent("exclude")
        // `.git` 可能是 file(worktree case)指向真 repo;那 .git/info/exclude 也
        // 跟着不一样。简单粗暴:不存在就跳过,不 risk 写错地方。
        let isFile = ((try? exclude.checkResourceIsReachable()) ?? false)
        guard isFile else { return }
        let needle = "/.pendingcrew/"
        if let content = try? String(contentsOf: exclude, encoding: .utf8),
           content.contains(needle) {
            return
        }
        // append `/<NL>.pendingcrew/`
        let toAppend = "\n# Added by PendingCrew — worktree scratch dir.\n\(needle)\n"
        if let handle = try? FileHandle(forWritingTo: exclude) {
            _ = try? handle.seekToEnd()
            if let data = toAppend.data(using: .utf8) {
                try? handle.write(contentsOf: data)
            }
            try? handle.close()
        }
    }

    /// 把任意字符串清成可作目录名的 ASCII slug:
    /// - 非 `[a-zA-Z0-9_-]` 一律替换成 `-`
    /// - 连续多个 `-` 合一个
    /// - 头尾 trim `-`
    /// - 截到 32 chars 以内
    /// - 空串兜底成 `"crew"`
    private static func sanitizeSlug(_ raw: String) -> String {
        var out = ""
        var lastWasDash = false
        for char in raw.unicodeScalars {
            let isAllowed = (char >= "a" && char <= "z")
                || (char >= "A" && char <= "Z")
                || (char >= "0" && char <= "9")
                || char == "_" || char == "-"
            if isAllowed {
                out.unicodeScalars.append(char)
                lastWasDash = (char == "-")
            } else if !lastWasDash {
                out.append("-")
                lastWasDash = true
            }
        }
        // trim leading / trailing dashes
        while out.hasPrefix("-") { out.removeFirst() }
        while out.hasSuffix("-") { out.removeLast() }
        if out.count > 32 { out = String(out.prefix(32)) }
        return out.isEmpty ? "crew" : out
    }

    /// 找 `git` 可执行路径。复用 `whichLookup` 的逻辑:env 短 PATH + Homebrew
    /// fallback。
    private static func locateGit() -> URL? {
        // 先看常见绝对路径(避免 fork+exec env 的成本)。
        for candidate in [
            "/usr/bin/git",
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
        ] {
            if FileManager.default.isExecutableFile(atPath: candidate) {
                return URL(fileURLWithPath: candidate)
            }
        }
        // 兜底跑 `env which git`。
        return whichGit()
    }

    private static func whichGit() -> URL? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["which", "--", "git"]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = Pipe()
        process.environment = [
            "PATH": ProcessInfo.processInfo.environment["PATH"]
                ?? "/usr/bin:/bin:/usr/sbin:/sbin",
        ]
        do {
            try process.run()
        } catch {
            return nil
        }
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let line = String(data: data, encoding: .utf8)?
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first(where: { !$0.isEmpty }) ?? ""
        guard !line.isEmpty,
              FileManager.default.isExecutableFile(atPath: line) else {
            return nil
        }
        return URL(fileURLWithPath: line)
    }

    private struct RunResult {
        let exitCode: Int32
        let stdout: String
        let stderr: String
    }

    /// 跑 `git <argv>` 同步,捕 stdout/stderr。
    /// 默认非 0 抛 `.commandFailed`。`acceptNonZero=true` 时返回 RunResult,
    /// caller 自己判 `exitCode`。
    private static func run(
        git: URL,
        cwd: URL,
        argv: [String],
        acceptNonZero: Bool = false
    ) throws -> RunResult {
        let process = Process()
        process.executableURL = git
        process.arguments = argv
        process.currentDirectoryURL = cwd
        // env 白名单同 LocalCodingAgentEnv(git 需要 HOME 读 ~/.gitconfig)。
        var env: [String: String] = [:]
        let parent = ProcessInfo.processInfo.environment
        for key in ["PATH", "HOME", "SHELL", "LANG", "TERM", "USER", "LOGNAME", "TMPDIR"] {
            if let v = parent[key] { env[key] = v }
        }
        for (k, v) in parent where k.hasPrefix("LC_") {
            env[k] = v
        }
        if env["PATH"] == nil {
            env["PATH"] = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        }
        // 防 git 弹 askpass GUI / 卡死要 credentials。`GIT_TERMINAL_PROMPT=0`
        // 让任何需要 stdin 输入的操作直接报错而不是 hang。
        env["GIT_TERMINAL_PROMPT"] = "0"
        process.environment = env

        let outPipe = Pipe()
        let errPipe = Pipe()
        process.standardOutput = outPipe
        process.standardError = errPipe

        do {
            try process.run()
        } catch {
            throw GitError.commandFailed(argv: argv, exitCode: -1, stderr: "\(error)")
        }
        process.waitUntilExit()
        let outData = outPipe.fileHandleForReading.readDataToEndOfFile()
        let errData = errPipe.fileHandleForReading.readDataToEndOfFile()
        guard let outStr = String(data: outData, encoding: .utf8) else {
            throw GitError.nonUTF8Output(argv: argv)
        }
        let errStr = String(data: errData, encoding: .utf8) ?? ""
        let code = process.terminationStatus
        if code != 0 && !acceptNonZero {
            throw GitError.commandFailed(argv: argv, exitCode: code, stderr: errStr)
        }
        return RunResult(exitCode: code, stdout: outStr, stderr: errStr)
    }
}
#endif
