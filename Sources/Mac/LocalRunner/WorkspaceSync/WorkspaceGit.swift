#if os(macOS)
import Foundation

/// workspace 同步专用的 git 原语层 —— 给后面的 SyncEngine 提供最小一组 git 操作,
/// 不做任何"帮用户决定何时联网/怎么合并"的策略判断。
///
/// **设计动机**：
/// - **不隐式 fetch**：`aheadBehind` 只读本地缓存的 remote-tracking ref
///   (`refs/remotes/<remote>/<branch>`),不会自己先跑一次 `fetch`。联网时机由
///   SyncEngine/调用方决定(可能是定时、可能是用户手动点"同步"),这层只负责
///   "问出当前状态"。想要新鲜数据,调用方自己先 `fetch(at:)` 再 `aheadBehind`。
///   如果 remote-tracking ref 压根不存在(从没 fetch/push 过),`rev-list` 会
///   非零退出 —— 这里让它原样冒泡成 `GitError.commandFailed`,不假装成
///   `(0, 0)`,因为"没有 upstream 数据"和"upstream 数据显示已同步"是两码事,
///   混在一起会让调用方误判成"已同步"。
/// - **push 永不带 `--force` 变体**：签名里没有 force 参数,任何一次 push 冲突
///   都应该原样失败,交给 SyncEngine 的冲突处理策略,而不是这一层悄悄覆盖对方
///   的提交。
/// - **`commitAll` 空变更 → 返回 `false` 不抛**：workspace 同步引擎大概率会
///   周期性地"看看有没有本地改动,有就提交",大多数时候没有变更是正常状态,
///   不该被当成错误处理。只有真正的 git 报错(不是"nothing to commit")才抛。
///
/// **不做**的事(职责边界)：
/// - 不管理 git 身份(`user.name`/`user.email`)—— 假设 caller 的 workspace 仓库
///   已经配好(全局或 repo-local),这层只是 plumbing,不该替用户悄悄写
///   `~/.gitconfig`。
/// - 不做冲突解决 / merge 策略选择 —— `pullFastForward` 失败就失败,
///   `pullRebase` 冲突就冲突,都原样抛给调用方。
///
/// **locateGit()/run() 为什么不共享 `GitWorktreeService` 的实现**：
/// 两处代码几乎一样(`/usr/bin/env git` 定位 + argv 数组不走 shell + env 白名单
/// PATH/HOME/SHELL/LANG/USER/LOGNAME/TMPDIR),但故意各自私有一份而不是抽公共
/// 工具类：
/// 1. 两者都是"源码直接编进 test bundle"的写法(测试文件直接引用
///    internal/private API),抽公共类型会把两个本该独立的 test target
///    耦合在一起。
/// 2. `GitWorktreeService` 服务 session 面板的"新建 worktree"场景,
///    `WorkspaceGit` 服务 workspace 同步场景——两者演化方向不同(比如未来
///    `WorkspaceGit` 可能要加认证 helper、`GitWorktreeService` 不需要),
///    共享实现会在其中一方需要定制时变成耦合负担。
public enum WorkspaceGit {

    /// 照抄 `GitWorktreeService.GitError` 的四个 case —— 两处独立演化,不合并
    /// 成同一个类型(理由同上,test bundle 解耦)。
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

    /// `git rev-parse HEAD` —— 当前 commit 的完整 SHA。
    public static func head(at url: URL) throws -> String {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let result = try run(git: git, cwd: url, argv: ["rev-parse", "HEAD"])
        return result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// 当前 checkout 的分支名(detached HEAD 时为 nil)。
    public static func currentBranch(at url: URL) throws -> String? {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
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

    /// 工作区(含已 add 未 commit)是否有变更:`status --porcelain` 非空即 dirty。
    public static func isDirty(at url: URL) throws -> Bool {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let result = try run(git: git, cwd: url, argv: ["status", "--porcelain"])
        return !result.stdout.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// 本地 `branch` 相对 `remote/branch` 的领先/落后提交数。
    ///
    /// **不隐式 fetch** —— 读的是本地缓存的 `refs/remotes/<remote>/<branch>`,
    /// 是否新鲜由调用方决定(先手动 `fetch(at:)` 再问)。如果这个
    /// remote-tracking ref 压根不存在(没 fetch/push 过),`rev-list` 非零退出,
    /// 原样冒泡成 `.commandFailed`,不假装 `(0, 0)`。
    public static func aheadBehind(
        at url: URL, branch: String, remote: String = "origin"
    ) throws -> (ahead: Int, behind: Int) {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let result = try run(
            git: git,
            cwd: url,
            argv: ["rev-list", "--left-right", "--count", "\(branch)...\(remote)/\(branch)"]
        )
        let parts = result.stdout
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .split(whereSeparator: { $0.isWhitespace })
        guard parts.count == 2, let ahead = Int(parts[0]), let behind = Int(parts[1]) else {
            // 这不是 git 退出非零,而是"命令成功但输出不是预期的两列数字"——
            // 理论上不该发生(rev-list --count 输出格式稳定)。仍复用
            // commandFailed 表达,exitCode 用 -1 sentinel(区别于真实退出码),
            // stderr 里写明是解析失败并附原文,不伪装成 git 自己报的错。
            throw GitError.commandFailed(
                argv: ["rev-list", "--left-right", "--count", "\(branch)...\(remote)/\(branch)"],
                exitCode: -1,
                stderr: "输出解析失败(预期两列数字): \(result.stdout)"
            )
        }
        return (ahead, behind)
    }

    /// `git fetch`(默认 remote,通常是 `origin`)。只更新 remote-tracking ref,
    /// 不碰工作区/本地分支。
    public static func fetch(at url: URL) throws {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        _ = try run(git: git, cwd: url, argv: ["fetch"])
    }

    /// `git push <remote> <branch>`。**永不带 `--force` 任何变体** —— 冲突就让
    /// 它原样失败,交给调用方的冲突处理策略。
    public static func push(at url: URL, remote: String = "origin", branch: String) throws {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        _ = try run(git: git, cwd: url, argv: ["push", remote, branch])
    }

    /// 查远端(任意 URL,不要求 `cwd` 是这个 remote 的本地 clone)某分支当前指向
    /// 的 SHA,不需要先 clone/fetch。分支不存在(空输出)→ nil。
    public static func lsRemoteHead(remoteURL: String, branch: String, cwd: URL) throws -> String? {
        let git = try locatedGit()
        let result = try run(
            git: git, cwd: cwd, argv: ["ls-remote", remoteURL, "refs/heads/\(branch)"]
        )
        let trimmed = result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        let firstLine = trimmed.split(separator: "\n").first.map(String.init) ?? trimmed
        let firstColumn = firstLine.split(whereSeparator: { $0.isWhitespace }).first.map(String.init)
        return firstColumn
    }

    /// `git clone <remoteURL> <to>`。目标目录的父目录不存在就先建(clone 本身
    /// 会创建 `to` 这一层,但跑 clone 的 cwd 得先存在)。
    public static func clone(remoteURL: String, to url: URL) throws {
        let git = try locatedGit()
        let parent = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
        _ = try run(git: git, cwd: parent, argv: ["clone", remoteURL, url.path])
    }

    /// `git pull --ff-only origin <当前分支>`。只在能 fast-forward 时才动,
    /// 否则原样失败(不合并/不 rebase)。
    ///
    /// 显式带 `origin <branch>` 而不是裸 `git pull --ff-only`:workspace 仓库
    /// 走的是 `initRepo` + `setRemote` 手工搭的路径,不一定像 `clone` 那样自动
    /// 设好 `branch.<name>.remote`/`.merge` tracking —— 显式指定就不依赖这份
    /// tracking 配置是否存在。
    public static func pullFastForward(at url: URL) throws {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let branch = try branchForPull(git: git, at: url)
        _ = try run(git: git, cwd: url, argv: ["pull", "--ff-only", "origin", branch])
    }

    /// `git switch <branch>`——**Task 7 最小新增**:`SyncEngine.executeDown` 的 clone
    /// 分支用它把 clone 下来的工作区切到 `last_sync.branch`。clone 默认签出的分支
    /// 未必就是这个项目声明的目标分支(比如项目默认分支是 `main`,但
    /// `last_sync.branch` 记的是用户当时在切的其他分支),而 clone 只会自动建好
    /// 默认分支的本地 tracking branch,其余分支只有 remote-tracking ref。
    /// `git switch <branch>` 在本地分支不存在、但唯一一个 remote 上有同名分支时
    /// 会自动创建并 track(git ≥ 2.23 的行为,workspace 场景恒定单 remote
    /// `origin`,不存在歧义)——不需要我们自己判断"建 tracking branch 还是切现有
    /// 分支"这两种情况,交给 git 原生行为。已经在目标分支上时 `git switch` 是
    /// no-op(幂等),呼应 brief"clone 默认分支可能就是目标分支,switch 幂等"。
    public static func switchBranch(at url: URL, branch: String) throws {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        _ = try run(git: git, cwd: url, argv: ["switch", branch])
    }

    /// `git merge-base --is-ancestor <ancestor> <descendant>`——只读判定:
    /// `ancestor` 是不是 `descendant` 的祖先提交(含相等)。**Task 7 最小新增**:
    /// `SyncEngine.planDown`/`executeDown` 用它判断"本机 head 有没有落后 manifest
    /// 记录的 head,还是本机自己有独立进度(不是简单的祖先关系,而是分叉/领先)"
    /// ——不自己重新实现一遍 merge-base 的判定逻辑,直接问 git。
    ///
    /// 退出码语义(`git merge-base --is-ancestor` 的标准约定):0 = 是祖先,
    /// 1 = 不是祖先,其余(2+)是真错误(比如给的提交 SHA 压根不存在)——这里把
    /// 退出码 0/1 都当合法结果读出布尔值,只有 2+ 才当 `GitError` 抛出。
    public static func isAncestor(at url: URL, ancestor: String, descendant: String) throws -> Bool {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let argv = ["merge-base", "--is-ancestor", ancestor, descendant]
        let result = try run(git: git, cwd: url, argv: argv, acceptNonZero: true)
        switch result.exitCode {
        case 0:
            return true
        case 1:
            return false
        default:
            throw GitError.commandFailed(argv: argv, exitCode: result.exitCode, stderr: result.stderr)
        }
    }

    /// `git pull --rebase origin <当前分支>`。仅给"workspace 仓库自身"(纯配置/
    /// manifest,非用户代码 checkout)的同步用 —— 那类仓库没有需要保护的长期
    /// 分叉历史,rebase 冲突概率低且线性历史更方便审计。显式带 remote/branch
    /// 的理由同 `pullFastForward`。
    public static func pullRebase(at url: URL) throws {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let branch = try branchForPull(git: git, at: url)
        _ = try run(git: git, cwd: url, argv: ["pull", "--rebase", "origin", branch])
    }

    /// `pullFastForward`/`pullRebase` 共用:拿当前分支名,detached HEAD 时报清楚
    /// 的错而不是让 git 原始报错("You are not currently on a branch")冒泡。
    private static func branchForPull(git: URL, at url: URL) throws -> String {
        let argv = ["symbolic-ref", "--quiet", "--short", "HEAD"]
        let result = try run(git: git, cwd: url, argv: argv, acceptNonZero: true)
        let trimmed = result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        guard result.exitCode == 0, !trimmed.isEmpty else {
            // argv/exitCode 如实反映真正跑的命令(symbolic-ref),不伪造成 pull;
            // 上下文(这是在为 pull 取分支名)写进 stderr 说明。
            throw GitError.commandFailed(
                argv: argv,
                exitCode: result.exitCode,
                stderr: "detached HEAD,无法确定 pull 的目标分支:\(url.path)"
            )
        }
        return trimmed
    }

    /// `git add -A` + `git commit -m <message>`。**没有变更时返回 `false`**,
    /// 不当错误抛 —— 同步引擎大概率会周期性调用这个,大多数时候没变更是正常
    /// 状态。
    ///
    /// 空变更判定走 **退出码** 而非文本匹配:`add -A` 之后先跑
    /// `git diff --cached --quiet`(0 = index 与 HEAD 无差异;1 = 有 staged
    /// 变更;>1 = 真错误)。不能用 `commit` 输出里的 "nothing to commit" 字符串
    /// —— `run()` 透传 LANG/LC_*,gettext 版 git 在非英文 locale 下会把这行
    /// 文案翻译掉,文本匹配会误把正常 no-op 当失败抛。退出码是 locale 无关的。
    @discardableResult
    public static func commitAll(at url: URL, message: String) throws -> Bool {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        _ = try run(git: git, cwd: url, argv: ["add", "-A"])
        let diffArgv = ["diff", "--cached", "--quiet"]
        let diff = try run(git: git, cwd: url, argv: diffArgv, acceptNonZero: true)
        switch diff.exitCode {
        case 0:
            // index 与 HEAD 一致 → 无可提交内容,直接短路,不跑 commit。
            return false
        case 1:
            break // 有 staged 变更,继续 commit。
        default:
            throw GitError.commandFailed(
                argv: diffArgv, exitCode: diff.exitCode, stderr: diff.stderr
            )
        }
        _ = try run(git: git, cwd: url, argv: ["commit", "-m", message])
        return true
    }

    /// `git init -b <initialBranch>`。目录不存在就先建(`git init` 本身会建,
    /// 但 `Process.currentDirectoryURL` 要求 cwd 先存在,所以我们自己 mkdir)。
    public static func initRepo(at url: URL, initialBranch: String = "main") throws {
        let git = try locatedGit()
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        _ = try run(git: git, cwd: url, argv: ["init", "-b", initialBranch])
    }

    /// 只读查询:`remote`(默认 `origin`)当前配置的 URL,没配就 `nil`。
    ///
    /// **Task 5 最小新增** —— `WorkspaceRepoService.syncUp` 需要判断"这个
    /// workspace 仓库有没有配 remote"来决定是 `.skipped("未配置 remote")` 还是
    /// 继续 push,而 `WorkspaceGit` 之前只有 `setRemote`(只写不读)。`git remote
    /// get-url` 在没配对应 remote 时退出非零 —— 这里吞成 `nil` 而不是抛,"没配
    /// remote"是正常状态,不是 git 层面的错误(呼应 `commitAll` 空变更返回
    /// `false` 而不抛的同一条纪律)。
    public static func remoteURL(at url: URL, remote: String = "origin") throws -> String? {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let result = try run(
            git: git, cwd: url, argv: ["remote", "get-url", remote], acceptNonZero: true
        )
        guard result.exitCode == 0 else { return nil }
        let trimmed = result.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// 配 `origin` remote:已存在就 `set-url`,不存在就 `add`。固定用
    /// `origin` 这个名字 —— workspace 仓库单 remote 场景,不需要多 remote 名。
    public static func setRemote(at url: URL, url remoteURL: String) throws {
        let git = try locatedGit()
        try requireRepo(git: git, at: url)
        let setURLResult = try run(
            git: git, cwd: url, argv: ["remote", "set-url", "origin", remoteURL], acceptNonZero: true
        )
        guard setURLResult.exitCode != 0 else { return }
        // set-url 失败通常因为 origin 压根不存在,退而 add。
        _ = try run(git: git, cwd: url, argv: ["remote", "add", "origin", remoteURL])
    }

    // MARK: - Internals

    /// `url` 是否是个 git work tree;不是就抛 `.notAGitRepo`。所有假设"这里已经
    /// 是个仓库"的操作(head/currentBranch/isDirty/aheadBehind/fetch/push/
    /// pull*/commitAll/setRemote)先过这关,报错信息比让 git 自己的 stderr
    /// 裸冒泡更明确。
    private static func requireRepo(git: URL, at url: URL) throws {
        let result = try run(
            git: git, cwd: url, argv: ["rev-parse", "--is-inside-work-tree"], acceptNonZero: true
        )
        guard result.exitCode == 0,
              result.stdout.trimmingCharacters(in: .whitespacesAndNewlines) == "true"
        else {
            throw GitError.notAGitRepo(url)
        }
    }

    private static func locatedGit() throws -> URL {
        guard let git = locateGit() else { throw GitError.gitNotInstalled }
        return git
    }

    /// 找 `git` 可执行路径。**照抄 `GitWorktreeService.locateGit()`** —— 见文件
    /// 头部"为什么不共享"注释。
    private static func locateGit() -> URL? {
        for candidate in [
            "/usr/bin/git",
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
        ] {
            if FileManager.default.isExecutableFile(atPath: candidate) {
                return URL(fileURLWithPath: candidate)
            }
        }
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

    /// 跑 `git <argv>` 同步,捕 stdout/stderr。**照抄
    /// `GitWorktreeService.run()`** —— 见文件头部"为什么不共享"注释:argv 数组
    /// 不走 shell、env 白名单只放 PATH/HOME/SHELL/LANG/USER/LOGNAME/TMPDIR(git
    /// 需要 HOME 读 `~/.gitconfig`/`~/.ssh/known_hosts`)、`GIT_TERMINAL_PROMPT=0`
    /// 防挂起等 credentials 输入。
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
