import Foundation
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// 系统剪贴板 → `CrewPasteboardSnapshot`（中立快照）的**唯一**一处读取。
///
/// 「读到了什么」必须与「该收哪些、按什么优先级」分开（见 `CrewPasteAttachments`：
/// Finder 拷一个文件会同时往剪贴板放文件 URL 和一张图标位图）。读取这半碰真
/// pasteboard、进不了单测，所以它只做搬运，一行判定都不写。
///
/// 原本长在 `CrewChatView` 里；Todo #52 让 Todo 追问也能粘贴图之后就有第二个调用
/// 方了 —— 抽出来，免得两处各读各的、优先级悄悄分叉。
enum CrewPasteboardReader {

    #if os(macOS)
    /// NSPasteboard → 中立快照。文件与位图都读进来，优先级判定归纯逻辑。
    static func snapshot() -> CrewPasteboardSnapshot {
        let pb = NSPasteboard.general
        var files: [CrewPasteboardSnapshot.FileItem] = []
        if let urls = pb.readObjects(
               forClasses: [NSURL.self],
               options: [.urlReadingFileURLsOnly: true]) as? [URL] {
            for url in urls {
                guard let data = try? Data(contentsOf: url) else { continue }
                files.append(.init(
                    filename: url.lastPathComponent,
                    data: data,
                    mime: CrewPasteAttachments.mime(forExtension: url.pathExtension)))
            }
        }
        // 位图（⌘⇧⌃4 截图 / 应用里拷图）：优先原生 PNG 字节，退回 TIFF 转码。
        var png = pb.data(forType: .png)
        if png == nil, let tiff = pb.data(forType: .tiff),
           let rep = NSBitmapImageRep(data: tiff) {
            png = rep.representation(using: .png, properties: [:])
        }
        return CrewPasteboardSnapshot(files: files, imagePNG: png)
    }
    #else
    /// UIPasteboard → 中立快照。iPad/iPhone 只收图片（文件走 + 面板的文件选择
    /// 器；系统粘贴板上的文件 provider 读取是另一套异步 API，不在这条路上）。
    static func snapshot() -> CrewPasteboardSnapshot {
        let pb = UIPasteboard.general
        guard pb.hasImages, let image = pb.image else { return CrewPasteboardSnapshot() }
        return CrewPasteboardSnapshot(imagePNG: image.pngData())
    }
    #endif

    /// 剪贴板里这一把能收的附件（空 = 没有可收的，调用方走默认文本粘贴）。
    static func attachments() -> [PendingComposerAttachment] {
        CrewPasteAttachments.attachments(from: snapshot())
    }
}
