#if os(macOS)
import XCTest

/// 前后端分离 P2 的 wire 护栏。
///
/// 最重要的不变量不是「今天能编解码」，而是旧端收到新字段/新消息时仍继续服务：
/// 更新 app 不能因为 daemon 少认识一个能力就打断正在跑的 session。
final class SessionProtocolCodecTests: XCTestCase {
    private let codec = SessionProtocolCodec()

    // MARK: - app -> daemon：8 条消息全部覆盖

    func testEveryAppMessageRoundTripsAndIgnoresAnUnknownField() throws {
        let messages: [SessionAppMessage] = [
            .hello(.init(protocolVersion: 1, appBuild: "app-101",
                         capabilities: ["state-seq", "terminal-bytes"])),
            .listSessions,
            .attach(.init(sessionId: "s-1", cols: 120, rows: 42)),
            .detach(.init(handle: 7)),
            .resize(.init(handle: 7, cols: 80, rows: 25)),
            .input(.init(handle: 7, bytes: [0x1b, 0x0d, 0xff])),
            .control(.init(requestId: "req-1", op: "stop",
                           arguments: ["force": .bool(false)])),
            .ping(.init(nonce: 99)),
        ]

        for message in messages {
            let encoded = try codec.encode(message)
            XCTAssertEqual(try codec.decodeApp(encoded), message, "plain round-trip: \(message)")

            let extended = try addingUnknownField(to: encoded)
            XCTAssertEqual(try codec.decodeApp(extended), message,
                           "unknown field must not change behavior: \(message)")
        }
    }

    func testAppUnknownTypeIsIgnoredAndNextKnownMessageStillDecodes() throws {
        let unknown = try controlFrame(json: [
            "type": "futureAppMessage",
            "futurePayload": ["nested": true],
        ])
        XCTAssertNil(try codec.decodeApp(unknown))
        XCTAssertEqual(try codec.decodeApp(codec.encode(.ping(.init(nonce: 7)))),
                       .ping(.init(nonce: 7)))
    }

    // MARK: - daemon -> app：8 条消息全部覆盖

    func testEveryDaemonControlMessageRoundTripsAndIgnoresAnUnknownField() throws {
        let state = SessionProtocolState(
            status: .running,
            isWorking: true,
            displayIsTyping: false,
            health: .init(kind: "rateLimited", detail: "wait"),
            pendingDecision: .init(prompt: "Continue?", options: ["Yes", "No"]),
            kind: "claude_code",
            launchParameterProblem: .init(kind: "effortIgnored", value: "auto", quote: "ignored"),
            scrollState: .init(canScroll: true, position: 0.5, thumbSize: 0.2,
                               userScrollTick: 3))
        let summary = SessionSummary(sessionId: "s-1", stateSeq: 12, state: state)
        let messages: [SessionDaemonMessage] = [
            .hello(.init(protocolVersion: 1, daemonBuild: "daemon-99",
                         capabilities: ["state-seq"], sessionCount: 1, pid: 4321)),
            .sessions(.init(sessions: [summary])),
            .attached(.init(sessionId: "s-1", handle: 7, snapshotFrames: 0)),
            .state(.init(sessionId: "s-1", stateSeq: 13, delta: state)),
            .resync(.init(handle: 7)),
            .event(.init(kind: "profileSwitchResult", requestId: "req-1",
                         fields: ["outcome": .string("unsupported")])),
            .pong(.init(nonce: 99)),
        ]

        for message in messages {
            let encoded = try codec.encode(message)
            XCTAssertEqual(try codec.decodeDaemon(encoded), message,
                           "plain round-trip: \(message)")

            let extended = try addingUnknownField(to: encoded)
            XCTAssertEqual(try codec.decodeDaemon(extended), message,
                           "unknown field must not change behavior: \(message)")
        }
    }

    func testDaemonDataMessageUsesKindOneRawBytesNotJSONOrBase64() throws {
        let message = SessionDaemonMessage.data(.init(handle: 0x0102_0304,
                                                       bytes: [0x00, 0xff, 0x1b, 0x5b]))
        let encoded = try codec.encode(message)
        let frames = try SessionFrameDecoder.decodeAll(encoded)
        XCTAssertEqual(frames, [.terminal(handle: 0x0102_0304,
                                           bytes: [0x00, 0xff, 0x1b, 0x5b])])
        XCTAssertEqual(try codec.decodeDaemon(encoded), message)
        XCTAssertFalse(encoded.contains(Data("AA==".utf8)), "热路径不许出现 base64")
        XCTAssertFalse(encoded.contains(Data("bytes".utf8)), "热路径不许出现 JSON 字段名")
    }

    func testDaemonUnknownTypeIsIgnoredWithoutPoisoningBinaryDataThatFollows() throws {
        let unknown = try controlFrame(json: ["type": "futureDaemonMessage", "x": 1])
        XCTAssertNil(try codec.decodeDaemon(unknown))
        let data = try codec.encode(.data(.init(handle: 11, bytes: [1, 2, 3])))
        XCTAssertEqual(try codec.decodeDaemon(data), .data(.init(handle: 11, bytes: [1, 2, 3])))
    }

    // MARK: - 三种帧

    func testControlTerminalAndSnapshotFramesUseLengthPrefixAndFixedHeaders() throws {
        let frames: [SessionWireFrame] = [
            .control(Data(#"{"type":"ping","nonce":1}"#.utf8)),
            .terminal(handle: 0x0102_0304, bytes: [0x41, 0x00, 0xff]),
            .snapshot(handle: 9, seq: 17, isLast: true, bytes: [0xde, 0xad]),
        ]
        let stream = try frames.reduce(into: Data()) { out, frame in
            out.append(try SessionFrameEncoder.encode(frame))
        }
        XCTAssertEqual(try SessionFrameDecoder.decodeAll(stream), frames)
    }

    func testIncrementalDecoderWaitsForACompleteFrame() throws {
        let bytes = try SessionFrameEncoder.encode(.terminal(handle: 44, bytes: [1, 2, 3, 4]))
        var decoder = SessionFrameDecoder()
        XCTAssertEqual(try decoder.append(bytes.prefix(3)), [])
        XCTAssertEqual(try decoder.append(bytes.dropFirst(3).prefix(4)), [])
        XCTAssertEqual(try decoder.append(bytes.dropFirst(7)),
                       [.terminal(handle: 44, bytes: [1, 2, 3, 4])])
    }

    func testSnapshotChunksCarryHandleAndMonotonicSequenceWithoutSerializingSnapshotContent() throws {
        let first = try SessionFrameEncoder.encode(
            .snapshot(handle: 5, seq: 0, isLast: false, bytes: [0, 1]))
        let second = try SessionFrameEncoder.encode(
            .snapshot(handle: 5, seq: 1, isLast: true, bytes: [2, 3]))
        XCTAssertEqual(try SessionFrameDecoder.decodeAll(first + second), [
            .snapshot(handle: 5, seq: 0, isLast: false, bytes: [0, 1]),
            .snapshot(handle: 5, seq: 1, isLast: true, bytes: [2, 3]),
        ])
    }

    func testSnapshotExactlyMultipleOf64KiBEndsOnANonEmptyLastFrame() {
        let bytes = [UInt8](repeating: 0xa5, count: 2 * 64 * 1024)
        let frames = SessionFrameEncoder.snapshotFrames(handle: 5, serializedBytes: bytes)

        XCTAssertEqual(frames.count, 2)
        XCTAssertEqual(frames.last,
                       .snapshot(handle: 5, seq: 1, isLast: true,
                                 bytes: [UInt8](repeating: 0xa5, count: 64 * 1024)))
    }

    func testEmptySnapshotIsOneEmptyLastFrame() {
        XCTAssertEqual(SessionFrameEncoder.snapshotFrames(handle: 8, serializedBytes: []), [
            .snapshot(handle: 8, seq: 0, isLast: true, bytes: []),
        ])
    }

    func testSnapshotUnknownHighFlagBitsAreIgnored() throws {
        var encoded = try SessionFrameEncoder.encode(
            .snapshot(handle: 9, seq: 3, isLast: false, bytes: [0xde, 0xad]))
        // [u32 length][u8 kind][u32 handle][u32 seq][u8 flags][bytes]
        encoded[encoded.startIndex + 13] = 0x80

        XCTAssertEqual(try SessionFrameDecoder.decodeAll(encoded), [
            .snapshot(handle: 9, seq: 3, isLast: false, bytes: [0xde, 0xad]),
        ])
    }

    // MARK: - hello 能力协商（不拿版本号当功能开关）

    func testCapabilitiesNegotiateByIntersectionAndMissingCapabilityDegrades() {
        let negotiated = SessionCapabilities.negotiate(
            app: ["state-seq", "terminal-bytes", "future-app-feature"],
            daemon: ["state-seq", "terminal-bytes", "old-only-feature"])
        XCTAssertEqual(negotiated, ["state-seq", "terminal-bytes"])
        XCTAssertFalse(SessionCapabilities.supports("future-app-feature", in: negotiated))
    }

    func testSameProtocolVersionAcceptsDifferentBuildsAndCapabilities() {
        XCTAssertEqual(
            SessionCompatibility.evaluate(
                appProtocolVersion: 1, daemonProtocolVersion: 1,
                appCapabilities: ["new-ui"], daemonCapabilities: []),
            .compatible(capabilities: []))
    }

    func testHelloMissingCapabilitiesDecodesAsEmptyAndDegrades() throws {
        let oldDaemonHello = try controlFrame(json: [
            "type": "hello", "protocolVersion": 1, "daemonBuild": "old",
            "sessionCount": 1, "pid": 42,
        ])
        XCTAssertEqual(try codec.decodeDaemon(oldDaemonHello),
                       .hello(.init(protocolVersion: 1, daemonBuild: "old",
                                    capabilities: [], sessionCount: 1, pid: 42)))
    }

    // MARK: - helpers

    private func addingUnknownField(to framed: Data) throws -> Data {
        let frames = try SessionFrameDecoder.decodeAll(framed)
        guard case let .control(payload) = try XCTUnwrap(frames.only) else {
            XCTFail("unknown JSON fields only apply to control frames")
            return framed
        }
        var object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: payload) as? [String: Any])
        object["futureField"] = ["nested": [1, 2, 3], "enabled": true]
        return try controlFrame(json: object)
    }

    private func controlFrame(json: [String: Any]) throws -> Data {
        try SessionFrameEncoder.encode(.control(JSONSerialization.data(withJSONObject: json)))
    }
}

private extension Array {
    var only: Element? { count == 1 ? self[0] : nil }
}
#endif
