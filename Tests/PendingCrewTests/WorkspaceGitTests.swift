#if os(macOS)
import XCTest
// 同 GitWorktreeServiceTests:源码直接编进 test bundle,internal API 可见。

/// `WorkspaceGit` 的真 git fixture 测试。
///
/// fixture 拓扑:tmp 下一个 `git init --bare` 裸仓库当 remote(`file://` 路径),
/// 两个工作仓库(repoA 用 `WorkspaceGit.initRepo` 建 + clone 出的 repoB)分别
/// 模拟"本机"和"另一台机器",覆盖 push/fetch/aheadBehind 的双机同步语义。
final class WorkspaceGitTests: XCTestCase {

    // MARK: - 无 repo / 无 git 场景

    func testHeadThrowsOnNonRepo() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("workspacegit-nonrepo-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        do {
            _ = try WorkspaceGit.head(at: tmp)
            XCTFail("expected notAGitRepo")
        } catch WorkspaceGit.GitError.notAGitRepo {
            // expected
        } catch WorkspaceGit.GitError.gitNotInstalled {
            throw XCTSkip("git not on PATH — skipping")
        }
    }

    // MARK: - end-to-end 双机同步 flow

    func testTwoMachineSyncFlow() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("workspacegit-e2e-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        // 1. 裸仓库当 remote
        let bareDir = root.appendingPathComponent("remote.git", isDirectory: true)
        try FileManager.default.createDirectory(at: bareDir, withIntermediateDirectories: true)
        try runGitRaw(at: bareDir, args: ["init", "--bare", "-b", "main"])
        let remoteURL = "file://\(bareDir.path)"

        // 2. repoA = "本机" 工作仓库
        let repoA = root.appendingPathComponent("repoA", isDirectory: true)
        try WorkspaceGit.initRepo(at: repoA, initialBranch: "main")
        try configureIdentity(at: repoA)
        try WorkspaceGit.setRemote(at: repoA, url: remoteURL)

        // head/isDirty/commitAll roundtrip
        try "hello\n".write(
            to: repoA.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.isDirty(at: repoA), "未 commit 的新文件应算 dirty")
        let committed = try WorkspaceGit.commitAll(at: repoA, message: "init")
        XCTAssertTrue(committed, "首次 commit 应该成功(有变更)")
        XCTAssertFalse(try WorkspaceGit.isDirty(at: repoA), "commit 后应该 clean")

        // commitAll 空变更 → false 不抛
        let committedAgain = try WorkspaceGit.commitAll(at: repoA, message: "no-op")
        XCTAssertFalse(committedAgain, "无变更时 commitAll 应返回 false 而不是抛")

        let headA1 = try WorkspaceGit.head(at: repoA)
        XCTAssertEqual(try WorkspaceGit.currentBranch(at: repoA), "main")

        // push 后 lsRemoteHead == head
        try WorkspaceGit.push(at: repoA, branch: "main")
        let remoteHead1 = try WorkspaceGit.lsRemoteHead(remoteURL: remoteURL, branch: "main", cwd: repoA)
        XCTAssertEqual(remoteHead1, headA1)

        // 不存在的分支 → nil
        let missingHead = try WorkspaceGit.lsRemoteHead(remoteURL: remoteURL, branch: "no-such-branch", cwd: repoA)
        XCTAssertNil(missingHead)

        // 3. repoB = clone 出来的"另一台机器"
        let repoB = root.appendingPathComponent("repoB", isDirectory: true)
        try WorkspaceGit.clone(remoteURL: remoteURL, to: repoB)
        try configureIdentity(at: repoB)
        XCTAssertEqual(try WorkspaceGit.head(at: repoB), headA1)

        // repoB 提交新内容并 push
        try "world\n".write(
            to: repoB.appendingPathComponent("SECOND.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: repoB, message: "second"))
        try WorkspaceGit.push(at: repoB, branch: "main")
        let headB = try WorkspaceGit.head(at: repoB)
        XCTAssertNotEqual(headB, headA1)

        // 4. repoA 在未 fetch 前,aheadBehind 用的是本地缓存的 remote-tracking ref,
        //    不会自动感知 repoB 刚推的新提交 —— 印证"不隐式 fetch"。
        let staleAheadBehind = try WorkspaceGit.aheadBehind(at: repoA, branch: "main")
        XCTAssertEqual(staleAheadBehind.ahead, 0)
        XCTAssertEqual(staleAheadBehind.behind, 0)

        // 显式 fetch 后才能看到真实落后状态
        try WorkspaceGit.fetch(at: repoA)
        let freshAheadBehind = try WorkspaceGit.aheadBehind(at: repoA, branch: "main")
        XCTAssertEqual(freshAheadBehind.ahead, 0)
        XCTAssertEqual(freshAheadBehind.behind, 1)

        // 5. pullFastForward 收敛
        try WorkspaceGit.pullFastForward(at: repoA)
        XCTAssertEqual(try WorkspaceGit.head(at: repoA), headB)
        let convergedAheadBehind = try WorkspaceGit.aheadBehind(at: repoA, branch: "main")
        XCTAssertEqual(convergedAheadBehind.ahead, 0)
        XCTAssertEqual(convergedAheadBehind.behind, 0)
    }

    /// detached HEAD → currentBranch 应为 nil。
    func testCurrentBranchNilWhenDetached() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let repo = FileManager.default.temporaryDirectory
            .appendingPathComponent("workspacegit-detached-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: repo) }
        try WorkspaceGit.initRepo(at: repo, initialBranch: "main")
        try configureIdentity(at: repo)
        try "hello\n".write(to: repo.appendingPathComponent("a.txt"), atomically: true, encoding: .utf8)
        _ = try WorkspaceGit.commitAll(at: repo, message: "init")
        let sha = try WorkspaceGit.head(at: repo)
        try runGitRaw(at: repo, args: ["checkout", "--detach", sha])
        XCTAssertNil(try WorkspaceGit.currentBranch(at: repo))
    }

    /// `pullRebase` 也应该能把远端新提交拉平(workspace 仓库自身同步用)。
    func testPullRebaseConverges() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("workspacegit-rebase-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let bareDir = root.appendingPathComponent("remote.git", isDirectory: true)
        try FileManager.default.createDirectory(at: bareDir, withIntermediateDirectories: true)
        try runGitRaw(at: bareDir, args: ["init", "--bare", "-b", "main"])
        let remoteURL = "file://\(bareDir.path)"

        let repoA = root.appendingPathComponent("repoA", isDirectory: true)
        try WorkspaceGit.initRepo(at: repoA, initialBranch: "main")
        try configureIdentity(at: repoA)
        try WorkspaceGit.setRemote(at: repoA, url: remoteURL)
        try "hello\n".write(to: repoA.appendingPathComponent("a.txt"), atomically: true, encoding: .utf8)
        _ = try WorkspaceGit.commitAll(at: repoA, message: "init")
        try WorkspaceGit.push(at: repoA, branch: "main")

        let repoB = root.appendingPathComponent("repoB", isDirectory: true)
        try WorkspaceGit.clone(remoteURL: remoteURL, to: repoB)
        try configureIdentity(at: repoB)
        try "world\n".write(to: repoB.appendingPathComponent("b.txt"), atomically: true, encoding: .utf8)
        _ = try WorkspaceGit.commitAll(at: repoB, message: "second")
        try WorkspaceGit.push(at: repoB, branch: "main")

        try WorkspaceGit.fetch(at: repoA)
        try WorkspaceGit.pullRebase(at: repoA)
        XCTAssertFalse(try WorkspaceGit.isDirty(at: repoA))
        let ab = try WorkspaceGit.aheadBehind(at: repoA, branch: "main")
        XCTAssertEqual(ab.ahead, 0)
        XCTAssertEqual(ab.behind, 0)
    }

    // MARK: - helpers

    /// fixture 专用:给刚 initRepo/clone 出来的仓库配本地 `user.name`/`user.email`。
    /// `WorkspaceGit` 本身不管理 git 身份(不是它的职责范围),这纯粹是测试基建。
    private func configureIdentity(at repo: URL) throws {
        try runGitRaw(at: repo, args: ["config", "user.email", "test@example.com"])
        try runGitRaw(at: repo, args: ["config", "user.name", "Test"])
    }

    private func runGitRaw(at cwd: URL, args: [String]) throws {
        let process = Process()
        process.executableURL = locateGitForTest()!
        process.arguments = args
        process.currentDirectoryURL = cwd
        process.standardOutput = Pipe()
        process.standardError = Pipe()
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
