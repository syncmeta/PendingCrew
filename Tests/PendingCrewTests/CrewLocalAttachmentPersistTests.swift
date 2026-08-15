import XCTest

/// 托盘待发附件 → 落盘条目（Todo #52 把这条路从 `CrewChatView.send` 抽出来，
/// 群聊 / 新建 Todo / Todo 追问三个入口共用）。跑的是生产类型本身。
final class CrewLocalAttachmentPersistTests: XCTestCase {

    private var root: URL!
    private var staging: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("CrewPersistTests-\(UUID().uuidString)", isDirectory: true)
        staging = root.appendingPathComponent("staging", isDirectory: true)
        try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func stagedFile(_ name: String, bytes: Int = 8) throws -> URL {
        let src = root.appendingPathComponent(name)
        try Data(repeating: 0x41, count: bytes).write(to: src)
        return try CrewChatAttachmentStore.stage(fileAt: src, into: staging)
    }

    func testPersistsInMemoryBytesAndKeepsOrder() throws {
        let items = [
            PendingComposerAttachment(filename: "pasted-image.png", mime: "image/png",
                                      data: Data([0x89, 0x50])),
            PendingComposerAttachment(filename: "note.txt", mime: "text/plain",
                                      data: Data("hi".utf8)),
        ]
        let saved = try CrewLocalAttachmentPersist.persist(items, crewId: "c", root: root)
        XCTAssertEqual(saved.map(\.mime), ["image/png", "text/plain"])
        // 图不记原名（气泡直接渲染缩略图）；文件留原名给 chip 显示。
        XCTAssertNil(saved[0].filename)
        XCTAssertEqual(saved[1].filename, "note.txt")
        // 落进与群聊同一个 `<root>/<crewId>/` 目录，路径是绝对路径（agent 要 Read 它）。
        for att in saved {
            XCTAssertTrue(att.path.hasPrefix(root.appendingPathComponent("c").path + "/"), att.path)
            XCTAssertTrue(FileManager.default.fileExists(atPath: att.path))
        }
        XCTAssertTrue(saved[0].path.hasSuffix(".png"))
    }

    func testPersistsStagedFileByMovingIt() throws {
        let staged = try stagedFile("shot.png", bytes: 32)
        let item = PendingComposerAttachment(
            filename: "shot.png", mime: "image/png", stagedAt: staged, byteSize: 32)
        let saved = try CrewLocalAttachmentPersist.persist([item], crewId: "c", root: root)
        XCTAssertEqual(saved.count, 1)
        XCTAssertEqual(saved[0].size, 32)
        XCTAssertTrue(FileManager.default.fileExists(atPath: saved[0].path))
        // 搬走了，不是复制 —— 暂存那份不该还在。
        XCTAssertFalse(FileManager.default.fileExists(atPath: staged.path))
        // 清场把空壳目录也收掉。
        CrewLocalAttachmentPersist.discardStaging([item], staging: staging)
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: staged.deletingLastPathComponent().path))
    }

    func testThrowsWithFilenameWhenNothingToSave() {
        // 暂存文件被人在「拖进来」和「点发送」之间挪走了 —— 必须报出来，
        // 不许静默少一张（附件是人要给 agent 看的东西）。
        let ghost = staging.appendingPathComponent("gone/ghost.png")
        let item = PendingComposerAttachment(
            filename: "ghost.png", mime: "image/png", stagedAt: ghost, byteSize: 4)
        XCTAssertThrowsError(
            try CrewLocalAttachmentPersist.persist([item], crewId: "c", root: root)
        ) { error in
            XCTAssertEqual((error as? CrewLocalAttachmentPersist.Failure)?.filename, "ghost.png")
            XCTAssertEqual(error.localizedDescription, "附件保存失败：ghost.png")
        }
    }

    func testEmptyInputYieldsEmptyOutput() throws {
        XCTAssertTrue(try CrewLocalAttachmentPersist.persist([], crewId: "c", root: root).isEmpty)
    }
}
