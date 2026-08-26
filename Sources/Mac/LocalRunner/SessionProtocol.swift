#if os(macOS)
import Foundation

// MARK: - Wire frames (§4.1)

/// §4.1 的三种帧。`length` 统一表示「kind + payload」的字节数，所有 u32 使用
/// network byte order（big-endian）；这两条布局一旦改变才允许 protocolVersion +1。
enum SessionWireFrame: Equatable {
    case control(Data)
    case terminal(handle: UInt32, bytes: [UInt8])
    /// P2 只定义分片格式；快照内容的序列化属于 P3。
    /// `seq` 从 0 单调递增；bit0 `isLast` 是唯一的结束依据。其余 flags 位保留，
    /// 解码方必须忽略，不能因未知位断连。
    case snapshot(handle: UInt32, seq: UInt32, isLast: Bool, bytes: [UInt8])
}

enum SessionFrameError: Error, Equatable {
    case frameTooShort
    case frameTooLarge(Int)
    case unknownKind(UInt8)
    case malformedPayload(kind: UInt8, minimum: Int, actual: Int)
    case trailingPartialFrame(Int)
}

enum SessionFrameEncoder {
    static func encode(_ frame: SessionWireFrame) throws -> Data {
        var payload = Data()
        let kind: UInt8
        switch frame {
        case let .control(json):
            kind = 0
            payload = json
        case let .terminal(handle, bytes):
            kind = 1
            append(handle, to: &payload)
            payload.append(contentsOf: bytes)
        case let .snapshot(handle, seq, isLast, bytes):
            kind = 2
            append(handle, to: &payload)
            append(seq, to: &payload)
            payload.append(isLast ? 0x01 : 0x00)
            payload.append(contentsOf: bytes)
        }
        let length = 1 + payload.count
        guard length <= Int(UInt32.max) else { throw SessionFrameError.frameTooLarge(length) }
        var out = Data()
        append(UInt32(length), to: &out)
        out.append(kind)
        out.append(payload)
        return out
    }

    /// 对已经序列化的快照做 wire 分片；不参与 §5.3 的快照序列化。
    /// 显式 `isLast` 避免整除 64 KiB 时生成伪空终止片；空快照则恰好一片。
    static func snapshotFrames(
        handle: UInt32, serializedBytes: [UInt8], maximumPayloadSize: Int = 64 * 1024
    ) -> [SessionWireFrame] {
        precondition(maximumPayloadSize > 0)
        guard !serializedBytes.isEmpty else {
            return [.snapshot(handle: handle, seq: 0, isLast: true, bytes: [])]
        }

        let count = (serializedBytes.count + maximumPayloadSize - 1) / maximumPayloadSize
        precondition(count <= Int(UInt32.max))
        return (0..<count).map { index in
            let start = index * maximumPayloadSize
            let end = min(start + maximumPayloadSize, serializedBytes.count)
            return .snapshot(handle: handle, seq: UInt32(index), isLast: index == count - 1,
                             bytes: Array(serializedBytes[start..<end]))
        }
    }

    fileprivate static func append(_ value: UInt32, to data: inout Data) {
        data.append(UInt8((value >> 24) & 0xff))
        data.append(UInt8((value >> 16) & 0xff))
        data.append(UInt8((value >> 8) & 0xff))
        data.append(UInt8(value & 0xff))
    }
}

struct SessionFrameDecoder {
    /// 防止损坏/恶意长度前缀让连接一次性申请无界内存。快照片段规范是 64 KiB；
    /// 64 MiB 给控制帧留足余量，又能 fail-closed。
    static let maximumFrameLength = 64 * 1024 * 1024
    private var buffer = Data()

    mutating func append<S: DataProtocol>(_ bytes: S) throws -> [SessionWireFrame] {
        buffer.append(contentsOf: bytes)
        var frames: [SessionWireFrame] = []
        while buffer.count >= 4 {
            let length = Int(readUInt32(buffer, at: 0))
            guard length >= 1 else { throw SessionFrameError.frameTooShort }
            guard length <= Self.maximumFrameLength else {
                throw SessionFrameError.frameTooLarge(length)
            }
            let total = 4 + length
            guard buffer.count >= total else { break }
            let kind = buffer[buffer.startIndex + 4]
            let bodyStart = buffer.startIndex + 5
            let bodyEnd = buffer.startIndex + total
            let body = buffer[bodyStart..<bodyEnd]
            frames.append(try Self.decode(kind: kind, body: body))
            buffer.removeFirst(total)
        }
        return frames
    }

    static func decodeAll(_ data: Data) throws -> [SessionWireFrame] {
        var decoder = SessionFrameDecoder()
        let frames = try decoder.append(data)
        guard decoder.buffer.isEmpty else {
            throw SessionFrameError.trailingPartialFrame(decoder.buffer.count)
        }
        return frames
    }

    private static func decode(kind: UInt8, body: Data.SubSequence) throws -> SessionWireFrame {
        switch kind {
        case 0:
            return .control(Data(body))
        case 1:
            guard body.count >= 4 else {
                throw SessionFrameError.malformedPayload(kind: kind, minimum: 4, actual: body.count)
            }
            let data = Data(body)
            return .terminal(handle: readUInt32(data, at: 0),
                             bytes: Array(data.dropFirst(4)))
        case 2:
            guard body.count >= 9 else {
                throw SessionFrameError.malformedPayload(kind: kind, minimum: 9, actual: body.count)
            }
            let data = Data(body)
            return .snapshot(handle: readUInt32(data, at: 0),
                             seq: readUInt32(data, at: 4),
                             isLast: data[data.startIndex + 8] & 0x01 != 0,
                             bytes: Array(data.dropFirst(9)))
        default:
            throw SessionFrameError.unknownKind(kind)
        }
    }
}

private func readUInt32(_ data: Data, at offset: Int) -> UInt32 {
    let i = data.startIndex + offset
    return (UInt32(data[i]) << 24)
        | (UInt32(data[i + 1]) << 16)
        | (UInt32(data[i + 2]) << 8)
        | UInt32(data[i + 3])
}

// MARK: - JSON values used by extensible control/event payloads

enum SessionWireJSONValue: Equatable, Codable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: SessionWireJSONValue])
    case array([SessionWireJSONValue])
    case null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let value = try? c.decode(Bool.self) { self = .bool(value) }
        else if let value = try? c.decode(Double.self) { self = .number(value) }
        else if let value = try? c.decode(String.self) { self = .string(value) }
        else if let value = try? c.decode([String: SessionWireJSONValue].self) { self = .object(value) }
        else { self = .array(try c.decode([SessionWireJSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case let .string(value): try c.encode(value)
        case let .number(value): try c.encode(value)
        case let .bool(value): try c.encode(value)
        case let .object(value): try c.encode(value)
        case let .array(value): try c.encode(value)
        case .null: try c.encodeNil()
        }
    }
}

// MARK: - app -> daemon (8 messages)

struct SessionAppHello: Codable, Equatable {
    var protocolVersion: Int
    var appBuild: String
    var capabilities: [String] = []

    private enum CodingKeys: String, CodingKey { case protocolVersion, appBuild, capabilities }

    init(protocolVersion: Int, appBuild: String, capabilities: [String] = []) {
        self.protocolVersion = protocolVersion
        self.appBuild = appBuild
        self.capabilities = capabilities
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        protocolVersion = try c.decode(Int.self, forKey: .protocolVersion)
        appBuild = try c.decode(String.self, forKey: .appBuild)
        capabilities = try c.decodeIfPresent([String].self, forKey: .capabilities) ?? []
    }
}

struct SessionAttach: Codable, Equatable {
    var sessionId: String
    var cols: Int
    var rows: Int
}

struct SessionDetach: Codable, Equatable { var handle: UInt32 }

struct SessionResize: Codable, Equatable {
    var handle: UInt32
    var cols: Int
    var rows: Int
}

struct SessionInput: Codable, Equatable {
    var handle: UInt32
    var bytes: [UInt8]
}

struct SessionControl: Codable, Equatable {
    var requestId: String?
    var op: String
    var arguments: [String: SessionWireJSONValue] = [:]

    private enum CodingKeys: String, CodingKey { case requestId, op, arguments }

    init(requestId: String?, op: String,
         arguments: [String: SessionWireJSONValue] = [:]) {
        self.requestId = requestId
        self.op = op
        self.arguments = arguments
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decodeIfPresent(String.self, forKey: .requestId)
        op = try c.decode(String.self, forKey: .op)
        arguments = try c.decodeIfPresent(
            [String: SessionWireJSONValue].self, forKey: .arguments) ?? [:]
    }
}

struct SessionPing: Codable, Equatable {
    var nonce: UInt64? = nil
}

enum SessionAppMessage: Equatable {
    case hello(SessionAppHello)
    case listSessions
    case attach(SessionAttach)
    case detach(SessionDetach)
    case resize(SessionResize)
    case input(SessionInput)
    case control(SessionControl)
    case ping(SessionPing)
}

// MARK: - daemon -> app (8 messages)

struct SessionDaemonHello: Codable, Equatable {
    var protocolVersion: Int
    var daemonBuild: String
    var capabilities: [String] = []
    var sessionCount: Int
    var pid: Int32

    private enum CodingKeys: String, CodingKey {
        case protocolVersion, daemonBuild, capabilities, sessionCount, pid
    }

    init(protocolVersion: Int, daemonBuild: String, capabilities: [String] = [],
         sessionCount: Int, pid: Int32) {
        self.protocolVersion = protocolVersion
        self.daemonBuild = daemonBuild
        self.capabilities = capabilities
        self.sessionCount = sessionCount
        self.pid = pid
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        protocolVersion = try c.decode(Int.self, forKey: .protocolVersion)
        daemonBuild = try c.decode(String.self, forKey: .daemonBuild)
        capabilities = try c.decodeIfPresent([String].self, forKey: .capabilities) ?? []
        sessionCount = try c.decode(Int.self, forKey: .sessionCount)
        pid = try c.decode(Int32.self, forKey: .pid)
    }
}

enum SessionWireStatus: Equatable, Codable {
    case running
    case exited(Int32?)

    private enum CodingKeys: String, CodingKey { case kind, code }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "running": self = .running
        case "exited": self = .exited(try c.decodeIfPresent(Int32.self, forKey: .code))
        default:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: c,
                                                   debugDescription: "unknown session status")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .running:
            try c.encode("running", forKey: .kind)
        case let .exited(code):
            try c.encode("exited", forKey: .kind)
            try c.encodeIfPresent(code, forKey: .code)
        }
    }
}

struct SessionHealthWire: Codable, Equatable {
    var kind: String
    var detail: String
}

struct SessionPendingDecisionWire: Codable, Equatable {
    var prompt: String
    var options: [String]
}

struct SessionLaunchParameterProblemWire: Codable, Equatable {
    var kind: String
    var value: String
    var quote: String
}

struct SessionScrollStateWire: Codable, Equatable {
    var canScroll: Bool
    var position: Double
    var thumbSize: Double
    var userScrollTick: Int
}

/// P2 先发完整状态作为 delta（字段集小、单向权威）；`stateSeq` 仍逐次递增。
/// 新字段只追加且带默认值时不升级版本，旧端会按 Codable 规则忽略。
struct SessionProtocolState: Codable, Equatable {
    var status: SessionWireStatus
    var isWorking: Bool
    var displayIsTyping: Bool
    var health: SessionHealthWire?
    var pendingDecision: SessionPendingDecisionWire?
    var kind: String
    var launchParameterProblem: SessionLaunchParameterProblemWire?
    var scrollState: SessionScrollStateWire?
}

struct SessionSummary: Codable, Equatable {
    var sessionId: String
    var stateSeq: UInt64
    var state: SessionProtocolState
}

struct SessionList: Codable, Equatable { var sessions: [SessionSummary] }

struct SessionAttached: Codable, Equatable {
    var sessionId: String
    var handle: UInt32
    /// 仅供进度展示；kind=2 的 bit0 `isLast` 才是结束依据，接收方不得依赖此值。
    var snapshotFrames: UInt32
}

struct SessionStateChange: Codable, Equatable {
    var sessionId: String
    var stateSeq: UInt64
    var delta: SessionProtocolState
}

struct SessionTerminalData: Equatable {
    var handle: UInt32
    var bytes: [UInt8]
}

struct SessionResync: Codable, Equatable {
    var handle: UInt32
}

struct SessionEvent: Codable, Equatable {
    var kind: String
    var requestId: String?
    var fields: [String: SessionWireJSONValue] = [:]

    private enum CodingKeys: String, CodingKey { case kind, requestId, fields }

    init(kind: String, requestId: String?, fields: [String: SessionWireJSONValue] = [:]) {
        self.kind = kind
        self.requestId = requestId
        self.fields = fields
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decode(String.self, forKey: .kind)
        requestId = try c.decodeIfPresent(String.self, forKey: .requestId)
        fields = try c.decodeIfPresent(
            [String: SessionWireJSONValue].self, forKey: .fields) ?? [:]
    }
}

struct SessionPong: Codable, Equatable { var nonce: UInt64? = nil }

enum SessionDaemonMessage: Equatable {
    case hello(SessionDaemonHello)
    case sessions(SessionList)
    case attached(SessionAttached)
    case state(SessionStateChange)
    case data(SessionTerminalData)
    case resync(SessionResync)
    case event(SessionEvent)
    case pong(SessionPong)
}

// MARK: - Message codec and forward compatibility (§4.4)

struct SessionProtocolCodec {
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()

    func encode(_ message: SessionAppMessage) throws -> Data {
        let json: Data
        switch message {
        case let .hello(value): json = try encodeControl(type: "hello", value: value)
        case .listSessions: json = try encodeEmptyControl(type: "listSessions")
        case let .attach(value): json = try encodeControl(type: "attach", value: value)
        case let .detach(value): json = try encodeControl(type: "detach", value: value)
        case let .resize(value): json = try encodeControl(type: "resize", value: value)
        case let .input(value): json = try encodeControl(type: "input", value: value)
        case let .control(value): json = try encodeControl(type: "control", value: value)
        case let .ping(value): json = try encodeControl(type: "ping", value: value)
        }
        return try SessionFrameEncoder.encode(.control(json))
    }

    func encode(_ message: SessionDaemonMessage) throws -> Data {
        switch message {
        case let .data(value):
            return try SessionFrameEncoder.encode(.terminal(handle: value.handle, bytes: value.bytes))
        case let .hello(value):
            return try SessionFrameEncoder.encode(.control(encodeControl(type: "hello", value: value)))
        case let .sessions(value):
            return try SessionFrameEncoder.encode(.control(encodeControl(type: "sessions", value: value)))
        case let .attached(value):
            return try SessionFrameEncoder.encode(.control(encodeControl(type: "attached", value: value)))
        case let .state(value):
            return try SessionFrameEncoder.encode(.control(encodeControl(type: "state", value: value)))
        case let .resync(value):
            return try SessionFrameEncoder.encode(.control(encodeControl(type: "resync", value: value)))
        case let .event(value):
            return try SessionFrameEncoder.encode(.control(encodeControl(type: "event", value: value)))
        case let .pong(value):
            return try SessionFrameEncoder.encode(.control(encodeControl(type: "pong", value: value)))
        }
    }

    /// 未知 `type` 返回 nil：调用方继续读下一帧，不报错、不断连。
    func decodeApp(_ framed: Data) throws -> SessionAppMessage? {
        guard case let .control(json) = try exactlyOneFrame(framed) else { return nil }
        switch try type(of: json) {
        case "hello": return .hello(try decoder.decode(SessionAppHello.self, from: json))
        case "listSessions": return .listSessions
        case "attach": return .attach(try decoder.decode(SessionAttach.self, from: json))
        case "detach": return .detach(try decoder.decode(SessionDetach.self, from: json))
        case "resize": return .resize(try decoder.decode(SessionResize.self, from: json))
        case "input": return .input(try decoder.decode(SessionInput.self, from: json))
        case "control": return .control(try decoder.decode(SessionControl.self, from: json))
        case "ping": return .ping(try decoder.decode(SessionPing.self, from: json))
        default: return nil
        }
    }

    /// 未知控制消息与 app 侧相同地忽略；kind=1 直接还原原始 PTY 字节。
    func decodeDaemon(_ framed: Data) throws -> SessionDaemonMessage? {
        switch try exactlyOneFrame(framed) {
        case let .terminal(handle, bytes): return .data(.init(handle: handle, bytes: bytes))
        case .snapshot: return nil // P3 消费；P2 只冻结帧格式。
        case let .control(json):
            switch try type(of: json) {
            case "hello": return .hello(try decoder.decode(SessionDaemonHello.self, from: json))
            case "sessions": return .sessions(try decoder.decode(SessionList.self, from: json))
            case "attached": return .attached(try decoder.decode(SessionAttached.self, from: json))
            case "state": return .state(try decoder.decode(SessionStateChange.self, from: json))
            case "resync": return .resync(try decoder.decode(SessionResync.self, from: json))
            case "event": return .event(try decoder.decode(SessionEvent.self, from: json))
            case "pong": return .pong(try decoder.decode(SessionPong.self, from: json))
            default: return nil
            }
        }
    }

    private func exactlyOneFrame(_ data: Data) throws -> SessionWireFrame {
        let frames = try SessionFrameDecoder.decodeAll(data)
        guard frames.count == 1, let frame = frames.first else {
            throw SessionFrameError.trailingPartialFrame(data.count)
        }
        return frame
    }

    private func encodeEmptyControl(type: String) throws -> Data {
        try JSONSerialization.data(withJSONObject: ["type": type])
    }

    private func encodeControl<T: Encodable>(type: String, value: T) throws -> Data {
        var object = try JSONSerialization.jsonObject(with: encoder.encode(value)) as? [String: Any]
            ?? [:]
        object["type"] = type
        return try JSONSerialization.data(withJSONObject: object)
    }

    private func type(of data: Data) throws -> String? {
        (try JSONSerialization.jsonObject(with: data) as? [String: Any])?["type"] as? String
    }
}

// MARK: - Capability and version discipline

enum SessionCapabilities {
    static func negotiate(app: [String], daemon: [String]) -> [String] {
        Array(Set(app).intersection(daemon)).sorted()
    }

    static func supports(_ capability: String, in negotiated: [String]) -> Bool {
        negotiated.contains(capability)
    }
}

enum SessionCompatibility: Equatable {
    case compatible(capabilities: [String])
    case daemonTooOld
    case daemonTooNew

    static func evaluate(
        appProtocolVersion: Int,
        daemonProtocolVersion: Int,
        appCapabilities: [String],
        daemonCapabilities: [String]
    ) -> SessionCompatibility {
        if daemonProtocolVersion < appProtocolVersion { return .daemonTooOld }
        if daemonProtocolVersion > appProtocolVersion { return .daemonTooNew }
        return .compatible(capabilities: SessionCapabilities.negotiate(
            app: appCapabilities, daemon: daemonCapabilities))
    }
}
#endif
