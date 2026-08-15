#if os(macOS)
import XCTest
// 同 WorkspaceGitTests/ProjectSyncServiceTests:源码直接编进 test bundle,internal API 可见。

/// `WorkspaceRepoService` 的真 git fixture 测试——覆盖 brief 行为清单:
/// `ensure` 三分支(已是 workspace 直接返回 / clone / scaffold 从零建)+
/// clone 到非 workspace 抛 `notAWorkspace`;`syncUp`/`syncDown` 的双机同步流
/// (uploaded + lsRemote 复核、pulled(newHead)、pullRebase 收敛、upToDate、
/// 无 remote → skipped)。
final class WorkspaceRepoServiceTests: XCTestCase {

    // MARK: - 测试专用 git 身份注入(假 HOME,不碰生产 env 白名单)

    /// 有些路径(尤其是 `ensure` 的 scaffold 分支)内部会直接跑
    /// `WorkspaceGit.commitAll`,测试没有机会在中间插入 `configureIdentity`
    /// (不像 `WorkspaceGitTests`/`ProjectSyncServiceTests` 那样先 `initRepo`
    /// 再显式配置本地身份,再自己调 `commitAll`)。之前依赖跑测试的机器已经有
    /// 全局 `~/.gitconfig`,在没有全局身份的机器(比如干净的 CI 跑者)上不保证
    /// 一定能提交。
    ///
    /// **不走 `GIT_AUTHOR_*`/`GIT_COMMITTER_*` 环境变量**(review 打回的方案)
    /// ——那需要把这几个变量加进 `WorkspaceGit.run()` 的**生产共享** env 白名单,
    /// 而 `WorkspaceGit` 头注释明确的不变量是"不管理 git 身份":一旦真的跑在
    /// GUI 启动的进程里、这几个变量恰好被外部环境残留设置了,`commitAll` 会
    /// 静默地把提交作者标记成别的身份,直接破坏"回执等于事实/谁什么时候改了
    /// 什么"这个审计目标。风险和"省一次 configureIdentity"完全不对等。
    ///
    /// 改用**假 `HOME`**:`HOME` 本来就在白名单里(git 读 `~/.gitconfig` 需要
    /// 它),给这个假目录写一份带 `[user] name/email` 的 `.gitconfig`,
    /// `setenv("HOME", ...)` 指过去——生产 env 白名单一行都不用改,`ensure`
    /// 内部那次不受控制的 `commitAll` 也能读到身份。`setUp`/`tearDown` 成对
    /// 出现,恢复原 `HOME`,并清理临时目录。
    ///
    /// 假 `HOME` 在这个测试类的方法运行期间对*同一进程*里其它同时依赖
    /// `~/.gitconfig` 的代码也可见——`WorkspaceGitTests`/`ProjectSyncServiceTests`
    /// 都是各自 `configureIdentity(at: repo)` 显式写 repo-local 身份(不依赖
    /// 全局 `~/.gitconfig`),不受这里的假 HOME 影响;XCTest 默认同一 bundle
    /// 内串行跑各测试方法,不会有真正的并发竞争。
    private var savedHome: String?
    private var fakeHome: URL?

    override func setUp() {
        super.setUp()
        savedHome = ProcessInfo.processInfo.environment["HOME"]

        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("workspacerepo-fakehome-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let gitconfig = """
        [user]
        \tname = WorkspaceRepoServiceTests
        \temail = workspacerepo-tests@example.com
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

    // MARK: - ensure 三分支

    /// 分支 1:目录已是 workspace → 直接返回,不碰 git、不被 `name` 参数覆盖。
    /// 刻意不 `git init`——如果这个分支偷偷跑了任何 git 操作,会因为目录不是
    /// git repo 而抛错,从而证明它真的"什么都没做,原样返回"。
    func testEnsureReturnsExistingWorkspaceDirectly() throws {
        let tmp = tmpDir("ensure-existing")
        defer { try? FileManager.default.removeItem(at: tmp) }
        _ = try WorkspaceRepoLayout.scaffold(at: tmp, name: "existing-ws")

        let layout = try WorkspaceRepoService.ensure(at: tmp, remoteURL: nil, name: "ignored-name")
        let manifest = try layout.loadWorkspace()
        XCTAssertEqual(manifest.name, "existing-ws", "已存在的 workspace 不该被 name 参数覆盖")
    }

    /// 目录存在但不是 workspace(比如误指了个普通目录)→ 原样抛 `notAWorkspace`,
    /// 不静默降级去 scaffold。
    func testEnsureExistingNonWorkspaceDirectoryThrows() throws {
        let tmp = tmpDir("ensure-existing-plain")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        try "just a file".write(
            to: tmp.appendingPathComponent("note.txt"), atomically: true, encoding: .utf8)

        do {
            _ = try WorkspaceRepoService.ensure(at: tmp, remoteURL: nil, name: "x")
            XCTFail("expected notAWorkspace")
        } catch WorkspaceRepoLayout.LayoutError.notAWorkspace {
            // expected
        }
    }

    /// 分支 3:目录不存在且 `remoteURL` 为 nil → scaffold + initRepo + 首次
    /// commitAll,不配 remote。
    func testEnsureScaffoldsWhenMissingAndNoRemote() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let tmp = tmpDir("ensure-scaffold")
        defer { try? FileManager.default.removeItem(at: tmp) }

        let layout = try WorkspaceRepoService.ensure(at: tmp, remoteURL: nil, name: "new-ws")
        let manifest = try layout.loadWorkspace()
        XCTAssertEqual(manifest.name, "new-ws")
        XCTAssertNil(try WorkspaceGit.remoteURL(at: tmp), "没传 remoteURL 不该配 remote")
        XCTAssertFalse(try WorkspaceGit.isDirty(at: tmp), "scaffold 后应已首次 commit,工作区应 clean")
        _ = try WorkspaceGit.head(at: tmp) // 不抛即证明已经有至少一个 commit
    }

    /// 分支 2:目录不存在且 `remoteURL` 非空 → clone,内容以远端为准(不被
    /// `name` 参数覆盖),且 clone 下来的仓库确实指向传入的 remote。
    func testEnsureClonesWhenMissingAndRemotePresent() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = tmpDir("ensure-clone")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let remoteURL = try makeBareRemote(at: root, name: "remote.git")

        let seedRoot = root.appendingPathComponent("seed", isDirectory: true)
        let seedLayout = try WorkspaceRepoService.ensure(at: seedRoot, remoteURL: nil, name: "seeded-ws")
        try configureIdentity(at: seedRoot)
        try WorkspaceGit.setRemote(at: seedRoot, url: remoteURL)
        let pushReceipt = WorkspaceRepoService.syncUp(layout: seedLayout, message: "seed")
        guard case .uploaded = pushReceipt.outcome else {
            return XCTFail("seed push 应该 uploaded,got \(pushReceipt.outcome)")
        }

        let clonedRoot = root.appendingPathComponent("cloned", isDirectory: true)
        let clonedLayout = try WorkspaceRepoService.ensure(at: clonedRoot, remoteURL: remoteURL, name: "ignored")
        let manifest = try clonedLayout.loadWorkspace()
        XCTAssertEqual(manifest.name, "seeded-ws", "clone 出来的内容应以远端为准,不被 name 参数覆盖")
        XCTAssertEqual(try WorkspaceGit.remoteURL(at: clonedRoot), remoteURL)
    }

    /// **Critical fix 回归测试**:分支 2 的核心场景——remote 提前配好但还是个
    /// 空 bare 仓库(没有任何提交)。这是"第一台机器,但 workspace 的远端已经
    /// 建好"的正常引导路径,`ensure` 应该在 clone 下来的目录里直接 scaffold +
    /// 首次 commit,而不是把它误判成"不是 workspace"抛错。
    func testEnsureBootstrapsWhenRemoteConfiguredButEmpty() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = tmpDir("ensure-empty-remote")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        // 空 bare 仓库——只 init,不塞任何提交。
        let remoteURL = try makeBareRemote(at: root, name: "empty-remote.git")

        let bootstrapRoot = root.appendingPathComponent("bootstrap", isDirectory: true)
        let layout = try WorkspaceRepoService.ensure(
            at: bootstrapRoot, remoteURL: remoteURL, name: "bootstrap-ws"
        )

        let manifest = try layout.loadWorkspace()
        XCTAssertEqual(manifest.name, "bootstrap-ws")
        XCTAssertEqual(
            try WorkspaceGit.remoteURL(at: bootstrapRoot), remoteURL,
            "clone 已经配好 origin,不需要 ensure 再 setRemote"
        )
        XCTAssertFalse(try WorkspaceGit.isDirty(at: bootstrapRoot), "scaffold 后应已首次 commit")

        // 首次 commit 之后应该能正常 syncUp 推上去。
        let pushReceipt = WorkspaceRepoService.syncUp(layout: layout, message: "first sync")
        guard case .uploaded(let remoteHead) = pushReceipt.outcome else {
            return XCTFail("bootstrap 后首推应该 uploaded,got \(pushReceipt.outcome)")
        }
        XCTAssertEqual(remoteHead, try WorkspaceGit.head(at: bootstrapRoot))
    }

    /// clone 下来的东西不是 workspace(远端仓库存在,但没有 `workspace.toml`)
    /// → 原样抛 `notAWorkspace`;clone 本身已经落地(不是 clone 步骤失败)。
    func testEnsureClonedNonWorkspaceThrows() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = tmpDir("ensure-badclone")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let remoteURL = try makeBareRemote(at: root, name: "remote.git")

        // 造一个"看起来像仓库但不是 workspace"的远端。
        let plainRoot = root.appendingPathComponent("plain", isDirectory: true)
        try WorkspaceGit.initRepo(at: plainRoot, initialBranch: "main")
        try configureIdentity(at: plainRoot)
        try WorkspaceGit.setRemote(at: plainRoot, url: remoteURL)
        try "not a workspace\n".write(
            to: plainRoot.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: plainRoot, message: "plain"))
        try WorkspaceGit.push(at: plainRoot, branch: "main")

        let clonedRoot = root.appendingPathComponent("cloned", isDirectory: true)
        do {
            _ = try WorkspaceRepoService.ensure(at: clonedRoot, remoteURL: remoteURL, name: "x")
            XCTFail("expected notAWorkspace")
        } catch WorkspaceRepoLayout.LayoutError.notAWorkspace {
            // expected
        }
        XCTAssertTrue(
            FileManager.default.fileExists(atPath: clonedRoot.appendingPathComponent(".git").path),
            "clone 本身应该已经落地,只是校验没通过"
        )
    }

    // MARK: - syncUp 无 remote

    func testSyncUpSkippedWhenNoRemote() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let tmp = tmpDir("syncup-noremote")
        defer { try? FileManager.default.removeItem(at: tmp) }

        let layout = try WorkspaceRepoService.ensure(at: tmp, remoteURL: nil, name: "ws")
        let receipt = WorkspaceRepoService.syncUp(layout: layout, message: "noop")
        XCTAssertEqual(receipt.outcome, .skipped(reason: "未配置 remote"))
        XCTAssertEqual(receipt.kind, .workspaceRepo)
        XCTAssertEqual(receipt.itemId, "workspace")
    }

    // MARK: - 双机同步 end-to-end

    /// 覆盖 brief 的完整双机流程:
    /// A(scaffold,手工挂 remote)首推 → uploaded + lsRemote 复核;再推一次
    /// (无变更且远端已一致)→ upToDate;B clone → 内容可见;B 推自己的变更 →
    /// uploaded;A(未 fetch,落后)推自己的变更 → 先 pullRebase 兜
    /// non-fast-forward 再 push → uploaded;B syncDown → pulled(newHead) 且
    /// 新内容可见;B 再 syncDown(无增量)→ upToDate;B 本地也有未推 commit,
    /// A 又推了新内容,B syncDown → pullRebase 收敛,两边内容都在。
    func testTwoMachineSyncUpAndDownFlow() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = tmpDir("syncflow-e2e")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let remoteURL = try makeBareRemote(at: root, name: "remote.git")

        // --- A:本机从零建(scaffold,无 remote)+ 手工挂 remote ---
        let repoA = root.appendingPathComponent("repoA", isDirectory: true)
        let layoutA = try WorkspaceRepoService.ensure(at: repoA, remoteURL: nil, name: "flow-ws")
        try configureIdentity(at: repoA)
        try WorkspaceGit.setRemote(at: repoA, url: remoteURL)

        // 固定 now,顺带验证 now() 注入贯通到 receipt.at。
        let fixedNow = Date(timeIntervalSince1970: 1_752_900_000)
        let expectedAt = ISO8601DateFormatter().string(from: fixedNow)

        // 1. A 首推(远端此前是空 bare 仓库)→ uploaded,lsRemote 复核通过。
        let firstPush = WorkspaceRepoService.syncUp(
            layout: layoutA, message: "first sync", now: { fixedNow }
        )
        guard case .uploaded(let remoteHead1) = firstPush.outcome else {
            return XCTFail("首推应该 uploaded,got \(firstPush.outcome)")
        }
        XCTAssertEqual(firstPush.at, expectedAt)
        XCTAssertEqual(firstPush.kind, .workspaceRepo)
        XCTAssertEqual(firstPush.itemId, "workspace")
        let headA1 = try WorkspaceGit.head(at: repoA)
        XCTAssertEqual(remoteHead1, headA1)
        let verifyRemoteHead1 = try WorkspaceGit.lsRemoteHead(remoteURL: remoteURL, branch: "main", cwd: repoA)
        XCTAssertEqual(verifyRemoteHead1, headA1, "回执里的 remoteHead 应该和独立复核一致")

        // 2. 再推一次:没有本地新变更,远端本就一致 → upToDate,不碰网络出错。
        let noopPush = WorkspaceRepoService.syncUp(layout: layoutA, message: "noop")
        XCTAssertEqual(noopPush.outcome, .upToDate)

        // 3. B:clone,内容可见(workspace name 与 A 一致)。
        let repoB = root.appendingPathComponent("repoB", isDirectory: true)
        let layoutB = try WorkspaceRepoService.ensure(at: repoB, remoteURL: remoteURL, name: "ignored")
        try configureIdentity(at: repoB)
        XCTAssertEqual(try layoutB.loadWorkspace().name, "flow-ws")

        // 4. B 推自己的变更(新增一个 project 声明)→ uploaded。
        try layoutB.writeProject(
            id: "proj-b",
            ProjectManifest(name: "B 的项目", remote: "git@example.com:b.git", localPath: "~/b", defaultBranch: "main")
        )
        let bPush = WorkspaceRepoService.syncUp(layout: layoutB, message: "B adds proj-b")
        guard case .uploaded(let remoteHead2) = bPush.outcome else {
            return XCTFail("B 推送应该 uploaded,got \(bPush.outcome)")
        }
        let headB1 = try WorkspaceGit.head(at: repoB)
        XCTAssertEqual(remoteHead2, headB1)

        // 5. A(还没 fetch 过 B 的推送,本地又有新变更)推送 → 应该先
        //    pullRebase 兜 non-fast-forward,再 push,而不是直接失败。
        try layoutA.writeProject(
            id: "proj-a",
            ProjectManifest(name: "A 的项目", remote: "git@example.com:a.git", localPath: "~/a", defaultBranch: "main")
        )
        let aSecondPush = WorkspaceRepoService.syncUp(layout: layoutA, message: "A adds proj-a")
        guard case .uploaded(let remoteHead3) = aSecondPush.outcome else {
            return XCTFail("A 第二次推送应该 uploaded(经 pullRebase 收敛),got \(aSecondPush.outcome)")
        }
        let headA2 = try WorkspaceGit.head(at: repoA)
        XCTAssertEqual(remoteHead3, headA2)
        // A 的历史现在应该同时包含 B 的 proj-b(rebase 收敛的证据)。
        let projectsOnA = try layoutA.loadProjects()
        XCTAssertNotNil(projectsOnA["proj-a"])
        XCTAssertNotNil(projectsOnA["proj-b"], "A push 前应先 pullRebase 拉到 B 的提交")

        // 6. B syncDown → 拉到 A 的新提交,outcome = pulled(newHead),
        //    新内容(proj-a)在 B 上也可见。
        let bPull = WorkspaceRepoService.syncDown(layout: layoutB)
        guard case .pulled(let newHead) = bPull.outcome else {
            return XCTFail("B syncDown 应该 pulled,got \(bPull.outcome)")
        }
        XCTAssertEqual(newHead, headA2)
        XCTAssertEqual(try WorkspaceGit.head(at: repoB), headA2)
        let projectsOnB = try layoutB.loadProjects()
        XCTAssertNotNil(projectsOnB["proj-a"], "syncDown 后 B 应该能看到 A 新增的内容")
        XCTAssertNotNil(projectsOnB["proj-b"])

        // 7. 再 syncDown 一次:无增量 → upToDate。
        let bPullAgain = WorkspaceRepoService.syncDown(layout: layoutB)
        XCTAssertEqual(bPullAgain.outcome, .upToDate)

        // 8. B 本地也有未推的 commit,同时 A 又推了新内容;B syncDown 应该
        //    pullRebase 收敛,不炸,两边内容都在。
        try layoutB.writeProject(
            id: "proj-b2",
            ProjectManifest(name: "B 的第二个项目", remote: "git@example.com:b2.git", localPath: "~/b2", defaultBranch: "main")
        )
        XCTAssertTrue(try WorkspaceGit.commitAll(at: repoB, message: "B local proj-b2, not yet pushed"))
        XCTAssertTrue(try WorkspaceGit.isDirty(at: repoB) == false)

        try layoutA.writeProject(
            id: "proj-a2",
            ProjectManifest(name: "A 的第二个项目", remote: "git@example.com:a2.git", localPath: "~/a2", defaultBranch: "main")
        )
        let aThirdPush = WorkspaceRepoService.syncUp(layout: layoutA, message: "A adds proj-a2")
        guard case .uploaded = aThirdPush.outcome else {
            return XCTFail("A 第三次推送应该 uploaded,got \(aThirdPush.outcome)")
        }

        let bConvergePull = WorkspaceRepoService.syncDown(layout: layoutB)
        guard case .pulled = bConvergePull.outcome else {
            return XCTFail("B syncDown 应该 pullRebase 收敛成 pulled,got \(bConvergePull.outcome)")
        }
        XCTAssertFalse(try WorkspaceGit.isDirty(at: repoB), "rebase 收敛后应该 clean")
        let projectsOnBFinal = try layoutB.loadProjects()
        XCTAssertNotNil(projectsOnBFinal["proj-a2"], "B 应该能看到 A 最新推送的内容")
        XCTAssertNotNil(projectsOnBFinal["proj-b2"], "B 自己本地未推的 commit 应该在 rebase 后仍然保留")
    }

    // MARK: - fixture helpers(套路同 WorkspaceGitTests / ProjectSyncServiceTests)

    private func tmpDir(_ prefix: String) -> URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("workspacerepo-\(prefix)-\(UUID().uuidString)", isDirectory: true)
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
