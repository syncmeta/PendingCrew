#if os(macOS)
import XCTest
// LocalRunner 源码直接编进 PendingCrewTests target（见 project.yml），无需 import。

/// Tests assert the Swift codec matches the **shipped** server wire protocol
/// (`apps/edge/src/lib/session-proxy-protocol.ts`) byte-for-shape: outbound
/// frames encode to the keys the DO's `parseClientMessage` accepts, and
/// inbound frames decode from the exact shapes the DO emits.
final class SessionProxyProtocolTests: XCTestCase {

    private func decode(_ data: Data) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    // MARK: - outbound

    func testSubscribeFrameShape() throws {
        let d = decode(try SessionProxyOutbound.subscribe(role: .runner, token: "pdg_x").jsonData())
        XCTAssertEqual(d["type"] as? String, "subscribe")
        XCTAssertEqual(d["role"] as? String, "runner")
        XCTAssertEqual(d["token"] as? String, "pdg_x")
    }

    func testSubscribeOmitsTokenWhenNil() throws {
        let d = decode(try SessionProxyOutbound.subscribe(role: .viewer, token: nil).jsonData())
        XCTAssertEqual(d["role"] as? String, "viewer")
        XCTAssertNil(d["token"], "nil token must be omitted, not encoded as null")
    }

    func testSessionStateFrameShape() throws {
        let state = SessionState(status: "running", eventCount: 3, lastEvent: "ls -la")
        let d = decode(try SessionProxyOutbound.sessionState(state).jsonData())
        XCTAssertEqual(d["type"] as? String, "session.state")
        let s = d["state"] as? [String: Any]
        XCTAssertEqual(s?["status"] as? String, "running")
        XCTAssertEqual(s?["eventCount"] as? Int, 3)
        XCTAssertEqual(s?["lastEvent"] as? String, "ls -la")
    }

    func testPermissionRequestFrameShape() throws {
        let payload: [String: JSONValue] = ["path": .string("/etc/hosts"), "bytes": .number(42)]
        let frame = SessionProxyOutbound.permissionRequest(
            id: nil, action: "write_file", payload: payload, riskLevel: "high")
        let d = decode(try frame.jsonData())
        XCTAssertEqual(d["type"] as? String, "permission.request")
        let req = d["request"] as? [String: Any]
        XCTAssertNil(req?["id"], "nil id must be omitted, not encoded as null")
        XCTAssertEqual(req?["action"] as? String, "write_file")
        XCTAssertEqual(req?["riskLevel"] as? String, "high")
        let p = req?["payload"] as? [String: Any]
        XCTAssertEqual(p?["path"] as? String, "/etc/hosts")
        XCTAssertEqual((p?["bytes"] as? NSNumber)?.intValue, 42)
    }

    func testPermissionRequestFrameCarriesClientId() throws {
        // `id` = the runner's local approval id — the DO echoes it back on
        // permission.request.ack so decisions can be correlated.
        let frame = SessionProxyOutbound.permissionRequest(
            id: "local-1", action: "computer-use", payload: nil, riskLevel: nil)
        let req = decode(try frame.jsonData())["request"] as? [String: Any]
        XCTAssertEqual(req?["id"] as? String, "local-1")
        XCTAssertEqual(req?["action"] as? String, "computer-use")
    }

    // MARK: - inbound

    func testParseSubscribedAck() {
        let raw = #"{"type":"subscribed","sessionId":"sess-1","role":"runner"}"#
        XCTAssertEqual(SessionProxyInbound.parse(raw),
                       .subscribed(sessionId: "sess-1", role: .runner))
    }

    func testParseLiveCommand() {
        let raw = #"{"type":"session.command","commandId":"c1","command":{"kind":"send_prompt","payload":{"text":"hi","n":2}}}"#
        XCTAssertEqual(SessionProxyInbound.parse(raw),
                       .command(commandId: "c1", kind: "send_prompt",
                                payload: ["text": .string("hi"), "n": .number(2)],
                                queued: false))
    }

    func testParseQueuedCommandWithoutPayload() {
        let raw = #"{"type":"session.command","commandId":"c2","command":{"kind":"cancel"},"queued":true}"#
        XCTAssertEqual(SessionProxyInbound.parse(raw),
                       .command(commandId: "c2", kind: "cancel", payload: nil, queued: true))
    }

    func testParsePermissionDecision() {
        let raw = #"{"type":"permission.decision","requestId":"r9","decision":"approve"}"#
        XCTAssertEqual(SessionProxyInbound.parse(raw),
                       .permissionDecision(requestId: "r9", decision: "approve"))
    }

    func testParsePermissionRequestAck() {
        let raw = #"{"type":"permission.request.ack","requestId":"srv-9","clientRequestId":"local-1"}"#
        XCTAssertEqual(SessionProxyInbound.parse(raw),
                       .permissionRequestAck(clientRequestId: "local-1", requestId: "srv-9"))
    }

    func testParsePermissionRequestAckWithoutClientId() {
        let raw = #"{"type":"permission.request.ack","requestId":"srv-9"}"#
        XCTAssertEqual(SessionProxyInbound.parse(raw),
                       .permissionRequestAck(clientRequestId: nil, requestId: "srv-9"))
    }

    func testParseErrorFrame() {
        let raw = #"{"type":"error","code":"role_mismatch","message":"nope"}"#
        XCTAssertEqual(SessionProxyInbound.parse(raw),
                       .error(code: "role_mismatch", message: "nope"))
    }

    func testViewerFacingFramesDegradeToUnknown() {
        // session.state / permission.request flow DO → viewers, not → runner.
        for raw in [
            #"{"type":"session.state","state":{"status":"running"}}"#,
            #"{"type":"permission.request","request":{"action":"x"}}"#,
            "not json at all",
            #"{"type":"session.command","commandId":"c"}"#, // missing command.kind
        ] {
            XCTAssertEqual(SessionProxyInbound.parse(raw), .unknown(raw),
                           "should degrade to .unknown: \(raw)")
        }
    }

    // MARK: - JSONValue bool/number disambiguation

    func testJSONValueKeepsBoolDistinctFromNumber() {
        let raw = #"{"type":"session.command","commandId":"c","command":{"kind":"k","payload":{"flag":true,"count":1}}}"#
        guard case let .command(_, _, payload, _) = SessionProxyInbound.parse(raw) else {
            return XCTFail("expected .command")
        }
        XCTAssertEqual(payload?["flag"], .bool(true))
        XCTAssertEqual(payload?["count"], .number(1))
    }

    // MARK: - SessionProxyClient.makeConnectRequest (pure request building)

    func testConnectRequestUpgradesHttpsToWss() {
        let req = SessionProxyClient.makeConnectRequest(
            baseURL: URL(string: "https://api.pendingname.com")!,
            sessionId: "sess-1", role: .runner, token: "pdg_abc")
        let url = req.url!
        XCTAssertEqual(url.scheme, "wss")
        XCTAssertEqual(url.host, "api.pendingname.com")
        XCTAssertEqual(url.path, "/v1/sessions/sess-1/proxy/connect")
        XCTAssertTrue(url.query?.contains("role=runner") ?? false, "role query missing: \(url)")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer pdg_abc")
    }

    func testConnectRequestUpgradesHttpToWs() {
        let req = SessionProxyClient.makeConnectRequest(
            baseURL: URL(string: "http://127.0.0.1:8787")!,
            sessionId: "s2", role: .viewer, token: nil)
        let url = req.url!
        XCTAssertEqual(url.scheme, "ws")
        XCTAssertEqual(url.host, "127.0.0.1")
        XCTAssertEqual(url.port, 8787)
        XCTAssertTrue(url.query?.contains("role=viewer") ?? false)
    }

    func testConnectRequestOmitsAuthWhenTokenNil() {
        let req = SessionProxyClient.makeConnectRequest(
            baseURL: URL(string: "https://api.pendingname.com")!,
            sessionId: "s3", role: .runner, token: nil)
        XCTAssertNil(req.value(forHTTPHeaderField: "Authorization"))
    }

    // MARK: - SessionProxyClient.dispatch (inbound routing → AsyncStream)

    func testDispatchYieldsRunnerActionableFramesOnly() async {
        let client = SessionProxyClient(
            baseURL: URL(string: "https://api.pendingname.com")!,
            sessionId: "s4", token: "pdg_x")
        // subscribed ack + an error are handled internally and must NOT leak to
        // the inbound stream; command + decision must.
        await client.dispatch(.subscribed(sessionId: "s4", role: .runner))
        await client.dispatch(.command(commandId: "c1", kind: "send_prompt",
                                       payload: ["text": .string("hi")], queued: false))
        await client.dispatch(.error(code: "x", message: "y"))
        await client.dispatch(.permissionDecision(requestId: "r1", decision: "approve"))
        await client.close() // finishes the stream so the for-await terminates

        var received: [SessionProxyInbound] = []
        for await frame in client.inbound { received.append(frame) }
        XCTAssertEqual(received, [
            .command(commandId: "c1", kind: "send_prompt",
                     payload: ["text": .string("hi")], queued: false),
            .permissionDecision(requestId: "r1", decision: "approve"),
        ])
    }

    // MARK: - SessionProxyViewerOutbound (viewer → DO)

    func testViewerCommandFrameShape() throws {
        let frame = SessionProxyViewerOutbound.command(kind: "cancel", payload: nil)
        let d = decode(try frame.jsonData())
        XCTAssertEqual(d["type"] as? String, "session.command")
        let cmd = d["command"] as? [String: Any]
        XCTAssertEqual(cmd?["kind"] as? String, "cancel")
        XCTAssertNil(cmd?["payload"], "nil payload must be omitted")
    }

    func testViewerCommandFrameWithPayload() throws {
        let frame = SessionProxyViewerOutbound.command(
            kind: "send_prompt", payload: ["text": .string("再跑一遍测试")])
        let d = decode(try frame.jsonData())
        let cmd = d["command"] as? [String: Any]
        XCTAssertEqual(cmd?["kind"] as? String, "send_prompt")
        XCTAssertEqual((cmd?["payload"] as? [String: Any])?["text"] as? String, "再跑一遍测试")
    }

    func testViewerPermissionDecisionFrameShape() throws {
        let frame = SessionProxyViewerOutbound.permissionDecision(requestId: "r1", decision: "approve")
        let d = decode(try frame.jsonData())
        XCTAssertEqual(d["type"] as? String, "permission.decision")
        XCTAssertEqual(d["requestId"] as? String, "r1")
        XCTAssertEqual(d["decision"] as? String, "approve")
    }

    // MARK: - SessionStateSnapshot (viewer-side inbound decode)

    func testSnapshotDecodesWhatRunnerPublishes() {
        // The runner publishes SessionState.dict(); the DO fans it out verbatim
        // inside {type:"session.state", state:{…}}. The viewer must decode the
        // exact bytes back — round-trip through the real outbound encoder.
        let state = SessionState(status: "running", eventCount: 5, lastEvent: "Bash：ls")
        let data = try! SessionProxyOutbound.sessionState(state).jsonData()
        let wire = String(data: data, encoding: .utf8)!
        let snap = SessionStateSnapshot.parse(wire)
        XCTAssertEqual(snap, SessionStateSnapshot(status: "running", eventCount: 5, lastEvent: "Bash：ls"))
    }

    func testSnapshotRejectsNonStateFrames() {
        XCTAssertNil(SessionStateSnapshot.parse(#"{"type":"subscribed","sessionId":"s","role":"viewer"}"#))
        XCTAssertNil(SessionStateSnapshot.parse("not json"))
        XCTAssertNil(SessionStateSnapshot.parse(#"{"type":"session.command","commandId":"c"}"#))
    }

    func testViewerDispatchYieldsLiveStateOnTheStatesStream() async {
        let client = SessionProxyClient(
            baseURL: URL(string: "https://api.pendingname.com")!,
            sessionId: "s5", role: .viewer, token: "pdg_x")
        // The receive loop parses the frame as SessionProxyInbound first, where a
        // viewer-facing session.state degrades to .unknown; dispatch then decodes
        // it onto `states`. Drive that exact path.
        let raw = #"{"type":"session.state","state":{"status":"completed","eventCount":2,"lastEvent":"done"}}"#
        await client.dispatch(SessionProxyInbound.parse(raw))
        await client.close() // finish the stream so for-await terminates

        var got: [SessionStateSnapshot] = []
        for await snap in client.states { got.append(snap) }
        XCTAssertEqual(got, [SessionStateSnapshot(status: "completed", eventCount: 2, lastEvent: "done")])
    }
}
#endif
