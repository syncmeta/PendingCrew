// SHIM for PendingBot Components/ServerImage.swift — internals route through
// CrewRemoteImage (macOS) or CrewCrossRemoteImage (iOS).
//
// PendingBot's ServerImage fetches auth-gated images via SupabaseStack JWT.
// PendingCrew has no Supabase — it talks only to the edge with a device-grant
// bearer. CrewRemoteImage (Sources/Mac/Support/CrewImageLoader.swift) already
// handles the auth-gated fetch via AppModel.imageAuth on macOS.
//
// Public init signature is IDENTICAL to PendingBot's ServerImage so that
// AttachmentGrid in the vendored BubbleView constructs it unchanged:
//
//   ServerImage(path: att.url, serverURL: serverURL, contentMode: .fill)
//
// The `serverURL` parameter is accepted but ignored — the loader uses
// AppModel.imageAuth.baseURL which is set from AppModel.apiBaseURL, and the
// path is the relative wire URL (e.g. `/v1/uploads/<id>`).
//
// macOS: delegates to CrewRemoteImage (NSImage-backed, actor-cached).
// iOS:   CrewCrossRemoteImage — same auth-gated fetch using PlatformImage (UIImage).

import SwiftUI

struct ServerImage: View {
    let path: String
    let serverURL: URL
    var contentMode: ContentMode = .fit
    /// 本地图（`file://`）的降采样上限（像素长边）。缩略图格子传显示尺寸；
    /// nil = 原图（看大图）。远端图不受影响 —— 那条路本来就在后台且已缓存。
    /// 见 #443：主线程解全尺寸原图是「点进群聊转彩虹圈」的放大器。
    var maxPixelSize: Int? = nil

    var body: some View {
        #if os(macOS)
        CrewRemoteImage(path: path, contentMode: contentMode, maxPixelSize: maxPixelSize)
        #else
        CrewCrossRemoteImage(path: path, contentMode: contentMode)
        #endif
    }
}

/// Identifiable wrapper so a tapped image path can drive `.fullScreenCover`.
/// Vendored BubbleView's AttachmentGrid uses this. Defined here so it is
/// visible alongside ServerImage (its only consumer prior to A4).
///
/// NOTE: BubbleView.swift defines its own `private struct ZoomTarget` — when
/// BubbleView is copied in A4 the file-private definition will shadow this one
/// within that file. This top-level version is provided as a compile-time
/// anchor in case anything references it outside BubbleView; it causes no
/// conflict because the BubbleView copy is `private`.
// (No struct definition needed here — BubbleView defines ZoomTarget privately.)

#if os(iOS)
// MARK: - iOS cross-platform image loader

/// Actor-isolated image cache using PlatformImage (UIImage on iOS).
/// Mirrors the structure of CrewImageCache but works cross-platform.
actor CrewCrossImageCache {
    static let shared = CrewCrossImageCache()

    private var cache: [String: PlatformImage] = [:]
    private var inFlight: [String: Task<PlatformImage?, Never>] = [:]

    func image(relativePath: String, baseURL: URL, token: String) async -> PlatformImage? {
        if let hit = cache[relativePath] { return hit }
        if let task = inFlight[relativePath] { return await task.value }

        let task = Task<PlatformImage?, Never> { [relativePath, baseURL, token] in
            await Self.fetch(relativePath: relativePath, baseURL: baseURL, token: token)
        }
        inFlight[relativePath] = task
        let result = await task.value
        inFlight[relativePath] = nil
        if let result { cache[relativePath] = result }
        return result
    }

    private static func fetch(relativePath: String, baseURL: URL, token: String) async -> PlatformImage? {
        let trimmed = relativePath.hasPrefix("/") ? String(relativePath.dropFirst()) : relativePath
        let absolute = baseURL.appendingPathComponent(trimmed)
        var req = URLRequest(url: absolute)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        do {
            let (data, response) = try await URLSession.shared.data(for: req)
            guard let http = response as? HTTPURLResponse,
                  (200..<300).contains(http.statusCode) else { return nil }
            return PlatformImage.decode(data)
        } catch {
            return nil
        }
    }
}

/// A UIImage-backed SwiftUI view that loads an auth-gated crew attachment
/// image on iOS, showing a placeholder while loading and an error glyph on
/// failure. Mirrors CrewRemoteImage but uses PlatformImage (UIImage) instead
/// of NSImage so it works cross-platform.
struct CrewCrossRemoteImage: View {
    let path: String
    var contentMode: ContentMode = .fill

    @EnvironmentObject private var appModel: AppModel
    @State private var image: PlatformImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(platformImage: image)
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
                    ProgressView()
                }
            }
        }
        .task(id: path) { await load() }
    }

    private func load() async {
        image = nil
        failed = false
        guard let auth = appModel.imageAuth else {
            failed = true
            return
        }
        let loaded = await CrewCrossImageCache.shared.image(
            relativePath: path, baseURL: auth.baseURL, token: auth.token)
        if let loaded {
            image = loaded
        } else {
            failed = true
        }
    }
}
#endif
