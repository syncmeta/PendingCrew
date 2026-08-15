#if os(macOS)
import XCTest
// 同 LocalCodingAgentTests:源码直接编进 test bundle,internal API 可见。

final class GitWorktreeServiceTests: XCTestCase {

    // MARK: - non-repo behaviour

    func testIsGitRepositoryReturnsFalseForTmpDir() throws {
        // /tmp 一般不是 git work tree。如果 CI 跑在某种奇怪 sandbox 下,
        // 这条假设不成立 —— 那应该没人跑这套了。
        let tmp = URL(fileURLWithPath: NSTemporaryDirectory())
        // 仅当机器装了 git 才能跑该断言;没装会 throw .gitNotInstalled,跳过。
        do {
            let isRepo = try GitWorktreeService.isGitRepository(at: tmp)
            XCTAssertFalse(isRepo, "/tmp should not be a git work tree")
        } catch GitWorktreeService.GitError.gitNotInstalled {
            throw XCTSkip("git not on PATH — skipping")
        }
    }

    func testListBranchesThrowsOnNonRepo() throws {
        let tmp = URL(fileURLWithPath: NSTemporaryDirectory())
        do {
            _ = try GitWorktreeService.listLocalBranches(at: tmp)
            XCTFail("expected notAGitRepo")
        } catch GitWorktreeService.GitError.notAGitRepo {
            // expected
        } catch GitWorktreeService.GitError.gitNotInstalled {
            throw XCTSkip("git not on PATH — skipping")
        }
    }

    // MARK: - end-to-end repo flow

    /// 起一个临时 git repo,塞一个 commit,跑 isGitRepository / listBranches /
    /// addWorktree。验证 worktree 真的建出来 + 在新分支上。
    func testAddWorktreeCreatesNewWorktreeOnNewBranch() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let repo = try makeTempRepo()
        defer { try? FileManager.default.removeItem(at: repo) }

        XCTAssertTrue(try GitWorktreeService.isGitRepository(at: repo))
        let branches = try GitWorktreeService.listLocalBranches(at: repo)
        XCTAssertFalse(branches.isEmpty, "fresh repo should have at least one branch")
        let baseBranch = branches.first!

        // 选随机不冲突的新分支名。
        let newBranch = "test-branch-\(UUID().uuidString.prefix(8))"
        let worktreeURL = try GitWorktreeService.addWorktree(
            sourceRepository: repo,
            branchName: baseBranch,
            newBranchName: newBranch,
            worktreeNameHint: "smoke test"  // 空格会被 sanitize 成 dash
        )
        // 1. 目录存在 + 是 dir
        var isDir: ObjCBool = false
        XCTAssertTrue(FileManager.default.fileExists(atPath: worktreeURL.path, isDirectory: &isDir))
        XCTAssertTrue(isDir.boolValue)
        // 2. 路径在 repo/.pendingcrew/worktrees/<slug>-<hash>
        // 用 resolvingSymlinksInPath:macOS tmpdir 是 /var/folders → /private/var/folders
        // 的 symlink,git 经常 resolve 出 /private/ 前缀,而 repo URL 没解;两边都解就一致。
        let resolvedContainer = repo
            .appendingPathComponent(".pendingcrew/worktrees")
            .resolvingSymlinksInPath()
            .path
        let resolvedWorktree = worktreeURL.resolvingSymlinksInPath().path
        XCTAssertTrue(resolvedWorktree.hasPrefix(resolvedContainer),
                      "worktree should live under .pendingcrew/worktrees (got: \(resolvedWorktree), container: \(resolvedContainer))")
        // 3. slug 应该是 "smoke-test" (空格变 dash)
        XCTAssertTrue(worktreeURL.lastPathComponent.hasPrefix("smoke-test-"),
                      "got worktree dir: \(worktreeURL.lastPathComponent)")
        // 4. checkout 的是新分支
        let currentBranch = try GitWorktreeService.currentBranch(at: worktreeURL)
        XCTAssertEqual(currentBranch, newBranch)
    }

    // MARK: - helpers

    private func makeTempRepo() throws -> URL {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("pendingcrew-git-test-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)

        // git init + 一个文件 + commit。`init -b main` 强制默认分支名,跨机一致。
        try runGit(at: tmp, args: ["init", "-b", "main"])
        // 配 user/email — 否则 commit 会拒(全局没配的话)。
        try runGit(at: tmp, args: ["config", "user.email", "test@example.com"])
        try runGit(at: tmp, args: ["config", "user.name", "Test"])
        // 写文件 + add + commit
        let f = tmp.appendingPathComponent("README.md")
        try "hello\n".write(to: f, atomically: true, encoding: .utf8)
        try runGit(at: tmp, args: ["add", "README.md"])
        try runGit(at: tmp, args: ["commit", "-m", "init"])
        return tmp
    }

    private func runGit(at cwd: URL, args: [String]) throws {
        let process = Process()
        process.executableURL = locateGitForTest()!
        process.arguments = args
        process.currentDirectoryURL = cwd
        process.standardOutput = Pipe()
        process.standardError = Pipe()
        // env: PATH + HOME(git 要 HOME 才能 mkconfig)
        process.environment = [
            "PATH": ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin",
            "HOME": ProcessInfo.processInfo.environment["HOME"] ?? NSTemporaryDirectory(),
            "GIT_TERMINAL_PROMPT": "0",
        ]
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0, "git \(args.joined(separator: " ")) failed")
    }

    private func locateGitForTest() -> URL? {
        for path in ["/usr/bin/git", "/opt/homebrew/bin/git", "/usr/local/bin/git"] {
            if FileManager.default.isExecutableFile(atPath: path) {
                return URL(fileURLWithPath: path)
            }
        }
        return nil
    }
}
#endif
