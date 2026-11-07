#if os(macOS)
import Foundation
import XCTest

/// The production read store and wire types; no Core, provider or GUI is started.
final class CrewdAdminAskGroupsStoreTests: XCTestCase {
    private typealias Wire = CrewdAdminAskGroupsWire

    @MainActor private final class RPC {
        var origin: CrewdKnowledgeStore.Origin? = .init(
            scope: .init(namespace: UUID(), localDirectory: "/private/tmp/unused-ask-groups-fixture",
                         authorityID: nil, databaseID: "fixture-db", grantID: nil, grantRevision: nil),
            observation: UUID(), connection: UUID(), crew: "crew-a")
        var available = true
        var calls = 0
        var hold = false
        var waiting: CheckedContinuation<CrewdValue, Error>?
        var reply: CrewdValue = .null
        func store() -> CrewdAdminAskGroupsStore {
            CrewdAdminAskGroupsStore(origin: { self.origin }, rpc: { method, _ in
                XCTAssertEqual(method, Wire.list)
                self.calls += 1
                if self.hold { return try await withCheckedThrowingContinuation { self.waiting = $0 } }
                return self.reply
            }, available: { self.available })
        }
    }

    @MainActor private func page() throws -> CrewdValue {
        func question(_ id: String) -> Wire.Question {
            .init(id: id, question: "Fixture " + id, questionTruncated: false,
                  resolutionRevision: String(repeating: "b", count: 64),
                  sourceChannelId: "crew-a:human", sourceMessageId: "source-" + id,
                  askedAtMs: 1000, default: nil, defaultTruncated: false, defaultDueAtMs: nil)
        }
        return try CrewdKnowledgeStore.encode(Wire.Page(
            scope: .init(dbId: "fixture-db", restoreEpoch: "00000000-0000-4000-8000-000000000001"),
            crewId: "crew-a", availability: .ready, observedAtMs: 2000,
            items: [.init(id: String(repeating: "a", count: 64), first: question("H1"),
                          second: question("H2"), state: .suggested, probability: 0.9,
                          observedAtMs: 2000, excerptTruncated: false)],
            nextAfter: nil, partialCoverage: false, historicalBaseline: false))
    }

    @MainActor func testActualSourceAndSettingsEventsRevokeDisplayedComparison() async throws {
        let rpc = RPC(), store = rpc.store()
        rpc.reply = try page()
        for event in ["todo.attributesChanged", Wire.invalidated] {
            await store.reload(crew: "crew-a", current: { true })
            XCTAssertEqual(store.items.count, 1)
            XCTAssertTrue(store.sourceCurrent)
            store.handleEvent(event, .object([:]), connection: rpc.origin!.connection)
            XCTAssertTrue(store.items.isEmpty)
            XCTAssertNil(store.page)
            XCTAssertFalse(store.sourceCurrent)
        }
    }

    @MainActor func testAdviceHintKeepsCardsAndGlobalInvalidationIsStrictAndConnectionBound() async throws {
        let rpc = RPC(), store = rpc.store()
        rpc.reply = try page()
        await store.reload(crew: "crew-a", current: { true })
        store.handleEvent(Wire.changed, .object(["crewId": .string("crew-a")]), connection: rpc.origin!.connection)
        XCTAssertEqual(store.items.count, 1)
        XCTAssertTrue(store.hasUpdates)
        for payload in [CrewdValue.array([]), .null, .object(["crewId": .string("crew-a")])] {
            store.handleEvent(Wire.invalidated, payload, connection: rpc.origin!.connection)
            XCTAssertEqual(store.items.count, 1)
        }
        store.handleEvent(Wire.invalidated, .object([:]), connection: UUID())
        XCTAssertEqual(store.items.count, 1)
        store.handleEvent(Wire.invalidated, .object([:]), connection: rpc.origin!.connection)
        XCTAssertTrue(store.items.isEmpty)
        XCTAssertEqual(rpc.calls, 1, "an invalidation is not permission to issue a new RPC")
    }

    @MainActor func testInvalidatedHeldReadCannotRepopulateCards() async throws {
        let rpc = RPC(), store = rpc.store()
        rpc.reply = try page()
        rpc.hold = true
        let pending = Task { await store.reload(crew: "crew-a", current: { true }) }
        for _ in 0..<200 {
            if rpc.waiting != nil { break }
            await Task.yield()
        }
        guard let reply = rpc.waiting else {
            pending.cancel()
            await pending.value
            return XCTFail("the controlled read did not reach the actual RPC boundary")
        }
        store.handleEvent(Wire.invalidated, .object([:]), connection: rpc.origin!.connection)
        rpc.waiting = nil
        reply.resume(returning: rpc.reply)
        await pending.value
        XCTAssertTrue(store.items.isEmpty)
        XCTAssertNil(store.page)
        XCTAssertFalse(store.busy)
    }

    @MainActor func testUnsupportedBackendIsNeverQueried() async {
        let rpc = RPC(), store = rpc.store()
        rpc.available = false
        await store.reload(crew: "crew-a", current: { true })
        XCTAssertEqual(rpc.calls, 0)
        XCTAssertTrue(store.items.isEmpty)
        for personal in [false, true] {
            XCTAssertFalse(CrewdClient.eventTopics(capabilities: [], includePersonalDraft: personal).contains(Wire.topic))
        }
    }
}
#endif
