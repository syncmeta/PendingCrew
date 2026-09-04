import XCTest

/// 数据根的唯一真值。
///
/// **这一组里最重要的是第一条**：不设环境变量时它必须逐字等于原来那七处各自算出来的
/// 路径。改坏默认解析 → 所有 store 一起读错账本，**而且不会响**（CONTRIBUTING 第 4 条
/// 那种形状：生产上 `--dir` 恰好等于默认值，写错了看不出来）。所以这条断言写的是
/// 一个**字面路径**，不是「跟 `PendingCrewDataRoot.url` 一致」那种自证。
final class PendingCrewDataRootTests: XCTestCase {

    private let support = URL(fileURLWithPath: "/Users/someone/Library/Application Support")

    func test_不设环境变量时逐字等于原来那条路径() {
        XCTAssertEqual(
            PendingCrewDataRoot.resolve(environment: [:], applicationSupport: support).path,
            "/Users/someone/Library/Application Support/PendingCrew")
    }

    func test_子目录名与搬家前一致() {
        let root = PendingCrewDataRoot.resolve(environment: [:], applicationSupport: support)
        XCTAssertEqual(root.appendingPathComponent("whiteboards", isDirectory: true).path,
                       "/Users/someone/Library/Application Support/PendingCrew/whiteboards")
        XCTAssertEqual(root.appendingPathComponent("attachments", isDirectory: true).path,
                       "/Users/someone/Library/Application Support/PendingCrew/attachments")
        XCTAssertEqual(root.appendingPathComponent("crashes", isDirectory: true).path,
                       "/Users/someone/Library/Application Support/PendingCrew/crashes")
    }

    func test_设了就整个挪走() {
        XCTAssertEqual(
            PendingCrewDataRoot.resolve(
                environment: [PendingCrewDataRoot.overrideEnvKey: "/tmp/pcrew-iso"],
                applicationSupport: support).path,
            "/tmp/pcrew-iso")
    }

    /// 空串 / 全空白**不算设了**。误设成空串时静默换到某个奇怪的地方，比报错难查得多。
    func test_空串和空白不算设了() {
        for value in ["", "   ", "\t\n"] {
            XCTAssertEqual(
                PendingCrewDataRoot.resolve(
                    environment: [PendingCrewDataRoot.overrideEnvKey: value],
                    applicationSupport: support).path,
                "/Users/someone/Library/Application Support/PendingCrew",
                "「\(value)」不该被当成有效覆盖")
        }
    }

    func test_波浪号展开() {
        let path = PendingCrewDataRoot.resolve(
            environment: [PendingCrewDataRoot.overrideEnvKey: "~/pcrew-iso"],
            applicationSupport: support).path
        XCTAssertFalse(path.hasPrefix("~"), "波浪号没展开会造出一个名叫 `~` 的目录：\(path)")
        XCTAssertTrue(path.hasSuffix("/pcrew-iso"), path)
    }

    /// `applicationSupport` 取不到时的兜底 —— 与被取代的那七处一致（临时目录）。
    func test_取不到AppSupport时退到临时目录() {
        let path = PendingCrewDataRoot.resolve(environment: [:], applicationSupport: nil).path
        XCTAssertTrue(path.hasSuffix("/PendingCrew"), path)
        XCTAssertNotEqual(path, "/PendingCrew")
    }

    /// 真实进程里那一份（`static let`，解析一次）也得指向数据根 —— 上面全是纯函数，
    /// 这条盯的是**接线接对了没有**。
    func test_进程里那份与纯判定一致() {
        XCTAssertEqual(
            PendingCrewDataRoot.url.path,
            PendingCrewDataRoot.resolve(
                environment: ProcessInfo.processInfo.environment,
                applicationSupport: FileManager.default.urls(
                    for: .applicationSupportDirectory, in: .userDomainMask).first).path)
        XCTAssertEqual(LocalWhiteboardStore.defaultDirectory.path,
                       PendingCrewDataRoot.url.appendingPathComponent("whiteboards").path)
    }
}
