#if os(macOS)
import XCTest
// 同 WorkspaceGitTests:源码直接编进 test bundle,internal API 可见。

/// `ProjectSyncService` 的真 git fixture 测试——覆盖 brief 行为清单:
/// 干净且 ahead=0 → upToDate;有新 commit → uploaded 且 LastSync.head == 本地
/// head;dirty+wipCommit=false → skipped;dirty+wipCommit=true → 自动 commit 后
/// uploaded;remote 不可达 → failed(不上抛)。
final class ProjectSyncServiceTests: XCTestCase {

    // MARK: - scan

    func testScanMissingDirectory() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("projectsync-missing-\(UUID().uuidString)", isDirectory: true)
        // 故意不创建这个目录。
        let status = try ProjectSyncService.scan(projectId: "proj-1", localPath: tmp, fetchFirst: false)
        XCTAssertTrue(status.missing)
        XCTAssertNil(status.branch)
        XCTAssertNil(status.head)
        XCTAssertFalse(status.dirty)
        XCTAssertEqual(status.ahead, 0)
        XCTAssertEqual(status.behind, 0)
    }

    func testScanNonRepoThrows() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("projectsync-nonrepo-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        do {
            _ = try ProjectSyncService.scan(projectId: "proj-1", localPath: tmp, fetchFirst: false)
            XCTFail("expected notAGitRepo to bubble up, not be swallowed into a clean status")
        } catch WorkspaceGit.GitError.notAGitRepo {
            // expected —— 目录存在但不是 git repo,这是配置级错误,必须原样上抛。
        } catch WorkspaceGit.GitError.gitNotInstalled {
            throw XCTSkip("git not on PATH — skipping")
        }
    }

    func testScanCleanRepoMatchesGitState() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let status = try ProjectSyncService.scan(
            projectId: "proj-1", localPath: fixture.repoA, fetchFirst: false
        )
        XCTAssertFalse(status.missing)
        XCTAssertEqual(status.branch, "main")
        XCTAssertEqual(status.head, fixture.initialHead)
        XCTAssertFalse(status.dirty)
        XCTAssertEqual(status.ahead, 0)
        XCTAssertEqual(status.behind, 0)
    }

    // MARK: - pushCurrentBranch

    /// 干净且 ahead=0 → upToDate,不碰网络,LastSync 为 nil。
    func testPushUpToDateWhenCleanAndAheadZero() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
            projectId: "proj-1",
            localPath: fixture.repoA,
            remoteURL: fixture.remoteURL,
            wipCommit: false,
            machineId: "machine-a"
        )
        XCTAssertEqual(receipt.outcome, .upToDate)
        XCTAssertEqual(receipt.kind, .project)
        XCTAssertEqual(receipt.itemId, "proj-1")
        XCTAssertNil(lastSync)
    }

    /// 有新 commit(ahead>0,clean)→ push 成功,receipt=.uploaded,
    /// LastSync.head == 本地 head。
    func testPushUploadsNewCommit() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        try "second\n".write(
            to: fixture.repoA.appendingPathComponent("SECOND.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: fixture.repoA, message: "second commit"))
        let localHead = try WorkspaceGit.head(at: fixture.repoA)

        // 固定 now,顺带验证 now() 注入贯通 receipt.at 与 LastSync.pushedAt。
        let fixedNow = Date(timeIntervalSince1970: 1_752_900_000)
        let expectedAt = ISO8601DateFormatter().string(from: fixedNow)

        let (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
            projectId: "proj-1",
            localPath: fixture.repoA,
            remoteURL: fixture.remoteURL,
            wipCommit: false,
            machineId: "machine-a",
            now: { fixedNow }
        )
        XCTAssertEqual(receipt.outcome, .uploaded(remoteHead: localHead))
        XCTAssertEqual(receipt.at, expectedAt)
        guard let lastSync else {
            return XCTFail("expected LastSync on successful upload")
        }
        XCTAssertEqual(lastSync.head, localHead)
        XCTAssertEqual(lastSync.branch, "main")
        XCTAssertEqual(lastSync.machine, "machine-a")
        XCTAssertEqual(lastSync.pushedAt, expectedAt)

        // 独立复核:远端确实收到了这个 head。
        let remoteHead = try WorkspaceGit.lsRemoteHead(
            remoteURL: fixture.remoteURL, branch: "main", cwd: fixture.repoA)
        XCTAssertEqual(remoteHead, localHead)
    }

    /// dirty + wipCommit=false → skipped,不 commit、不 push、LastSync 为 nil。
    func testPushSkippedWhenDirtyAndNoWipCommit() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        try "uncommitted\n".write(
            to: fixture.repoA.appendingPathComponent("DIRTY.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.isDirty(at: fixture.repoA))

        let (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
            projectId: "proj-1",
            localPath: fixture.repoA,
            remoteURL: fixture.remoteURL,
            wipCommit: false,
            machineId: "machine-a"
        )
        XCTAssertEqual(receipt.outcome, .skipped(reason: "有未提交变更"))
        XCTAssertNil(lastSync)
        // 仍然 dirty —— 没有偷偷 commit。
        XCTAssertTrue(try WorkspaceGit.isDirty(at: fixture.repoA))
    }

    /// dirty + wipCommit=true → 自动 commit 后 uploaded。
    func testPushAutoCommitsWhenDirtyAndWipCommitTrue() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        try "wip content\n".write(
            to: fixture.repoA.appendingPathComponent("WIP.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.isDirty(at: fixture.repoA))

        let (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
            projectId: "proj-1",
            localPath: fixture.repoA,
            remoteURL: fixture.remoteURL,
            wipCommit: true,
            machineId: "machine-a"
        )
        XCTAssertFalse(try WorkspaceGit.isDirty(at: fixture.repoA), "应已自动 commit 收敛成 clean")
        let localHead = try WorkspaceGit.head(at: fixture.repoA)
        XCTAssertEqual(receipt.outcome, .uploaded(remoteHead: localHead))
        XCTAssertEqual(lastSync?.head, localHead)
    }

    /// remote 不可达(origin 指向不存在的路径)→ push 失败被包成 .failed,
    /// **不上抛**。
    func testPushFailedWhenRemoteUnreachable() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        // 造一个领先提交,确保 ahead>0 会真的走到 push 这一步。
        try "second\n".write(
            to: fixture.repoA.appendingPathComponent("SECOND.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: fixture.repoA, message: "second commit"))

        // 把 origin 改指向一个压根不存在的路径,模拟"远端不可达"。
        let badRemote = fixture.root.appendingPathComponent("no-such-remote.git", isDirectory: true)
        let badRemoteURL = "file://\(badRemote.path)"
        try WorkspaceGit.setRemote(at: fixture.repoA, url: badRemoteURL)

        let (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
            projectId: "proj-1",
            localPath: fixture.repoA,
            remoteURL: badRemoteURL,
            wipCommit: false,
            machineId: "machine-a"
        )
        guard case .failed = receipt.outcome else {
            return XCTFail("expected .failed outcome, got \(receipt.outcome)")
        }
        XCTAssertNil(lastSync)
    }

    /// detached HEAD + dirty + wipCommit=true → skipped 且**不做任何 mutate**
    /// (不能先 commitAll 再发现无分支可推——那会造出孤儿 wip commit)。
    func testPushSkippedOnDetachedHeadWithoutMutation() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        try runGitRaw(at: fixture.repoA, args: ["checkout", "--detach", fixture.initialHead])
        try "detached wip\n".write(
            to: fixture.repoA.appendingPathComponent("DETACHED.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.isDirty(at: fixture.repoA))

        let (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
            projectId: "proj-1",
            localPath: fixture.repoA,
            remoteURL: fixture.remoteURL,
            wipCommit: true, // 即使允许自动 commit,detached 下也不该动仓库
            machineId: "machine-a"
        )
        XCTAssertEqual(receipt.outcome, .skipped(reason: "detached HEAD,请先切回分支"))
        XCTAssertNil(lastSync)
        // 仓库未被 mutate:head 不变、仍 dirty(没有偷偷 commit)。
        XCTAssertEqual(try WorkspaceGit.head(at: fixture.repoA), fixture.initialHead)
        XCTAssertTrue(try WorkspaceGit.isDirty(at: fixture.repoA))
    }

    /// 复核纪律的核心承诺:push 到 origin 成功,但 `remoteURL` 复核对象是另一个
    /// bare 仓库(拿不到同一个 head)→ 绝不虚报 `.uploaded`,如实 `.failed`。
    func testPushFailedWhenVerificationRemoteMismatches() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        try "second\n".write(
            to: fixture.repoA.appendingPathComponent("SECOND.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: fixture.repoA, message: "second commit"))
        let localHead = try WorkspaceGit.head(at: fixture.repoA)

        // 另一个空 bare 仓库当"复核对象"——push 走 origin(fixture 的 remote),
        // 复核却问这个空仓库,main 分支不存在 → lsRemoteHead 为 nil → 不一致。
        let otherBare = fixture.root.appendingPathComponent("other-remote.git", isDirectory: true)
        try FileManager.default.createDirectory(at: otherBare, withIntermediateDirectories: true)
        try runGitRaw(at: otherBare, args: ["init", "--bare", "-b", "main"])
        let otherRemoteURL = "file://\(otherBare.path)"

        let (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
            projectId: "proj-1",
            localPath: fixture.repoA,
            remoteURL: otherRemoteURL,
            wipCommit: false,
            machineId: "machine-a"
        )
        XCTAssertEqual(receipt.outcome, .failed(reason: "push 后远端 ref 与本地不一致"))
        XCTAssertNil(lastSync)
        // push 本身确实到了 origin(说明 failed 是复核环节拦下的,不是 push 挂了)。
        let originHead = try WorkspaceGit.lsRemoteHead(
            remoteURL: fixture.remoteURL, branch: "main", cwd: fixture.repoA)
        XCTAssertEqual(originHead, localHead)
    }

    // MARK: - fixture helpers(套路同 WorkspaceGitTests)

    private struct Fixture {
        let root: URL
        let repoA: URL
        let remoteURL: String
        let initialHead: String
    }

    /// 建裸仓库当 remote + repoA 工作仓库,首次 commit+push 建立好 tracking
    /// (`refs/remotes/origin/main`),让后续 `aheadBehind` 有本地缓存可读,不用
    /// 每个测试都显式 fetch。
    private func makeFixture() throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("projectsync-e2e-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

        let bareDir = root.appendingPathComponent("remote.git", isDirectory: true)
        try FileManager.default.createDirectory(at: bareDir, withIntermediateDirectories: true)
        try runGitRaw(at: bareDir, args: ["init", "--bare", "-b", "main"])
        let remoteURL = "file://\(bareDir.path)"

        let repoA = root.appendingPathComponent("repoA", isDirectory: true)
        try WorkspaceGit.initRepo(at: repoA, initialBranch: "main")
        try configureIdentity(at: repoA)
        try WorkspaceGit.setRemote(at: repoA, url: remoteURL)

        try "hello\n".write(
            to: repoA.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: repoA, message: "init"))
        try WorkspaceGit.push(at: repoA, branch: "main")
        let initialHead = try WorkspaceGit.head(at: repoA)

        return Fixture(root: root, repoA: repoA, remoteURL: remoteURL, initialHead: initialHead)
    }

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
