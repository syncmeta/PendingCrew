import XCTest

/// 自绘滚动条的几何换算（Todo #34 第 4 条）。真实历史长度经 SwiftTerm 的
/// `scrollThumbsize` / `scrollPosition` 进来（那条链在 `TerminalScrollbackTests` 里钉），
/// 这里保证换算本身不引入偏差：knob 两端各自对准历史两端，画上去和拖回来互为逆运算。
final class TerminalScrollbarGeometryTests: XCTestCase {

    /// 与 `TerminalScrollbarOverlay` 上的常量一致。
    private let geometry = TerminalScrollbarGeometry(trackExtent: 600, inset: 3, minKnobExtent: 28)

    func testKnobSpansTheWholeTrackWhenThereIsNoHistory() {
        // thumbSize = 1 表示内容不足一屏：knob 顶满，拖动没有余量。
        XCTAssertEqual(geometry.knobExtent(thumbSize: 1), geometry.usableExtent)
        XCTAssertEqual(geometry.travel(thumbSize: 1), 0)
        XCTAssertEqual(geometry.position(forPointerAt: 400, thumbSize: 1), 0)
    }

    func testKnobShrinksProportionallyAsHistoryGrows() {
        // 一屏 48 行：历史 480 行 → 1/10 条；历史 4800 行 → 1/100 条。
        XCTAssertEqual(geometry.knobExtent(thumbSize: 48.0 / 480), geometry.usableExtent / 10, accuracy: 0.001)
        let long = geometry.knobExtent(thumbSize: 48.0 / 4800)
        XCTAssertLessThan(long, geometry.usableExtent / 10, "历史越长条越细")
        XCTAssertEqual(long, geometry.minKnobExtent, "细到抓不住时兜到最小长度")
    }

    func testKnobEndsLineUpWithTheEndsOfTheHistory() {
        let thumb = 48.0 / 1000
        // 顶 = 最老一行，底 = 最新一行：knob 必须正好贴住轨道两端，不多不少。
        XCTAssertEqual(geometry.knobOffset(position: 0, thumbSize: thumb), geometry.inset, accuracy: 0.0001)
        XCTAssertEqual(geometry.knobOffset(position: 1, thumbSize: thumb) + geometry.knobExtent(thumbSize: thumb),
                       geometry.trackExtent - geometry.inset, accuracy: 0.0001)
    }

    /// 拖到某处读回来的 position，再画出去必须落回同一处 —— 否则「能滑的比条显示的多/少」。
    func testPointerAndKnobAreInverses() {
        let thumb = 48.0 / 1000
        for position in stride(from: 0.0, through: 1.0, by: 0.05) {
            let knobCenter = geometry.knobOffset(position: position, thumbSize: thumb)
                + geometry.knobExtent(thumbSize: thumb) / 2
            XCTAssertEqual(geometry.position(forPointerAt: knobCenter, thumbSize: thumb), position,
                           accuracy: 0.0001, "position \(position) 拖动往返对不上")
        }
    }

    func testPointerBeyondTheTrackIsClampedToTheEndsOfTheHistory() {
        let thumb = 48.0 / 1000
        XCTAssertEqual(geometry.position(forPointerAt: -500, thumbSize: thumb), 0)
        XCTAssertEqual(geometry.position(forPointerAt: 5000, thumbSize: thumb), 1)
    }

    func testDegenerateTrackDoesNotProduceNaN() {
        let tiny = TerminalScrollbarGeometry(trackExtent: 0, inset: 3, minKnobExtent: 28)
        XCTAssertEqual(tiny.usableExtent, 0)
        XCTAssertEqual(tiny.travel(thumbSize: 0.1), 0)
        XCTAssertEqual(tiny.position(forPointerAt: 10, thumbSize: 0.1), 0)
        XCTAssertFalse(tiny.knobOffset(position: 0.5, thumbSize: 0.1).isNaN)
    }
}
