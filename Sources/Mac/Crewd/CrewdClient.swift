import Foundation
import Network
import Security
import Combine

/// Expected durable scope comes from an independently verified pairing receipt.
/// These fields never replace TLS certificate verification or server grant checks.
struct CrewdRemoteBindingExpectation: Equatable, Sendable {
    let authorityID: String
    let dbID: String
    let grantID: String
    let grantRevision: String
    func accepts(_ binding: CrewdValue) -> Bool {
        binding["authorityId"].string == authorityID && binding["dbId"].string == dbID
        && binding["grantId"].string == grantID && binding["grantRevision"].string == grantRevision
        && binding["runtimeBindingId"].string.flatMap(UUID.init(uuidString:)) != nil
    }
}

/// Preserve signed and unsigned 64-bit wire integers; domain state stays owned by crewd.
indirect enum CrewdValue: Codable, Equatable, Sendable {
    case null, bool(Bool), integer(Int64), unsigned(UInt64), number(Double), string(String), array([CrewdValue]), object([String: CrewdValue])
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([String: CrewdValue].self) { self = .object(v) }
        else if let v = try? c.decode([CrewdValue].self) { self = .array(v) }
        else if let v = try? c.decode(Int64.self) { self = .integer(v) }
        else if let v = try? c.decode(UInt64.self) { self = .unsigned(v) }
        else { self = .number(try c.decode(Double.self)) }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let v): try c.encode(v)
        case .integer(let v): try c.encode(v)
        case .unsigned(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        }
    }
    subscript(_ key: String) -> CrewdValue { if case .object(let v) = self { return v[key] ?? .null }; return .null }
    var string: String? { if case .string(let v) = self { return v }; return nil }
    var array: [CrewdValue] { if case .array(let v) = self { return v }; return [] }
    var int: Int? { if case .integer(let v) = self { return Int(exactly: v) }; if case .unsigned(let v) = self { return Int(exactly: v) }; if case .number(let v) = self, v.isFinite, v >= Double(Int.min), v < Double(Int.max), v.rounded() == v { return Int(v) }; return nil }
    var pretty: String { let e = JSONEncoder(); e.outputFormatting = [.prettyPrinted, .sortedKeys]; return (try? e.encode(self)).flatMap { String(data: $0, encoding: .utf8) } ?? "" }
}

struct CrewdClientError: LocalizedError {
    enum Kind { case general, backendHandshake, notSubmitted }
    let message: String
    var kind: Kind = .general
    var code: Int? = nil
    var data: CrewdValue = .null
    static func isUncertainSubmission(_ error:Error)->Bool {
        guard let e=error as? CrewdClientError else {return true}
        if case .notSubmitted = e.kind {return false}
        return e.code == nil
    }
    var refusal: String? { data["refusal"].string }
    var currentWork: CrewdValue { data["currentWork"] }
    var errorDescription: String? { message }
}

/// The same bounded NDJSON decoder is used on Mac and mobile. A transport may
/// split a UTF-8 scalar or an RPC frame at any byte; only LF closes a frame.
final class CrewdNDJSONDecoder {
    private var buffer = Data()
    let maximumFrameBytes: Int
    init(maximumFrameBytes: Int = 64 * 1024 * 1024) {
        precondition(maximumFrameBytes > 0)
        self.maximumFrameBytes = maximumFrameBytes
    }
    func append(_ data: Data, onFrame: (CrewdValue) throws -> Void) throws {
        var start = data.startIndex
        while start < data.endIndex {
            let end = data[start...].firstIndex(of: 10) ?? data.endIndex
            let count = data.distance(from: start, to: end)
            guard count <= maximumFrameBytes - buffer.count else {
                throw CrewdClientError(message: "响应超过大小上限。")
            }
            buffer.append(contentsOf: data[start..<end])
            if end == data.endIndex { break }
            if !buffer.isEmpty {
                let frame = try JSONDecoder().decode(CrewdValue.self, from: buffer)
                buffer.removeAll(keepingCapacity: true)
                try onFrame(frame)
            }
            buffer.removeAll(keepingCapacity: true)
            start = data.index(after: end)
        }
    }
    func finish() throws {
        guard buffer.isEmpty else { throw CrewdClientError(message: "后台响应在消息完成前中断。") }
    }
}

/// NDJSON is a different wire protocol from the legacy length-prefixed daemon.
/// Each request has one generation, one response and a deadline; connection loss
/// fails pending work instead of replaying mutations or native human replies.
@MainActor
final class CrewdClient: ObservableObject {
    @Published private(set) var connected = false
    @Published private(set) var version = ""
    @Published private(set) var authorityBinding: CrewdValue = .null
    @Published private(set) var capabilities:Set<String>=[]
    /// The authoritative hello advertises the orchestra capability group,
    /// not an individual capability named after every session RPC.
    var supportsSessionStart: Bool { Self.sessionStartSupported(connected: connected, capabilities: capabilities) }
    static func sessionStartSupported(connected: Bool, capabilities: Set<String>) -> Bool {
        connected && capabilities.contains("orchestra")
    }
    @Published private(set) var lastError: String?
    var onEvent: ((String, CrewdValue) -> Void)?
    var onDisconnect: (() -> Void)?
    private var connection: NWConnection?
    #if os(macOS)
    private var localConnection: CrewdLocalIPC?
    func verifiedLocalPeerIdentity() throws -> CrewdKernelProcessIdentity {
        guard connected, let localConnection else { throw CrewdLocalIPCError(message: "当前连接没有可信本机内核对端。") }
        return try localConnection.verifiedPeer()
    }
    #endif
    @Published private(set) var runtimeIdentity: CrewdValue = .null
    private var hasTransport: Bool {
        #if os(macOS)
        return connection != nil || localConnection != nil
        #else
        return connection != nil
        #endif
    }
    private var generation = UUID()
    private var qualifiedForWork = true
    private var retired = false
    private var decoder = CrewdNDJSONDecoder()
    private var receivePending = false
    private var terminalFailure: String?
    private var nextID = 1
    private var pending: [Int: CheckedContinuation<CrewdValue, Error>] = [:]
    private var timeouts: [Int: Task<Void, Never>] = [:]
    private var ready: CheckedContinuation<Void, Error>?

    #if os(macOS)
    func connect(path: String) async throws {
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        disconnect(); qualifiedForWork = false
        let current = generation
        let local: CrewdLocalIPC
        do { local = try await CrewdLocalIPC.connect(path: path) }
        catch { throw CrewdClientError(message: error.localizedDescription, kind: .notSubmitted) }
        guard generation == current, !retired, !Task.isCancelled else { local.close(); throw CancellationError() }
        localConnection = local
        local.onData = { [weak self] data in
            guard let self, self.generation == current else { return }
            do { try self.ingest(data) }
            catch { self.fail("后台响应无法解析：\(error.localizedDescription)", notify: true) }
        }
        local.onEnd = { [weak self] message in
            guard let self, self.generation == current else { return }
            do { try self.decoder.finish(); self.fail(message, notify: true) }
            catch { self.fail(error.localizedDescription, notify: true) }
        }
        connected = true; local.startReading()
        do {
            _ = try local.verifiedPeer()
            try await acceptHello(clientName: "PendingCrew Mac", expectedRemote: nil, current: current)
            _ = try local.verifiedPeer()
        } catch {
            if generation == current { fail(error.localizedDescription, notify: false) }
            throw CrewdClientError(message: error.localizedDescription, kind: .backendHandshake)
        }
    }
    #endif

    /// Callers construct a mutually authenticated TLS configuration from a stored
    /// pairing. No work RPC or event is admitted until its actual hello matches.
    func connect(host: String, port: UInt16, parameters: NWParameters, expectedRemote: CrewdRemoteBindingExpectation) async throws {
        guard !host.isEmpty, host.utf8.count <= 255, port != 0,
              let networkPort = NWEndpoint.Port(rawValue: port) else {
            throw CrewdClientError(message: "电脑连接地址无效，尚未提交。", kind: .notSubmitted)
        }
        try await connect(NWConnection(host: .init(host), port: networkPort, using: parameters), clientName: "PendingCrew", expectedRemote: expectedRemote)
    }

    private func connect(_ c: NWConnection, clientName: String, expectedRemote: CrewdRemoteBindingExpectation?) async throws {
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        disconnect()
        qualifiedForWork = expectedRemote == nil
        let current = generation
        connection = c
        // Keep callback delivery FIFO on the main queue: an already received final
        // reply must be ingested before a later transport failure clears pending calls.
        c.stateUpdateHandler = { [weak self] state in
            DispatchQueue.main.async {
                guard let self, self.generation == current else { return }
                switch state {
                case .ready:
                    self.connected = true
                    self.ready?.resume(); self.ready = nil
                    self.receive(current)
                case .failed(let error): self.transportEnded(error.localizedDescription)
                case .cancelled: self.transportEnded("后台连接已关闭。")
                default: break
                }
            }
        }
        let deadline = Task { [weak self] in
            try? await Task.sleep(for: .seconds(5))
            guard !Task.isCancelled, let self, self.generation == current, self.ready != nil else { return }
            self.fail("连接后台超时。", notify: true)
        }
        var reachedBackend = false
        do {
            try await withCheckedThrowingContinuation { continuation in
                ready = continuation
                c.start(queue: DispatchQueue(label: "pendingcrew.crewd.client"))
            }
            guard generation == current, !Task.isCancelled else { throw CancellationError() }
            deadline.cancel()
            reachedBackend = true
            if expectedRemote != nil {
                guard let tls = c.metadata(definition: NWProtocolTLS.definition) as? NWProtocolTLS.Metadata,
                      sec_protocol_metadata_get_negotiated_tls_protocol_version(tls.securityProtocolMetadata) == .TLSv13 else {
                    throw CrewdClientError(message: "连接没有使用所需的安全协议。")
                }
            }
            try await acceptHello(clientName: clientName, expectedRemote: expectedRemote, current: current)
        } catch {
            deadline.cancel()
            if generation == current { fail(error.localizedDescription, notify: false) }
            if reachedBackend { throw CrewdClientError(message: error.localizedDescription, kind: .backendHandshake) }
            throw error
        }
    }

    private func acceptHello(clientName: String, expectedRemote: CrewdRemoteBindingExpectation?, current: UUID) async throws {
    let hello = try await call("system.hello", .object(["client": .object(["name": .string(clientName), "version": .string("Codex first")]), "apiVersion": .number(3)]))
    guard generation == current, !Task.isCancelled else { throw CancellationError() }
    guard hello["server"].string == "crewd", let api = hello["apiVersion"].int, api >= 3, (hello["minApiVersion"].int ?? 1) <= 3 else { throw CrewdClientError(message: "后台协议版本与客户端不兼容。") }
    if let expectedRemote {
        guard expectedRemote.accepts(hello["remoteBinding"]) else { throw CrewdClientError(message: "电脑身份或设备授权已改变，请重新配对。") }
        qualifiedForWork = true
    }
    guard expectedRemote != nil || hello["capabilities"].array.contains(.string("session.respondRunnerRequest")) else { throw CrewdClientError(message: "后台尚不支持 Codex 人工答复，请使用配套新版后台。") }
    authorityBinding = hello["remoteBinding"]
    version = hello["version"].string ?? "未知"
    capabilities=Set(hello["capabilities"].array.compactMap(\.string))
    let topics=["crew", "session", "message", "channel", "ask", "work", "restart", "system"] + ["documents","viewFolders"].filter {capabilities.contains($0)}
    _ = try await call("events.subscribe", .object(["topics": .array(topics.map(CrewdValue.string))]))
    guard generation == current, !Task.isCancelled else { throw CancellationError() }
    lastError = nil
    }

    func call(_ method: String, _ params: CrewdValue = .null, timeout: Duration = .seconds(40)) async throws -> CrewdValue {
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        guard !Task.isCancelled else { throw CrewdClientError(message: "请求已取消，尚未提交。", kind: .notSubmitted) }
        guard qualifiedForWork || method == "system.hello" else { throw CrewdClientError(message: "尚未核实电脑身份，操作未提交。", kind: .notSubmitted) }
        guard connected, terminalFailure == nil, hasTransport else { throw CrewdClientError(message: "后台尚未连接。",kind:.notSubmitted) }
        let id = nextID; nextID += 1
        var data:Data
        do {data = try JSONEncoder().encode(CrewdValue.object(["id": .number(Double(id)), "method": .string(method), "params": params]))}
        catch {throw CrewdClientError(message:"请求无法编码，尚未提交。",kind:.notSubmitted)}
        data.append(10)
        let current = generation
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
            guard !Task.isCancelled else {
                continuation.resume(throwing: CrewdClientError(message: "请求已取消，尚未提交。", kind: .notSubmitted))
                return
            }
            pending[id] = continuation
            timeouts[id] = Task { [weak self] in
                try? await Task.sleep(for: timeout)
                guard !Task.isCancelled, let self, self.generation == current else { return }
                self.finish(id, .failure(CrewdClientError(message: "请求超时，结果未知；请检查当前状态后再决定是否重试。")))
            }
            #if os(macOS)
            if let local = localConnection {
                do { try local.send(data) }
                catch { finish(id, .failure(CrewdClientError(message: error.localizedDescription, kind: .notSubmitted))) }
            } else if let c = connection {
            c.send(content: data, completion: .contentProcessed { [weak self] error in
                guard let error else { return }
                DispatchQueue.main.async {
                    guard let self, self.generation == current else { return }
                    self.transportEnded(error.localizedDescription)
                }
            })
            }
            #else
            if let c = connection {
            c.send(content: data, completion: .contentProcessed { [weak self] error in
                guard let error else { return }
                DispatchQueue.main.async {
                    guard let self, self.generation == current else { return }
                    self.transportEnded(error.localizedDescription)
                }
            })
            }
            #endif
            }
        } onCancel: {
            Task { @MainActor [weak self] in
                guard let self, self.generation == current else { return }
                self.finish(id, .failure(CrewdClientError(message: "等待已取消，提交结果未知；请检查当前状态。")))
            }
        }
    }

    func retire() { retired = true; disconnect() }
    func disconnect() { fail("连接已断开。", notify: false); lastError = nil }

    private func transportEnded(_ message: String) {
        // Network can announce failure before completing an outstanding receive.
        // Let that receive drain final bytes; only received RPC frames prove success.
        if connected && receivePending { terminalFailure = message }
        else { fail(message, notify: true) }
    }

    private func receive(_ current: UUID) {
        receivePending = true
        connection?.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, complete, error in
            DispatchQueue.main.async {
                guard let self, self.generation == current else { return }
                self.receivePending = false
                do {
                    if let data { try self.ingest(data) }
                    guard self.generation == current else { return }
                    if let error { self.fail(error.localizedDescription, notify: true) }
                    else if complete { try self.decoder.finish(); self.fail(self.terminalFailure ?? "后台连接已结束。", notify: true) }
                    else { self.receive(current) }
                } catch { self.fail("后台响应无法解析：\(error.localizedDescription)", notify: true) }
            }
        }
    }
    private func ingest(_ data: Data) throws {
        let current = generation
        try decoder.append(data) { frame in
            guard generation == current else { return }
            if let id = frame["id"].int {
                if frame["error"] != .null { finish(id, .failure(CrewdClientError(message: frame["error"]["message"].string ?? "后台请求失败。", code: frame["error"]["code"].int, data: frame["error"]["data"]))) }
                else { finish(id, .success(frame["result"])) }
            } else if let event = frame["event"].string, qualifiedForWork { onEvent?(event, frame["data"]) }
        }
    }
    private func finish(_ id: Int, _ result: Result<CrewdValue, Error>) {
        timeouts.removeValue(forKey: id)?.cancel()
        pending.removeValue(forKey: id)?.resume(with: result)
    }
    private func fail(_ message: String, notify: Bool) {
        let hadConnection = hasTransport
        generation = UUID()
        let c = connection; connection = nil
        c?.stateUpdateHandler = nil; c?.cancel()
        #if os(macOS)
        let local = localConnection; localConnection = nil
        local?.onData = nil; local?.onEnd = nil; local?.close()
        #endif
        runtimeIdentity = .null; authorityBinding = .null
        qualifiedForWork = false
        connected = false; capabilities=[]; receivePending = false; terminalFailure = nil; decoder = CrewdNDJSONDecoder(); lastError = message
        ready?.resume(throwing: CrewdClientError(message: message)); ready = nil
        let ids = Array(pending.keys)
        for id in ids { finish(id, .failure(CrewdClientError(message: "连接中断，结果未知；请检查当前状态。"))) }
        if notify && hadConnection { onDisconnect?() }
    }
}
