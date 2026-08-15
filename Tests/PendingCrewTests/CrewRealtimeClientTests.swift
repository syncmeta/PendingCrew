#if os(macOS)
import XCTest
// LocalRunner 源码直接编进 PendingCrewTests target（见 project.yml），无需 import。

/// 断言 hub WS 客户端的 codec 对得上 **已部署** 的 hub 线缆协议
/// （`apps/edge/src/lib/realtime-publish.ts` 的 `{type:'change', table, op,
/// record}` + `apps/edge/src/durable-objects/hub.ts` 的 `{type:'ready', ts}`），
/// 以及连接请求的 ws(s) 升级形状。活体 socket 留 E2E。
final class CrewRealtimeClientTests: XCTestCase {

    // MARK: - CrewRealtimeEvent.parse (inbound decode)

    func testParsesCrewAnnouncementsInsert() {
        let raw = #"{"type":"change","table":"crew_announcements","op":"insert","record":{"id":"ann-1","content":"hi"}}"#
        XCTAssertEqual(
            CrewRealtimeEvent.parse(raw),
            .changed(table: "crew_announcements", op: "insert", recordId: "ann-1"))
    }

    func testParsesCrewSessionsUpdate() {
        let raw = #"{"type":"change","table":"crew_sessions","op":"update","record":{"id":"sess-9","status":"running"}}"#
        XCTAssertEqual(
            CrewRealtimeEvent.parse(raw),
            .changed(table: "crew_sessions", op: "update", recordId: "sess-9"))
    }

    func testParsesMessagesChange() {
        let raw = #"{"type":"change","table":"messages","op":"insert","record":{"id":"m-3"}}"#
        XCTAssertEqual(
            CrewRealtimeEvent.parse(raw),
            .changed(table: "messages", op: "insert", recordId: "m-3"))
    }

    func testChangeWithoutRecordIdStillParses() {
        // DELETE 帧的 record 可能没 id（webhook old_record 形状各异）—— 仍是有效
        // 的拉取信号，recordId 兜底 nil。
        let raw = #"{"type":"change","table":"crew_sessions","op":"delete","record":{}}"#
        XCTAssertEqual(
            CrewRealtimeEvent.parse(raw),
            .changed(table: "crew_sessions", op: "delete", recordId: nil))
    }

    func testReadyHandshakeFrame() {
        XCTAssertEqual(CrewRealtimeEvent.parse(#"{"type":"ready","ts":12345}"#), .ready)
    }

    func testNonCrewAndMalformedFramesIgnored() {
        for raw in [
            #"{"type":"voice_call","event":"state","conversation_id":"c"}"#,
            #"{"type":"voice_cost","conversation_id":"c","session_id":"s"}"#,
            #"{"type":"change","table":"crew_sessions"}"#,      // missing op
            #"{"type":"change","op":"insert","record":{}}"#,    // missing table
            #"{"no":"type"}"#,
            "not json at all",
        ] {
            XCTAssertEqual(CrewRealtimeEvent.parse(raw), .ignored,
                           "should degrade to .ignored: \(raw)")
        }
    }

    // MARK: - dispatch (inbound routing → AsyncStream)

    func testDispatchYieldsOnlyChangedEvents() async {
        let client = CrewRealtimeClient(
            baseURL: URL(string: "https://api.pendingname.com")!,
            crewId: "11111111-1111-1111-1111-111111111111",
            token: "pdg_x")
        // ready 握手 + ignored 帧内部消化，不得泄漏到事件流；change 必须上抛。
        await client.dispatch(.ready)
        await client.dispatch(.changed(table: "crew_announcements", op: "insert", recordId: "a1"))
        await client.dispatch(.ignored)
        await client.dispatch(.changed(table: "crew_sessions", op: "update", recordId: "s2"))
        await client.close() // finish 流，让 for-await 终止

        var received: [CrewRealtimeEvent] = []
        for await e in client.events { received.append(e) }
        XCTAssertEqual(received, [
            .changed(table: "crew_announcements", op: "insert", recordId: "a1"),
            .changed(table: "crew_sessions", op: "update", recordId: "s2"),
        ])
    }

    // MARK: - makeConnectRequest (pure request building)

    func testConnectRequestUpgradesHttpsToWss() {
        let req = CrewRealtimeClient.makeConnectRequest(
            baseURL: URL(string: "https://api.pendingname.com")!,
            crewId: "abc-123", token: "pdg_abc")
        let url = req.url!
        XCTAssertEqual(url.scheme, "wss")
        XCTAssertEqual(url.host, "api.pendingname.com")
        XCTAssertEqual(url.path, "/v1/realtime-hub/conv/abc-123")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer pdg_abc")
    }

    func testConnectRequestUpgradesHttpToWs() {
        let req = CrewRealtimeClient.makeConnectRequest(
            baseURL: URL(string: "http://127.0.0.1:8787")!,
            crewId: "c2", token: nil)
        let url = req.url!
        XCTAssertEqual(url.scheme, "ws")
        XCTAssertEqual(url.host, "127.0.0.1")
        XCTAssertEqual(url.port, 8787)
        XCTAssertEqual(url.path, "/v1/realtime-hub/conv/c2")
    }

    func testConnectRequestOmitsAuthWhenTokenNil() {
        let req = CrewRealtimeClient.makeConnectRequest(
            baseURL: URL(string: "https://api.pendingname.com")!,
            crewId: "c3", token: nil)
        XCTAssertNil(req.value(forHTTPHeaderField: "Authorization"))
    }
}
#endif
