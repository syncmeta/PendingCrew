import XCTest
import Foundation

/// 排序原料那张表（Todo #102 / 人类 #113 追问：三列原样交出去，不加权）。
///
/// 这里钉的是那条反复栽过的：**「读不出来」和「确实没有」不许压成同一句话。**
final class CrewOrderingSignalsTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func row(_ id: String, any: TimeInterval? = nil,
                     human: TimeInterval? = nil, opened: TimeInterval? = nil)
        -> CrewOrderingSignals.Row {
        CrewOrderingSignals.Row(
            crewId: id, title: id,
            lastAnyMessageAt: any.map { now.addingTimeInterval(-$0) },
            lastHumanMessageAt: human.map { now.addingTimeInterval(-$0) },
            lastOpenedAt: opened.map { now.addingTimeInterval(-$0) })
    }

    func testThreeColumnsComeOutRawWithNoScore() {
        let text = CrewOrderingSignals.render(
            rows: [row("a", any: 60, human: 86_400 * 3, opened: 300)],
            now: now, openedColumn: .hasData)
        XCTAssertTrue(text.contains("① 1 分钟前"), text)
        XCTAssertTrue(text.contains("② 3 天前"), text)
        XCTAssertTrue(text.contains("③ 5 分钟前"), text)
        // 不许吐出「综合得分」那一族的东西 —— 判断是 agent 的活，加权只是把
        // 「替他算」算得更花哨。
        //
        // **这条断言我写错过一次**：原本还断言正文里不出现「推荐」二字，结果红了 ——
        // 红在表头那句「没有推荐顺序」的免责声明上。查的是「有没有吐出推荐」，
        // 断言却写成了「有没有出现这两个字」，**那是拿字符串出现与否代替那件事本身**。
        // 改成查真正会出现的形状：带名次的行、或者「分」那一列。
        XCTAssertFalse(text.contains("得分"), text)
        for line in text.split(separator: "\n") where line.hasPrefix("- ") {
            XCTAssertFalse(line.contains("分数"), String(line))
            XCTAssertNil(line.range(of: #"^\- \d+\."#, options: .regularExpression),
                         "行首不该有名次：\(line)")
        }
        XCTAssertTrue(text.contains("没有加权"), text)
    }

    func testUnreadableColumnIsNotReportedAsEmpty() {
        // 这条是这个类型存在的理由：读不出来 ≠ 一条都没有。
        let unreadable = CrewOrderingSignals.render(
            rows: [row("a", any: 60)], now: now, openedColumn: .unreadable)
        XCTAssertTrue(unreadable.contains("我看不出来"), unreadable)
        XCTAssertFalse(unreadable.contains("确实还一条都没有"), unreadable)

        let empty = CrewOrderingSignals.render(
            rows: [row("a", any: 60)], now: now, openedColumn: .emptySoFar)
        XCTAssertTrue(empty.contains("确实还一条都没有"), empty)
        XCTAssertFalse(empty.contains("我看不出来"), empty)
    }

    func testEveryColumnShipsWithItsOwnBoundary() {
        // 一个没有边界的读数不是更干净，是这栏没被要过。
        let text = CrewOrderingSignals.render(
            rows: [row("a", any: 60)], now: now, openedColumn: .hasData)
        XCTAssertTrue(text.contains("动静大多是 agent 发的"), text)
        XCTAssertTrue(text.contains("分辨率很低"), text)
        XCTAssertTrue(text.contains("早期很稀疏"), text)
    }

    func testMissingValueRendersAsAnExplicitBlankNotZero() {
        XCTAssertEqual(CrewOrderingSignals.ago(nil, now: now), "—")
        XCTAssertEqual(CrewOrderingSignals.ago(now, now: now), "0 秒前")
    }

    func testNoCrewsSaysSoInsteadOfPrintingAnEmptyTable() {
        XCTAssertEqual(CrewOrderingSignals.render(rows: [], now: now, openedColumn: .hasData),
                       "（这台机器上没有 crew）")
    }

    func testRowOrderIsStableButNotAdvice() {
        // 表里行序只是为了每次一样（按 ① 倒序 + id 兜底），**不是建议顺序**。
        let text = CrewOrderingSignals.render(
            rows: [row("old", any: 6000), row("new", any: 60)],
            now: now, openedColumn: .hasData)
        let newIdx = text.range(of: "- new")!.lowerBound
        let oldIdx = text.range(of: "- old")!.lowerBound
        XCTAssertTrue(newIdx < oldIdx)
        XCTAssertTrue(text.contains("判断是你的活"), text)
    }
}
