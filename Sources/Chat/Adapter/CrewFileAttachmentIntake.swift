import Foundation
import ImageIO
import UniformTypeIdentifiers

/// 「这个文件收不收」的候选描述。宿主（拖入 / `+ 面板▸文件`）用 FileManager 的
/// 资源值填好它，判定本身是下面的纯函数 —— 两个入口共用同一套口径，也才测得住。
struct CrewFileCandidate: Equatable, Sendable {
    var filename: String
    /// 字节数。目录/package 时无意义（判定先被 `isDirectory` 截住）。
    var byteSize: Int
    /// 目录 —— **也含 package/bundle**（`.app` / `.key` / `.xcodeproj`）。
    /// 文件口径从 `.data` 放宽到 `.item` 之后这些能被选中/拖进来，但它们不是
    /// 单个文件：照着单文件落盘只会产出一个坏附件。
    var isDirectory: Bool
    var mime: String
}

/// 判定结果。非 `.accept` 一律要给人看得见的话（`rejectionMessage`），
/// 不许静默丢。
enum CrewFileIntakeVerdict: Equatable, Sendable {
    case accept
    case rejectDirectory
    case rejectTooLarge(limit: Int)
    /// 读不到大小 / 0 字节 —— 收下也只是个空附件。
    case rejectUnreadable
}

/// 拖入 / 文件选择器 → 附件托盘的**唯一**一条路。
///
/// 纯判定（`verdict` / `rejectionMessage`）与真正碰磁盘的那半（`intake`）都在
/// 这里，且只依赖 Foundation/ImageIO —— 所以整条路能直接进 test bundle 跑，
/// 不用在测试里复刻一份同形的假货。
enum CrewFileAttachmentIntake {

    /// 本地附件上限。
    ///
    /// - macOS 走本地落盘（`CrewChatAttachmentStore`），没有服务端把关，所以这条
    ///   线是唯一的闸门。取 512 MiB：宽到日常拖录屏 / 打包产物都过得去，又远低于
    ///   「拖一个几 G 的磁盘镜像进来」那种把机器拖垮的量级。
    /// - iOS 走 edge `/v1/upload`，那边硬上限 25 MiB（`apps/edge/src/lib/attachments.ts`），
    ///   本地就对齐到同一个数：收下一份注定 413 的文件，还得先把它整份读进内存
    ///   才发现发不出去。
    static let maxBytes: Int = {
        #if os(macOS)
        return 512 * 1024 * 1024
        #else
        return 25 * 1024 * 1024
        #endif
    }()

    static func verdict(for c: CrewFileCandidate, limit: Int = maxBytes) -> CrewFileIntakeVerdict {
        if c.isDirectory { return .rejectDirectory }
        if c.byteSize <= 0 { return .rejectUnreadable }
        if c.byteSize > limit { return .rejectTooLarge(limit: limit) }
        return .accept
    }

    /// 给人看的软报错文案（`.accept` → nil）。
    static func rejectionMessage(_ v: CrewFileIntakeVerdict, filename: String) -> String? {
        switch v {
        case .accept:
            return nil
        case .rejectDirectory:
            return "「\(filename)」是文件夹（或 .app/.key 这类包），不能整个当附件发 —— 先压成 zip 再拖进来。"
        case .rejectTooLarge(let limit):
            return "「\(filename)」超过 \(byteText(limit)) 上限，没有发出去。"
        case .rejectUnreadable:
            return "「\(filename)」读不到内容（文件可能已被移走或是空的），没有发出去。"
        }
    }

    /// 一次拖入 / 一次多选的结果。
    struct Result: Sendable {
        var accepted: [PendingComposerAttachment] = []
        /// 逐条软报错（超限 / 目录 / 读不到）。空 = 全收下了。
        var errors: [String] = []
    }

    /// 真正干活的那半：逐个 URL 读元信息 → 判定 → 原样复制进暂存区 → 出附件。
    ///
    /// **不读整份字节**（图片缩略图除外，那走 ImageIO 只解一张小图）。会碰磁盘，
    /// 所以调用方应该把它丢到后台线程 —— 复制一个 400 MB 的文件不能卡住主线程。
    static func intake(
        urls: [URL],
        limit: Int = maxBytes,
        staging: URL = CrewChatAttachmentStore.stagingDirectory
    ) -> Result {
        var out = Result()
        for url in urls {
            // 非沙盒的 macOS 上恒 false 且无所谓；iOS 的文件 App 拖放/选择器给的是
            // security-scoped URL，必须在作用域内把文件复制完。
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }

            let candidate = describe(fileAt: url)
            let call = verdict(for: candidate, limit: limit)
            guard call == .accept else {
                if let msg = rejectionMessage(call, filename: candidate.filename) {
                    out.errors.append(msg)
                }
                continue
            }
            guard let staged = try? CrewChatAttachmentStore.stage(fileAt: url, into: staging) else {
                out.errors.append("「\(candidate.filename)」复制失败，没有发出去。")
                continue
            }
            out.accepted.append(PendingComposerAttachment(
                filename: candidate.filename,
                mime: candidate.mime,
                stagedAt: staged,
                byteSize: candidate.byteSize,
                previewData: candidate.mime.lowercased().hasPrefix("image/")
                    ? thumbnailPNG(fileAt: staged) : nil))
        }
        return out
    }

    /// URL → 候选描述（文件名 / 大小 / 是不是目录 / mime）。
    static func describe(fileAt url: URL) -> CrewFileCandidate {
        let values = try? url.resourceValues(
            forKeys: [.fileSizeKey, .isDirectoryKey, .contentTypeKey])
        let mime = values?.contentType?.preferredMIMEType
            ?? UTType(filenameExtension: url.pathExtension)?.preferredMIMEType
            ?? "application/octet-stream"
        return CrewFileCandidate(
            filename: url.lastPathComponent,
            byteSize: values?.fileSize ?? 0,
            isDirectory: values?.isDirectory ?? false,
            mime: mime)
    }

    /// 托盘缩略图：走 ImageIO 直接从文件生成一张小图，**不整份解码**。
    /// 生成不出来（不是图 / 坏文件）返回 nil，chip 退回文件样式，不崩。
    static func thumbnailPNG(fileAt url: URL, maxPixel: Int = 512) -> Data? {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixel,
        ]
        let buffer = NSMutableData()
        guard let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, options as CFDictionary),
              let dest = CGImageDestinationCreateWithData(
                buffer as CFMutableData, UTType.png.identifier as CFString, 1, nil)
        else { return nil }
        CGImageDestinationAddImage(dest, cg, nil)
        guard CGImageDestinationFinalize(dest) else { return nil }
        return buffer as Data
    }

    /// 「512 MB」这种给人看的量。上限是 2 的幂，所以直接按 MiB/GiB 整除报 ——
    /// `ByteCountFormatter` 按 1000 进制会把 512 MiB 说成「536.9 MB」，对不上文案。
    static func byteText(_ bytes: Int) -> String {
        let mb = bytes / (1024 * 1024)
        return mb >= 1024 ? "\(mb / 1024) GB" : "\(mb) MB"
    }
}
