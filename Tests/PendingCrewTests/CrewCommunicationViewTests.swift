import XCTest

final class CrewCommunicationViewTests: XCTestCase {
    private func row(_ id: String, audience: String? = nil,
                     senderKind: String = "session", category: String? = nil,
                     mentions: [CrewMention]? = nil, replyTo: String? = nil) throws -> CrewWhiteboardEntry {
        let entry = CrewWhiteboardEntry(
            id: id, senderKind: senderKind, senderSessionId: "worker",
            senderUserId: senderKind == "user" ? "person" : nil,
            senderBotId: nil, messageKind: "announcement", summary: "same body",
            createdAt: "2026-10-01T00:00:00Z", payload: nil, attachments: nil,
            senderDisplayName: nil, senderMemberId: nil, inReplyTo: replyTo,
            mentions: mentions, category: category)
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(entry)) as? [String: Any])
        json["audience"] = audience
        return try JSONDecoder().decode(CrewWhiteboardEntry.self, from: JSONSerialization.data(withJSONObject: json))
    }

    private func timeline(_ entries: [CrewWhiteboardEntry], search: String = "",
                          view: CrewTimelineFilter.CommunicationView = .human) -> [String] {
        CrewTimelineFilter.resolve(.init(
            entries: entries, onlyMentions: false,
            roster: .init(humanNames: ["person"], otherNames: []), localUserId: "person",
            searchText: search, crewId: "crew", crewTitle: "Crew",
            communicationView: view)).map(\.id)
    }

    func testHumanTimelineHidesExplicitAgentDiscussionAndPreservesLegacy() throws {
        let rows = try [row("old", category: "note"), row("internal", audience: "agents"),
                        row("delivery", audience: "human")]
        XCTAssertEqual(timeline(rows), ["old", "delivery"])
        XCTAssertEqual(timeline(rows, search: "same"), ["old", "delivery"])
        XCTAssertEqual(timeline(rows, view: .agents), ["old", "internal", "delivery"])
    }

    func testHumanInputExplicitHumanMentionTodoAndReplyCannotBeHidden() throws {
        let rows = try [row("person", audience: "agents", senderKind: "user"),
                        row("askPerson", audience: "agents", mentions: [.init(kind: "human", targetId: nil)]),
                        row("todo", audience: "agents", category: "human_todo"),
                        row("answer", audience: "agents", replyTo: "person"),
                        row("discussion", audience: "agents", category: "question"),
                        row("finding", audience: "agents", category: "finding"),
                        row("blocked", audience: "agents", category: "blocked")]
        XCTAssertEqual(timeline(rows), ["person", "askPerson", "todo", "answer"])
    }

    func testUnknownAudienceIsConservativelyVisibleAndViewIsInCacheKey() throws {
        let rows = try [row("unknown", audience: "future-value"),
                        row("internal", audience: "agents")]
        let cache = CrewTimelineFilterCache()
        func input(_ view: CrewTimelineFilter.CommunicationView) -> CrewTimelineFilter.Inputs {
            .init(entries: rows, onlyMentions: false,
                  roster: .init(humanNames: [], otherNames: []), localUserId: nil,
                  searchText: "", crewId: "crew", crewTitle: "Crew",
                  communicationView: view)
        }
        XCTAssertEqual(cache.entries(for: input(.human)).map(\.id), ["unknown"])
        XCTAssertEqual(cache.entries(for: input(.agents)).map(\.id), ["unknown", "internal"])
        XCTAssertEqual(cache.entries(for: input(.human)).map(\.id), ["unknown"])
    }

    func testPersistedAudienceSurvivesReloadAndMappingWithoutChangingAgentRows() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("crew-communication-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let writer = LocalWhiteboardStore(directory: directory)
        writer.appendSessionMessage(crewId: "crew", sessionId: "agent", text: "private",
                                    category: "question", audience: "agents")
        writer.appendSessionMessage(crewId: "crew", sessionId: "agent", text: "public",
                                    category: "progress", audience: "human")
        let persisted = LocalWhiteboardStore(directory: directory).list(crewId: "crew")
        XCTAssertEqual(persisted.map(\.audience), ["agents", "human"])
        XCTAssertEqual(persisted.map(\.agentText), ["private", "public"])
        let entries = persisted.map(CrewLocalWhiteboardMapping.entry)
        XCTAssertEqual(timeline(entries), [persisted[1].id])
        XCTAssertEqual(timeline(entries, view: .agents), persisted.map(\.id))
    }
}
