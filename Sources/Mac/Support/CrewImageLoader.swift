#if os(macOS)
import SwiftUI
import AppKit

/// Minimal auth-gated image loader for crew attachments (macOS native port of
/// PendingBot's `ServerImage`). The `/v1/uploads/<id>` endpoint sits behind
/// `requireSession()` and only honours `Authorization: Bearer <token>`, so a
/// bare `AsyncImage` always 401s. We fetch the bytes ourselves with the
/// device-grant token attached and cache the decoded `NSImage` by URL string.
///
/// PendingCrew talks only to the edge (no direct Supabase), so the token is the
/// device-grant `pdg_*` bearer surfaced via `AppModel.imageAuth`.
actor CrewImageCache {
    static let shared = CrewImageCache()

    private var cache: [String: NSImage] = [:]
    /// Coalesce concurrent loads of the same url so two bubbles referencing the
    /// same image don't double-fetch.
    private var inFlight: [String: Task<NSImage?, Never>] = [:]

    /// Resolve an attachment image. `relativePath` is the wire `url`
    /// (e.g. `/v1/uploads/<id>`); `baseURL` + `token` come from
    /// `AppModel.imageAuth`. Returns nil on any failure (caller shows the
    /// error placeholder).
    func image(relativePath: String, baseURL: URL, token: String) async -> NSImage? {
        if let hit = cache[relativePath] { return hit }
        if let task = inFlight[relativePath] { return await task.value }

        let task = Task<NSImage?, Never> { [relativePath, baseURL, token] in
            await Self.fetch(relativePath: relativePath, baseURL: baseURL, token: token)
        }
        inFlight[relativePath] = task
        let result = await task.value
        inFlight[relativePath] = nil
        if let result { cache[relativePath] = result }
        return result
    }

    private static func fetch(relativePath: String, baseURL: URL, token: String) async -> NSImage? {
        // Strip a single leading slash so appendingPathComponent doesn't
        // produce a doubled `//uploads/…`.
        let trimmed = relativePath.hasPrefix("/") ? String(relativePath.dropFirst()) : relativePath
        let absolute = baseURL.appendingPathComponent(trimmed)
        var req = URLRequest(url: absolute)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        do {
            let (data, response) = try await URLSession.shared.data(for: req)
            guard let http = response as? HTTPURLResponse,
                  (200..<300).contains(http.statusCode),
                  let img = NSImage(data: data) else {
                return nil
            }
            return img
        } catch {
            return nil
        }
    }
}

/// An NSImage-backed SwiftUI view that loads an auth-gated crew attachment
/// image, showing a placeholder while loading and an error glyph on failure.
/// Reads auth (baseURL + device-grant token) from the `AppModel` in the
/// environment — the same one `CrewChatView` injects.
struct CrewRemoteImage: View {
    /// Wire `url`, e.g. `/v1/uploads/<id>`.
    let path: String
    var contentMode: ContentMode = .fill
    /// 本地图（`file://`）解码时的降采样上限（像素长边）。nil = 原图 —— 看大图
    /// 用。缩略图格子传显示尺寸，别让 4000px 原图为了 110pt 的格子解一遍（#443）。
    var maxPixelSize: Int? = nil

    @EnvironmentObject private var appModel: AppModel
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

    private func load() async {
        // 本地附件（Todo #3）：`file://` 绝对 URL 直接从磁盘读，不走 edge 鉴权
        // （本地模式没有 imageAuth）。**要缓存、要后台解码** —— 见 loadLocal。
        if path.hasPrefix("file://") {
            await loadLocal()
            return
        }
        image = nil
        failed = false
        guard let auth = appModel.imageAuth else {
            failed = true
            return
        }
        let loaded = await CrewImageCache.shared.image(
            relativePath: path, baseURL: auth.baseURL, token: auth.token)
        if let loaded {
            image = loaded
        } else {
            failed = true
        }
    }

    /// 本地图（#443 病根 2）。此前这里是主线程同步 `NSImage(contentsOf:)` 且不
    /// 缓存，每次白板刷新都把 5 张图重解一遍。
    private func loadLocal() async {
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
