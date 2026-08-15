import XCTest
import AppKit
// CrewPasteAttachments + PendingComposerAttachment 直接编进 PendingCrewTests target。

/// Todo #9「群聊粘不了图」的两半：
/// ① 剪贴板快照 → 附件的优先级判定（文件先于位图 —— Finder 拷文件会同时带图标位图）；
/// ② **病根守卫**：⌘V 能不能派发下来，取决于文本框声明的 `readablePasteboardTypes`
///    与剪贴板类型有没有交集。纯文本 NSTextView 的默认清单里一个图片类型都没有，
///    所以「剪贴板只有截图」时系统直接把粘贴项置灰、⌘V 毫无反应，composer 里那个
///    paste 拦截器永远等不到调用。谁把 `macPasteboardTypes` 改回去，界面上看不出
///    任何异常 —— 只是又粘不了图，所以这里钉死。
final class CrewPasteAttachmentsTests: XCTestCase {

    private func png(_ byte: UInt8 = 0x89) -> Data { Data([byte, 0x50, 0x4E, 0x47]) }

    // MARK: - ① 快照 → 附件

    func testBitmapOnlyBecomesSinglePNGAttachment() {
        let out = CrewPasteAttachments.attachments(
            from: CrewPasteboardSnapshot(imagePNG: png()))
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out.first?.mime, "image/png")
        XCTAssertEqual(out.first?.filename, "pasted-image.png")
        XCTAssertTrue(out.first?.isImage == true)
    }

    /// Finder 拷一个 PDF 会**同时**往剪贴板放文件 URL 和一张图标位图。先判位图
    /// 就会把 PDF 误收成 pasted-image.png —— 文件必须优先。
    func testFileWinsOverIconBitmap() {
        let snapshot = CrewPasteboardSnapshot(
            files: [.init(filename: "spec.pdf", data: Data([1, 2, 3]), mime: "application/pdf")],
            imagePNG: png())
        let out = CrewPasteAttachments.attachments(from: snapshot)
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out.first?.filename, "spec.pdf")
        XCTAssertEqual(out.first?.mime, "application/pdf")
    }

    func testMultipleFilesAllCollected() {
        let snapshot = CrewPasteboardSnapshot(files: [
            .init(filename: "a.png", data: Data([1]), mime: "image/png"),
            .init(filename: "b.txt", data: Data([2]), mime: "text/plain"),
        ])
        XCTAssertEqual(
            CrewPasteAttachments.attachments(from: snapshot).map(\.filename),
            ["a.png", "b.txt"])
    }

    func testUnknownMimeFallsBackToOctetStream() {
        let snapshot = CrewPasteboardSnapshot(
            files: [.init(filename: "weird.qzx", data: Data([1]), mime: nil)])
        XCTAssertEqual(
            CrewPasteAttachments.attachments(from: snapshot).first?.mime,
            "application/octet-stream")
    }

    /// 空 = 宿主不吞这次 paste，走默认文本粘贴（纯文字剪贴板不能被吞掉）。
    func testEmptySnapshotYieldsNothing() {
        XCTAssertTrue(CrewPasteAttachments.attachments(from: CrewPasteboardSnapshot()).isEmpty)
        // 读得到文件 URL 但字节是空的（读失败）也不算附件。
        let emptyFile = CrewPasteboardSnapshot(
            files: [.init(filename: "x.png", data: Data(), mime: "image/png")])
        XCTAssertTrue(CrewPasteAttachments.attachments(from: emptyFile).isEmpty)
    }

    func testMimeForExtension() {
        XCTAssertEqual(CrewPasteAttachments.mime(forExtension: "png"), "image/png")
        XCTAssertEqual(CrewPasteAttachments.mime(forExtension: "pdf"), "application/pdf")
        XCTAssertEqual(
            CrewPasteAttachments.mime(forExtension: "zzznotathing"),
            "application/octet-stream")
    }

    // MARK: - ② 病根守卫：⌘V 派发的前置条件

    /// 与 `ComposerNSTextView` 的 override **同形**的探针。
    ///
    /// 为什么是复刻而不是直接测那个类：`ComposerNSTextView` 住在 ComposerView.swift
    /// 里，那文件拖着 Theme / ChatActionSheet 一整串 SwiftUI 依赖，进不了这个
    /// test bundle。所以这里钉的是「清单内容 + AppKit 的校验行为」，**钉不住
    /// 「那个 override 还在」** —— 有人把 override 删了这里照样绿。活体那半归
    /// QA（真机 ⌘V 粘图）。
    private final class PasteProbeTextView: NSTextView {
        override var readablePasteboardTypes: [NSPasteboard.PasteboardType] {
            super.readablePasteboardTypes + CrewPasteAttachments.macPasteboardTypes
        }
    }

    /// AppKit 校验「编辑▸粘贴」的那步：`preferredPasteboardType(from:
    /// restrictedToTypesFrom:)` 为 nil ⇒ 菜单项置灰 ⇒ ⌘V 根本不派发。
    /// 注意这个方法**认收信人**：同样传 `[.png]`，只声明了文本类型的视图返回
    /// nil，声明了图片类型的视图返回 public.png —— 所以必须在各自的实例上问。
    private func pasteDispatches(_ tv: NSTextView, clipboard: [NSPasteboard.PasteboardType]) -> Bool {
        tv.preferredPasteboardType(
            from: clipboard, restrictedToTypesFrom: tv.readablePasteboardTypes) != nil
    }

    private func plainTextView() -> NSTextView {
        let tv = NSTextView(frame: .init(x: 0, y: 0, width: 100, height: 40))
        tv.isRichText = false
        return tv
    }

    private func probeTextView() -> NSTextView {
        let tv = PasteProbeTextView(frame: .init(x: 0, y: 0, width: 100, height: 40))
        tv.isRichText = false
        return tv
    }

    /// 病根本身：纯文本框对图片类型一律不认 —— 这就是 Todo #9 的「⌘V 无反应」。
    /// 这条不是在测我们的代码，是把 AppKit 的这个前提钉在案卷里；哪天它变了，
    /// 这条会红，提醒后人重新审视那个 override 还需不需要。
    func testPlainTextViewRejectsImagePaste_thisIsTheBug() {
        let tv = plainTextView()
        XCTAssertFalse(pasteDispatches(tv, clipboard: [.png, .tiff]))
        XCTAssertFalse(pasteDispatches(tv, clipboard: [.tiff]))
    }

    /// 修复：声明了 `macPasteboardTypes` 后，只有截图的剪贴板也能派发 ⌘V。
    func testDeclaringMacPasteboardTypesLetsImagePasteDispatch() {
        let tv = probeTextView()
        XCTAssertTrue(pasteDispatches(tv, clipboard: [.png, .tiff]))
        XCTAssertTrue(pasteDispatches(tv, clipboard: [.tiff]))
        XCTAssertTrue(pasteDispatches(tv, clipboard: [.fileURL]))
    }

    /// 图片与文件三类缺一不可（截图给 png/tiff，Finder 给 file-url）。
    func testMacPasteboardTypesCoverBitmapAndFile() {
        XCTAssertTrue(CrewPasteAttachments.macPasteboardTypes.contains(.png))
        XCTAssertTrue(CrewPasteAttachments.macPasteboardTypes.contains(.tiff))
        XCTAssertTrue(CrewPasteAttachments.macPasteboardTypes.contains(.fileURL))
    }

    /// 纯文本剪贴板本来就走得通，别因为我们的追加把它搞坏。
    func testPlainTextStillDispatches() {
        // AppKit 把 .string 桥接成 legacy NSStringPboardType 一并放上剪贴板。
        let clipboard: [NSPasteboard.PasteboardType] = [
            .string, NSPasteboard.PasteboardType("NSStringPboardType"),
        ]
        XCTAssertTrue(pasteDispatches(plainTextView(), clipboard: clipboard))
        XCTAssertTrue(pasteDispatches(probeTextView(), clipboard: clipboard))
    }
}
