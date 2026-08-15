import XCTest
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
// CrewFileAttachmentIntake / CrewChatAttachmentStore / PendingComposerAttachment
// 都直接编进 PendingCrewTests target —— 这里跑的就是线上那份，不是复刻件。

/// 「把文件拖进群聊」这条路的守卫。
///
/// 覆盖三层：
/// ① 纯判定（收不收：目录/package、超限、空文件）；
/// ② 真碰磁盘的 intake（原样复制进暂存区、**整份字节不进内存**、逐条软报错）；
/// ③ 落盘（`save(fileAt:)` 搬文件、扩展名推导、暂存清场）。
///
/// 跑的是生产类型本身；测不到的只剩 SwiftUI 那层 `.onDrop` 接线（拖拽手势进不了
/// 单测），那部分明确交给人手验（QA #443）。
final class CrewFileAttachmentIntakeTests: XCTestCase {

    private var root: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("CrewFileIntakeTests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    // MARK: - helpers

    private func makeFile(_ name: String, bytes: Int = 8) throws -> URL {
        let url = root.appendingPathComponent(name)
        try Data(repeating: 0x41, count: bytes).write(to: url)
        return url
    }

    private func makeDirectory(_ name: String) throws -> URL {
        let url = root.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    /// 真 PNG（缩略图那条路要能被 ImageIO 认出来，随手拿几个字节冒充不行）。
    private func makePNG(_ name: String, side: Int = 24) throws -> URL {
        let url = root.appendingPathComponent(name)
        let cs = CGColorSpace(name: CGColorSpace.sRGB)!
        let ctx = CGContext(
            data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
            space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.setFillColor(CGColor(srgbRed: 0.2, green: 0.6, blue: 0.9, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: side, height: side))
        let image = ctx.makeImage()!
        let dest = CGImageDestinationCreateWithURL(
            url as CFURL, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(dest, image, nil)
        XCTAssertTrue(CGImageDestinationFinalize(dest))
        return url
    }

    private func staging() -> URL {
        root.appendingPathComponent("staging", isDirectory: true)
    }

    // MARK: - ① 纯判定

    func testAcceptsOrdinaryFile() {
        let c = CrewFileCandidate(
            filename: "spec.pdf", byteSize: 1024, isDirectory: false, mime: "application/pdf")
        XCTAssertEqual(CrewFileAttachmentIntake.verdict(for: c), .accept)
        XCTAssertNil(CrewFileAttachmentIntake.rejectionMessage(.accept, filename: "spec.pdf"))
    }

    /// `.item` 口径放开后 `.key` / `.app` / `.xcodeproj` 能被选中/拖进来，
    /// 但它们是目录 —— 必须明确拒，不许静默产出一个坏附件。
    func testRejectsDirectoryAndPackage() {
        let pkg = CrewFileCandidate(
            filename: "Deck.key", byteSize: 4096, isDirectory: true,
            mime: "application/x-iwork-keynote-sffkey")
        XCTAssertEqual(CrewFileAttachmentIntake.verdict(for: pkg), .rejectDirectory)
        let msg = CrewFileAttachmentIntake.rejectionMessage(.rejectDirectory, filename: "Deck.key")
        XCTAssertNotNil(msg)
        XCTAssertTrue(msg!.contains("Deck.key"))
        XCTAssertTrue(msg!.contains("zip"))
    }

    func testRejectsOverLimitAndReportsTheLimit() {
        let limit = 512 * 1024 * 1024
        let big = CrewFileCandidate(
            filename: "recording.mov", byteSize: limit + 1, isDirectory: false,
            mime: "video/quicktime")
        XCTAssertEqual(
            CrewFileAttachmentIntake.verdict(for: big, limit: limit),
            .rejectTooLarge(limit: limit))
        let msg = CrewFileAttachmentIntake.rejectionMessage(
            .rejectTooLarge(limit: limit), filename: "recording.mov")
        // fail-loud：文案里必须同时有文件名和上限，人才知道发生了什么。
        XCTAssertTrue(msg!.contains("recording.mov"))
        XCTAssertTrue(msg!.contains("512 MB"))
    }

    /// 边界：正好等于上限收下，超一个字节才拒。
    func testLimitBoundaryIsInclusive() {
        let limit = 1000
        let exact = CrewFileCandidate(
            filename: "a.bin", byteSize: limit, isDirectory: false, mime: "application/octet-stream")
        XCTAssertEqual(CrewFileAttachmentIntake.verdict(for: exact, limit: limit), .accept)
    }

    func testRejectsEmptyOrUnreadable() {
        let empty = CrewFileCandidate(
            filename: "x.txt", byteSize: 0, isDirectory: false, mime: "text/plain")
        XCTAssertEqual(CrewFileAttachmentIntake.verdict(for: empty), .rejectUnreadable)
        XCTAssertNotNil(
            CrewFileAttachmentIntake.rejectionMessage(.rejectUnreadable, filename: "x.txt"))
    }

    func testByteTextUsesBinaryUnits() {
        XCTAssertEqual(CrewFileAttachmentIntake.byteText(512 * 1024 * 1024), "512 MB")
        XCTAssertEqual(CrewFileAttachmentIntake.byteText(25 * 1024 * 1024), "25 MB")
        XCTAssertEqual(CrewFileAttachmentIntake.byteText(2 * 1024 * 1024 * 1024), "2 GB")
    }

    /// macOS 本地落盘没有服务端把关，这条常量就是唯一的闸门 —— 别把它改没了。
    func testLocalLimitExistsAndIsSane() {
        XCTAssertEqual(CrewFileAttachmentIntake.maxBytes, 512 * 1024 * 1024)
    }

    // MARK: - ② describe：URL → 候选

    func testDescribeReadsSizeAndMime() throws {
        let url = try makeFile("notes.txt", bytes: 33)
        let c = CrewFileAttachmentIntake.describe(fileAt: url)
        XCTAssertEqual(c.filename, "notes.txt")
        XCTAssertEqual(c.byteSize, 33)
        XCTAssertFalse(c.isDirectory)
        XCTAssertEqual(c.mime, "text/plain")
    }

    func testDescribeFlagsDirectory() throws {
        let url = try makeDirectory("Deck.key")
        XCTAssertTrue(CrewFileAttachmentIntake.describe(fileAt: url).isDirectory)
    }

    func testDescribeFallsBackToOctetStreamForUnknownExtension() throws {
        let url = try makeFile("weird.zzznotathing")
        XCTAssertEqual(
            CrewFileAttachmentIntake.describe(fileAt: url).mime, "application/octet-stream")
    }

    // MARK: - ③ intake：复制进暂存区 + 逐条软报错

    func testIntakeStagesFileWithoutTouchingOriginal() throws {
        let src = try makeFile("report.pdf", bytes: 64)
        let result = CrewFileAttachmentIntake.intake(urls: [src], staging: staging())

        XCTAssertTrue(result.errors.isEmpty)
        XCTAssertEqual(result.accepted.count, 1)
        let att = result.accepted[0]
        XCTAssertEqual(att.filename, "report.pdf")
        XCTAssertEqual(att.mime, "application/pdf")
        XCTAssertEqual(att.byteSize, 64)
        // 关键：字节**没有**被读进内存 —— 托盘拿的是路径，不是 Data。
        XCTAssertNil(att.inMemoryData)
        let staged = try XCTUnwrap(att.stagedURL)
        XCTAssertTrue(FileManager.default.fileExists(atPath: staged.path))
        XCTAssertTrue(staged.path.hasPrefix(staging().path))
        // 原文件原地不动（拖进来不等于搬走用户的文件）。
        XCTAssertTrue(FileManager.default.fileExists(atPath: src.path))
    }

    func testIntakeTakesMultipleFilesAtOnce() throws {
        let a = try makeFile("a.png", bytes: 4)
        let b = try makeFile("b.txt", bytes: 4)
        let result = CrewFileAttachmentIntake.intake(urls: [a, b], staging: staging())
        XCTAssertEqual(result.accepted.map(\.filename), ["a.png", "b.txt"])
        XCTAssertTrue(result.errors.isEmpty)
    }

    /// 超限的那份被拒且**有话说**，同一批里合规的照收 —— 不整批丢弃、也不静默。
    func testIntakeRejectsOversizeButKeepsTheRest() throws {
        let big = try makeFile("big.bin", bytes: 4096)
        let small = try makeFile("small.bin", bytes: 16)
        let result = CrewFileAttachmentIntake.intake(
            urls: [big, small], limit: 1024, staging: staging())

        XCTAssertEqual(result.accepted.map(\.filename), ["small.bin"])
        XCTAssertEqual(result.errors.count, 1)
        XCTAssertTrue(result.errors[0].contains("big.bin"))
        // 超限的那份一个字节都不该被复制进暂存区。
        let stagedNames = (try? FileManager.default.subpathsOfDirectory(atPath: staging().path)) ?? []
        XCTAssertFalse(stagedNames.contains { $0.hasSuffix("big.bin") })
    }

    func testIntakeRejectsDirectoryWithMessage() throws {
        let dir = try makeDirectory("Project.xcodeproj")
        let result = CrewFileAttachmentIntake.intake(urls: [dir], staging: staging())
        XCTAssertTrue(result.accepted.isEmpty)
        XCTAssertEqual(result.errors.count, 1)
        XCTAssertTrue(result.errors[0].contains("Project.xcodeproj"))
    }

    func testIntakeMakesThumbnailForImagesOnly() throws {
        let png = try makePNG("shot.png")
        let txt = try makeFile("notes.txt", bytes: 12)
        let result = CrewFileAttachmentIntake.intake(urls: [png, txt], staging: staging())
        XCTAssertEqual(result.accepted.count, 2)
        XCTAssertNotNil(result.accepted[0].previewData, "图片要有托盘缩略图")
        XCTAssertTrue(result.accepted[0].isImage)
        XCTAssertNil(result.accepted[1].previewData, "非图片不做缩略图")
    }

    /// 缩略图是**缩**出来的，不是把原图整份塞进内存。
    func testThumbnailIsSmallerThanSource() throws {
        let png = try makePNG("big.png", side: 900)
        let source = try Data(contentsOf: png)
        let thumb = try XCTUnwrap(CrewFileAttachmentIntake.thumbnailPNG(fileAt: png, maxPixel: 64))
        XCTAssertLessThan(thumb.count, source.count)
    }

    func testThumbnailNilForNonImage() throws {
        let txt = try makeFile("notes.txt", bytes: 12)
        XCTAssertNil(CrewFileAttachmentIntake.thumbnailPNG(fileAt: txt))
    }

    // MARK: - ④ 落盘：save(fileAt:) 搬文件

    func testSaveFileAtMovesStagedFileIntoCrewDirectory() throws {
        let src = try makeFile("report.pdf", bytes: 64)
        let staged = try CrewChatAttachmentStore.stage(fileAt: src, into: staging())
        let store = root.appendingPathComponent("attachments", isDirectory: true)

        let saved = try XCTUnwrap(CrewChatAttachmentStore.save(
            fileAt: staged, mime: "application/pdf", filename: "report.pdf",
            crewId: "crew-1", root: store))

        XCTAssertEqual(saved.size, 64)
        XCTAssertEqual(saved.mime, "application/pdf")
        XCTAssertEqual(saved.filename, "report.pdf")
        XCTAssertTrue(saved.path.contains("/crew-1/"))
        XCTAssertTrue(saved.path.hasSuffix(".pdf"))
        XCTAssertTrue(FileManager.default.fileExists(atPath: saved.path))
        // move 而不是 copy —— 暂存那份该走掉，不留一份重复的字节。
        XCTAssertFalse(FileManager.default.fileExists(atPath: staged.path))
    }

    func testSaveFileAtReturnsNilWhenSourceIsGone() throws {
        let missing = root.appendingPathComponent("nope.bin")
        XCTAssertNil(CrewChatAttachmentStore.save(
            fileAt: missing, mime: "application/octet-stream", crewId: "crew-1",
            root: root.appendingPathComponent("attachments", isDirectory: true)))
    }

    /// 图片走 `IMG-` 前缀、其它走 `ATT-`（气泡渲染与人眼辨识都靠这个）。
    func testDiskNamePrefixSplitsImagesFromFiles() throws {
        let store = root.appendingPathComponent("attachments", isDirectory: true)
        let img = try XCTUnwrap(CrewChatAttachmentStore.save(
            data: Data([1, 2, 3]), mime: "image/png", crewId: "c", root: store))
        let doc = try XCTUnwrap(CrewChatAttachmentStore.save(
            data: Data([1, 2, 3]), mime: "application/pdf", filename: "a.pdf",
            crewId: "c", root: store))
        XCTAssertTrue((img.path as NSString).lastPathComponent.hasPrefix("IMG-"))
        XCTAssertTrue((doc.path as NSString).lastPathComponent.hasPrefix("ATT-"))
    }

    /// 扩展名口径：已知 mime 走映射；未知 mime 退回原文件名的扩展名；再兜底 bin。
    func testExtensionDerivation() throws {
        let store = root.appendingPathComponent("attachments", isDirectory: true)
        func ext(mime: String, filename: String?) throws -> String {
            let saved = try XCTUnwrap(CrewChatAttachmentStore.save(
                data: Data([9]), mime: mime, filename: filename, crewId: "c", root: store))
            return (saved.path as NSString).pathExtension
        }
        XCTAssertEqual(try ext(mime: "image/jpeg", filename: nil), "jpg")
        XCTAssertEqual(try ext(mime: "application/pdf", filename: nil), "pdf")
        XCTAssertEqual(try ext(mime: "application/zip", filename: "bundle.ZIP"), "zip")
        XCTAssertEqual(try ext(mime: "application/octet-stream", filename: nil), "bin")
        XCTAssertEqual(try ext(mime: "image/x-weird", filename: nil), "png")
    }

    // MARK: - ⑤ 暂存清场

    func testDiscardStagedRemovesTheBox() throws {
        let src = try makeFile("a.txt", bytes: 4)
        let staged = try CrewChatAttachmentStore.stage(fileAt: src, into: staging())
        CrewChatAttachmentStore.discardStaged(staged, staging: staging())
        XCTAssertFalse(FileManager.default.fileExists(atPath: staged.path))
    }

    /// 别让它变成「能删任意路径」的口子：暂存区外的路径一概不碰。
    func testDiscardStagedIgnoresPathsOutsideStaging() throws {
        let outsider = try makeFile("precious.txt", bytes: 4)
        CrewChatAttachmentStore.discardStaged(outsider, staging: staging())
        XCTAssertTrue(FileManager.default.fileExists(atPath: outsider.path))
    }
}
