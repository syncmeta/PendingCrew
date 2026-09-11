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
        let cache = CrewLocalImageCache(storage: DictionaryStorage())
        let url = try writePNG(width: 200, height: 200)
        let key = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))
        XCTAssertNil(cache.peek(key), "还没存过")

        let image = try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: 100))
        cache.store(key, image)
        XCTAssertTrue(cache.peek(key) === image, "存储仍持有时，同一 key 返回原对象，调用方可以跳过重解")
    }

    /// 文件被覆盖 → mtime/size 变 → key 变 → 旧图自然失效，不用手工 invalidate。
    func testOverwritingFileInvalidatesKey() throws {
        let cache = CrewLocalImageCache(storage: DictionaryStorage())
        let url = try writePNG(width: 200, height: 200)
        let oldKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))
        let oldImage = try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: 100))
        cache.store(oldKey, oldImage)
        XCTAssertTrue(cache.peek(oldKey) === oldImage)

        try writePNG(width: 320, height: 240, at: url)
        let newKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))

        XCTAssertNotEqual(oldKey, newKey, "同路径不同内容必须是不同的 key")
        XCTAssertNil(cache.peek(newKey), "覆盖后不该拿到旧解码结果")
        XCTAssertTrue(cache.peek(oldKey) === oldImage, "未命中源于 key 改变，而非旧条目已被驱逐")
    }

    /// 缩略图和「看大图」的原图是两份，不能互相顶替。
    func testDifferentMaxPixelIsDifferentEntry() throws {
        let cache = CrewLocalImageCache(storage: DictionaryStorage())
        let url = try writePNG(width: 400, height: 400)
        let thumbKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: 100))
        let fullKey = try XCTUnwrap(CrewLocalImageCache.key(for: url, maxPixel: nil))
        XCTAssertNotEqual(thumbKey, fullKey)

        let thumb = try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: 100))
        let full = try XCTUnwrap(CrewLocalImageCache.decode(url: url, maxPixel: nil))
        cache.store(thumbKey, thumb)
        XCTAssertTrue(cache.peek(thumbKey) === thumb)
        XCTAssertNil(cache.peek(fullKey), "看大图不该拿到 100px 的缩略图")
        cache.store(fullKey, full)
        XCTAssertTrue(cache.peek(fullKey) === full)
        XCTAssertTrue(cache.peek(thumbKey) === thumb, "原图不能覆盖缩略图的桶")
    }

    func testMissingFileHasNoKeyAndDoesNotDecode() {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("\(UUID().uuidString).png")
        XCTAssertNil(CrewLocalImageCache.key(for: url, maxPixel: 100))
        XCTAssertNil(CrewLocalImageCache.decode(url: url, maxPixel: 100))
    }

    func testEvictedEntryMayMiss() {
        let cache = CrewLocalImageCache(storage: ImmediateEvictionStorage())
        let key = CrewLocalImageCache.Key(path: "/evicted.png", modified: 1, size: 10, maxPixel: nil)
        cache.store(key, NSImage(size: NSSize(width: 10, height: 20)))
        XCTAssertNil(cache.peek(key), "存储可立即驱逐；调用方仍须处理 miss")
    }

    func testInjectedStorageReceivesKeyCostReplacementAndClear() {
        let storage = DictionaryStorage()
        let cache = CrewLocalImageCache(storage: storage)
        let key = CrewLocalImageCache.Key(path: "/cost.png", modified: 123, size: 456, maxPixel: 100)
        let storageKey: NSString = "/cost.png|123.0|456|100"
        let first = NSImage(size: NSSize(width: 10, height: 20))
        cache.store(key, first)
        XCTAssertTrue(storage.object(forKey: storageKey) === first, "必须写到注入的存储")
        XCTAssertEqual(storage.cost(forKey: storageKey), 800)
        let replacement = NSImage(size: .zero)
        cache.store(key, replacement)
        XCTAssertTrue(cache.peek(key) === replacement)
        XCTAssertEqual(storage.cost(forKey: storageKey), 1, "无有效像素时成本下限为 1")
        cache.removeAll()
        XCTAssertNil(storage.object(forKey: storageKey))
        XCTAssertNil(cache.peek(key))
    }

    /// Dictionary lifetime is controlled by the test, never by system memory pressure.
    private final class DictionaryStorage: CrewLocalImageStorage, @unchecked Sendable {
        private let lock = NSLock()
        private var entries: [String: (image: NSImage, cost: Int)] = [:]

        func object(forKey key: NSString) -> NSImage? {
            lock.withLock { entries[key as String]?.image }
        }
        func setObject(_ image: NSImage, forKey key: NSString, cost: Int) {
            lock.withLock { entries[key as String] = (image, cost) }
        }
        func removeAllObjects() { lock.withLock { entries.removeAll() } }
        func cost(forKey key: NSString) -> Int? {
            lock.withLock { entries[key as String]?.cost }
        }
    }

    private final class ImmediateEvictionStorage: CrewLocalImageStorage {
        func object(forKey key: NSString) -> NSImage? { nil }
        func setObject(_ image: NSImage, forKey key: NSString, cost: Int) {}
        func removeAllObjects() {}
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
