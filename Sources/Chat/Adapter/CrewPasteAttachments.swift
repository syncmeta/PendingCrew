import Foundation
import UniformTypeIdentifiers
#if os(macOS)
import AppKit
#endif

/// 剪贴板的中立快照。UI 层（macOS `NSPasteboard` / iOS `UIPasteboard`）负责
/// 「读到了什么」，这里负责「该收哪些附件、按什么优先级」——拆开是因为
/// pasteboard 进不了单测，而优先级恰恰是会出错的那半：Finder 拷一个文件会
/// **同时**往剪贴板放文件 URL 和一张图标位图，先判位图就会把 PDF 误收成
/// 「pasted-image.png」。
struct CrewPasteboardSnapshot: Equatable {
    struct FileItem: Equatable {
        var filename: String
        var data: Data
        /// UI 层按扩展名推断出的 MIME；nil = 推不出来，落到二进制流。
        var mime: String?
    }

    /// 剪贴板里的文件（Finder 拷贝 / 拖进来的）。
    var files: [FileItem] = []
    /// 位图（⌘⇧⌃4 截图、应用里「拷贝图片」），已由 UI 层统一转成 PNG 字节。
    var imagePNG: Data?

    init(files: [FileItem] = [], imagePNG: Data? = nil) {
        self.files = files
        self.imagePNG = imagePNG
    }
}

/// 剪贴板 → 附件托盘的纯判定。返回空数组 = 这次 ⌘V 里没有可收的附件，宿主
/// 不吞，走默认文本粘贴。
enum CrewPasteAttachments {
    /// 位图没有原始文件名，统一用这个（与 macOS 截图粘贴的既有行为一致）。
    static let bitmapFilename = "pasted-image.png"
    static let fallbackMIME = "application/octet-stream"

    static func attachments(from snapshot: CrewPasteboardSnapshot) -> [PendingComposerAttachment] {
        // 文件优先于位图 —— 见类型注释里的 Finder 图标位图坑。
        let files = snapshot.files.filter { !$0.data.isEmpty }
        if !files.isEmpty {
            return files.map {
                PendingComposerAttachment(
                    filename: $0.filename,
                    mime: $0.mime ?? fallbackMIME,
                    data: $0.data)
            }
        }
        if let png = snapshot.imagePNG, !png.isEmpty {
            return [PendingComposerAttachment(
                filename: bitmapFilename, mime: "image/png", data: png)]
        }
        return []
    }

    /// 扩展名 → MIME。UI 层读到文件 URL 时调，结果填进 `FileItem.mime`。
    static func mime(forExtension ext: String) -> String {
        UTType(filenameExtension: ext)?.preferredMIMEType ?? fallbackMIME
    }
}

#if os(macOS)
extension CrewPasteAttachments {
    /// **Todo #9 的病根就在这个清单上。** macOS 的 ⌘V 要先过「编辑▸粘贴」菜单项
    /// 校验：AppKit 拿剪贴板类型与文本框的 `readablePasteboardTypes` 求交集，交集
    /// 为空就把菜单项置灰 —— 于是 ⌘V **连派发都不发生**，composer 那个 `paste(_:)`
    /// 拦截器永远等不到调用。纯文本 `NSTextView` 的默认清单里没有任何图片类型，
    /// 所以「剪贴板里只有截图」= 粘贴键彻底没反应。
    ///
    /// `ComposerNSTextView.readablePasteboardTypes` 把这些追加进默认清单，校验才
    /// 放行。谁把它删了，界面上看不出任何异常 —— 只是又粘不了图了，所以有单测
    /// （`CrewPasteAttachmentsTests`）钉着。
    static let macPasteboardTypes: [NSPasteboard.PasteboardType] = [.png, .tiff, .fileURL]
}
#endif
