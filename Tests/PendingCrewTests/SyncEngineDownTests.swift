#if os(macOS)
import XCTest
// 同 SyncEngineTests:源码直接编进 test bundle,internal API 可见。

/// `SyncEngine.planDown`/`executeDown` 的真 git fixture 测试——覆盖 brief 行为
/// 清单(跨机器场景,机器 A `executeUp` 全绿后,机器 B 独立 `machineId` + 独立 tmp
/// 目录 clone workspace 仓库):
/// - 含 clone 项 → `executeDown` 后 B 项目 head == A 推的 head,receipt `.pulled`。
/// - B 本地造 dirty → 该项 `actionable = false`。
/// - B 有独立(不是祖先关系的)本地提交 → `actionable = false`。
/// - remote 比 manifest 记录的更新 → pull 后 `.pulled` 且 `newHead` 如实超过
///   `last_sync.head`。
/// - 从未同步(无 `last_sync`)的项目不进 plan,即便 `planDown` 第 0 步
///   `WorkspaceRepoService.syncDown` 本身失败(没配 remote)也照常出清单。
final class SyncEngineDownTests: XCTestCase {

    // MARK: - 测试专用 git 身份注入(假 HOME,套路照抄 SyncEngineTests)

    private var savedHome: String?
    private var fakeHome: URL?

    override func setUp() {
        super.setUp()
        savedHome = ProcessInfo.processInfo.environment["HOME"]

        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("syncenginedown-fakehome-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let gitconfig = """
        [user]
        \tname = SyncEngineDownTests
        \temail = syncenginedown-tests@example.com
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

    // MARK: - planDown / executeDown

    /// 从未同步(无 `last_sync`)的项目不进 plan,即使这次 `planDown` 第 0 步
    /// `syncDown` 本身失败(这个孤立 workspace 压根没配 remote,`git fetch`
    /// 会报错)——`workspaceReceipt` 如实带着失败,但 plan 依然用本机现有
    /// manifest 照常出清单。
    func testPlanDownExcludesProjectWithoutLastSyncEvenWhenWorkspaceSyncDownFails() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = tmpDir("syncenginedown-no-lastsync")
        defer { try? FileManager.default.removeItem(at: root) }

        let workspaceRoot = root.appendingPathComponent("workspace", isDirectory: true)
        let layout = try WorkspaceRepoService.ensure(at: workspaceRoot, remoteURL: nil, name: "no-lastsync-ws")

        try layout.writeProject(
            id: "proj-never-synced",
            ProjectManifest(
                name: "从未同步的项目", remote: "file:///no-such-remote.git",
                localPath: root.appendingPathComponent("never-synced", isDirectory: true).path,
                defaultBranch: "main"
            )
        )

        let engine = SyncEngine(layout: layout, machineId: "machine-solo")
        let (workspaceReceipt, items) = try engine.planDown()

        guard case .failed = workspaceReceipt.outcome else {
            return XCTFail(
                "这个 workspace 仓库没配 remote,syncDown 应该 failed,got \(workspaceReceipt.outcome)")
        }
        XCTAssertFalse(
            items.contains { $0.id == "proj-never-synced" },
            "从未同步过的项目不该出现在下行 plan 里")
    }

    /// 主场景:B 是全新机器,含 clone 项 → `executeDown` 后 B 项目仓库 head ==
    /// A 推的 head,receipt `.pulled(newHead:)`。
    func testPlanDownIncludesCloneItemAndExecuteDownConvergesToPushedHead() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeCrossMachineFixture()
        defer { fixture.cleanup() }

        let engine = SyncEngine(layout: fixture.bLayout, machineId: "machine-b")
        let (_, items) = try engine.planDown()

        guard let cloneItem = items.first(where: { $0.id == "proj-a" }) else {
            return XCTFail("expected proj-a to appear as a clone item")
        }
        XCTAssertTrue(cloneItem.actionable)
        XCTAssertEqual(cloneItem.kind, .project)

        var progressed: [SyncReceipt] = []
        let receipts = try engine.executeDown(items: items) { progressed.append($0) }

        var isDirectory: ObjCBool = false
        let exists = FileManager.default.fileExists(
            atPath: fixture.bProjLocal.path, isDirectory: &isDirectory)
        XCTAssertTrue(exists && isDirectory.boolValue, "clone 之后本机应该有这个目录")

        let bHead = try WorkspaceGit.head(at: fixture.bProjLocal)
        XCTAssertEqual(bHead, fixture.aPushedHead, "clone 后 B 的 head 应该等于 A 推的 head")

        guard let projReceipt = receipts.first(where: { $0.itemId == "proj-a" }) else {
            return XCTFail("expected a receipt for proj-a")
        }
        guard case .pulled(let newHead) = projReceipt.outcome else {
            return XCTFail("expected pulled, got \(projReceipt.outcome)")
        }
        XCTAssertEqual(newHead, fixture.aPushedHead)
        XCTAssertEqual(progressed.count, receipts.count)
    }

    /// B 本地造出未提交变更(dirty)→ 该项 `actionable = false`,不自动动用户
    /// 仓库。
    func testPlanDownMarksDirtyLocalProjectNonActionable() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeCrossMachineFixture()
        defer { fixture.cleanup() }

        let engine = SyncEngine(layout: fixture.bLayout, machineId: "machine-b")
        try convergeBViaCloneItem(fixture: fixture, engine: engine)

        // 本地造 dirty:改一个已 track 的文件,不提交。
        try "locally edited, not committed\n".write(
            to: fixture.bProjLocal.appendingPathComponent("README.md"),
            atomically: true, encoding: .utf8)

        let (_, items) = try engine.planDown()
        guard let item = items.first(where: { $0.id == "proj-a" }) else {
            return XCTFail("expected proj-a to still appear in plan (dirty, not actionable)")
        }
        XCTAssertFalse(item.actionable)
        XCTAssertEqual(item.detail, "本机有未同步变更,请先上行或手动处理")
    }

    /// B 有本机独立进度(本地新提交,不是 `last_sync.head` 的祖先关系)→ 该项
    /// `actionable = false`——这种情况应该走上行/手动处理,不该被自动 pull
    /// 覆盖。
    func testPlanDownMarksLocalDivergentCommitNonActionable() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeCrossMachineFixture()
        defer { fixture.cleanup() }

        let engine = SyncEngine(layout: fixture.bLayout, machineId: "machine-b")
        try convergeBViaCloneItem(fixture: fixture, engine: engine)

        // B 本地新提交(领先 last_sync.head,不是它的祖先)。
        try "b-only change\n".write(
            to: fixture.bProjLocal.appendingPathComponent("B-ONLY.md"),
            atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: fixture.bProjLocal, message: "b-only local commit"))

        let (_, items) = try engine.planDown()
        guard let item = items.first(where: { $0.id == "proj-a" }) else {
            return XCTFail("expected proj-a to still appear in plan (diverged, not actionable)")
        }
        XCTAssertFalse(item.actionable)
        XCTAssertEqual(item.detail, "本机有未同步变更,请先上行或手动处理")
    }

    /// remote 比 manifest 记录的更新:另一台机器(这里直接用 A 模拟)在 workspace
    /// 仓库 manifest 更新之后,又直接推了一个 B 还不知道的新提交到项目远端,却
    /// 没有(还没来得及)把这次同步再记进 manifest。B pull 之后本机 head 会
    /// 超过 `last_sync.head` 记的那个旧点——这不是错误,`.pulled(newHead:)`
    /// 如实携带那个更靠前的真实值。
    func testExecuteDownPullPastManifestRecordedHeadStillReportsPulled() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeCrossMachineFixture()
        defer { fixture.cleanup() }

        let engine = SyncEngine(layout: fixture.bLayout, machineId: "machine-b")
        try convergeBViaCloneItem(fixture: fixture, engine: engine)
        // 此刻 B 的 proj-a head == fixture.aPushedHead == manifest 记录的
        // last_sync.head。

        // --- A 侧:再提交一次并推,同时把这次同步记进 manifest(模拟"正常走过
        // 一轮上行,manifest 已更新到 commit2")。---
        try "second-sync\n".write(
            to: fixture.aProjLocal.appendingPathComponent("SECOND-SYNC.md"),
            atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: fixture.aProjLocal, message: "second sync round"))
        try WorkspaceGit.push(at: fixture.aProjLocal, branch: "main")
        let commit2 = try WorkspaceGit.head(at: fixture.aProjLocal)

        var projects = try fixture.aLayout.loadProjects()
        guard var updatedProject = projects["proj-a"] else {
            return XCTFail("expected proj-a to still be declared in A's manifest")
        }
        updatedProject.lastSync = LastSync(
            machine: "machine-a", branch: "main", head: commit2, pushedAt: "2026-07-20T00:00:00Z")
        try fixture.aLayout.writeProject(id: "proj-a", updatedProject)
        projects = try fixture.aLayout.loadProjects() // 保持局部变量语义清晰,后面不再用。
        _ = projects

        // 把这次 manifest 更新推上 workspace 远端,B 才能在 syncDown 里看到它。
        let workspaceSyncReceipt = WorkspaceRepoService.syncUp(
            layout: fixture.aLayout, message: "chore: record second sync", now: Date.init)
        guard case .uploaded = workspaceSyncReceipt.outcome else {
            return XCTFail("expected workspace manifest push to succeed, got \(workspaceSyncReceipt.outcome)")
        }

        // --- A 侧(模拟"另一台机器"):再直接推一个 commit3,不记进 manifest。---
        try "third-untracked\n".write(
            to: fixture.aProjLocal.appendingPathComponent("THIRD-UNTRACKED.md"),
            atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: fixture.aProjLocal, message: "untracked third push"))
        try WorkspaceGit.push(at: fixture.aProjLocal, branch: "main")
        let commit3 = try WorkspaceGit.head(at: fixture.aProjLocal)
        XCTAssertNotEqual(commit2, commit3)

        // --- B 侧:planDown 第 0 步会把 manifest 刷新到 commit2,B 本地还停在
        // commit1(== fixture.aPushedHead),落后且是祖先关系 → pull 项。---
        let (workspaceReceipt, items) = try engine.planDown()
        if case .failed(let reason) = workspaceReceipt.outcome {
            return XCTFail("expected B's workspace syncDown to succeed, got failed: \(reason)")
        }
        guard let item = items.first(where: { $0.id == "proj-a" }) else {
            return XCTFail("expected proj-a to appear as a pull item")
        }
        XCTAssertTrue(item.actionable, "detail was: \(item.detail)")

        let receipts = try engine.executeDown(items: items) { _ in }
        guard let projReceipt = receipts.first(where: { $0.itemId == "proj-a" }) else {
            return XCTFail("expected a receipt for proj-a")
        }
        guard case .pulled(let newHead) = projReceipt.outcome else {
            return XCTFail("expected pulled (remote ahead of manifest is not an error), got \(projReceipt.outcome)")
        }
        // pullFastForward 天然拉到 origin 当下的真实 HEAD(commit3),不是
        // manifest 记的那个旧点(commit2)——如实超过。
        XCTAssertEqual(newHead, commit3)
        XCTAssertNotEqual(newHead, commit2)
        let bHead = try WorkspaceGit.head(at: fixture.bProjLocal)
        XCTAssertEqual(bHead, commit3)

        // controller 裁决 Important #2:「已超过 manifest 记录」这句解释必须
        // 落在 receipt 上,不能无处安放。
        XCTAssertNotNil(projReceipt.detail, "expected a non-nil detail explaining newHead > last_sync.head")
        XCTAssertEqual(projReceipt.detail, "已超过 manifest 记录(remote 更新)")
    }

    /// controller 裁决 Critical #1 回归测试:项目已经收敛(本机 head ==
    /// `last_sync.head`)且不 dirty 时,`planDown` 不该为了"确认已经干净"去碰
    /// 网络——即使这个项目声明的 `remote` 压根不可达,已收敛的项目也该照旧
    /// 干净、不出现在 plan 里,不能被误报成"扫描失败"。
    func testPlanDownAlreadyConvergedProjectSkipsNetworkAndStaysClean() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = tmpDir("syncenginedown-offline-clean")
        defer { try? FileManager.default.removeItem(at: root) }

        let workspaceRoot = root.appendingPathComponent("workspace", isDirectory: true)
        let layout = try WorkspaceRepoService.ensure(at: workspaceRoot, remoteURL: nil, name: "offline-ws")

        let projLocal = root.appendingPathComponent("proj-a", isDirectory: true)
        try WorkspaceGit.initRepo(at: projLocal)
        try configureIdentity(at: projLocal)
        try "hello\n".write(
            to: projLocal.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: projLocal, message: "init"))
        let head = try WorkspaceGit.head(at: projLocal)
        // 声明一个压根不存在的 remote——已收敛的项目不该因为这个而受影响。
        try WorkspaceGit.setRemote(at: projLocal, url: "file:///no-such-remote-\(UUID().uuidString).git")

        try layout.writeProject(
            id: "proj-a",
            ProjectManifest(
                name: "Proj A", remote: "file:///no-such-remote.git",
                localPath: projLocal.path, defaultBranch: "main",
                lastSync: LastSync(machine: "machine-a", branch: "main", head: head, pushedAt: "2026-07-20T00:00:00Z")
            )
        )

        let engine = SyncEngine(layout: layout, machineId: "machine-b")
        let (_, items) = try engine.planDown()

        XCTAssertFalse(
            items.contains { $0.id == "proj-a" },
            "已收敛且 remote 不可达时不该触发 fetch,应该照旧判定干净、不进 plan")
    }

    /// controller 裁决 Critical #1 的另一半:本机落后 manifest 记录,但项目声明
    /// 的 remote 不可达——`planDown` 需要联网才能安全判定"能不能自动 pull",
    /// fetch 失败时**不能静默跳过**,得如实呈现"无法联网核对",`actionable`
    /// 保持 false(同"绝不自动动用户仓库"的红线——不知道能不能安全收敛就不能
    /// 假装能)。
    func testPlanDownBehindWithUnreachableRemoteReportsNetworkFailureDetail() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let root = tmpDir("syncenginedown-offline-behind")
        defer { try? FileManager.default.removeItem(at: root) }

        let workspaceRoot = root.appendingPathComponent("workspace", isDirectory: true)
        let layout = try WorkspaceRepoService.ensure(at: workspaceRoot, remoteURL: nil, name: "offline-ws")

        let projLocal = root.appendingPathComponent("proj-a", isDirectory: true)
        try WorkspaceGit.initRepo(at: projLocal)
        try configureIdentity(at: projLocal)
        try "hello\n".write(
            to: projLocal.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: projLocal, message: "init"))
        // 声明一个压根不存在的 remote,同时 manifest 记的 last_sync.head 是本机
        // 压根没有的一个提交(模拟"落后")——不管这个假 SHA 是否真实存在,
        // fetch 都会先因为 remote 不可达而失败,这正是要覆盖的路径。
        try WorkspaceGit.setRemote(at: projLocal, url: "file:///no-such-remote-\(UUID().uuidString).git")

        try layout.writeProject(
            id: "proj-a",
            ProjectManifest(
                name: "Proj A", remote: "file:///no-such-remote.git",
                localPath: projLocal.path, defaultBranch: "main",
                lastSync: LastSync(
                    machine: "machine-a", branch: "main",
                    head: "0000000000000000000000000000000000000a", pushedAt: "2026-07-20T00:00:00Z")
            )
        )

        let engine = SyncEngine(layout: layout, machineId: "machine-b")
        let (_, items) = try engine.planDown()

        guard let item = items.first(where: { $0.id == "proj-a" }) else {
            return XCTFail("expected proj-a to appear in plan (head differs from last_sync.head)")
        }
        XCTAssertFalse(item.actionable)
        XCTAssertTrue(
            item.detail.contains("无法联网核对"),
            "expected a network-failure detail, got: \(item.detail)")
    }

    /// controller 裁决 Important #3:下行版的隔离回归测试,照抄
    /// `SyncEngineTests.testExecuteUpProjectDirectoryRemovedBetweenPlanAndExecuteBecomesFailedReceipt`
    /// ——`planDown` 之后、`executeDown` 之前,其中一个 pull 项的本机目录被整个
    /// 删掉(模拟环境被破坏),这一项应该单独 `.failed`,不能带崩其余项目的
    /// `executeDown`。
    func testExecuteDownIsolatesProjectDirectoryRemovedBetweenPlanAndExecute() throws {
        guard locateGitForTest() != nil else {
            throw XCTSkip("git not on PATH — skipping")
        }
        let fixture = try makeCrossMachineFixture()
        defer { fixture.cleanup() }

        let engine = SyncEngine(layout: fixture.bLayout, machineId: "machine-b")
        try convergeBViaCloneItem(fixture: fixture, engine: engine)
        // 此刻 B 的 proj-a 已经收敛(clean,不在 plan 里)。

        // --- 造第二个项目:A 侧再推一个 commit,manifest 更新,让 B 也有一个
        // 真正的 pull 项(不只是 proj-a 那个已经被删的)。---
        try "second-project\n".write(
            to: fixture.aProjLocal.appendingPathComponent("SECOND-PROJECT-SYNC.md"),
            atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: fixture.aProjLocal, message: "another round"))
        try WorkspaceGit.push(at: fixture.aProjLocal, branch: "main")
        let commit2 = try WorkspaceGit.head(at: fixture.aProjLocal)

        guard var updatedProject = try fixture.aLayout.loadProjects()["proj-a"] else {
            return XCTFail("expected proj-a to still be declared in A's manifest")
        }
        updatedProject.lastSync = LastSync(
            machine: "machine-a", branch: "main", head: commit2, pushedAt: "2026-07-20T00:00:00Z")
        try fixture.aLayout.writeProject(id: "proj-a", updatedProject)

        // 再声明一个从没同步过、始终缺目录的项目 `proj-b`(始终是 clone 项)。
        let projBRemoteURL = try makeBareRemote(at: fixture.remotesRoot, name: "proj-b-remote.git")
        let aProjBLocal = fixture.aRoot.appendingPathComponent("proj-b", isDirectory: true)
        try WorkspaceGit.initRepo(at: aProjBLocal)
        try configureIdentity(at: aProjBLocal)
        try WorkspaceGit.setRemote(at: aProjBLocal, url: projBRemoteURL)
        try "hello\n".write(
            to: aProjBLocal.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: aProjBLocal, message: "init"))
        try WorkspaceGit.push(at: aProjBLocal, branch: "main")
        let projBHead = try WorkspaceGit.head(at: aProjBLocal)
        try fixture.aLayout.writeProject(
            id: "proj-b",
            ProjectManifest(
                name: "Proj B", remote: projBRemoteURL, localPath: aProjBLocal.path, defaultBranch: "main",
                lastSync: LastSync(machine: "machine-a", branch: "main", head: projBHead, pushedAt: "2026-07-20T00:00:00Z")
            )
        )

        let workspaceSyncReceipt = WorkspaceRepoService.syncUp(
            layout: fixture.aLayout, message: "chore: record second project + second sync", now: Date.init)
        guard case .uploaded = workspaceSyncReceipt.outcome else {
            return XCTFail("expected workspace manifest push to succeed, got \(workspaceSyncReceipt.outcome)")
        }

        let bProjBLocal = fixture.bRoot.appendingPathComponent("proj-b", isDirectory: true)
        var bMachine = try fixture.bLayout.loadMachine(id: "machine-b") ?? MachineManifest(displayName: "Machine B")
        bMachine.localPathOverrides["proj-b"] = bProjBLocal.path
        try fixture.bLayout.writeMachine(id: "machine-b", bMachine)

        let (_, items) = try engine.planDown()
        XCTAssertTrue(items.contains { $0.id == "proj-a" && $0.actionable }, "proj-a 应该是 pull 项")
        XCTAssertTrue(items.contains { $0.id == "proj-b" && $0.actionable }, "proj-b 应该是 clone 项")

        // plan 之后、执行之前:proj-a 的本机目录被整个删掉(模拟环境被破坏)。
        try FileManager.default.removeItem(at: fixture.bProjLocal)

        var progressed: [SyncReceipt] = []
        let receipts = try engine.executeDown(items: items) { progressed.append($0) }

        guard let aReceipt = receipts.first(where: { $0.itemId == "proj-a" }) else {
            return XCTFail("expected a receipt for proj-a even though its directory vanished")
        }
        guard case .failed = aReceipt.outcome else {
            return XCTFail("expected proj-a to fail (directory vanished), got \(aReceipt.outcome)")
        }

        guard let bReceipt = receipts.first(where: { $0.itemId == "proj-b" }) else {
            return XCTFail("expected a receipt for proj-b despite proj-a failing")
        }
        guard case .pulled = bReceipt.outcome else {
            return XCTFail("expected proj-b to still succeed via clone, got \(bReceipt.outcome)")
        }

        XCTAssertEqual(progressed.count, receipts.count)
    }

    // MARK: - 共用步骤:让 B 通过一次 clone 项收敛到 A 推的 head

    private func convergeBViaCloneItem(fixture: CrossMachineFixture, engine: SyncEngine) throws {
        let (_, items) = try engine.planDown()
        guard items.contains(where: { $0.id == "proj-a" && $0.actionable }) else {
            XCTFail("expected proj-a clone item as a precondition")
            return
        }
        _ = try engine.executeDown(items: items) { _ in }
    }

    // MARK: - fixture helpers(套路照抄 SyncEngineTests,新增跨机器 A/B 结构)

    private struct CrossMachineFixture {
        let remotesRoot: URL
        let aRoot: URL
        let bRoot: URL
        let workspaceRemoteURL: String
        let projRemoteURL: String
        let aLayout: WorkspaceRepoLayout
        let aProjLocal: URL
        let bLayout: WorkspaceRepoLayout
        let bProjLocal: URL
        let aPushedHead: String

        func cleanup() {
            for dir in [remotesRoot, aRoot, bRoot] {
                try? FileManager.default.removeItem(at: dir)
            }
        }
    }

    private enum FixtureError: Error {
        case unexpectedState(String)
    }

    /// 建一套跨机器 fixture:
    /// - 1 个 workspace bare remote + 1 个 proj-a bare remote(都放在独立的
    ///   `remotesRoot`,不属于 A 或 B 任何一方的目录树,模拟"远端")。
    /// - 机器 A:全新建 workspace(`ensure(remoteURL: nil)`)+ 手动 `setRemote`,
    ///   proj-a 本地仓库先推一次 init commit(建立 tracking),再本地追加一个
    ///   未推的 commit,写入项目声明后跑一次完整的 `planUp`/`executeUp`——
    ///   "机器 A executeUp 全绿"的字面要求。
    /// - 机器 B:全新 tmp 目录 + 独立 `machineId`,`ensure(remoteURL:
    ///   workspaceRemoteURL)` clone 已经非空的 workspace 仓库(含 A 写入、推
    ///   送过的 proj-a 声明 + `last_sync`),并注册一条 `machines/machine-b.toml`
    ///   override,把 proj-a 的本机路径指到 B 自己 tmp 树下一个尚不存在的目录
    ///   (确保 `planDown` 判定为"缺目录 → clone",而不是意外撞上 A 的路径)。
    private func makeCrossMachineFixture() throws -> CrossMachineFixture {
        let remotesRoot = tmpDir("syncenginedown-remotes")
        try FileManager.default.createDirectory(at: remotesRoot, withIntermediateDirectories: true)
        let workspaceRemoteURL = try makeBareRemote(at: remotesRoot, name: "workspace-remote.git")
        let projRemoteURL = try makeBareRemote(at: remotesRoot, name: "proj-a-remote.git")

        // --- 机器 A ---
        let aRoot = tmpDir("syncenginedown-machine-a")
        try FileManager.default.createDirectory(at: aRoot, withIntermediateDirectories: true)

        let aWorkspaceRoot = aRoot.appendingPathComponent("workspace", isDirectory: true)
        let aLayout = try WorkspaceRepoService.ensure(at: aWorkspaceRoot, remoteURL: nil, name: "cross-machine-ws")
        try WorkspaceGit.setRemote(at: aWorkspaceRoot, url: workspaceRemoteURL)

        let aProjLocal = aRoot.appendingPathComponent("proj-a", isDirectory: true)
        try WorkspaceGit.initRepo(at: aProjLocal)
        try configureIdentity(at: aProjLocal)
        try WorkspaceGit.setRemote(at: aProjLocal, url: projRemoteURL)
        try "hello\n".write(
            to: aProjLocal.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: aProjLocal, message: "init"))
        try WorkspaceGit.push(at: aProjLocal, branch: "main")
        try "second\n".write(
            to: aProjLocal.appendingPathComponent("SECOND.md"), atomically: true, encoding: .utf8)
        XCTAssertTrue(try WorkspaceGit.commitAll(at: aProjLocal, message: "second, not yet pushed"))

        try aLayout.writeProject(
            id: "proj-a",
            ProjectManifest(
                name: "Proj A", remote: projRemoteURL,
                localPath: aProjLocal.path, defaultBranch: "main"
            )
        )

        let aEngine = SyncEngine(layout: aLayout, machineId: "machine-a")
        let aItems = try aEngine.planUp(fetchFirst: false)
        guard aItems.contains(where: { $0.id == "proj-a" && $0.actionable }) else {
            throw FixtureError.unexpectedState("expected proj-a to be actionable on machine A's planUp")
        }

        let aReceipts = try aEngine.executeUp(items: aItems, wipCommit: false) { _ in }
        guard let aProjReceipt = aReceipts.first(where: { $0.itemId == "proj-a" }),
              case .uploaded = aProjReceipt.outcome
        else {
            throw FixtureError.unexpectedState("expected machine A's executeUp to upload proj-a cleanly")
        }
        guard let aWorkspaceReceipt = aReceipts.first(where: { $0.kind == .workspaceRepo }),
              case .uploaded = aWorkspaceReceipt.outcome
        else {
            throw FixtureError.unexpectedState("expected machine A's executeUp to push the workspace repo cleanly")
        }

        let aPushedHead = try WorkspaceGit.head(at: aProjLocal)

        // --- 机器 B:全新 tmp + 独立 machineId,clone 已经非空的 workspace 仓库 ---
        let bRoot = tmpDir("syncenginedown-machine-b")
        try FileManager.default.createDirectory(at: bRoot, withIntermediateDirectories: true)

        let bWorkspaceRoot = bRoot.appendingPathComponent("workspace", isDirectory: true)
        let bLayout = try WorkspaceRepoService.ensure(
            at: bWorkspaceRoot, remoteURL: workspaceRemoteURL, name: "cross-machine-ws")

        let bProjLocal = bRoot.appendingPathComponent("proj-a", isDirectory: true)
        try bLayout.writeMachine(
            id: "machine-b",
            MachineManifest(displayName: "Machine B", localPathOverrides: ["proj-a": bProjLocal.path])
        )

        return CrossMachineFixture(
            remotesRoot: remotesRoot, aRoot: aRoot, bRoot: bRoot,
            workspaceRemoteURL: workspaceRemoteURL, projRemoteURL: projRemoteURL,
            aLayout: aLayout, aProjLocal: aProjLocal,
            bLayout: bLayout, bProjLocal: bProjLocal,
            aPushedHead: aPushedHead
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
