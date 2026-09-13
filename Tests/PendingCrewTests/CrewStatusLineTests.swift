import XCTest
import Foundation

/// 侧栏「总机长」视图每行那句话（人类 Todo #136，#145 改过口径）。
///
/// 换掉的是「这个群最后一条消息」——那是一手的、永远为真。所以这里钉的不是
/// 「显示得好不好看」，是**它有没有把自己的可信度一起说出来**：
/// 没有就说没有（别编）、谁写的要说清、**写完之后又有人说了话的要标成已过时**。
///
/// 2026-09-13 两次改口径：
/// - 人类要求消息位不再拼时间 → 原来「年龄必须在正文里」的用例换成「正文里没有时间 +
///   旧的必须标已过时」；
/// - 父机长抓到第一版拿「末条消息」判过期，系统通知一刷就全变「已过时」→ 判据改成
///   「最后一条**算数的发言**」，系统通知和回执不算（下面「算不算发言」那一组）。
final class CrewStatusLineTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func iso(_ secondsAgo: TimeInterval) -> String {
        ISO8601DateFormatter().string(from: now.addingTimeInterval(-secondsAgo))
    }

    /// 机长发言（算数）。
    private func msg(_ id: String, status: String? = nil,
                     secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "captain", senderUserId: nil, senderSessionId: "captain-1",
            category: nil, text: "正文 \(id)", createdAt: iso(secondsAgo), crewStatus: status)
    }

    private func summary(_ text: String, secondsAgo: TimeInterval) -> CrewChiefSummary {
        CrewChiefSummary(text: text, writtenAt: iso(secondsAgo),
                         bySessionId: "chief", bySenderName: "总机长")
    }

    /// `last` = 最后一条**算数的发言**（`nil` = 快照里没有这个机组）。
    private func line(summary: CrewChiefSummary? = nil,
                      carrier: LocalWhiteboardMessage? = nil,
                      last: LocalWhiteboardMessage?) -> CrewStatusLine.Line {
        CrewStatusLine.make(summary: summary, statusCarrier: carrier,
                            activity: last.map(CrewStatusLine.Activity.latest) ?? .unknown)
    }

    /// **走生产那条路**：整板消息 → `CrewLastMessageCache.digest(of:)`（同一次解码）→
    /// `CrewStatusLine.activity` → 那一行。系统消息那几条都用它，别在测试里另写判据。
    private func line(summary: CrewChiefSummary? = nil,
                      board: [LocalWhiteboardMessage]) -> CrewStatusLine.Line {
        let d = CrewLastMessageCache.digest(of: board)
        return CrewStatusLine.make(
            summary: summary, statusCarrier: d?.status,
            activity: CrewStatusLine.activity(lastMessage: d?.last, lastActivity: d?.lastActivity))
    }

    // MARK: - 按落盘形状造的几类消息（**不按显示名**）

    /// 老数据里的系统通知：`senderKind: session` + `sessionId: system`（本机真数据里有）。
    private func legacySystemNotice(_ id: String, secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "session", senderUserId: nil, senderSessionId: "system",
            category: "progress", text: "Session「x」自己结束了。", createdAt: iso(secondsAgo),
            senderName: "系统")
    }

    /// 新写入会被正规化成的形状：`senderKind: pendingcrew`。
    private func normalizedSystemNotice(_ id: String, secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: PendingCrewSystemMessage.senderKind, senderUserId: nil,
            senderSessionId: PendingCrewSystemMessage.sessionId, category: nil,
            text: "白板文件存在但暂时无法读取", createdAt: iso(secondsAgo),
            senderName: PendingCrewSystemMessage.senderName)
    }

    /// 「已送达」回执（`CrewStore.postSystemNotice` 的形状）。
    private func deliveredReceipt(_ id: String, secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "session", senderUserId: nil, senderSessionId: "system",
            category: nil, text: "已送达「PendingCrew」群聊。", createdAt: iso(secondsAgo),
            senderName: "系统")
    }

    /// 「已联系」回执：以机长**自己的身份**写回本群，靠 category 标记区分。
    private func contactReceipt(_ id: String, secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "captain", senderUserId: nil, senderSessionId: "captain-1",
            category: CrewActivityMessage.contactReceiptCategory,
            text: "已联系 37-1（发布准备 · 机长）：进度", createdAt: iso(secondsAgo), senderName: "机长")
    }

    private func human(_ id: String, secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "user", senderUserId: LocalWhiteboardStore.localUserId,
            senderSessionId: nil, category: nil, text: "人说的", createdAt: iso(secondsAgo),
            senderName: "人")
    }

    private func agent(_ id: String, secondsAgo: TimeInterval) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "session", senderUserId: nil, senderSessionId: "worker-1",
            category: "progress", text: "worker 的进展", createdAt: iso(secondsAgo),
            senderName: "worker")
    }

    /// 一串「不算发言」的消息，三类都有，模仿一次故障恢复时的补发。
    private func noticeBurst(from secondsAgo: TimeInterval, count: Int) -> [LocalWhiteboardMessage] {
        (0..<count).map { i in
            let t = secondsAgo - Double(i)
            switch i % 4 {
            case 0: return legacySystemNotice("n\(i)", secondsAgo: t)
            case 1: return normalizedSystemNotice("n\(i)", secondsAgo: t)
            case 2: return deliveredReceipt("n\(i)", secondsAgo: t)
            default: return contactReceipt("n\(i)", secondsAgo: t)
            }
        }
    }

    // MARK: - 算不算发言（#145 追加）

    func testSystemNoticesAndReceiptsDoNotCount() {
        for m in [legacySystemNotice("a", secondsAgo: 1), normalizedSystemNotice("b", secondsAgo: 1),
                  deliveredReceipt("c", secondsAgo: 1), contactReceipt("d", secondsAgo: 1)] {
            XCTAssertFalse(CrewActivityMessage.counts(m), "「\(m.text)」不是发言，却算了")
        }
    }

    func testHumanAndAgentSpeechCounts() {
        let report = LocalWhiteboardMessage(
            id: "r", senderKind: "session", senderUserId: nil, senderSessionId: "captain-parent",
            category: "report", text: "父机长派下来的活", createdAt: iso(1), senderName: "PendingCrew·机长")
        let inboundContact = LocalWhiteboardMessage(
            id: "i", senderKind: "session", senderUserId: nil, senderSessionId: "worker-9",
            category: "contact", text: "外线打进来的话", createdAt: iso(1),
            senderName: "发布准备 · 37-2", externalContactFrom: "37-2")
        for m in [human("h", secondsAgo: 1), agent("w", secondsAgo: 1), msg("c", secondsAgo: 1),
                  report, inboundContact] {
            XCTAssertTrue(CrewActivityMessage.counts(m), "「\(m.text)」是发言，却没算")
        }
    }

    /// 判据不看显示名：一个 agent 的 label 叫「系统」、一个机组叫「PendingCrew」，都照样算发言。
    func testDisplayNameIsNotTheCriterion() {
        let namedLikeSystem = LocalWhiteboardMessage(
            id: "x", senderKind: "session", senderUserId: nil, senderSessionId: "worker-2",
            category: "progress", text: "我叫系统但我是 worker", createdAt: iso(1), senderName: "系统")
        let namedLikeApp = LocalWhiteboardMessage(
            id: "y", senderKind: "captain", senderUserId: nil, senderSessionId: "captain-2",
            category: nil, text: "机组名叫 PendingCrew", createdAt: iso(1), senderName: "PendingCrew")
        XCTAssertTrue(CrewActivityMessage.counts(namedLikeSystem))
        XCTAssertTrue(CrewActivityMessage.counts(namedLikeApp))
    }

    /// 真写一条系统通知进白板再读回来（读的时候会被正规化），它仍然不算发言。
    func testSystemNoticeWrittenThroughTheStoreReadsBackAsNotCounting() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("activity-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "人先说一句", senderName: "人")
        store.appendSessionMessage(crewId: "c", sessionId: "system",
                                   text: "已送达「PendingCrew」群聊。", senderName: "系统")

        let digest = try XCTUnwrap(CrewLastMessageCache.digest(of: store.list(crewId: "c")))
        XCTAssertEqual(digest.last.text, "已送达「PendingCrew」群聊。")
        XCTAssertEqual(digest.lastActivity?.text, "人先说一句")
    }

    // MARK: - 一串系统消息之后仍然新鲜（这组是 #145 追加那一刀的承重墙）

    func testSummaryStaysFreshAfterABurstOfSystemMessages() {
        var board = [human("h0", secondsAgo: 900)]
        let s = summary("在改侧栏", secondsAgo: 600)
        board += noticeBurst(from: 300, count: 74)
        let l = line(summary: s, board: board)
        XCTAssertEqual(l.source, .chiefSummary)
        XCTAssertFalse(l.isStale, "74 条系统通知 / 回执把摘要判成了已过时")
    }

    func testSummaryGoesStaleWhenAHumanOrAgentSpeaksAfterTheBurst() {
        let base = [human("h0", secondsAgo: 900)] + noticeBurst(from: 300, count: 8)
        let s = summary("在改侧栏", secondsAgo: 600)
        XCTAssertTrue(line(summary: s, board: base + [human("h1", secondsAgo: 30)]).isStale,
                      "人类又说了话，摘要却还是新鲜的")
        XCTAssertTrue(line(summary: s, board: base + [agent("w1", secondsAgo: 30)]).isStale,
                      "agent 又说了话，摘要却还是新鲜的")
    }

    func testStatusStaysFreshAfterABurstOfSystemMessagesAndGoesStaleOnSpeech() {
        let carrier = msg("c1", status: "在跑全量", secondsAgo: 600)
        let board = [carrier] + noticeBurst(from: 300, count: 12)
        let fresh = line(board: board)
        XCTAssertEqual(fresh.source, .captainStatus)
        XCTAssertFalse(fresh.isStale, "系统通知 / 回执把机长自报判成了已过时")

        let afterHuman = line(board: board + [human("h1", secondsAgo: 30)])
        XCTAssertEqual(afterHuman.source, .captainStatus)
        XCTAssertTrue(afterHuman.isStale, "人类又说了话，机长自报却还是新鲜的")
    }

    func testBoardWithOnlySystemMessagesKeepsTheSummaryFreshButAnUnknownSnapshotDoesNot() {
        let s = summary("还没开张", secondsAgo: 600)
        XCTAssertFalse(line(summary: s, board: noticeBurst(from: 300, count: 5)).isStale,
                       "读到了白板、一条发言都没有 —— 写完之后没人说过话，应是新鲜的")
        XCTAssertTrue(line(summary: s, board: []).isStale,
                      "快照里没有这个机组 —— 证明不了新鲜，应是过期")
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

    // MARK: - 过期：写完之后又有人说了话

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
        XCTAssertTrue(CrewStatusLine.summaryIsStale(
            writtenAt: iso(60), activity: .latest(msg("m", secondsAgo: 60))))
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let sameSecondLater = LocalWhiteboardMessage(
            id: "f", senderKind: "user", senderUserId: nil, senderSessionId: nil, category: nil,
            text: "x", createdAt: fractional.string(from: now.addingTimeInterval(-60 + 0.4)))
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: iso(60), activity: .latest(sameSecondLater)))
        XCTAssertFalse(CrewStatusLine.summaryIsStale(
            writtenAt: iso(60), activity: .latest(msg("m", secondsAgo: 61))))
    }

    func testUnknownTimesCountAsStale() {
        // 证明不了它新鲜，就不许画成新鲜的。
        XCTAssertTrue(CrewStatusLine.summaryIsStale(
            writtenAt: "不是时间", activity: .latest(msg("m", secondsAgo: 600))))
        let badTime = LocalWhiteboardMessage(
            id: "b", senderKind: "user", senderUserId: nil, senderSessionId: nil, category: nil,
            text: "x", createdAt: "坏的")
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: iso(60), activity: .latest(badTime)))
        XCTAssertTrue(CrewStatusLine.summaryIsStale(writtenAt: iso(60), activity: .unknown))
        XCTAssertTrue(CrewStatusLine.statusIsStale(carrierId: "m1", activity: .unknown))
        XCTAssertTrue(CrewStatusLine.statusIsStale(carrierId: "m1", activity: .noneYet))
    }

    func testActivityTellsUnknownFromNoneYet() {
        let notice = deliveredReceipt("n", secondsAgo: 1)
        XCTAssertEqual(CrewStatusLine.activity(lastMessage: nil, lastActivity: nil), .unknown)
        XCTAssertEqual(CrewStatusLine.activity(lastMessage: notice, lastActivity: nil), .noneYet)
        let h = human("h", secondsAgo: 2)
        XCTAssertEqual(CrewStatusLine.activity(lastMessage: notice, lastActivity: h), .latest(h))
    }

    func testCarriedForwardStatusIsStale() {
        // 「这次没填就沿用上一次」—— 沿用 = 之后又有了没带状态的发言 = 过期。
        // 去掉正文里的时间之后，这条就是不让三天前那句冒充刚刚的那道闸。
        let carrier = msg("m1", status: "在等 CI", secondsAgo: 3 * 86_400)
        let l = line(carrier: carrier, last: msg("m2", secondsAgo: 30))
        XCTAssertEqual(l.source, .captainStatus)
        XCTAssertTrue(l.isStale)
        XCTAssertEqual(l.displayText, "已过时 · 在等 CI")
    }

    func testStatusStalenessIsByMessageIdNotByClock() {
        // 同一秒两条发言：状态挂在前一条上，就是过期了，不管时间戳长得一不一样。
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
