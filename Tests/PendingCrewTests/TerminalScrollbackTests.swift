#if os(macOS)
import XCTest
import SwiftTerm

/// Todo #34「往上滑看不到完整历史 / 滑上去排版是乱的」的回归。
///
/// 两条病根，都钉在这里：
/// 1. **上限太小** —— SwiftTerm 默认 `scrollback = 500` 行，agent 跑一会儿就顶满。
/// 2. **幻影布局把历史绞了** —— SwiftUI 重挂终端视图必经一拍 `.zero` frame（切 crew
///    就会发生），SwiftTerm 照单全收 resize 到 `MINIMUM_COLS = 2`，reflow 按 2 字宽把
///    所有历史重新折行 → 行数炸穿上限 → 顶部被永久裁掉；折回真实宽度只能拼回幸存的
///    碎片，最老一行还断在半截字上。这既吃历史也把排版拼乱。
@MainActor
final class TerminalScrollbackTests: XCTestCase {

    /// 宽到能排下 `lineWidth` 个字符的视口（默认 13pt 等宽 cell ≈ 7.8pt，900pt → 103 列）。
    private let viewport = NSSize(width: 900, height: 600)
    /// 贴近 agent 真实输出的行宽。**这个数不能随便调小**：幻影 `.zero` 布局的破坏力
    /// 取决于「历史行数 × ⌈行宽 ÷ 2⌉ 是否冲破 scrollback 上限」——行太窄就折不炸上限，
    /// 测试会假绿（本轮就先踩了一次：40 字宽 × 400 行只折出 8000 行，没到 10000）。
    private let lineWidth = 90

    private func makeView() -> TerminalMirrorView {
        let view = TerminalMirrorView(frame: NSRect(origin: .zero, size: viewport))
        // init(frame:) 不走 setFrameSize，这里显式跑一次真布局把 cols/rows 定下来。
        view.setFrameSize(viewport)
        return view
    }

    private func feedLines(_ view: TerminalMirrorView, count: Int) {
        for i in 1...count {
            let tag = "L\(i)-"
            view.feed(text: tag + String(repeating: "#", count: max(0, lineWidth - tag.count)) + "\r\n")
        }
    }

    /// 回滚缓冲里现存的全部非空行（含被卷上去的历史 + 当前视口）。
    /// `getScrollInvariantLine` 用的是「从没被裁掉过的绝对行号」，所以顶部被裁时低位
    /// 行号会返回 nil —— 正好用来看有没有掉历史。
    private func retainedLines(_ view: TerminalMirrorView) -> [String] {
        let terminal = view.getTerminal()
        var out: [String] = []
        for row in 0..<200_000 {
            guard let line = terminal.getScrollInvariantLine(row: row) else {
                if out.isEmpty { continue }   // 顶部被裁掉的那段，跳过去继续找
                break                          // 已经走完现存区间
            }
            let text = line.translateToString(trimRight: true)
            if !text.isEmpty { out.append(text) }
        }
        return out
    }

    // MARK: - 病根 1：上限

    func testScrollbackLimitIsRaisedWellAboveSwiftTermDefault() {
        let view = makeView()
        XCTAssertEqual(view.getTerminal().options.scrollback, TerminalMirrorView.scrollbackLines,
                       "终端构造完必须把 scrollback 顶上去，别留 SwiftTerm 的 500 行默认值")
        XCTAssertGreaterThanOrEqual(TerminalMirrorView.scrollbackLines, 10_000,
                                    "10000 行是这次定的量级（对齐 Terminal.app），调低要连注释里的理由一起改")
    }

    /// 3000 行历史必须一行不掉 —— 这个量在旧的 500 行上限下会被裁掉 5/6。
    func testHistoryFarBeyondTheOldDefaultIsFullyRetained() {
        let view = makeView()
        feedLines(view, count: 3000)

        let lines = retainedLines(view)
        XCTAssertEqual(lines.count, 3000, "3000 行历史应当一行不掉（旧的 500 行上限只留得下 500）")
        XCTAssertTrue(lines.first?.hasPrefix("L1-") == true,
                      "最老一行应当还是 L1，实际是 \(lines.first ?? "nil")")
        XCTAssertTrue(lines.last?.hasPrefix("L3000-") == true,
                      "最新一行应当是 L3000，实际是 \(lines.last ?? "nil")")
    }

    // MARK: - 病根 2：幻影 .zero 布局

    /// 切 crew 时 SwiftUI 会先把视图尺寸打成 `.zero` 再给真尺寸。历史必须**原样**活下来。
    /// 没有守卫时实测：400 行历史过一次 `.zero` 只剩 223 行（旧的 500 行上限下只剩 12 行），
    /// 且最老一行断在半截字上 —— 那就是用户看到的「滑上去排版是乱的」。
    func testPhantomZeroSizedLayoutDoesNotShredHistory() {
        let view = makeView()
        feedLines(view, count: 400)
        let before = retainedLines(view)
        XCTAssertEqual(before.count, 400, "前置条件：400 行历史都在")

        view.setFrameSize(.zero)        // SwiftUI 重挂必经的那一拍
        view.setFrameSize(viewport)     // 真尺寸随后到达

        let after = retainedLines(view)
        XCTAssertEqual(after.count, before.count, "过一趟幻影 .zero 布局不许掉行")
        XCTAssertEqual(after, before, "历史内容必须逐行原样，不许被 2 列 reflow 拼成碎片")
        XCTAssertTrue(after.first?.hasPrefix("L1-") == true,
                      "最老一行仍应是完整的 L1，而不是断在半截的碎片：\(after.first ?? "nil")")
    }

    func testDegenerateSizesAreRejectedAndRealOnesAccepted() {
        XCTAssertFalse(TerminalMirrorView.isRealLayout(.zero))
        XCTAssertFalse(TerminalMirrorView.isRealLayout(NSSize(width: 900, height: 0)))
        XCTAssertFalse(TerminalMirrorView.isRealLayout(NSSize(width: 0, height: 600)))
        XCTAssertTrue(TerminalMirrorView.isRealLayout(NSSize(width: 900, height: 600)))
        // 用户自己把栏拖窄那种**真实**窄布局照常放行 —— 那是终端应有的 reflow 行为。
        XCTAssertTrue(TerminalMirrorView.isRealLayout(NSSize(width: 120, height: 80)))
    }

    // MARK: - 自绘滚动条的可滚范围 vs 真实历史

    /// overlay 的几何全部取自 SwiftTerm 的 `canScroll` / `scrollPosition` / `scrollThumbsize`，
    /// 这里钉住它们确实跟着真实行数走，且两端各自落到最老 / 最新一行。
    func testScrollbarRangeMatchesRealHistory() {
        let view = makeView()
        let rows = view.getTerminal().rows
        feedLines(view, count: 1000)

        XCTAssertTrue(view.canScroll, "有 1000 行历史时必须可滚")

        // thumbSize = 可见行数 ÷ 总行数。总行数 = 1000 行历史 + 光标所在的空行。
        let totalLines = retainedLines(view).count + 1
        XCTAssertEqual(Double(view.scrollThumbsize), Double(rows) / Double(totalLines), accuracy: 0.005,
                       "条的粗细必须等于「一屏 ÷ 全部历史」，否则能滑的跟条显示的对不上")

        // 拉到顶 → 视口第一行就是最老那行；拉到底 → 回到最新。
        view.scroll(toPosition: 0)
        XCTAssertEqual(view.scrollPosition, 0, accuracy: 0.0001)
        XCTAssertTrue(view.getTerminal().getLine(row: 0)?.translateToString(trimRight: true)
            .hasPrefix("L1-") == true, "position 0 必须落在最老的一行 L1")

        view.scroll(toPosition: 1)
        XCTAssertEqual(view.scrollPosition, 1, accuracy: 0.0001)
        XCTAssertTrue(view.getTerminal().getLine(row: rows - 2)?.translateToString(trimRight: true)
            .hasPrefix("L1000-") == true, "position 1 必须落在最新的一行 L1000")
    }
}
#endif
