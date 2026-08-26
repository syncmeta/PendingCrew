// SHIM for PendingBot Components/ServerImage.swift — internals route through
// CrewRemoteImage on macOS.
//
// Public init signature is IDENTICAL to PendingBot's ServerImage so that
// AttachmentGrid in the vendored BubbleView constructs it unchanged:
//
//   ServerImage(path: att.url, serverURL: serverURL, contentMode: .fill)
//
// The `serverURL` parameter is accepted but ignored — `path` 是本地落盘附件的
// `file://` 绝对 URL（Todo #3），加载器直接从磁盘读。
//
// macOS: delegates to CrewRemoteImage (NSImage-backed, actor-cached).
// iOS:   本地后端是 macOS-only，iOS 上没有任何附件来源（#63 第二期删掉 edge
//        上传通道之后更是如此）—— 画一块占位，别假装在加载。

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
        ZStack {
            Theme.Palette.surfaceMuted
            Image(systemName: "photo")
                .foregroundStyle(Theme.Palette.inkMuted)
        }
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
