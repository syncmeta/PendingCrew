#if os(macOS)
import XCTest
import AppKit
// 源码直接编进 test bundle（见 project.yml），无需 import module。

/// #443 病根 2：本地附件图此前在主线程同步解码、且完全不缓存，每次白板刷新都
/// 把整群的图重解一遍。这里钉住缓存 key 的失效语义与降采样。
final class CrewLocalImageCacheTests: XCTestCase {

    func testDecodeDownsamplesToMaxPixel() throws {
        let url = try writePNG(width: 800, height: 400)
        let image = try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: 100))
        XCTAssertLessThanOrEqual(max(image.size.width, image.size.height), 100,
                                 "缩略图格子不该为了 110pt 去解一张 800px 的图")
        XCTAssertGreaterThan(min(image.size.width, image.size.height), 0)
    }

    func testDecodeWithoutLimitKeepsFullSize() throws {
        let url = try writePNG(width: 300, height: 150)
        let image = try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: nil))
        XCTAssertEqual(image.size.width, 300)
        XCTAssertEqual(image.size.height, 150)
    }

    func testStoreThenPeekHits() throws {
        let cache = CrewLocalImageCache()
        let url = try writePNG(width: 200, height: 200)
        let key = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))
        XCTAssertNil(cache.peek(key), "还没存过")

        let image = try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: 100))
        cache.store(key, image)
        XCTAssertTrue(cache.peek(key) === image, "同一 key 必须命中同一张，不该重解")
    }

    /// 文件被覆盖 → mtime/size 变 → key 变 → 旧图自然失效，不用手工 invalidate。
    func testOverwritingFileInvalidatesKey() throws {
        let cache = CrewLocalImageCache()
        let url = try writePNG(width: 200, height: 200)
        let oldKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))
        cache.store(oldKey, try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: 100)))

        try writePNG(width: 320, height: 240, at: url)
        let newKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))

        XCTAssertNotEqual(oldKey, newKey, "同路径不同内容必须是不同的 key")
        XCTAssertNil(cache.peek(newKey), "覆盖后不该拿到旧解码结果")
    }

    /// 缩略图和「看大图」的原图是两份，不能互相顶替。
    func testDifferentMaxPixelIsDifferentEntry() throws {
        let cache = CrewLocalImageCache()
        let url = try writePNG(width: 400, height: 400)
        let thumbKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))
        let fullKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: nil))
        XCTAssertNotEqual(thumbKey, fullKey)

        cache.store(thumbKey, try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: 100)))
        XCTAssertNotNil(cache.peek(thumbKey))
        XCTAssertNil(cache.peek(fullKey), "看大图不该拿到 100px 的缩略图")
    }

    func testMissingFileHasNoKeyAndDoesNotDecode() {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("\(UUID().uuidString).png")
        XCTAssertNil(CrewLocalImageCache.key(for: url, maxPixel: 100))
        XCTAssertNil(CrewLocalImageCache.decode(url: url, maxPixel: 100))
    }

    // MARK: - fixtures

    @discardableResult
    private func writePNG(width: Int, height: Int, at existing: URL? = nil) throws -> URL {
        let url = existing ?? FileManager.default.temporaryDirectory
            .appendingPathComponent("crewimg-\(UUID().uuidString).png")
        if existing == nil {
            addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        }
        let rep = try XCTUnwrap(NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSColor.systemBlue.setFill()
        NSRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)).fill()
        NSGraphicsContext.restoreGraphicsState()
        let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        try data.write(to: url)
        return url
    }
}
#endif
