#if os(macOS)
import SwiftUI
import AppKit

/// An NSImage-backed SwiftUI view that renders a crew attachment image,
/// showing a placeholder while decoding and an error glyph on failure.
///
/// 名字里的 "Remote" 是历史：它原本还带一条 auth-gated 的 `/v1/uploads/<id>`
/// 取图路（device-grant bearer + `CrewImageCache` actor 缓存）。#63 第二期删掉
/// 跨端遥控整层之后，附件只有本地落盘的 `file://` 一种来源，那条路和它的缓存
/// 一起去掉了。名字没改是因为它是 `ServerImage` / `BubbleView` 的构造点，
/// 改名属于另一件事。
struct CrewRemoteImage: View {
    /// 附件 url —— 本地落盘附件的 `file://` 绝对路径（Todo #3）。
    let path: String
    var contentMode: ContentMode = .fill
    /// 本地图（`file://`）解码时的降采样上限（像素长边）。nil = 原图 —— 看大图
    /// 用。缩略图格子传显示尺寸，别让 4000px 原图为了 110pt 的格子解一遍（#443）。
    var maxPixelSize: Int? = nil

    @State private var image: NSImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: contentMode)
            } else if failed {
                ZStack {
                    Theme.Palette.surfaceMuted
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(Theme.Palette.inkMuted)
                }
            } else {
                ZStack {
                    Theme.Palette.surfaceMuted
                    ProgressView().controlSize(.small)
                }
            }
        }
        .task(id: path) { await load() }
    }

    /// 从磁盘读 + 解码（#443 病根 2）。此前这里是主线程同步 `NSImage(contentsOf:)`
    /// 且不缓存，每次白板刷新都把 5 张图重解一遍。
    private func load() async {
        guard let url = URL(string: path),
              let key = CrewLocalImageCache.key(for: url, maxPixel: maxPixelSize) else {
            image = nil
            failed = true
            return
        }
        // 命中：直接换上。**不先置 nil** —— 那会先渲染一轮占位、再渲染一轮图，
        // 白白多一次整表布局，还闪。LazyVStack 回收重建时走的就是这条路。
        if let hit = CrewLocalImageCache.shared.peek(key) {
            if image !== hit { image = hit }
            if failed { failed = false }
            return
        }
        image = nil
        failed = false
        let target = maxPixelSize
        let decoded = await Task.detached(priority: .userInitiated) {
            CrewLocalImageCache.decode(url: url, maxPixel: target)
        }.value
        guard !Task.isCancelled else { return }
        if let decoded {
            CrewLocalImageCache.shared.store(key, decoded)
            image = decoded
        } else {
            failed = true
        }
    }
}
#endif
