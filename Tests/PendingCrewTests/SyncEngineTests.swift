#if os(macOS)
import XCTest
// 同 WorkspaceRepoServiceTests/ProjectSyncServiceTests:源码直接编进 test bundle,internal API 可见。

/// `SyncEngine` 的真 git fixture 测试——覆盖 brief 行为清单:
/// 1 个 workspace + 2 个项目仓库(一个有新 commit、一个干净)→ planUp 只含 1 项目 +
/// workspace 项 → executeUp 后:项目 receipt uploaded、manifest 里 last_sync.head
/// 更新、workspace 仓库远端 head == 本地、干净项目无 receipt;坏 remote 项目 →
/// 该项 failed 但其余照常成功,progress 回调收到每一项;missing 项目 → plan 里
/// actionable=false 且 executeUp 跳过。
final class SyncEngineTests: XCTestCase {

    // MARK: - 测试专用 git 身份注入(假 HOME,套路照抄 WorkspaceRepoServiceTests)

    private var savedHome: String?
    private var fakeHome: URL?

    override func setUp() {
        super.setUp()
        savedHome = ProcessInfo.processInfo.environment["HOME"]

        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("syncengine-fakehome-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let gitconfig = """
        [user]
        \tname = SyncEngineTests
        \temail = syncengine-tests@example.com
        """
        try? gitconfig.write(
            to: home.appendingPathComponent(".gitconfig"), atomically: true, encoding: .utf8)
        fakeHome = home

        setenv("HOME", home.path, 1)
    }

    override func tearDown() {
        if let savedHome {
            setenv("HOME", savedHome, 1)
        } else {
            unsetenv("HOME")
        }
        if let fakeHome {
            try? FileManager.default.removeItem(at: fakeHome)
        }
        savedHome = nil
        fakeHome = nil
        super.tearDown()
    }

    // MARK: - planUp

    func testPlanUpOnlyIncludesDirtyProjectAndAlwaysWorkspace() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)

        let projectItems = items.filter { $0.kind == .project }
        XCTAssertEqual(projectItems.count, 1, "干净项目不该进 plan")
        XCTAssertEqual(projectItems.first?.id, "proj-dirty")
        XCTAssertTrue(projectItems.first?.actionable ?? false)

        let workspaceItems = items.filter { $0.kind == .workspaceRepo }
        XCTAssertEqual(workspaceItems.count, 1, "workspace 项永远出现一次")
        XCTAssertEqual(workspaceItems.first?.id, "workspace")
        XCTAssertTrue(workspaceItems.first?.actionable ?? false)
    }

    func testPlanUpMissingProjectIsNotActionable() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let missingLocalPath = fixture.root.appendingPathComponent("never-checked-out", isDirectory: true)
        try fixture.layout.writeProject(
            id: "proj-missing",
            ProjectManifest(
                name: "Missing 项目", remote: "file:///no-such-remote.git",
                localPath: missingLocalPath.path, defaultBranch: "main"
            )
        )

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)

        let missingItem = items.first { $0.id == "proj-missing" }
        guard let missingItem else {
            return XCTFail("missing 项目应该出现在 plan 里,只是 actionable=false")
        }
        XCTAssertFalse(missingItem.actionable)
        XCTAssertEqual(missingItem.detail, "本机未 checkout")
    }

    // MARK: - executeUp

    func testExecuteUpUploadsDirtyProjectAndSyncsWorkspace() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)

        var progressed: [SyncReceipt] = []
        let receipts = try engine.executeUp(items: items, wipCommit: false) { receipt in
            progressed.append(receipt)
        }

        // 项目 receipt uploaded。
        let projectReceipts = receipts.filter { $0.kind == .project }
        XCTAssertEqual(projectReceipts.count, 1, "干净项目不该产生 receipt")
        guard case .uploaded(let projectRemoteHead) = projectReceipts.first?.outcome else {
            return XCTFail("expected uploaded, got \(String(describing: projectReceipts.first?.outcome))")
        }
        let dirtyLocalHead = try WorkspaceGit.head(at: fixture.dirtyProjectLocalPath)
        XCTAssertEqual(projectRemoteHead, dirtyLocalHead)

        // manifest 里 last_sync.head 更新。
        let projects = try fixture.layout.loadProjects()
        XCTAssertEqual(projects["proj-dirty"]?.lastSync?.head, dirtyLocalHead)
        XCTAssertEqual(projects["proj-dirty"]?.lastSync?.machine, "machine-a")
        // 干净项目未被同步引擎碰过,manifest 里没有 last_sync。
        XCTAssertNil(projects["proj-clean"]?.lastSync)

        // workspace 仓库远端 head == 本地。
        let workspaceReceipts = receipts.filter { $0.kind == .workspaceRepo }
        XCTAssertEqual(workspaceReceipts.count, 1)
        guard case .uploaded(let workspaceRemoteHead) = workspaceReceipts.first?.outcome else {
            return XCTFail("expected workspace uploaded, got \(String(describing: workspaceReceipts.first?.outcome))")
        }
        let workspaceLocalHead = try WorkspaceGit.head(at: fixture.layout.root)
        XCTAssertEqual(workspaceRemoteHead, workspaceLocalHead)
        let verifiedWorkspaceRemoteHead = try WorkspaceGit.lsRemoteHead(
            remoteURL: fixture.workspaceRemoteURL, branch: "main", cwd: fixture.layout.root)
        XCTAssertEqual(verifiedWorkspaceRemoteHead, workspaceLocalHead)

        // progress 回调收到了每一项。
        XCTAssertEqual(progressed.count, receipts.count)
    }

    func testExecuteUpSkipsNonActionableItems() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let missingLocalPath = fixture.root.appendingPathComponent("never-checked-out", isDirectory: true)
        try fixture.layout.writeProject(
            id: "proj-missing",
            ProjectManifest(
                name: "Missing 项目", remote: "file:///no-such-remote.git",
                localPath: missingLocalPath.path, defaultBranch: "main"
            )
        )

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)
        XCTAssertTrue(items.contains { $0.id == "proj-missing" && !$0.actionable })

        let receipts = try engine.executeUp(items: items, wipCommit: false) { _ in }
        XCTAssertFalse(
            receipts.contains { $0.itemId == "proj-missing" },
            "missing 项目 actionable=false,executeUp 应该跳过,不产出 receipt"
        )
    }

    /// 坏 remote 项目 → 该项 failed,但其余项目照常成功,progress 回调收到每一项。
    func testExecuteUpContinuesAfterOneProjectFails() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        // 造第三个项目:先真实 push 建好 tracking,再本地追加一个提交(ahead>0),
        // 最后把 origin 改指向不存在的路径,模拟"远端不可达"(同
        // ProjectSyncServiceTests.testPushFailedWhenRemoteUnreachable 的套路)。
        let badRemoteURL = try makeBareRemote(at: fixture.root, name: "bad-project-remote.git")
        let badLocal = fixture.root.appendingPathComponent("bad-project", isDirectory: true)
        try WorkspaceGit.initRepo(at: badLocal)
        try configureIdentity(at: badLocal)
        try WorkspaceGit.setRemote(at: badLocal, url: badRemoteURL)
        try "hello\n".write(
            to: badLocal.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: badLocal, message: "init"))
        try WorkspaceGit.push(at: badLocal, branch: "main")
        try "second\n".write(
            to: badLocal.appendingPathComponent("SECOND.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: badLocal, message: "second"))

        let unreachableRemote = fixture.root.appendingPathComponent("no-such-remote.git", isDirectory: true)
        let unreachableRemoteURL = "file://\(unreachableRemote.path)"
        try WorkspaceGit.setRemote(at: badLocal, url: unreachableRemoteURL)

        try fixture.layout.writeProject(
            id: "proj-bad",
            ProjectManifest(
                name: "Bad 项目", remote: unreachableRemoteURL,
                localPath: badLocal.path, defaultBranch: "main"
            )
        )

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)
        XCTAssertTrue(items.contains { $0.id == "proj-bad" && $0.actionable })
        XCTAssertTrue(items.contains { $0.id == "proj-dirty" && $0.actionable })

        var progressed: [SyncReceipt] = []
        let receipts = try engine.executeUp(items: items, wipCommit: false) { receipt in
            progressed.append(receipt)
        }

        guard let badReceipt = receipts.first(where: { $0.itemId == "proj-bad" }) else {
            return XCTFail("expected a receipt for proj-bad")
        }
        guard case .failed = badReceipt.outcome else {
            return XCTFail("expected proj-bad to fail, got \(badReceipt.outcome)")
        }

        guard let dirtyReceipt = receipts.first(where: { $0.itemId == "proj-dirty" }) else {
            return XCTFail("expected a receipt for proj-dirty despite proj-bad failing")
        }
        guard case .uploaded = dirtyReceipt.outcome else {
            return XCTFail("expected proj-dirty to still succeed, got \(dirtyReceipt.outcome)")
        }

        // workspace 仍然照常同步(收尾总是跑)。
        XCTAssertTrue(receipts.contains { $0.kind == .workspaceRepo })

        // progress 回调收到了每一项(包括失败的那项)。
        XCTAssertEqual(progressed.count, receipts.count)
        XCTAssertTrue(progressed.contains { $0.itemId == "proj-bad" })
    }

    /// controller review critical fix 回归测试:push 成功(远端确实收到了)
    /// 但 manifest 回写(`writeProject`)失败(`projects/` 目录被 chmod 只读)
    /// → uploaded 与 failed(回写失败)两条 receipt 都要出现,循环不能被带崩、
    /// 其余项目照常处理、workspace 收尾照跑。
    func testExecuteUpManifestWriteBackFailureIsIsolatedAndDoesNotAbortBatch() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        let projectsDir = fixture.layout.root.appendingPathComponent("projects", isDirectory: true)
        defer {
            // 必须先恢复可写权限,否则 tmp 目录树都删不掉。
            try? FileManager.default.setAttributes(
                [.posixPermissions: 0o755], ofItemAtPath: projectsDir.path)
            try? FileManager.default.removeItem(at: fixture.root)
        }

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)

        // plan 之后、执行之前才把 projects/ 目录改只读——scan 阶段不受影响,
        // 只影响 executeUp 里的回写。
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o555], ofItemAtPath: projectsDir.path)

        var progressed: [SyncReceipt] = []
        let receipts = try engine.executeUp(items: items, wipCommit: false) { receipt in
            progressed.append(receipt)
        }

        // 立刻恢复权限,后面的断言/defer 清理都需要能写。
        try? FileManager.default.setAttributes(
            [.posixPermissions: 0o755], ofItemAtPath: projectsDir.path)

        let dirtyReceipts = receipts.filter { $0.itemId == "proj-dirty" }
        XCTAssertEqual(dirtyReceipts.count, 2, "push 成功一条 + 回写失败一条,缺一不可")
        guard case .uploaded = dirtyReceipts[0].outcome else {
            return XCTFail("第一条应该是 push 的 uploaded, got \(dirtyReceipts[0].outcome)")
        }
        guard case .failed(let reason) = dirtyReceipts[1].outcome else {
            return XCTFail("第二条应该是回写失败的 failed, got \(dirtyReceipts[1].outcome)")
        }
        XCTAssertTrue(
            reason.contains("manifest 回写失败"),
            "回写失败的 reason 应该说明是回写这一步失败,不是 push,got: \(reason)"
        )

        // 循环没被带崩:workspace 收尾照常跑。
        XCTAssertTrue(receipts.contains { $0.kind == .workspaceRepo })

        // progress 回调收到了全部 receipt,包括回写失败那一条。
        XCTAssertEqual(progressed.count, receipts.count)
    }

    /// controller review important fix 回归测试:真正穿过 `SyncEngine` 自己的
    /// `do/catch`——plan 之后、执行之前把项目目录整个删掉,`ProjectSyncService
    /// .pushCurrentBranch` 内部第一步 `WorkspaceGit.currentBranch` 就会因为
    /// cwd 不存在而抛出(`requireRepo` 起不来 git 进程),这个 throw 必须被
    /// `executeUp` 的 do/catch 兜成 failed receipt,不能上抛中断其余项目。
    func testExecuteUpProjectDirectoryRemovedBetweenPlanAndExecuteBecomesFailedReceipt() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)
        XCTAssertTrue(items.contains { $0.id == "proj-dirty" && $0.actionable })

        // 模拟"plan 之后、执行之前"环境被破坏。
        try FileManager.default.removeItem(at: fixture.dirtyProjectLocalPath)

        var progressed: [SyncReceipt] = []
        let receipts = try engine.executeUp(items: items, wipCommit: false) { receipt in
            progressed.append(receipt)
        }

        guard let dirtyReceipt = receipts.first(where: { $0.itemId == "proj-dirty" }) else {
            return XCTFail("expected a receipt for proj-dirty even though its directory vanished")
        }
        guard case .failed = dirtyReceipt.outcome else {
            return XCTFail("expected failed, got \(dirtyReceipt.outcome)")
        }

        // 其余(workspace 收尾)照常处理,没被这次抛错打断。
        XCTAssertTrue(receipts.contains { $0.kind == .workspaceRepo })
        XCTAssertEqual(progressed.count, receipts.count)
    }

    /// controller review minor fix 回归测试:plan 之后该项目声明本身从
    /// manifest 里消失(比如并发删除)→ 不能静默 continue,得留一条 failed
    /// receipt,其余项目/workspace 收尾照常处理。
    func testExecuteUpProjectVanishedFromManifestProducesFailedReceipt() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeFixture()
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let engine = SyncEngine(layout: fixture.layout, machineId: "machine-a")
        let items = try engine.planUp(fetchFirst: false)
        XCTAssertTrue(items.contains { $0.id == "proj-dirty" && $0.actionable })

        let projectFile = fixture.layout.root
            .appendingPathComponent("projects", isDirectory: true)
            .appendingPathComponent("proj-dirty.toml")
        try FileManager.default.removeItem(at: projectFile)

        var progressed: [SyncReceipt] = []
        let receipts = try engine.executeUp(items: items, wipCommit: false) { receipt in
            progressed.append(receipt)
        }

        guard let dirtyReceipt = receipts.first(where: { $0.itemId == "proj-dirty" }) else {
            return XCTFail("expected a receipt for proj-dirty even though it vanished from manifest")
        }
        guard case .failed(let reason) = dirtyReceipt.outcome else {
            return XCTFail("expected failed, got \(dirtyReceipt.outcome)")
        }
        XCTAssertEqual(reason, "plan 后项目从 manifest 消失")

        XCTAssertTrue(receipts.contains { $0.kind == .workspaceRepo })
        XCTAssertEqual(progressed.count, receipts.count)
    }

    // MARK: - fixture helpers(套路照抄 WorkspaceRepoServiceTests / ProjectSyncServiceTests)

    private struct Fixture {
        let root: URL
        let layout: WorkspaceRepoLayout
        let workspaceRemoteURL: String
        let cleanProjectLocalPath: URL
        let dirtyProjectLocalPath: URL
    }

    /// 建 1 个 workspace 仓库(自带 bare remote)+ 2 个项目仓库:`proj-clean`
    /// (已推到底,ahead=0,clean)、`proj-dirty`(有一个本地提交尚未推送,
    /// ahead=1)。
    private func makeFixture() throws -> Fixture {
        let root = tmpDir("syncengine-e2e")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

        // --- workspace 仓库自身 ---
        let workspaceRemoteURL = try makeBareRemote(at: root, name: "workspace-remote.git")
        let workspaceRoot = root.appendingPathComponent("workspace", isDirectory: true)
        let layout = try WorkspaceRepoService.ensure(at: workspaceRoot, remoteURL: nil, name: "sync-engine-ws")
        try WorkspaceGit.setRemote(at: workspaceRoot, url: workspaceRemoteURL)

        // --- proj-clean:推到底,ahead=0 ---
        let cleanRemoteURL = try makeBareRemote(at: root, name: "clean-remote.git")
        let cleanLocal = root.appendingPathComponent("clean-project", isDirectory: true)
        try WorkspaceGit.initRepo(at: cleanLocal)
        try configureIdentity(at: cleanLocal)
        try WorkspaceGit.setRemote(at: cleanLocal, url: cleanRemoteURL)
        try "hello\n".write(
            to: cleanLocal.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: cleanLocal, message: "init"))
        try WorkspaceGit.push(at: cleanLocal, branch: "main")

        // --- proj-dirty:推到底后又本地追加一个未推提交,ahead=1 ---
        let dirtyRemoteURL = try makeBareRemote(at: root, name: "dirty-remote.git")
        let dirtyLocal = root.appendingPathComponent("dirty-project", isDirectory: true)
        try WorkspaceGit.initRepo(at: dirtyLocal)
        try configureIdentity(at: dirtyLocal)
        try WorkspaceGit.setRemote(at: dirtyLocal, url: dirtyRemoteURL)
        try "hello\n".write(
            to: dirtyLocal.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: dirtyLocal, message: "init"))
        try WorkspaceGit.push(at: dirtyLocal, branch: "main")
        try "second\n".write(
            to: dirtyLocal.appendingPathComponent("SECOND.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: dirtyLocal, message: "second, not yet pushed"))

        try layout.writeProject(
            id: "proj-clean",
            ProjectManifest(
                name: "Clean 项目", remote: cleanRemoteURL,
                localPath: cleanLocal.path, defaultBranch: "main"
            )
        )
        try layout.writeProject(
            id: "proj-dirty",
            ProjectManifest(
                name: "Dirty 项目", remote: dirtyRemoteURL,
                localPath: dirtyLocal.path, defaultBranch: "main"
            )
        )

        return Fixture(
            root: root, layout: layout, workspaceRemoteURL: workspaceRemoteURL,
            cleanProjectLocalPath: cleanLocal, dirtyProjectLocalPath: dirtyLocal
        )
    }

    private func tmpDir(_ prefix: String) -> URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("\(prefix)-\(UUID().uuidString)", isDirectory: true)
    }

    @discardableResult
    private func makeBareRemote(at root: URL, name: String) throws -> String {
        let bareDir = root.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: bareDir, withIntermediateDirectories: true)
        try runGitRaw(at: bareDir, args: ["init", "--bare", "-b", "main"])
        return "file://\(bareDir.path)"
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
