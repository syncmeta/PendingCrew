#if os(macOS)
import Foundation
import Network
import Combine

/// The wire value stays lossless; domain state remains owned by crewd.
indirect enum CrewdValue: Codable, Equatable, Sendable {
    case null, bool(Bool), integer(Int64), number(Double), string(String), array([CrewdValue]), object([String: CrewdValue])
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([String: CrewdValue].self) { self = .object(v) }
        else if let v = try? c.decode([CrewdValue].self) { self = .array(v) }
        else if let v = try? c.decode(Int64.self) { self = .integer(v) }
        else { self = .number(try c.decode(Double.self)) }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let v): try c.encode(v)
        case .integer(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        }
    }
    subscript(_ key: String) -> CrewdValue { if case .object(let v) = self { return v[key] ?? .null }; return .null }
    var string: String? { if case .string(let v) = self { return v }; return nil }
    var array: [CrewdValue] { if case .array(let v) = self { return v }; return [] }
    var int: Int? { if case .integer(let v) = self { return Int(exactly: v) }; if case .number(let v) = self, v.isFinite, v >= Double(Int.min), v < Double(Int.max), v.rounded() == v { return Int(v) }; return nil }
    var pretty: String { let e = JSONEncoder(); e.outputFormatting = [.prettyPrinted, .sortedKeys]; return (try? e.encode(self)).flatMap { String(data: $0, encoding: .utf8) } ?? "" }
}

struct CrewdClientError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// NDJSON is a different wire protocol from the legacy length-prefixed daemon.
/// Each request has one generation, one response and a deadline; connection loss
/// fails pending work instead of replaying mutations or native human replies.
@MainActor
final class CrewdClient: ObservableObject {
    @Published private(set) var connected = false
    @Published private(set) var version = ""
    @Published private(set) var lastError: String?
    var onEvent: ((String, CrewdValue) -> Void)?
    var onDisconnect: (() -> Void)?
    private var connection: NWConnection?
    private var generation = UUID()
    private var buffer = Data()
    private var nextID = 1
    private var pending: [Int: CheckedContinuation<CrewdValue, Error>] = [:]
    private var timeouts: [Int: Task<Void, Never>] = [:]
    private var ready: CheckedContinuation<Void, Error>?

    func connect(path: String) async throws {
        disconnect()
        guard !path.utf8.contains(0), path.utf8.count <= 103 else { throw CrewdClientError(message: "后台连接路径无效或过长。") }
        let current = generation
        let c = NWConnection(to: .unix(path: path), using: .tcp)
        connection = c
        c.stateUpdateHandler = { [weak self] state in
            Task { @MainActor in
                guard let self, self.generation == current else { return }
                switch state {
                case .ready:
                    self.connected = true
                    self.ready?.resume(); self.ready = nil
                    self.receive(current)
                case .failed(let error): self.fail(error.localizedDescription, notify: true)
                case .cancelled: self.fail("后台连接已关闭。", notify: true)
                default: break
                }
            }
        }
        let deadline = Task { [weak self] in
            try? await Task.sleep(for: .seconds(5))
            guard !Task.isCancelled, let self, self.generation == current, self.ready != nil else { return }
            self.fail("连接后台超时。", notify: true)
        }
        do {
            try await withCheckedThrowingContinuation { continuation in
                ready = continuation
                c.start(queue: DispatchQueue(label: "pendingcrew.crewd.client"))
            }
            deadline.cancel()
            let hello = try await call("system.hello", .object(["client": .object(["name": .string("PendingCrew Mac"), "version": .string("Codex first")]), "apiVersion": .number(3)]))
            guard hello["server"].string == "crewd", let api = hello["apiVersion"].int, api >= 3, (hello["minApiVersion"].int ?? 1) <= 3 else { throw CrewdClientError(message: "后台协议版本与客户端不兼容。") }
            guard hello["capabilities"].array.contains(.string("session.respondRunnerRequest")) else { throw CrewdClientError(message: "后台尚不支持 Codex 人工答复，请使用配套新版后台。") }
            version = hello["version"].string ?? "未知"
            _ = try await call("events.subscribe", .object(["topics": .array(["crew", "session", "message", "channel", "ask", "system"].map(CrewdValue.string))]))
            lastError = nil
        } catch { deadline.cancel(); fail(error.localizedDescription, notify: false); throw error }
    }

    func call(_ method: String, _ params: CrewdValue = .null, timeout: Duration = .seconds(40)) async throws -> CrewdValue {
        guard connected, let c = connection else { throw CrewdClientError(message: "后台尚未连接。") }
        let id = nextID; nextID += 1
        var data = try JSONEncoder().encode(CrewdValue.object(["id": .number(Double(id)), "method": .string(method), "params": params]))
        data.append(10)
        let current = generation
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            timeouts[id] = Task { [weak self] in
                try? await Task.sleep(for: timeout)
                guard !Task.isCancelled, let self, self.generation == current else { return }
                self.finish(id, .failure(CrewdClientError(message: "请求超时，结果未知；请检查当前状态后再决定是否重试。")))
            }
            c.send(content: data, completion: .contentProcessed { [weak self] error in
                guard let error else { return }
                Task { @MainActor in
                    guard let self, self.generation == current else { return }
                    self.fail(error.localizedDescription, notify: true)
                }
            })
        }
    }

    func disconnect() { fail("连接已断开。", notify: false); lastError = nil }

    private func receive(_ current: UUID) {
        connection?.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, complete, error in
            Task { @MainActor in
                guard let self, self.generation == current else { return }
                do {
                    if let data { try self.ingest(data) }
                    if let error { self.fail(error.localizedDescription, notify: true) }
                    else if complete { self.fail("后台连接已结束。", notify: true) }
                    else { self.receive(current) }
                } catch { self.fail("后台响应无法解析：\(error.localizedDescription)", notify: true) }
            }
        }
    }
    private func ingest(_ data: Data) throws {
        buffer.append(data)
        while let newline = buffer.firstIndex(of: 10) {
            let line = Data(buffer[..<newline]); buffer.removeSubrange(...newline)
            if line.isEmpty { continue }
            guard line.count <= 64 * 1024 * 1024 else { throw CrewdClientError(message: "响应超过大小上限。") }
            let frame = try JSONDecoder().decode(CrewdValue.self, from: line)
            if let id = frame["id"].int {
                if frame["error"] != .null { finish(id, .failure(CrewdClientError(message: frame["error"]["message"].string ?? "后台请求失败。"))) }
                else { finish(id, .success(frame["result"])) }
            } else if let event = frame["event"].string { onEvent?(event, frame["data"]) }
        }
        guard buffer.count <= 64 * 1024 * 1024 else { throw CrewdClientError(message: "响应超过大小上限。") }
    }
    private func finish(_ id: Int, _ result: Result<CrewdValue, Error>) {
        timeouts.removeValue(forKey: id)?.cancel()
        pending.removeValue(forKey: id)?.resume(with: result)
    }
    private func fail(_ message: String, notify: Bool) {
        let hadConnection = connection != nil
        generation = UUID()
        let c = connection; connection = nil
        c?.stateUpdateHandler = nil; c?.cancel()
        connected = false; buffer.removeAll(); lastError = message
        ready?.resume(throwing: CrewdClientError(message: message)); ready = nil
        let ids = Array(pending.keys)
        for id in ids { finish(id, .failure(CrewdClientError(message: "连接中断，结果未知；请检查当前状态。"))) }
        if notify && hadConnection { onDisconnect?() }
    }
}
#endif
