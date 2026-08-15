#if os(macOS)
import XCTest
// 同 WorkspaceRepoLayoutTests:源码直接编进 test bundle,internal API 可见。

/// `MachineRegistration` 的纯函数单测——不碰 git,只测「写 machine manifest」
/// 这一步的落盘/幂等行为(Task 9 brief:注册后文件存在/displayName 非空/幂等/
/// overrides 保留)。
final class WorkspaceSetupTests: XCTestCase {
    private var tmpRoot: URL!

    override func setUpWithError() throws {
        tmpRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: tmpRoot)
    }

    /// 注册后 `machines/<machineId>.toml` 存在,且 displayName 非空、如实等于
    /// 传入值。
    func testRegisterWritesMachineFileWithDisplayName() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "ws")

        try MachineRegistration.register(
            layout: layout, machineId: "mac-1", displayName: "小明的 Mac")

        let loaded = try layout.loadMachine(id: "mac-1")
        XCTAssertNotNil(loaded)
        XCTAssertEqual(loaded?.displayName, "小明的 Mac")
        XCTAssertFalse(loaded?.displayName.isEmpty ?? true)
        XCTAssertEqual(loaded?.localPathOverrides, [:])
    }

    /// 幂等:该机器已注册过(带一些 override)——再次 register 只更新
    /// displayName,不动已有 overrides。
    func testRegisterIsIdempotentAndPreservesOverrides() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "ws")
        try layout.writeMachine(
            id: "mac-1",
            MachineManifest(
                displayName: "旧名字",
                localPathOverrides: ["proj-a": "~/custom/proj-a"]))

        try MachineRegistration.register(
            layout: layout, machineId: "mac-1", displayName: "新名字")

        let loaded = try layout.loadMachine(id: "mac-1")
        XCTAssertEqual(loaded?.displayName, "新名字", "displayName 应该被更新")
        XCTAssertEqual(
            loaded?.localPathOverrides, ["proj-a": "~/custom/proj-a"],
            "已有 overrides 不该被幂等注册抹掉")
    }

    /// 不同 machineId 各自独立落盘,互不覆盖。
    func testRegisterDifferentMachineIdsDoNotCollide() throws {
        let layout = try WorkspaceRepoLayout.scaffold(at: tmpRoot, name: "ws")

        try MachineRegistration.register(layout: layout, machineId: "mac-1", displayName: "机器一")
        try MachineRegistration.register(layout: layout, machineId: "mac-2", displayName: "机器二")

        XCTAssertEqual(try layout.loadMachine(id: "mac-1")?.displayName, "机器一")
        XCTAssertEqual(try layout.loadMachine(id: "mac-2")?.displayName, "机器二")
    }
}
#endif
