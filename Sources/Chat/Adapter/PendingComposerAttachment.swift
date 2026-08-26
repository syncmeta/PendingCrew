import Foundation

/// Pending (not-yet-uploaded) composer attachment. Source-of-truth type held by
/// `CrewChatView` (the send path uploads from these, then clears them). Bridged
/// to the vendored `ComposerView`'s `PendingAttachment` for tray display.
///
/// Extracted from the now-deleted `CrewComposer.swift` (A11) so it stays
/// available cross-platform once the chat views are un-gated (A12).
///
/// **字节不一定在内存里。** 拖进来 / 文件选择器选中的文件走 `.staged` ——
/// 只记一个暂存区路径，整份字节从头到尾不进内存（几百 MB 的文件曾经会在这里
/// 被 `Data(contentsOf:)` 整份读进来，再 write 一遍）。粘贴的位图 / 相册 /
/// 拍照本来就是内存里的字节，走 `.data`。
struct PendingComposerAttachment: Identifiable, Equatable, Sendable {
    enum Source: Equatable, Sendable {
        /// 字节本来就在内存里（⌘V 位图、PhotosPicker、拍照）。
        case data(Data)
        /// 已原样复制到 composer 暂存区的文件（拖入 / 文件选择器）。
        /// 发送时 move 进附件目录（同卷即改名，零字节拷贝）。
        case staged(URL)
    }

    let id = UUID()
    let filename: String
    let mime: String
    let source: Source
    /// 字节数。`.staged` 时取自 intake 时读到的文件大小 —— 不为了数大小去读字节。
    let byteSize: Int
    /// 托盘缩略图字节（图片才有）。`.staged` 的图走 ImageIO 缩略图，不整份解码。
    let previewData: Data?

    init(filename: String, mime: String, data: Data) {
        self.filename = filename
        self.mime = mime
        self.source = .data(data)
        self.byteSize = data.count
        self.previewData = mime.lowercased().hasPrefix("image/") ? data : nil
    }

    init(
        filename: String, mime: String, stagedAt url: URL,
        byteSize: Int, previewData: Data? = nil
    ) {
        self.filename = filename
        self.mime = mime
        self.source = .staged(url)
        self.byteSize = byteSize
        self.previewData = previewData
    }

    var isImage: Bool { mime.lowercased().hasPrefix("image/") }

    /// 已经在内存里的字节。`.staged` 恒 nil —— **别在这条路上把大文件读进内存**。
    var inMemoryData: Data? {
        if case .data(let d) = source { return d }
        return nil
    }

    /// 暂存文件路径。`.data` 恒 nil。
    var stagedURL: URL? {
        if case .staged(let u) = source { return u }
        return nil
    }
}
