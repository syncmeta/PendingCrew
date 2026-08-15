#if os(macOS)
import XCTest
import Foundation

/// 源码级防线：**SwiftUI 的 `.repeatForever` 在本 app 里一律不许写**。
///
/// 立这道闸是因为同一个病根已经冒头三次了：
/// - 2026-07-26 17:24：群聊「正在输入」三个点 → 闪退（#555）
/// - 2026-08-08：session 头像「需要人出手」呼吸红点（同一次修复里一并改掉）
/// - 2026-08-10 20:49：Todo 状态圆圈「进行中」呼吸 → 再次闪退
///
/// 每次都是**新写的**代码，不是旧修复失效。说明问题不在某个组件，在于
/// 「随手写一个 `.repeatForever` 呼吸动画」这件事本身没人拦。所以拦在这里。
///
/// ## 为什么禁
///
/// `repeatForever` 在 SwiftUI 的动画事务里**永不结束**。只要它和一个懒容器
/// （`LazyVStack`/`LazyHStack`）或一个「滚动位置被管理」的 `ScrollView` 同处一个
/// `NSHostingView`，每帧动画都会让视图图重解析 → 懒容器重算可见窗口、把子图摘下
/// 插回 → 重插的子图带 appearance effect（`.task`/`.onAppear`）→ `graphDidChange`
/// → `NSHostingView.requestUpdate` → `setNeedsUpdateConstraints` → 窗口又脏了。
/// 一个显示周期就此结束不了，AppKit 数到 275 次重标就抛 NSException 打死进程
/// （`+[NSApplication _crashOnException:]`）。**全程不需要任何人操作。**
///
/// 更阴的是这条环快慢不定：2026-07-26 那条上万 Hz，2026-08-10 这条只有 ~9.4Hz，
/// 慢到能骗过「安静窗口 layout() < 50 次」的判据（见 `LayoutLoopRegressionTests`）。
/// 所以光靠行为回归测试守不住，必须在源码这层直接堵。
///
/// ## 那要动效怎么办 —— 走 CoreAnimation，有现成件可抄/可复用
///
/// 让图层自己按 CoreAnimation 的时钟插值，SwiftUI 侧的几何自始至终静态
/// （固定尺寸、`sizeThatFits` 恒定），锚点解析一次即收敛：
///
/// - `TypingDotsLayerView`（`Sources/Mac/Views/Chat/`）—— 多点相位错开的透明度呼吸
/// - `BreathingDot`（`Sources/Views/BreathingDotView.swift`）—— 单个实心圆点呼吸
/// - `BreathingSymbol`（`Sources/Mac/Views/BreathingSymbolView.swift`）—— SF Symbol
///   呼吸（透明度 + 缩放）；布局与文字基线仍由 SwiftUI 的 `Image` 出，动画画在
///   `overlay` 的 `CALayer` 上，不回流几何。**新的呼吸动效优先直接用它。**
///
/// 三者都被 `LayoutLoopRegressionTests` 的「安静窗口」用例钉着。
///
/// ## 真要放行怎么办
///
/// 把文件加进 `allowlist` 并在这儿写清楚为什么它安全（例如：确定不与任何懒容器 /
/// 受管滚动视图共处同一个 hosting view，且几何静态）。**别删这条测试**，也别为了
/// 过测把 `repeatForever` 拆成字符串拼接绕开扫描 —— 那是骗自己。
final class NoSwiftUIRepeatForeverTests: XCTestCase {

    /// 允许出现 `repeatForever` **代码**的文件（相对 `Sources/`）。
    ///
    /// **目前是空的，这是刻意的** —— 全仓库没有一处该这么写。注释不受限（好几个文件
    /// 顶部的验尸说明都会提到这个词），扫描会跳过注释行，见 `isCommentLine`。
    /// 真要放行一处，加进来并在旁边写清楚为什么它安全。
    private static let allowlist: Set<String> = []

    func testNoSwiftUIRepeatForeverOutsideAllowlist() throws {
        let sources = Self.sourcesDirectory()
        let files = try Self.swiftFiles(under: sources)
        XCTAssertGreaterThan(
            files.count, 100,
            "只扫到 \(files.count) 个 .swift —— 扫描根目录多半找错了（\(sources.path)）。"
            + "这条测试靠遍历源码生效，扫不到文件等于没设防，先把路径修对。")

        var offenders: [String] = []
        for file in files {
            let rel = Self.relativePath(of: file, under: sources)
            guard !Self.allowlist.contains(rel) else { continue }
            let text = try String(contentsOf: file, encoding: .utf8)
            for (line, text) in Self.offendingLines(in: text) {
                offenders.append("\(rel):\(line): \(text)")
            }
        }

        XCTAssertTrue(
            offenders.isEmpty,
            """
            发现 SwiftUI `.repeatForever`，这是布局自激闪退的病根，一律不许写：

            \(offenders.joined(separator: "\n"))

            要做「一直循环」的动效，改用 CoreAnimation 那条路 —— 现成件可以直接用：
              · SF Symbol 呼吸 → `BreathingSymbol`（Sources/Mac/Views/BreathingSymbolView.swift）
              · 单个圆点呼吸   → `BreathingDot`（Sources/Views/BreathingDotView.swift）
              · 多点相位错开   → `TypingDotsLayerView`（Sources/Mac/Views/Chat/）
            为什么禁、以及确认安全后如何加白名单，见本文件顶部说明。
            """)
    }

    /// 闸自己的自检：**证明它真的抓得住**，而不是恒绿。
    /// 上面那条测试全绿有两种可能 —— 源码干净，或者扫描根本没在扫。这条把后者堵死。
    func testScannerCatchesRealCodeButNotProse() {
        let sample = """
        /// 说明里提到 .repeatForever 是允许的（这行是注释）。
        struct Dot: View {
            var body: some View {
                Circle().opacity(on ? 1 : 0.3)
                    .animation(.easeInOut(duration: 1).repeatForever(autoreverses: true), value: on)
            }
        }
        """
        let hits = Self.offendingLines(in: sample)
        XCTAssertEqual(hits.count, 1, "该只抓到那行代码，实际抓到 \(hits.map(\.line))")
        XCTAssertEqual(hits.first?.line, 5)
        XCTAssertTrue(hits.first?.text.hasPrefix(".animation(") == true)
    }

    // MARK: - 扫描

    /// 一份源码里所有「写了 `repeatForever` 的非注释行」（行号从 1 起）。
    private static func offendingLines(in text: String) -> [(line: Int, text: String)] {
        guard text.contains("repeatForever") else { return [] }
        return text.components(separatedBy: .newlines).enumerated().compactMap { i, line in
            guard line.contains("repeatForever"), !isCommentLine(line) else { return nil }
            return (i + 1, line.trimmingCharacters(in: .whitespaces))
        }
    }

    /// 整行注释就放过 —— 好几处验尸说明都要提这个词，禁的是**写出来的代码**。
    /// 故意只认「整行是注释」：真要写自激动画，那行长得像
    /// `.animation(.easeInOut(…).repeatForever(…), value: x)`，绝不会以 `//` 开头。
    /// 反过来说这确实挡不住把代码藏在行尾注释后面 —— 但那是骗自己，不是这条闸的职责。
    private static func isCommentLine(_ line: String) -> Bool {
        let t = line.trimmingCharacters(in: .whitespaces)
        return t.hasPrefix("//") || t.hasPrefix("*") || t.hasPrefix("/*")
    }

    /// 从本测试源文件位置回推仓库里的 `Sources/`（`#filePath` 指向
    /// `apps/pendingcrew/Tests/PendingCrewTests/…`，上跳两级即 `apps/pendingcrew/`）。
    /// 不用 `Bundle`：测试 bundle 里没有源码。
    private static func sourcesDirectory() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()    // PendingCrewTests
            .deletingLastPathComponent()    // Tests
            .deletingLastPathComponent()    // apps/pendingcrew
            .appendingPathComponent("Sources")
    }

    private static func swiftFiles(under root: URL) throws -> [URL] {
        guard let e = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: [.isRegularFileKey]) else { return [] }
        return e.compactMap { $0 as? URL }.filter { $0.pathExtension == "swift" }
    }

    private static func relativePath(of file: URL, under root: URL) -> String {
        let base = root.standardizedFileURL.path
        let full = file.standardizedFileURL.path
        guard full.hasPrefix(base + "/") else { return full }
        return String(full.dropFirst(base.count + 1))
    }
}
#endif
