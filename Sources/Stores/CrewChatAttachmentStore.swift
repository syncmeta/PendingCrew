import Foundation

/// 群聊附件本地落盘（Todo #3：群聊发/粘贴图片给 claude code）。
///
/// 存 **app 数据目录**（`LocalWhiteboardStore` 同级）：
/// `Application Support/PendingCrew/attachments/<crewId>/IMG-<yyyyMMdd-HHmmss>-<短随机>.<ext>`
///
/// 为什么不存 crew 工作目录：worktree 清理会把图带走，历史气泡就渲染不了；
/// app 数据目录随聊天记录（whiteboards/ 同级）持久。claude 读工作目录外的
/// 绝对路径在 auto 模式不弹审批（官方文档确认，前任 worker 调研）。
///
/// 只依赖 Foundation —— 与 `LocalWhiteboardStore` 同级的自包含 store。
enum CrewChatAttachmentStore {

    /// 附件根目录（`Application Support/PendingCrew/attachments/`）。
    static let defaultDirectory: URL = {
        let base = (try? FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask,
            appropriateFor: nil, create: true))
            ?? FileManager.default.temporaryDirectory
        return base
            .appendingPathComponent("PendingCrew", isDirectory: true)
            .appendingPathComponent("attachments", isDirectory: true)
    }()

    /// composer 暂存区（`<tmp>/PendingCrew/composer-staging/<uuid>/<原名>`）。
    ///
    /// 拖进来 / 文件选择器选中的文件先**原样复制**到这里，托盘里只拿着这个路径 ——
    /// 字节不进内存。发送时 `save(fileAt:)` 把它 move 进 `<attachments>/<crewId>/`
    /// （同卷即改名，零字节拷贝）。为什么要中转一道而不是直接拿着原路径：
    /// ① iOS 的拖放/文件选择给的是 security-scoped URL，作用域出了回调就没了；
    /// ② 用户可能在「拖进托盘」到「点发送」之间把原文件挪走/删掉。
    /// 暂存区在 temp 下，退出/系统清理都不留垃圾。
    static let stagingDirectory: URL = FileManager.default.temporaryDirectory
        .appendingPathComponent("PendingCrew", isDirectory: true)
        .appendingPathComponent("composer-staging", isDirectory: true)

    /// 把一份附件字节落盘到 `<root>/<crewId>/`，返回可直接挂到
    /// `LocalWhiteboardMessage.attachments` 的条目。失败（磁盘写不进等）返回 nil，
    /// 调用方自行决定软报错。
    ///
    /// - `mime`：决定扩展名与图/文件渲染分支。
    /// - `filename`：原始文件名（文件 chip 显示用；粘贴的图片传 nil）。
    /// - `root`：附件根目录，默认真实数据目录；单测传临时目录（同一条生产代码路径）。
    static func save(
        data: Data, mime: String, filename: String? = nil, crewId: String,
        root: URL = CrewChatAttachmentStore.defaultDirectory
    ) -> LocalWhiteboardAttachment? {
        do {
            let url = try makeDestination(
                mime: mime, filename: filename, crewId: crewId, root: root)
            try data.write(to: url, options: .atomic)
            return LocalWhiteboardAttachment(
                id: UUID().uuidString.lowercased(),
                mime: mime,
                size: data.count,
                path: url.path,
                filename: filename)
        } catch {
            return nil
        }
    }

    /// 把一个**已经在磁盘上**的文件收进 `<root>/<crewId>/`。
    ///
    /// 优先 `moveItem`（暂存区与数据目录同卷 → 只改目录项，一个字节都不搬），
    /// 跨卷失败退回 `copyItem`。**整份字节全程不进内存** —— 这是与 `save(data:)`
    /// 并列的另一条路，拖入 / 文件选择器走这条。
    static func save(
        fileAt source: URL, mime: String, filename: String? = nil, crewId: String,
        root: URL = CrewChatAttachmentStore.defaultDirectory
    ) -> LocalWhiteboardAttachment? {
        guard let size = byteSize(ofFileAt: source) else { return nil }
        do {
            let url = try makeDestination(
                mime: mime, filename: filename, crewId: crewId, root: root)
            do {
                try FileManager.default.moveItem(at: source, to: url)
            } catch {
                try FileManager.default.copyItem(at: source, to: url)
            }
            return LocalWhiteboardAttachment(
                id: UUID().uuidString.lowercased(),
                mime: mime,
                size: size,
                path: url.path,
                filename: filename)
        } catch {
            return nil
        }
    }

    /// 把一个文件原样复制进暂存区，返回暂存路径。**只复制、不读字节。**
    /// 调用方负责先判大小/目录（见 `CrewFileAttachmentIntake`）—— 这里只搬。
    static func stage(fileAt source: URL, into staging: URL = stagingDirectory) throws -> URL {
        let box = staging.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: box, withIntermediateDirectories: true)
        let name = source.lastPathComponent.isEmpty ? "file" : source.lastPathComponent
        let dest = box.appendingPathComponent(name)
        try FileManager.default.copyItem(at: source, to: dest)
        return dest
    }

    /// 丢弃一份暂存（托盘里删掉、或发完清场）。只删暂存区里的东西 —— 传进来的
    /// 路径不在暂存区下就原地返回，别让它变成一个「能删任意路径」的口子。
    static func discardStaged(_ url: URL, staging: URL = stagingDirectory) {
        let box = url.deletingLastPathComponent()
        guard box.path.hasPrefix(staging.path + "/") else { return }
        try? FileManager.default.removeItem(at: box)
    }

    /// 文件字节数；读不到（不存在 / 是目录）返回 nil。
    static func byteSize(ofFileAt url: URL) -> Int? {
        guard let values = try? url.resourceValues(forKeys: [.fileSizeKey, .isDirectoryKey]),
              values.isDirectory != true, let size = values.fileSize else { return nil }
        return size
    }

    /// 目标落盘路径（建目录 + 生成不冲突的文件名）。两条 save 共用。
    private static func makeDestination(
        mime: String, filename: String?, crewId: String, root: URL
    ) throws -> URL {
        let dir = root.appendingPathComponent(crewId, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent(Self.diskName(mime: mime, filename: filename))
    }

    /// 落盘文件名：`IMG-<yyyyMMdd-HHmmss>-<短随机>.<ext>`（图片）/ `ATT-…`（其它）。
    /// 时间戳可读 + 短随机防同秒冲突；不复用原始文件名（可能含路径不安全字符），
    /// 原名保留在 `LocalWhiteboardAttachment.filename` 供显示。
    private static func diskName(mime: String, filename: String?) -> String {
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyyMMdd-HHmmss"
        fmt.locale = Locale(identifier: "en_US_POSIX")
        let ts = fmt.string(from: Date())
        let rand = String(UUID().uuidString.lowercased().prefix(6))
        let isImage = mime.lowercased().hasPrefix("image/")
        let prefix = isImage ? "IMG" : "ATT"
        return "\(prefix)-\(ts)-\(rand).\(Self.ext(mime: mime, filename: filename))"
    }

    /// 扩展名：mime 映射优先，未知 mime 退回原始文件名的扩展名，再兜底 bin/png。
    private static func ext(mime: String, filename: String?) -> String {
        switch mime.lowercased() {
        case "image/png": return "png"
        case "image/jpeg": return "jpg"
        case "image/gif": return "gif"
        case "image/webp": return "webp"
        case "image/heic": return "heic"
        case "image/tiff": return "tiff"
        case "application/pdf": return "pdf"
        case "text/plain": return "txt"
        default:
            if let e = filename.flatMap({ ($0 as NSString).pathExtension }), !e.isEmpty {
                return e.lowercased()
            }
            return mime.lowercased().hasPrefix("image/") ? "png" : "bin"
        }
    }
}
