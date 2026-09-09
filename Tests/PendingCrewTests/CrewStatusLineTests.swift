import XCTest
import Foundation

/// 侧栏「总机长」视图每行那句状态（人类 Todo #136）。
///
/// 换掉的是「这个群最后一条消息」——那是一手的、永远为真。所以这里钉的不是
/// 「显示得好不好看」，是**它有没有把自己的可信度一起说出来**：
/// 没填过就说没填过（别编）、沿用来的要带年龄（别让人以为是刚刚的）。
final class CrewStatusLineTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func iso(_ secondsAgo: TimeInterval) -> String {
        ISO8601DateFormatter().string(from: now.addingTimeInterval(-secondsAgo))
    }

    private func line(_ messages: [(status: String?, createdAt: String)]) -> CrewStatusLine.Line {
        CrewStatusLine.make(resolved: CrewStatusLine.resolve(messages: messages), now: now)
    }

    // MARK: - 一次都没填过就说没填过（这条是承重的）

    func testNeverFilledSaysSoAndNeverInventsOne() {
        // 「还没有」本身是有用的信息：这个机组的机长还没报过状态。
        // 编一句（比如拿最后一条消息顶上）会让人以为他报过。
        let l = line([(nil, iso(60)), (nil, iso(30))])
        XCTAssertEqual(l.text, "还没有")
        XCTAssertTrue(l.isMissing)
    }

    func testEmptyMessageListIsAlsoNotFilled() {
        let l = line([])
        XCTAssertEqual(l.text, "还没有")
        XCTAssertTrue(l.isMissing)
    }

    func testBlankStatusIsTreatedAsNotFilled() {
        // 写了个空格就算填过，是最廉价的一种假账。
        for blank in ["", "   ", "\n", " \t "] {
            let l = line([(blank, iso(60))])
            XCTAssertEqual(l.text, "还没有", "「\(blank)」不该算填过")
            XCTAssertTrue(l.isMissing)
        }
    }

    // MARK: - 沿用上一次填的

    func testCarriesForwardTheMostRecentNonEmptyStatus() {
        // 这次没填就沿用上一次填的 —— 「往回找最近一条带状态的」自然实现了它，
        // 不需要任何额外的账。
        let l = line([("在等 CI", iso(7200)), (nil, iso(60)), (nil, iso(30))])
        XCTAssertFalse(l.isMissing)
        XCTAssertTrue(l.text.contains("在等 CI"), l.text)
    }

    func testNewerStatusWins() {
        let l = line([("老状态", iso(7200)), ("新状态", iso(60))])
        XCTAssertTrue(l.text.contains("新状态"), l.text)
        XCTAssertFalse(l.text.contains("老状态"), l.text)
    }

    func testAgeComesFromTheMessageThatCarriedItNotFromNow() {
        // 沿用来的那句可能来自三天前，而群里已经有更新的消息了。
        // **年龄必须是那条消息的年龄**，否则「沿用」看起来就像「刚刚报的」。
        let l = line([("在等人类拍板", iso(3 * 86_400)), (nil, iso(30))])
        XCTAssertTrue(l.text.hasPrefix("3 天前："), l.text)
    }

    func testAgeIsInTheBodyNotOnlyInATooltip() {
        // tooltip 要悬停才看得见，而「多久前报的」是「还算不算数」的前提。
        let l = line([("在跑全量", iso(1800))])
        XCTAssertEqual(l.text, "30 分钟前：在跑全量")
    }

    func testJustNowReadsAsJustNow() {
        XCTAssertEqual(line([("刚合完", iso(20))]).text, "刚刚：刚合完")
    }

    // MARK: - 时间戳坏了

    func testUnparseableTimestampSaysUnknownAgeInsteadOfPretendingItIsFresh() {
        let l = line([("在等 CI", "不是时间")])
        XCTAssertTrue(l.text.contains("不知道多久前"), l.text)
        XCTAssertTrue(l.text.contains("在等 CI"), l.text)
        XCTAssertFalse(l.isMissing, "有话就不是「还没有」")
    }

    func testFutureTimestampDoesNotRenderNegativeAge() {
        let future = ISO8601DateFormatter().string(from: now.addingTimeInterval(600))
        XCTAssertEqual(line([("x", future)]).text, "刚刚：x")
    }

    // MARK: - 落盘那一半

    func testStatusRoundTripsOnAWhiteboardMessage() throws {
        // 字段挂在消息上、不新开账本；老消息缺这个键要照样解得出来。
        let message = LocalWhiteboardMessage(
            id: "m1", senderKind: "captain", senderUserId: nil, senderSessionId: "s",
            category: "progress", text: "正文", createdAt: iso(60),
            senderName: "机长", inReplyTo: nil, mentions: nil, attachments: nil,
            crewStatus: "在等 CI")
        let data = try JSONEncoder().encode(message)
        XCTAssertEqual(try JSONDecoder().decode(LocalWhiteboardMessage.self, from: data).crewStatus,
                       "在等 CI")

        let legacy = """
        {"id":"m0","senderKind":"session","text":"老消息","createdAt":"2026-09-01T00:00:00Z"}
        """
        let old = try JSONDecoder().decode(LocalWhiteboardMessage.self, from: Data(legacy.utf8))
        XCTAssertNil(old.crewStatus, "老消息没有这个键，要解成 nil 而不是整条解不开")
    }
}
