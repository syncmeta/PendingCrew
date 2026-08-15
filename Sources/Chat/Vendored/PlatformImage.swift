// VENDORED from PendingBot apps/pendingbot/Sources/Components/PlatformImage.swift @ c63d3989
// 再对齐 = 对照源文件重拷。勿在 PendingCrew 侧手改主体;差异收进 shim/adapter。
import SwiftUI
#if os(iOS)
import UIKit
/// The platform's bitmap image type — `UIImage` on iOS, `NSImage` on macOS.
/// Lets cross-platform views (e.g. `ServerImage`) decode + hold an image without
/// `#if` at every call site.
typealias PlatformImage = UIImage
#elseif os(macOS)
import AppKit
typealias PlatformImage = NSImage
#endif

extension Image {
    /// Build a SwiftUI `Image` from a platform bitmap, hiding the
    /// `uiImage:` / `nsImage:` split.
    init(platformImage: PlatformImage) {
        #if os(iOS)
        self.init(uiImage: platformImage)
        #elseif os(macOS)
        self.init(nsImage: platformImage)
        #endif
    }
}

extension PlatformImage {
    /// Decode raw image bytes into a platform bitmap, hiding the
    /// `UIImage(data:)` / `NSImage(data:)` split.
    static func decode(_ data: Data) -> PlatformImage? {
        #if os(iOS)
        return UIImage(data: data)
        #elseif os(macOS)
        return NSImage(data: data)
        #endif
    }

    /// The backing `CGImage` for this bitmap, hiding the `UIImage.cgImage` /
    /// `NSImage.cgImage(forProposedRect:context:hints:)` split. Used for
    /// CoreGraphics-based downscaling on either platform.
    var uploadCGImage: CGImage? {
        #if os(iOS)
        return self.cgImage
        #elseif os(macOS)
        return self.cgImage(forProposedRect: nil, context: nil, hints: nil)
        #endif
    }

    /// Wrap a `CGImage` back into a platform bitmap, hiding the
    /// `UIImage(cgImage:)` / `NSImage(cgImage:size:)` split.
    static func fromCGImage(_ cg: CGImage) -> PlatformImage {
        #if os(iOS)
        return UIImage(cgImage: cg)
        #elseif os(macOS)
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
        #endif
    }

    /// JPEG-encode this bitmap, hiding the `UIImage.jpegData` /
    /// `NSBitmapImageRep` split. `quality` is the 0…1 compression quality.
    /// Returns nil if the platform can't produce a JPEG representation.
    func jpegData(quality: CGFloat) -> Data? {
        #if os(iOS)
        return self.jpegData(compressionQuality: quality)
        #elseif os(macOS)
        // NSImage has no direct JPEG export; go through a bitmap rep. Prefer
        // a rep built from the image's CGImage so the pixel dimensions match
        // the bitmap (NSImage.size is in points, which would otherwise under-
        // sample on a >1x backing).
        guard let cg = self.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            return nil
        }
        let rep = NSBitmapImageRep(cgImage: cg)
        return rep.representation(using: .jpeg, properties: [.compressionFactor: quality])
        #endif
    }
}
