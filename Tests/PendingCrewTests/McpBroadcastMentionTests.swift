import XCTest

/// `post_to_crew` 的 `broadcast` mention（Todo #62 ③ 的前置）。
///
/// A 线把 `broadcast` 定义成**显式放宽器**：`[broadcast, session(X)]` = 全组看得见、
/// 只叫醒 X。但 MCP 这边的 kind 枚举原来只有 session/captain/human ——
/// **agent 今天根本发不出 broadcast**，那套语义对 agent 不成立。这一族钉住它发得出、
/// 且原样落盘。
final class McpBroadcastMentionTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcp-broadcast-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1")
    }

    /// 枚举里得有 broadcast —— 不在枚举里，模型就不会去填它。
    func testToolsListAdvertisesBroadcastKind() throws {
        let r = try XCTUnwrap(server(tempDir())
            .handleLine(#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#))
        XCTAssertTrue(r.contains("broadcast"), "post_to_crew 的 mentions kind 枚举缺 broadcast")
    }

    /// 「又广播又叫醒」这组合原样落盘，一个都不许被静默滤掉。
    func testBroadcastPlusSessionRoundTripsOntoWhiteboard() {
        let s = server(tempDir())
        _ = s.handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"post_to_crew","arguments":{"message":"回应 人类 To Do #3：选 A","mentions":[{"kind":"broadcast"},{"kind":"session","target_id":"worker-42"}]}}}"#)
        let msgs = s.store.list(crewId: "c")
        XCTAssertEqual(msgs.count, 1)
        XCTAssertEqual(msgs[0].mentions?.map(\.kind), ["broadcast", "session"])
        XCTAssertNil(msgs[0].mentions?[0].targetId)
        XCTAssertEqual(msgs[0].mentions?[1].targetId, "worker-42")
    }

    /// 只给 broadcast 时也照样落盘（等价于广播，但不该变成 nil —— 落盘保真，
    /// 「发的时候写了什么」和「判定成什么」是两件事）。
    func testBroadcastAloneIsStoredAsWritten() {
        let s = server(tempDir())
        _ = s.handleLine(#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"post_to_crew","arguments":{"message":"全组看一下","mentions":[{"kind":"broadcast"}]}}}"#)
        XCTAssertEqual(s.store.list(crewId: "c")[0].mentions?.map(\.kind), ["broadcast"])
    }
}
