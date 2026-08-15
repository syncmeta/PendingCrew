import XCTest

final class WorkspaceManifestTests: XCTestCase {
    func testProjectManifestRoundtrip() throws {
        let project = ProjectManifest(
            name: "pendingbot",
            remote: "git@github.com:you/pendingbot.git",
            localPath: "~/Untitled/PendingBot",
            defaultBranch: "main",
            lastSync: LastSync(machine: "studio-mac", branch: "main",
                               head: "2e3939f9", pushedAt: "2026-07-19T09:30:00Z"))
        let toml = try WorkspaceManifestCodec.encode(project)
        XCTAssertTrue(toml.contains("local_path"))         // snake_case 落盘
        let back = try WorkspaceManifestCodec.decode(ProjectManifest.self, toml: toml)
        XCTAssertEqual(back, project)
    }

    func testProjectManifestWithoutLastSyncDecodes() throws {
        let toml = """
        name = "pendingbot"
        remote = "git@github.com:you/pendingbot.git"
        local_path = "~/x"
        default_branch = "main"
        """
        let p = try WorkspaceManifestCodec.decode(ProjectManifest.self, toml: toml)
        XCTAssertNil(p.lastSync)
    }

    func testWorkspaceManifestRoundtrip() throws {
        let ws = WorkspaceManifest(name: "home-studio", schemaVersion: 1)
        let back = try WorkspaceManifestCodec.decode(
            WorkspaceManifest.self, toml: try WorkspaceManifestCodec.encode(ws))
        XCTAssertEqual(back, ws)
    }

    func testMachineManifestOverrides() throws {
        let m = MachineManifest(displayName: "home-mac",
                                agePublicKeyFingerprint: nil,
                                localPathOverrides: ["pendingbot": "~/Code/pb"])
        let back = try WorkspaceManifestCodec.decode(
            MachineManifest.self, toml: try WorkspaceManifestCodec.encode(m))
        XCTAssertEqual(back.localPathOverrides["pendingbot"], "~/Code/pb")
    }
}
