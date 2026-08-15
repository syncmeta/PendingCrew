import XCTest

final class WorkspaceRepoLayoutTests: XCTestCase {
    private var tmpRoot: URL!

    override func setUpWithError() throws {
        tmpRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: tmpRoot)
    }

    func testScaffoldThenLoadWorkspaceRoundtrips() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")

        // 目录骨架都建出来了
        for sub in ["projects", "crews", "env", "secrets", "machines"] {
            var isDir: ObjCBool = false
            let exists = FileManager.default.fileExists(
                atPath: tmpRoot.appendingPathComponent(sub).path, isDirectory: &isDir)
            XCTAssertTrue(exists && isDir.boolValue, "\(sub) 目录应存在")
        }

        let ws = try layout.loadWorkspace()
        XCTAssertEqual(ws.name, "home-studio")
        XCTAssertEqual(ws.schemaVersion, 1)
    }

    func testLoadWorkspaceOnNonWorkspaceDirThrowsNotAWorkspace() throws {
        try FileManager.default.createDirectory(at: tmpRoot, withIntermediateDirectories: true)
        let layout = WorkspaceRepoLayout(root: tmpRoot)

        XCTAssertThrowsError(try layout.loadWorkspace()) { error in
            guard case WorkspaceRepoLayout.LayoutError.notAWorkspace(let url) = error else {
                XCTFail("expected notAWorkspace, got \(error)")
                return
            }
            XCTAssertEqual(url, tmpRoot)
        }
    }

    func testLoadProjectsOnMissingDirectoryReturnsEmpty() throws {
        try FileManager.default.createDirectory(at: tmpRoot, withIntermediateDirectories: true)
        let layout = WorkspaceRepoLayout(root: tmpRoot)

        let projects = try layout.loadProjects()
        XCTAssertEqual(projects, [:])
    }

    func testWriteProjectThenLoadProjectsRoundtrips() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")
        let project = ProjectManifest(
            name: "pendingbot",
            remote: "git@github.com:you/pendingbot.git",
            localPath: "~/Untitled/PendingBot",
            defaultBranch: "main")

        try layout.writeProject(id: "pendingbot", project)

        let projects = try layout.loadProjects()
        XCTAssertEqual(projects["pendingbot"], project)
    }

    func testWriteMachineThenLoadMachineRoundtrips() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")
        let machine = MachineManifest(
            displayName: "home-mac",
            agePublicKeyFingerprint: nil,
            localPathOverrides: ["pendingbot": "~/Code/pb"])

        try layout.writeMachine(id: "home-mac-id", machine)

        let loaded = try layout.loadMachine(id: "home-mac-id")
        XCTAssertEqual(loaded, machine)
    }

    func testLoadMachineReturnsNilWhenMissing() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")
        XCTAssertNil(try layout.loadMachine(id: "does-not-exist"))
    }

    func testEffectiveLocalPathUsesMachineOverrideWhenPresent() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")
        let project = ProjectManifest(
            name: "pendingbot",
            remote: "git@github.com:you/pendingbot.git",
            localPath: "~/default/path",
            defaultBranch: "main")
        try layout.writeProject(id: "pendingbot", project)
        try layout.writeMachine(
            id: "home-mac-id",
            MachineManifest(displayName: "home-mac",
                             localPathOverrides: ["pendingbot": "~/override/path"]))

        let resolved = try layout.effectiveLocalPath(
            projectId: "pendingbot", project: project, machineId: "home-mac-id")

        XCTAssertEqual(
            resolved.path,
            (("~/override/path" as NSString).expandingTildeInPath))
    }

    func testEffectiveLocalPathFallsBackToLocalPathWhenNoOverride() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")
        let project = ProjectManifest(
            name: "pendingbot",
            remote: "git@github.com:you/pendingbot.git",
            localPath: "~/default/path",
            defaultBranch: "main")
        try layout.writeProject(id: "pendingbot", project)
        try layout.writeMachine(
            id: "home-mac-id",
            MachineManifest(displayName: "home-mac", localPathOverrides: [:]))

        let resolved = try layout.effectiveLocalPath(
            projectId: "pendingbot", project: project, machineId: "home-mac-id")

        XCTAssertEqual(
            resolved.path,
            (("~/default/path" as NSString).expandingTildeInPath))
    }

    func testEffectiveLocalPathFallsBackWhenMachineFileMissing() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")
        let project = ProjectManifest(
            name: "pendingbot",
            remote: "git@github.com:you/pendingbot.git",
            localPath: "~/default/path",
            defaultBranch: "main")
        try layout.writeProject(id: "pendingbot", project)

        let resolved = try layout.effectiveLocalPath(
            projectId: "pendingbot", project: project, machineId: "unknown-machine")

        XCTAssertEqual(
            resolved.path,
            (("~/default/path" as NSString).expandingTildeInPath))
    }

    func testWriteWorkspaceThenLoadWorkspaceRoundtrips() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "home-studio")
        let updated = WorkspaceManifest(name: "renamed-studio", schemaVersion: 2)

        try layout.writeWorkspace(updated)

        let loaded = try layout.loadWorkspace()
        XCTAssertEqual(loaded, updated)
    }
}
