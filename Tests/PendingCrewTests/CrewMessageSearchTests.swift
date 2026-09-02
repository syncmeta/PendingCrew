import XCTest

final class CrewMessageSearchTests: XCTestCase {
    private func doc(
        crew: String = "crew-a",
        title: String = "项目甲",
        id: String,
        text: String,
        sender: String = "小绿",
        senderId: String? = "session-1",
        at: String = "2026-08-30T02:00:00Z",
        attachments: [String] = []
    ) -> CrewMessageSearchDocument {
        .init(
            crewId: crew, crewTitle: title, messageId: id, text: text,
            senderName: sender, senderId: senderId, createdAt: at,
            attachmentMetadata: attachments)
    }

    func testChineseSubstringSearchDoesNotRequireWhitespaceTokenization() {
        let rows = [
            doc(id: "hit", text: "前后端通信方案已经落地"),
            doc(id: "miss", text: "release build passed"),
        ]

        let result = CrewMessageSearch.search(rows, query: "通信方案")

        XCTAssertEqual(result.map(\.document.messageId), ["hit"])
        XCTAssertEqual(result.first?.matchedFields, [.text])
    }

    func testSearchMatchesSenderAndAttachmentMetadataButNotFileContents() {
        let rows = [
            doc(id: "sender", text: "收到", sender: "机长"),
            doc(id: "attachment", text: "如图", attachments: ["架构图.png", "image/png"]),
            doc(id: "plain", text: "没有附件"),
        ]

        XCTAssertEqual(
            CrewMessageSearch.search(rows, query: "机长").map(\.document.messageId),
            ["sender"])
        XCTAssertEqual(
            CrewMessageSearch.search(rows, query: "架构图").map(\.document.messageId),
            ["attachment"])
        XCTAssertEqual(
            CrewMessageSearch.search(rows, query: "架构图").first?.matchedFields,
            [.attachment])
    }

    func testAllWhitespaceSeparatedTermsMayMatchAcrossDifferentFields() {
        let rows = [doc(
            id: "cross-field", text: "已经完成", sender: "机长",
            attachments: ["验收截图.png"])]

        XCTAssertEqual(
            CrewMessageSearch.search(rows, query: "机长 截图 完成").map(\.document.messageId),
            ["cross-field"])
    }

    func testTimeRangeIsInclusiveAndNewestFirstIsStable() throws {
        let rows = [
            doc(id: "old", text: "命中", at: "2026-08-29T23:59:59Z"),
            doc(id: "edge-a", text: "命中", at: "2026-08-30T00:00:00Z"),
            doc(id: "new", text: "命中", at: "2026-08-30T12:00:00Z"),
            doc(id: "edge-b", text: "命中", at: "2026-08-30T23:59:59Z"),
            doc(id: "future", text: "命中", at: "2026-08-31T00:00:00Z"),
        ]
        let after = try XCTUnwrap(CrewMessageSearch.parseISO("2026-08-30T00:00:00Z"))
        let before = try XCTUnwrap(CrewMessageSearch.parseISO("2026-08-30T23:59:59Z"))

        let result = CrewMessageSearch.search(
            rows, query: "命中", after: after, before: before)

        XCTAssertEqual(result.map(\.document.messageId), ["edge-b", "new", "edge-a"])
    }

    func testLimitIsClampedAndCrewLocationSurvivesResult() {
        let rows = (0..<205).map { index in
            doc(
                crew: "crew-\(index % 2)", title: "群 \(index % 2)",
                id: "m-\(index)", text: "命中", at: String(format: "2026-08-30T00:%02d:%02dZ", (index / 60) % 60, index % 60))
        }

        let result = CrewMessageSearch.search(rows, query: "命中", limit: 999)

        XCTAssertEqual(result.count, CrewMessageSearch.maximumLimit)
        XCTAssertTrue(result.allSatisfy { !$0.document.crewId.isEmpty && !$0.document.crewTitle.isEmpty })
    }

    func testCrewTitleIsLocationMetadataNotAQueryThatMatchesEveryMessageInCrew() {
        let rows = [doc(title: "北斗搜索组", id: "only", text: "正文里没有项目名")]

        XCTAssertTrue(CrewMessageSearch.search(rows, query: "北斗搜索组").isEmpty)
    }
}
