// VENDORED from PendingBot apps/pendingbot/Sources/Components/MathRendering.swift @ 43c8ea2e
// 再对齐 = 对照源文件重拷。仅允许偏离打 `// PENDINGCREW SHIM:` 标注。
import SwiftUI
import MarkdownUI
import SwiftMath
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

// MarkdownUI has no LaTeX support — it renders `\(x^2\)` / `$$…$$` verbatim.
// The bridge here is: rewrite every math span into an `![](pendingbot-math://…)`
// image reference, then hand a math-aware image provider to the renderer that
// typesets the LaTeX (via SwiftMath) into a platform image. Inline math flows
// through MarkdownUI's `InlineImageProvider`; a formula alone in a paragraph
// (display math) flows through the block `ImageProvider`.
//
// Cross-platform: `MathMarkup` is pure Foundation. The image layer uses
// `UIImage` on iOS and `NSImage` on macOS — SwiftMath's `MTMathImage` is
// itself cross-platform (its `asImage()` returns `MTImage`, i.e. UIImage on
// iOS / NSImage on macOS), so both platforms share the exact same renderer.

// MARK: - Platform color shim

// `PlatformImage`(UIImage/NSImage)和 `Image(platformImage:)` 已在
// Components/PlatformImage.swift 提供,这里只补一个颜色别名。
#if canImport(UIKit)
private typealias PlatformColor = UIColor
#elseif canImport(AppKit)
private typealias PlatformColor = NSColor
#endif

private extension PlatformImage {
    var pixelWidthPoints: CGFloat { size.width }
}

// MARK: - Markup rewriting
//
// PENDINGCREW SHIM(代码块不渲染 2026-08-11): `MathMarkup`(纯 Foundation 的
// 分段 + 重写逻辑)已原样搬到同目录 MathMarkup.swift —— 为了能脱离
// SwiftUI/MarkdownUI/SwiftMath 编进 PendingCrewTests 做围栏回归守卫。
// 再对齐源文件时，两个文件合起来才等于 PendingBot 的 MathRendering.swift。

// MARK: - LaTeX → platform image

/// Typesets LaTeX into a platform image via SwiftMath and memoises the result.
/// Rendering is fast (sub-millisecond) but a chat scroll re-lays out
/// constantly, so the cache keeps it off the hot path.
@MainActor
final class MathImageRenderer {
    static let shared = MathImageRenderer()
    private var cache: [String: PlatformImage] = [:]

    func image(latex: String, display: Bool, color: Color, fontSize: CGFloat, scheme: ColorScheme) -> PlatformImage {
        // The palette colors are appearance-adaptive, so both the cache key and
        // the rasterised color must be pinned to an explicit scheme — keying on
        // `String(describing: color)` (identical for both variants of a dynamic
        // color) would serve light-ink bitmaps in dark mode.
        let platformColor = Self.resolve(color, for: scheme)
        let key = "\(display)|\(Int(fontSize * 2))|\(scheme == .dark ? "dark" : "light")|\(latex)"
        if let cached = cache[key] { return cached }
        let image = Self.render(latex: latex, display: display, color: platformColor, fontSize: fontSize)
        cache[key] = image
        return image
    }

    /// Resolve a (possibly dynamic) Color to the concrete variant for `scheme`,
    /// independent of whatever appearance is ambient on the calling thread.
    private static func resolve(_ color: Color, for scheme: ColorScheme) -> PlatformColor {
        #if canImport(UIKit)
        return PlatformColor(color).resolvedColor(
            with: UITraitCollection(userInterfaceStyle: scheme == .dark ? .dark : .light)
        )
        #elseif canImport(AppKit)
        let dynamic = PlatformColor(color)
        guard let appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua) else {
            return dynamic
        }
        var resolved = dynamic
        appearance.performAsCurrentDrawingAppearance {
            // `.cgColor` resolves a dynamic NSColor against the appearance that
            // is current at access time.
            resolved = PlatformColor(cgColor: dynamic.cgColor) ?? dynamic
        }
        return resolved
        #endif
    }

    private static func render(latex: String, display: Bool, color: PlatformColor, fontSize: CGFloat) -> PlatformImage {
        let math = MTMathImage(
            latex: latex,
            fontSize: fontSize,
            textColor: color,
            labelMode: display ? .display : .text
        )
        let (error, image) = math.asImage()
        if error == nil, let image { return image }
        // SwiftMath couldn't parse the LaTeX — show the raw source rather
        // than dropping the formula entirely.
        return textFallback(latex, color: color, fontSize: fontSize)
    }

    private static func textFallback(_ source: String, color: PlatformColor, fontSize: CGFloat) -> PlatformImage {
        #if canImport(UIKit)
        let font = UIFont.monospacedSystemFont(ofSize: fontSize * 0.92, weight: .regular)
        #elseif canImport(AppKit)
        let font = NSFont.monospacedSystemFont(ofSize: fontSize * 0.92, weight: .regular)
        #endif
        let attrs: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: color,
        ]
        let string = NSAttributedString(string: source, attributes: attrs)
        let size = string.size()
        let pixelSize = CGSize(width: max(1, ceil(size.width)), height: max(1, ceil(size.height)))
        #if canImport(UIKit)
        let renderer = UIGraphicsImageRenderer(size: pixelSize)
        return renderer.image { _ in string.draw(at: .zero) }
        #elseif canImport(AppKit)
        let image = NSImage(size: pixelSize)
        image.lockFocus()
        string.draw(at: .zero)
        image.unlockFocus()
        return image
        #endif
    }
}

// MARK: - MarkdownUI providers

/// Inline image provider — renders inline math, delegates everything else
/// (real `![]()` images) to MarkdownUI's default provider.
struct MathInlineImageProvider: InlineImageProvider {
    let textColor: Color
    let fontSize: CGFloat
    /// Pinned by the hosting view (MarkdownText reads `\.colorScheme`); the
    /// provider value changing on a mode flip is what re-triggers MarkdownUI
    /// to re-request inline images in the new ink color.
    let scheme: ColorScheme

    func image(with url: URL, label: String) async throws -> Image {
        if let math = MathMarkup.parse(url) {
            let platformImage = await MathImageRenderer.shared.image(
                latex: math.latex,
                display: math.display,
                color: textColor,
                fontSize: fontSize,
                scheme: scheme
            )
            return Image(platformImage: platformImage)
        }
        return try await DefaultInlineImageProvider.default.image(with: url, label: label)
    }
}

/// Block image provider — renders display math centered, delegates real
/// images to MarkdownUI's default provider.
struct MathBlockImageProvider: ImageProvider {
    let textColor: Color
    let fontSize: CGFloat

    @ViewBuilder
    func makeImage(url: URL?) -> some View {
        if let url, let math = MathMarkup.parse(url) {
            MathBlockImage(
                latex: math.latex,
                textColor: textColor,
                // Display math gets a touch more size than body text.
                fontSize: fontSize * 1.1
            )
        } else {
            DefaultImageProvider.default.makeImage(url: url)
        }
    }
}

private struct MathBlockImage: View {
    let latex: String
    let textColor: Color
    let fontSize: CGFloat

    @Environment(\.colorScheme) private var colorScheme
    @State private var image: PlatformImage?

    var body: some View {
        Group {
            if let image {
                Image(platformImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(maxWidth: image.pixelWidthPoints)
            } else {
                Color.clear.frame(height: fontSize)
            }
        }
        .frame(maxWidth: .infinity, alignment: .center)
        .padding(.vertical, 4)
        // Re-render on appearance change so the formula color tracks the
        // surrounding text.
        .task(id: colorScheme) {
            image = await MathImageRenderer.shared.image(
                latex: latex,
                display: true,
                color: textColor,
                fontSize: fontSize,
                scheme: colorScheme
            )
        }
    }
}
