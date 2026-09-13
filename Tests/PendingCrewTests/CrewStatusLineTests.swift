import XCTest
import Foundation

/// 侧栏「总机长」视图每行那句话（人类 Todo #136，#145 改过口径）。
///
/// 换掉的是「这个群最后一条消息」——那是一手的、永远为真。所以这里钉的不是
/// 「显示得好不好看」，是**它有没有把自己的可信度一起说出来**：
/// 没有就说没有（别编）、谁写的要说清、**写完之后有了新消息的要标成已过时**。
///
/// 2026-09-13 人类要求消息位不再拼时间。原来那几条「年龄必须在正文里」的用例
/// 换成了「正文里没有时间 + 旧的必须标已过时」—— 去掉时间那一刀不许让旧的看起来像新的。
final class CrewStatusLineTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func iso(_ secondsAgo: TimeInterval) -> String {
        ISO8601DateFormatter().string(from: now.addingTimeInterval(-secondsAgo))
    }

    private func msg(_ id: String, status: String? = nil,
                     secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "captain", senderUserId: nil, senderSessionId: "s",
            category: nil, text: "正文 \(id)", createdAt: iso(secondsAgo), crewStatus: status)
    }

    private func summary(_ text: String, secondsAgo: TimeInterval) -> CrewChiefSummary {
        CrewChiefSummary(text: text, writtenAt: iso(secondsAgo),
                         bySessionId: "chief", bySenderName: "总机长")
    }

    private func line(summary: CrewChiefSummary? = nil,
                      carrier: LocalWhiteboardMessage? = nil,
                      last: LocalWhiteboardMessage?) -> CrewStatusLine.Line {
        CrewStatusLine.make(summary: summary, statusCarrier: carrier, lastMessage: last)
    }

    // MARK: - 什么都没有就说没有（这条是承重的）

    func testNothingSaysSoAndNeverInventsOne() {
        // 「还没有」本身是有用的信息。编一句（比如拿最后一条消息顶上）会让人以为有人写过。
        let l = line(last: msg("m1", secondsAgo: 30))
        XCTAssertEqual(l.text, "还没有")
        XCTAssertEqual(l.source, .missing)
        XCTAssertTrue(l.isMissing)
        XCTAssertFalse(l.isStale)
        XCTAssertEqual(l.displayText, "还没有")
    }

    func testBlankStatusAndBlankSummaryAreTreatedAsNothing() {
        // 写了个空格就算写过，是最廉价的一种假账。
        for blank in ["", "   ", "\n", " \t "] {
            let m = msg("m1", status: blank, secondsAgo: 60)
            let l = line(summary: summary(blank, secondsAgo: 10), carrier: m, last: m)
            XCTAssertEqual(l.source, .missing, "「\(blank)」不该算写过")
        }
    }

    // MARK: - 优先级：总机长摘要 > 机长自报 > 还没有

    func testFreshSummaryBeatsFreshStatus() {
        let m = msg("m1", status: "机长报的", secondsAgo: 600)
        let l = line(summary: summary("总机长写的", secondsAgo: 60), carrier: m, last: m)
        XCTAssertEqual(l.source, .chiefSummary)
        XCTAssertEqual(l.text, "总机长写的")
        XCTAssertFalse(l.isStale)
    }

    func testStatusShowsWhenThereIsNoSummary() {
        let m = msg("m1", status: "在跑全量", secondsAgo: 60)
        let l = line(carrier: m, last: m)
        XCTAssertEqual(l.source, .captainStatus)
        XCTAssertEqual(l.text, "在跑全量")
        XCTAssertFalse(l.isStale)
    }

    func testFreshStatusBeatsStaleSummary() {
        // 摘要写完之后机长又报了一句：状态严格更新，别拿二手旧话盖住一手新话。
        let m = msg("m2", status: "机长刚报的", secondsAgo: 60)
        let l = line(summary: summary("旧摘要", secondsAgo: 600), carrier: m, last: m)
        XCTAssertEqual(l.source, .captainStatus)
        XCTAssertEqual(l.text, "机长刚报的")
        XCTAssertFalse(l.isStale)
    }

    func testWhenBothAreStaleTheSummaryStillWins() {
        let carrier = msg("m1", status: "机长很早报的", secondsAgo: 900)
        let last = msg("m2", secondsAgo: 60)
        let l = line(summary: summary("总机长写的", secondsAgo: 600), carrier: carrier, last: last)
        XCTAssertEqual(l.source, .chiefSummary)
        XCTAssertTrue(l.isStale)
    }

    // MARK: - 过期：写完之后又有了更新的消息

    func testSummaryGoesStaleWhenTheCrewHasANewerMessage() {
        let l = line(summary: summary("在改侧栏", secondsAgo: 600), last: msg("m9", secondsAgo: 60))
        XCTAssertTrue(l.isStale)
        XCTAssertEqual(l.displayText, "已过时 · 在改侧栏")
        XCTAssertTrue(l.help.hasPrefix("已过时"), l.help)
    }

    func testSummaryStaysFreshWhenNothingNewSinceWriting() {
        let l = line(summary: summary("在改侧栏", secondsAgo: 60), last: msg("m1", secondsAgo: 600))
        XCTAssertFalse(l.isStale)
        XCTAssertEqual(l.displayText, "在改侧栏")
        XCTAssertFalse(l.help.contains("已过时"), l.help)
    }

    func testAMessageInTheSameSecondCountsAsNewer() {
        // 时间戳只到秒，同一秒里分不出先后 —— 分不出就说旧。
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: iso(60), lastMessageCreatedAt: iso(60)))
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let sameSecondLater = fractional.string(from: now.addingTimeInterval(-60 + 0.4))
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: iso(60),
                                                    lastMessageCreatedAt: sameSecondLater))
        XCTAssertFalse(CrewStatusLine.summaryIsStale(writtenAt: iso(60),
                                                     lastMessageCreatedAt: iso(61)))
    }

    func testUnknownTimesCountAsStale() {
        // 证明不了它新鲜，就不许画成新鲜的。
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: "不是时间", lastMessageCreatedAt: iso(600)))
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: iso(60), lastMessageCreatedAt: "坏的"))
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: iso(60), lastMessageCreatedAt: nil))
        XCTAssertTrue(CrewStatusLine.statusIsStale(carrierId: "m1", lastMessageId: nil))
    }

    func testCarriedForwardStatusIsStale() {
        // 「这次没填就沿用上一次」—— 沿用 = 之后又有了没带状态的消息 = 过期。
        // 去掉正文里的时间之后，这条就是不让三天前那句冒充刚刚的那道闸。
        let carrier = msg("m1", status: "在等 CI", secondsAgo: 3 * 86_400)
        let l = line(carrier: carrier, last: msg("m2", secondsAgo: 30))
        XCTAssertEqual(l.source, .captainStatus)
        XCTAssertTrue(l.isStale)
        XCTAssertEqual(l.displayText, "已过时 · 在等 CI")
    }

    func testStatusStalenessIsByMessageIdNotByClock() {
        // 同一秒两条消息：状态挂在前一条上，就是过期了，不管时间戳长得一不一样。
        let carrier = msg("m1", status: "x", secondsAgo: 60)
        let later = msg("m2", secondsAgo: 60)
        XCTAssertTrue(line(carrier: carrier, last: later).isStale)
        XCTAssertFalse(line(carrier: carrier, last: carrier).isStale)
    }

    // MARK: - 正文里不再有时间（人类 2026-09-13）

    func testBodyCarriesNoAgeAnyMore() {
        let cases: [CrewStatusLine.Line] = [
            line(carrier: msg("m1", status: "在跑全量", secondsAgo: 1800),
                 last: msg("m1", status: "在跑全量", secondsAgo: 1800)),
            line(carrier: msg("m1", status: "在等人类拍板", secondsAgo: 3 * 86_400),
                 last: msg("m2", secondsAgo: 30)),
            line(summary: summary("在改侧栏", secondsAgo: 20), last: msg("m1", secondsAgo: 7200)),
        ]
        for l in cases {
            for age in ["刚刚", "分钟前", "小时前", "天前", "不知道多久前"] {
                XCTAssertFalse(l.displayText.contains(age), "正文里又拼了时间：\(l.displayText)")
            }
        }
        XCTAssertEqual(cases[0].displayText, "在跑全量")
    }

    // MARK: - 悬停提示：来源 + 写入时刻

    func testHelpNamesTheSourceAndTheWriteTime() {
        let s = summary("在改侧栏", secondsAgo: 60)
        let fromChief = line(summary: s, last: msg("m1", secondsAgo: 600))
        XCTAssertTrue(fromChief.help.contains("总机长"), fromChief.help)
        XCTAssertTrue(fromChief.help.contains(
            CrewStatusLine.clockText(CrewTimestamp.parse(s.writtenAt))), fromChief.help)

        let carrier = msg("m1", status: "在跑全量", secondsAgo: 60)
        let fromCaptain = line(carrier: carrier, last: carrier)
        XCTAssertTrue(fromCaptain.help.contains("机长发消息时自己报的"), fromCaptain.help)
        XCTAssertTrue(fromCaptain.help.contains(
            CrewStatusLine.clockText(CrewTimestamp.parse(carrier.createdAt))), fromCaptain.help)

        XCTAssertTrue(line(last: nil).help.contains("还没有总机长写的摘要"))
    }

    func testUnparseableCarrierTimeSaysUnknownInTheHelp() {
        let carrier = LocalWhiteboardMessage(
            id: "m1", senderKind: "captain", senderUserId: nil, senderSessionId: "s",
            category: nil, text: "x", createdAt: "不是时间", crewStatus: "在等 CI")
        let l = line(carrier: carrier, last: carrier)
        XCTAssertEqual(l.text, "在等 CI")
        XCTAssertTrue(l.help.contains("时间不详"), l.help)
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
