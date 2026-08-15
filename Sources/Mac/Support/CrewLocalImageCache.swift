#if os(macOS)
import AppKit
import Foundation
import ImageIO

/// 本地附件图（`file://`）的解码缓存（#443 病根 2）。
///
/// 此前 `CrewRemoteImage` 的 `file://` 分支直接 `NSImage(contentsOf:)`：**同步
/// 磁盘读 + 同步解码**，而 `.task` 继承 MainActor → 整件事跑在主线程；而且这条
/// 分支完全绕过缓存，每次重刷都把原图重解一遍。「LED驱动板」白板只有 53KB、65
/// 条，比它大四倍的群都不卡，差别就是这 5 张本地图 —— 病根 1 制造高频重刷，
/// 这里让每次重刷都在主线程解 5 张图，叠起来才是彻底卡死。
///
/// 这里三件事：
/// - 解码搬到后台（`Task.detached`），主线程只做一次 stat + 字典查。
/// - 按需降采样到显示尺寸（缩略图 110pt 的格子不需要 4000px 原图）。
/// - 结果进缓存，key 带 **mtime + size**，文件被覆盖自然失效，不用手工 invalidate。
final class CrewLocalImageCache: @unchecked Sendable {
    static let shared = CrewLocalImageCache()

    /// 缓存 key。`maxPixel` 进 key —— 同一张图的缩略图与看大图的原图是两份，
    /// 不能互相顶替。
    struct Key: Hashable {
        let path: String
        /// `contentModificationDate`（秒）。文件被覆盖 → 变 → 旧图自然失效。
        let modified: TimeInterval
        let size: Int
        /// 降采样上限（像素长边）；nil = 原图。
        let maxPixel: Int?

        var storageKey: String {
            "\(path)|\(modified)|\(size)|\(maxPixel.map(String.init) ?? "full")"
        }
    }

    /// 读文件元数据算 key。文件不存在 / 读不到属性 → nil（调用方走错误占位）。
    ///
    /// 这一次 stat 是留在主线程的唯一磁盘动作（微秒级），换来的是「命中缓存时
    /// 一次 await 都不用、也不必先把 image 置 nil」。
    static func key(for url: URL, maxPixel: Int?) -> Key? {
        let fresh = URL(fileURLWithPath: url.path)
        guard let values = try? fresh.resourceValues(
                forKeys: [.contentModificationDateKey, .fileSizeKey]),
              let modified = values.contentModificationDate,
              let size = values.fileSize else { return nil }
        return Key(path: url.path,
                   modified: modified.timeIntervalSinceReferenceDate,
                   size: size,
                   maxPixel: maxPixel)
    }

    /// `NSCache` 自带线程安全 + 内存压力下自动清空，按像素字节数计成本。
    private let cache = NSCache<NSString, NSImage>()

    init(costLimitBytes: Int = 64 * 1024 * 1024) {
        cache.totalCostLimit = costLimitBytes
    }

    /// 同步命中查询。命中就直接换上图，**不必先把 image 置 nil** —— 置 nil 会
    /// 先渲染一轮占位再渲染一轮图，白白多一次整表布局，还会闪。
    func peek(_ key: Key) -> NSImage? {
        cache.object(forKey: key.storageKey as NSString)
    }

    func store(_ key: Key, _ image: NSImage) {
        cache.setObject(image, forKey: key.storageKey as NSString, cost: Self.cost(of: image))
    }

    /// 测试用：清空（各用例之间不互相污染）。
    func removeAll() {
        cache.removeAllObjects()
    }

    private static func cost(of image: NSImage) -> Int {
        let px = image.size.width * image.size.height
        guard px.isFinite, px > 0 else { return 1 }
        return Int(px * 4)
    }

    // MARK: - 解码

    /// 从磁盘解一张图，`maxPixel` 非 nil 时降采样到长边不超过它。
    ///
    /// **别在主线程调。** 走 `CGImageSource` 而不是 `NSImage(contentsOf:)`：前者
    /// 能一次性出降采样缩略图（不必先解全图再缩），也能 `ShouldCacheImmediately`
    /// 把真正的位图解码完成在**这里**，而不是拖到主线程首次绘制时。
    /// 遇上 CGImageSource 认不了的格式（PDF/SVG 之类）回落 `NSImage(contentsOf:)`
    /// —— 仍在后台线程，只是不降采样。
    static func decode(url: URL, maxPixel: Int?) -> NSImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else {
            return NSImage(contentsOf: url)
        }
        if let maxPixel, maxPixel > 0 {
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                // EXIF 方向在这一步就应用掉，免得缩略图躺倒。
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceThumbnailMaxPixelSize: maxPixel,
            ]
            if let cg = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) {
                return image(from: cg)
            }
        }
        let options: [CFString: Any] = [kCGImageSourceShouldCacheImmediately: true]
        if let cg = CGImageSourceCreateImageAtIndex(source, 0, options as CFDictionary) {
            return image(from: cg)
        }
        return NSImage(contentsOf: url)
    }

    private static func image(from cg: CGImage) -> NSImage {
        // size 用像素数 —— 缩略图路径已经按 maxPixel 缩过，这里再按 DPI 换算只会
        // 让 SwiftUI 拿到不一致的固有尺寸。格子是 `.frame` 固定的，不受影响。
        NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
}
#endif
