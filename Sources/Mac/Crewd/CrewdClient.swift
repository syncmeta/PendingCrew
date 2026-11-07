import Foundation
import Network
import Security
import Combine
import CryptoKit

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
        else if let v = try? c.decode(Int64.self) {
            // Preserve typed Real -0 through generic transport and durable journals.
            // All other integral values retain their exact integer representation.
            if v==0,let real=try? c.decode(Double.self),real==0,real.sign == .minus {self = .number(real)}
            else {self = .integer(v)}
        }
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
    enum RecoveryDisposition: Equatable { case manual, transport, refused }
    private(set) var recoveryDisposition = RecoveryDisposition.manual
    static func recovery(for error: NWError) -> RecoveryDisposition {
        switch error {
        case .dns: return .transport
        case .posix(let code):
            switch code {
            case .ECONNREFUSED, .ECONNRESET, .ENETDOWN, .ENETUNREACH, .EHOSTDOWN, .EHOSTUNREACH, .ETIMEDOUT, .EPIPE, .ENOTCONN: return .transport
            default: return .refused
            }
        default: return .refused // TLS/authentication and unknown failures require review.
        }
    }
    @Published private(set) var connected = false
    /// Socket attachment is observable before hello and subscription are complete.
    @Published private(set) var readyForWork = false
    @Published private(set) var version = ""
    @Published private(set) var authorityBinding: CrewdValue = .null
    @Published private(set) var capabilities:Set<String>=[]
    /// The authoritative hello advertises the orchestra capability group,
    /// not an individual capability named after every session RPC.
    var supportsSessionStart: Bool { Self.sessionStartSupported(connected: readyForWork, capabilities: capabilities) }
    static func sessionStartSupported(connected: Bool, capabilities: Set<String>) -> Bool {
        connected && capabilities.contains("orchestra")
    }
    @Published private(set) var lastError: String?
    var onEvent: ((String, CrewdValue) -> Void)?
    var onDisconnect: (() -> Void)?
    var onManualDisconnect: (() -> Void)?
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
    var connectionEpoch: UUID { generation }
    private var qualifiedForWork = true
    private var retired = false
    private var decoder = CrewdNDJSONDecoder()
    private var receivePending = false
    private var terminalFailure: String?
    private var terminalRecovery = RecoveryDisposition.transport
    private var nextID = 1
    private var pending: [Int: CheckedContinuation<CrewdValue, Error>] = [:]
    private var timeouts: [Int: Task<Void, Never>] = [:]
    private var ready: CheckedContinuation<Void, Error>?

    #if os(macOS)
    func connect(path: String, expectedEpoch: UUID? = nil) async throws {
        try Task.checkCancellation()
        guard expectedEpoch == nil || expectedEpoch == generation else {throw CancellationError()}
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        resetConnection(); recoveryDisposition = .transport; qualifiedForWork = false
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
            do { try self.decoder.finish(); self.fail(message, notify: true, recovery: .transport) }
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
    func connect(host: String, port: UInt16, parameters: NWParameters, expectedRemote: CrewdRemoteBindingExpectation, expectedEpoch: UUID? = nil) async throws {
        try Task.checkCancellation()
        guard expectedEpoch == nil || expectedEpoch == generation else {throw CancellationError()}
        guard !host.isEmpty, host.utf8.count <= 255, port != 0,
              let networkPort = NWEndpoint.Port(rawValue: port) else {
            throw CrewdClientError(message: "电脑连接地址无效，尚未提交。", kind: .notSubmitted)
        }
        try await connect(NWConnection(host: .init(host), port: networkPort, using: parameters), clientName: "PendingCrew", expectedRemote: expectedRemote, expectedEpoch:expectedEpoch ?? generation)
    }

    private func connect(_ c: NWConnection, clientName: String, expectedRemote: CrewdRemoteBindingExpectation?, expectedEpoch:UUID) async throws {
        try Task.checkCancellation()
        guard expectedEpoch == generation else {throw CancellationError()}
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        resetConnection()
        recoveryDisposition = .transport
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
                case .failed(let error): self.transportEnded(error.localizedDescription, recovery: Self.recovery(for:error))
                case .waiting(let error):
                    if Self.recovery(for:error) == .refused {self.transportEnded(error.localizedDescription,recovery:.refused)}
                case .cancelled: self.transportEnded("后台连接已关闭。", recovery: .manual)
                default: break
                }
            }
        }
        let deadline = Task { [weak self] in
            try? await Task.sleep(for: .seconds(5))
            guard !Task.isCancelled, let self, self.generation == current, self.ready != nil else { return }
            self.fail("连接后台超时。", notify: true, recovery: .transport)
        }
        var reachedBackend = false
        do {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
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
        guard hello["server"].string == "crewd", let api = hello["apiVersion"].int, api >= 3,
              (hello["minApiVersion"].int ?? 1) <= 3 else { throw CrewdClientError(message: "后台协议版本与客户端不兼容。") }
        if let expectedRemote {
            guard expectedRemote.accepts(hello["remoteBinding"]) else { throw CrewdClientError(message: "电脑身份或设备授权已改变，请重新配对。") }
        } else {
            #if os(macOS)
            // The same live descriptor must still have the kernel peer observed
            // before hello. This qualifies local work, not child ownership/Quit.
            _ = try verifiedLocalPeerIdentity()
            #endif
            guard hello["capabilities"].array.contains(.string("session.respondRunnerRequest")) else {
                throw CrewdClientError(message: "后台尚不支持 Codex 人工答复，请使用配套新版后台。")
            }
        }
        let instance = hello["runtimeIdentity"]
        if instance != .null {
            guard let db = instance["dbId"].string, !db.isEmpty, db.utf8.count <= 256,
                  let binding = instance["runtimeBindingId"].string, UUID(uuidString: binding) != nil else {
                throw CrewdClientError(message: "后台实例信息不完整；连接尚未取得工作资格。")
            }
        }
        runtimeIdentity = instance // Correlation only; no kernel ownership grant.
        authorityBinding = hello["remoteBinding"]
        version = hello["version"].string ?? "未知"
        capabilities = Set(hello["capabilities"].array.compactMap(\.string))
        qualifiedForWork = true
        let topics = Self.eventTopics(capabilities:capabilities,includePersonalDraft:false)
        _ = try await call("events.subscribe", .object(["topics": .array(topics.map(CrewdValue.string))]))
        guard generation == current, !Task.isCancelled else { throw CancellationError() }
        readyForWork = true
        lastError = nil
    }

    func call(_ method: String, _ params: CrewdValue = .null, timeout: Duration = .seconds(40)) async throws -> CrewdValue {
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        guard !Task.isCancelled else { throw CrewdClientError(message: "请求已取消，尚未提交。", kind: .notSubmitted) }
        guard readyForWork || method == "system.hello" || (qualifiedForWork && method == "events.subscribe") else { throw CrewdClientError(message: "尚未核实电脑身份，操作未提交。", kind: .notSubmitted) }
        guard connected, terminalFailure == nil, hasTransport else { throw CrewdClientError(message: "后台尚未连接。",kind:.notSubmitted) }
        let id = nextID; nextID += 1
        var data:Data
        do {data = try JSONEncoder().encode(CrewdValue.object(["id": .number(Double(id)), "method": .string(method), "params": params]))}
        catch {throw CrewdClientError(message:"请求无法编码，尚未提交。",kind:.notSubmitted)}
        data.append(10)
        let current = generation
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<CrewdValue, Error>) in
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
                    self.transportEnded(error.localizedDescription, recovery: Self.recovery(for:error))
                }
            })
            }
            #else
            if let c = connection {
            c.send(content: data, completion: .contentProcessed { [weak self] error in
                guard let error else { return }
                DispatchQueue.main.async {
                    guard let self, self.generation == current else { return }
                    self.transportEnded(error.localizedDescription, recovery: Self.recovery(for:error))
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
    func disconnect() { resetConnection(); onManualDisconnect?() }
    private func resetConnection() { fail("连接已断开。", notify: false, recovery: .manual); lastError = nil }

    static func eventTopics(capabilities: Set<String>, includePersonalDraft: Bool) -> [String] {
        var topics = ["crew","session","message","channel","ask","work","todo","restart","system"]
        if includePersonalDraft { topics.append("personalDraft") }
        topics += ["documents","viewFolders","data"].filter { capabilities.contains($0) }
        if capabilities.contains("managedObjects.events.v1") { topics.append("onto") }
        if capabilities.contains("managedArtifacts.v1") { topics.append("artifact") }
        if capabilities.contains(CrewdAdminAskGroupsWire.capability) { topics.append(CrewdAdminAskGroupsWire.topic) }
        if capabilities.contains("managedLinks.v1") || capabilities.contains("managedTables.v3") { topics.append("links") }
        return topics
    }

    private var personalDraftEventEpoch:UUID?
    /// Only after a successful typed status read on this exact connection.
    func subscribePersonalDraftEvents()async throws{
        let epoch=connectionEpoch
        guard readyForWork,personalDraftEventEpoch != epoch else{return}
        let topics=Self.eventTopics(capabilities:capabilities,includePersonalDraft:true)
        _ = try await call("events.subscribe",.object(["topics":.array(topics.map(CrewdValue.string))]))
        guard connectionEpoch==epoch,readyForWork else{throw CancellationError()};personalDraftEventEpoch=epoch
    }
    private func transportEnded(_ message: String, recovery: RecoveryDisposition) {
        // Network can announce failure before completing an outstanding receive.
        // Let that receive drain final bytes; only received RPC frames prove success.
        if connected && receivePending {
            if terminalFailure == nil || terminalRecovery == .transport {terminalFailure = message; terminalRecovery = recovery}
        }
        else { fail(message, notify: true, recovery: recovery) }
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
                    if let error { self.fail(error.localizedDescription, notify: true, recovery: self.terminalFailure != nil && self.terminalRecovery != .transport ? self.terminalRecovery:Self.recovery(for:error)) }
                    else if complete { try self.decoder.finish(); self.fail(self.terminalFailure ?? "后台连接已结束。", notify: true, recovery: self.terminalFailure == nil ? .transport:self.terminalRecovery) }
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
                if frame["error"] != .null {
                    let error=CrewdClientError(message: frame["error"]["message"].string ?? "后台请求失败。", code: frame["error"]["code"].int, data: frame["error"]["data"])
                    finish(id, .failure(error))
                }
                else { finish(id, .success(frame["result"])) }
            } else if let event = frame["event"].string, readyForWork { onEvent?(event, frame["data"]) }
        }
    }
    private func finish(_ id: Int, _ result: Result<CrewdValue, Error>) {
        timeouts.removeValue(forKey: id)?.cancel()
        pending.removeValue(forKey: id)?.resume(with: result)
    }
    private func fail(_ message: String, notify: Bool, recovery: RecoveryDisposition = .refused) {
        let hadConnection = hasTransport
        recoveryDisposition = recovery
        generation = UUID()
        let c = connection; connection = nil
        c?.stateUpdateHandler = nil; c?.cancel()
        #if os(macOS)
        let local = localConnection; localConnection = nil
        local?.onData = nil; local?.onEnd = nil; local?.close()
        #endif
        runtimeIdentity = .null; authorityBinding = .null
        readyForWork = false
        qualifiedForWork = false
        connected = false; capabilities=[]; receivePending = false; terminalFailure = nil; decoder = CrewdNDJSONDecoder(); lastError = message
        ready?.resume(throwing: CrewdClientError(message: message)); ready = nil
        let ids = Array(pending.keys)
        for id in ids { finish(id, .failure(CrewdClientError(message: "连接中断，结果未知；请检查当前状态。"))) }
        if notify && hadConnection { onDisconnect?() }
    }
}

/// Names and selection come from authoritative readable Channel rows. Empty
/// system names are intentional protocol values, never invisible controls.
enum CrewdConversationProjection {
    static func title(_ channel: CrewdValue) -> String {
        switch channel["kind"].string {
        case "human": return "与我沟通"
        case "lobby": return "大厅"
        default:
            if let name = channel["name"].string, !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return name }
            return channel["kind"].string == "dm" ? "私聊" : "话题"
        }
    }
    static func context(_ channel:CrewdValue)->String {
        switch channel["kind"].string {
        case "human":return "人类沟通群：你、当前机长和有效的显式成员；不是仅 @ 你的消息筛选。"
        case "lobby":return "机组大厅：你和本机组的全部成员会话。"
        default:return ""
        }
    }
    static func selected(_ channels: [CrewdValue], requested: String) -> String {
        if channels.contains(where: { $0["id"].string == requested }) { return requested }
        return channels.first(where: { $0["kind"].string == "human" })?["id"].string ?? channels.first?["id"].string ?? ""
    }
}

enum CrewdAskResolutionWire {
    enum Disposition: String { case answer, withdraw }
    struct Review: Equatable {
        let ask: CrewdValue
        let revision: String?
        let inbox: CrewdAskInboxWire.Review?
        var crewID: String { ask["crewId"].string ?? "" }
        var askID: String { ask["ask"].string ?? "" }

        init(_ value: CrewdValue, crew: String, id: String) throws {
            guard case .object(let fields) = value,
                  ["ask","resolutionRevision"].allSatisfy({ fields[$0] != nil }),
                  Set(fields.keys).isSubset(of:["ask","resolutionRevision","inbox"]),
                  validAsk(value["ask"],crew:.string(crew),id:.string(id)),
                  value["resolutionRevision"] == .null || (digest(value["resolutionRevision"]) &&
                      value["ask"]["state"] == .string("open")) else { throw invalid() }
            ask = value["ask"]; revision = value["resolutionRevision"].string
            // Scheduling is an optional independent action. A missing or
            // malformed scheduling witness cannot disable the existing resolution.
            inbox = revision == nil ? nil : try? .init(value["inbox"])
        }

        func sameResolution(as other: Self) -> Bool { ask == other.ask && revision == other.revision }

        func parameters(_ disposition: Disposition, text: String) throws -> CrewdValue {
            guard let revision else { throw invalid() }
            return try canonical(.object(["crewId":.string(crewID),"ask":.string(askID),
                "expectedResolutionRevision":.string(revision),"disposition":.string(disposition.rawValue),"text":.string(text)]))
        }
    }

    static func canonical(_ params: CrewdValue) throws -> CrewdValue {
        guard case .object(let fields) = params,
              Set(fields.keys) == ["crewId","ask","expectedResolutionRevision","disposition","text"],
              let crew = params["crewId"].string, !crew.isEmpty, crew.utf8.count <= 4_096,
              canonicalID(params["ask"]), digest(params["expectedResolutionRevision"]),
              params["disposition"].string.flatMap(Disposition.init(rawValue:)) != nil,
              let text = params["text"].string, validInput(text) else { throw invalid() }
        return params
    }

    static func validInput(_ text: String) -> Bool { text.utf8.count <= 65_536 && !trim(text).isEmpty }

    static func validateResult(_ value: CrewdValue, params: CrewdValue, review: Review? = nil) throws {
        _ = try canonical(params)
        let resolved = value["resolved"]
        guard case .object(let fields) = value,
              Set(fields.keys) == ["crewId","ask","originalResolutionRevision","disposition","eventSeq","resolvedAtMs","resolved"],
              value["crewId"] == params["crewId"], value["ask"] == params["ask"],
              value["originalResolutionRevision"] == params["expectedResolutionRevision"], value["disposition"] == params["disposition"],
              let seq = value["eventSeq"].string, let number = Int64(seq), number > 0, String(number) == seq,
              let time = value["resolvedAtMs"].int, time >= 0,
              validAsk(resolved,crew:params["crewId"],id:params["ask"]),
              let at = resolved["closedAt"].string, dateMatches(at,milliseconds:time),
              let text = params["text"].string, let disposition = params["disposition"].string.flatMap(Disposition.init(rawValue:)) else { throw invalid() }
        switch disposition {
        case .answer:
            guard resolved["state"] == .string("answered"), resolved["answer"].string?.utf8.elementsEqual(trim(text).utf8) == true,
                  resolved["withdrawReason"] == .null else { throw invalid() }
        case .withdraw:
            guard resolved["state"] == .string("withdrawn"), resolved["withdrawReason"].string?.utf8.elementsEqual(trim(text).utf8) == true,
                  resolved["answer"] == .null else { throw invalid() }
        }
        if let review {
            let changed: Set<String> = ["state","answer","withdrawReason","closedAt"]
            guard let revision = review.revision, .string(revision) == params["expectedResolutionRevision"],
                  case .object(let before) = review.ask, case .object(let after) = resolved,
                  before.filter({ !changed.contains($0.key) }) == after.filter({ !changed.contains($0.key) }) else { throw invalid() }
        }
    }

    private static func validAsk(_ value: CrewdValue, crew: CrewdValue, id: CrewdValue) -> Bool {
        value["crewId"] == crew && value["ask"] == id && canonicalID(id) &&
            crew.string?.isEmpty == false && value["question"].string?.isEmpty == false &&
            value["askedAt"].string?.isEmpty == false &&
            ["open","answered","withdrawn","defaulted"].contains(value["state"].string ?? "")
    }
    fileprivate static func canonicalID(_ value: CrewdValue) -> Bool {
        guard let id = value.string, id.first == "H", let number = Int64(id.dropFirst()), number > 0 else { return false }
        return id == "H\(number)"
    }
    fileprivate static func digest(_ value: CrewdValue) -> Bool {
        guard let text = value.string, text.utf8.count == 64 else { return false }
        return text.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
    fileprivate static func trim(_ text: String) -> String {
        // Rust uses the Unicode White_Space property for str::trim.
        let start = text.unicodeScalars.drop(while:{ $0.properties.isWhitespace })
        return String(String.UnicodeScalarView(start.reversed().drop(while:{ $0.properties.isWhitespace }).reversed()))
    }
    private static func dateMatches(_ raw: String, milliseconds: Int) -> Bool {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime,.withFractionalSeconds]
        let precise = parser.date(from:raw)
        parser.formatOptions = [.withInternetDateTime]
        guard let date = precise ?? parser.date(from:raw) else { return false }
        return abs(date.timeIntervalSince1970 * 1_000 - Double(milliseconds)) < 0.5
    }
    static func invalid() -> CrewdClientError {
        .init(message:"原问题、版本或处理回执不符。请核对原请求。")
    }
}

enum CrewdStructuredAskWire {
    static let capability = "structuredAskDecisions.v1"
    static let method = "ask.decisionGet"
    static let originMethod = "managedObject.decisionOrigin"
    private static let optionIDs = Array("ABCDEFGH").map(String.init)

    struct Option: Equatable, Identifiable {
        let id: String
        let label: String
    }
    struct DefaultChoice: Equatable {
        let optionId: String
        let afterMinutes: Int
    }
    struct Capture: Equatable {
        let decisionId: String
        let eventSeq: String
        let optionId: String
        let source: Source
        let humanReason: String?
        enum Source: String {
            case human, systemDefault
            var label: String { self == .human ? "人类拍板" : "系统执行默认方案" }
        }
    }
    struct Review: Equatable {
        let crewId: String
        let ask: String
        let question: String
        let context: String
        let options: [Option]
        let defaultChoice: DefaultChoice?
        let capture: Capture?
        private let draftBytes: Data

        func sameDraft(as other: Self) -> Bool {
            crewId.utf8.elementsEqual(other.crewId.utf8) && ask == other.ask && draftBytes == other.draftBytes
        }
        func option(_ id: String) -> Option? { options.first { $0.id == id } }
        func matches(_ resolution: CrewdAskResolutionWire.Review) -> Bool {
            guard crewId.utf8.elementsEqual(resolution.crewID.utf8), ask == resolution.askID,
                  resolution.ask["default"]["text"].string == defaultChoice?.optionId,
                  resolution.ask["default"]["afterMinutes"].int == defaultChoice?.afterMinutes else { return false }
            switch (resolution.ask["state"].string,capture?.source) {
            case ("open",nil), ("withdrawn",nil): return true
            case ("answered",.human), ("defaulted",.systemDefault):
                guard let capture, let actual = resolution.ask["answer"].string else { return false }
                let expected=capture.optionId + (capture.humanReason.map { "\n" + $0 } ?? "")
                return actual.utf8.elementsEqual(expected.utf8)
            default: return false
            }
        }
        func answer(option id: String, reason: String) throws -> String {
            guard capture == nil, option(id) != nil else { throw CrewdAskResolutionWire.invalid() }
            let normalized = CrewdAskResolutionWire.trim(reason)
            if normalized.isEmpty { return id }
            guard validText(normalized,limit:500) else { throw CrewdAskResolutionWire.invalid() }
            return id + "\n" + normalized
        }

        init(_ value: CrewdValue, crew: String? = nil, ask expectedAsk: String? = nil) throws {
            guard case .object(let fields) = value, Set(fields.keys) == ["crewId","ask","draft","capture"],
                  let owner = value["crewId"].string, validID(owner),
                  crew.map({ owner.utf8.elementsEqual($0.utf8) }) ?? true,
                  let ask = value["ask"].string, CrewdAskResolutionWire.canonicalID(value["ask"]),
                  expectedAsk.map({ ask == $0 }) ?? true,
                  case .object(let draft) = value["draft"],
                  Set(draft.keys).isSubset(of:["question","context","options","about","default","reversibility"]),
                  let question = draft["question"]?.string, validText(question,limit:60), trimmedExactly(question),
                  let context = draft["context"]?.string, validText(context,limit:900),
                  case .array(let options) = draft["options"], (2...8).contains(options.count),
                  case .array(let about) = draft["about"], (1...8).contains(about.count)
            else { throw CrewdAskResolutionWire.invalid() }
            if let reversible = draft["reversibility"], reversible != .null,
               !["reversible","hardToReverse","irreversible"].contains(reversible.string ?? "") {
                throw CrewdAskResolutionWire.invalid()
            }
            var references=Set<Data>()
            for item in about {
                guard case .object(let keys) = item, Set(keys.keys) == ["kind","id"],
                      let kind=item["kind"].string, let id=item["id"].string,
                      validAbout(kind,id:id), references.insert(Data((kind+":"+id).utf8)).inserted
                else { throw CrewdAskResolutionWire.invalid() }
            }
            let choices = try options.enumerated().map { index,option -> Option in
                guard case .object(let keys) = option, Set(keys.keys) == ["label"],
                      let label=option["label"].string, validText(label,limit:120), trimmedExactly(label),
                      !label.unicodeScalars.contains(where:{ $0.properties.generalCategory == .control ||
                          $0.value == 0x2028 || $0.value == 0x2029 })
                else { throw CrewdAskResolutionWire.invalid() }
                return .init(id:optionIDs[index],label:label)
            }
            guard Set(choices.map { Data($0.label.utf8) }).count == choices.count else { throw CrewdAskResolutionWire.invalid() }
            var fallback: DefaultChoice?
            if let supplied = draft["default"], supplied != .null {
                guard case .object(let keys) = supplied, Set(keys.keys) == ["optionId","afterMinutes"],
                      let id = supplied["optionId"].string, choices.contains(where:{ $0.id == id }),
                      let minutes = supplied["afterMinutes"].int, (1...10_080).contains(minutes)
                else { throw CrewdAskResolutionWire.invalid() }
                fallback = .init(optionId:id,afterMinutes:minutes)
            }
            var captured: Capture?
            if value["capture"] != .null {
                let raw=value["capture"]
                guard case .object(let keys) = raw,
                      Set(keys.keys).isSubset(of:["decisionId","eventSeq","optionId","source","humanReason"]),
                      let decision=raw["decisionId"].string, validNumber(decision,prefix:"D"),
                      let decisionNumber=Int64(decision.dropFirst()), let askNumber=Int64(ask.dropFirst()),
                      decisionNumber > askNumber,
                      let sequence=raw["eventSeq"].string, positive(sequence),
                      let option=raw["optionId"].string, choices.contains(where:{ $0.id == option }),
                      let source=raw["source"].string.flatMap(Capture.Source.init(rawValue:)),
                      raw["humanReason"] == .null || raw["humanReason"].string.map({ validText($0,limit:500) }) == true
                else { throw CrewdAskResolutionWire.invalid() }
                let reason=raw["humanReason"].string
                guard source != .systemDefault || (reason == nil && fallback?.optionId == option)
                else { throw CrewdAskResolutionWire.invalid() }
                captured = .init(decisionId:decision,eventSeq:sequence,optionId:option,source:source,humanReason:reason)
            }
            let encoder=JSONEncoder()
            encoder.outputFormatting=[.sortedKeys]
            self.draftBytes=try encoder.encode(value["draft"])
            self.crewId=owner; self.ask=ask; self.question=question; self.context=context
            self.options=choices; self.defaultChoice=fallback; self.capture=captured
        }
    }

    private static func validAbout(_ kind:String,id:String) -> Bool {
        switch kind {
        case "crew": validID(id)
        case "work": validNumber(id,prefix:"W")
        case "requirement": validNumber(id,prefix:"#")
        case "row": validNumber(id,prefix:"O")
        case "risk": validNumber(id,prefix:"R")
        default: false
        }
    }
    private static func validID(_ id:String) -> Bool {
        !id.isEmpty && id.utf8.count <= 128 && trimmedExactly(id) &&
            !id.unicodeScalars.contains { $0.properties.generalCategory == .control }
    }
    private static func trimmedExactly(_ value:String) -> Bool {
        CrewdAskResolutionWire.trim(value).utf8.elementsEqual(value.utf8)
    }
    private static func positive(_ value:String) -> Bool {
        guard let number=Int64(value), number > 0 else { return false }
        return String(number)==value
    }
    private static func validNumber(_ value:String,prefix:Character) -> Bool {
        value.first==prefix && positive(String(value.dropFirst()))
    }
    private static func validText(_ text:String,limit:Int) -> Bool {
        !CrewdAskResolutionWire.trim(text).isEmpty && text.unicodeScalars.count <= limit &&
            !text.unicodeScalars.contains { $0.value == 0 }
    }
}

enum CrewdAskInboxWire {
    struct Review: Equatable {
        let revision: String
        let maxUntilMs: Int
        let deferredUntilMs: Int?

        init(_ value: CrewdValue) throws {
            guard case .object(let fields) = value,
                  Set(fields.keys).isSubset(of:["revision","maxUntilMs","deferredUntilMs"]),
                  CrewdAskResolutionWire.digest(value["revision"]),
                  let revision = value["revision"].string,
                  let maxUntilMs = milliseconds(value["maxUntilMs"]),
                  value["deferredUntilMs"] == .null || milliseconds(value["deferredUntilMs"]) != nil else { throw invalid() }
            self.revision = revision
            self.maxUntilMs = maxUntilMs
            deferredUntilMs = milliseconds(value["deferredUntilMs"])
        }

        func sameState(as other: Self) -> Bool {
            revision == other.revision && deferredUntilMs == other.deferredUntilMs
        }
    }

    static func parameters(_ review: CrewdAskResolutionWire.Review, untilMs: Int?, nowMs: Int) throws -> CrewdValue {
        guard let inbox = review.inbox, let revision = review.revision else { throw invalid() }
        if let untilMs {
            guard untilMs > nowMs, untilMs <= inbox.maxUntilMs, untilMs != inbox.deferredUntilMs else { throw invalid() }
        } else {
            guard inbox.deferredUntilMs != nil else { throw invalid() }
        }
        return try canonical(.object(["crewId":.string(review.crewID),"ask":.string(review.askID),
            "expectedResolutionRevision":.string(revision),"expectedInboxRevision":.string(inbox.revision),
            "untilMs":untilMs.map { .integer(Int64($0)) } ?? .null]))
    }

    static func canonical(_ params: CrewdValue) throws -> CrewdValue {
        guard case .object(let fields) = params,
              Set(fields.keys) == ["crewId","ask","expectedResolutionRevision","expectedInboxRevision","untilMs"],
              let crew = params["crewId"].string, !crew.isEmpty, crew.utf8.count <= 4_096,
              CrewdAskResolutionWire.canonicalID(params["ask"]),
              CrewdAskResolutionWire.digest(params["expectedResolutionRevision"]),
              CrewdAskResolutionWire.digest(params["expectedInboxRevision"]),
              params["untilMs"] == .null || milliseconds(params["untilMs"]) != nil else { throw invalid() }
        return params
    }

    static func validateResult(_ value: CrewdValue, params: CrewdValue) throws {
        _ = try canonical(params)
        let required: Set<String> = ["crewId","ask","originalInboxRevision","inboxRevision","acceptedAtMs"]
        guard case .object(let fields) = value, required.isSubset(of:fields.keys),
              Set(fields.keys).isSubset(of:required.union(["deferredUntilMs"])),
              let crew = params["crewId"].string, value["crewId"].string?.utf8.elementsEqual(crew.utf8) == true,
              value["ask"] == params["ask"], value["originalInboxRevision"] == params["expectedInboxRevision"],
              CrewdAskResolutionWire.digest(value["inboxRevision"]), value["inboxRevision"] != value["originalInboxRevision"],
              let acceptedAtMs = milliseconds(value["acceptedAtMs"]) else { throw invalid() }
        if let untilMs = milliseconds(params["untilMs"]) {
            guard untilMs > acceptedAtMs, milliseconds(value["deferredUntilMs"]) == untilMs else { throw invalid() }
        } else {
            guard fields["deferredUntilMs"] == nil else { throw invalid() }
        }
    }

    static func milliseconds(_ value: CrewdValue) -> Int? {
        guard case .integer(let time) = value, time > 0 else { return nil }
        return Int(exactly:time)
    }

    static func nowMs(_ date: Date = Date()) -> Int { Int(date.timeIntervalSince1970 * 1_000) }
    static func date(_ milliseconds: Int) -> Date { Date(timeIntervalSince1970:Double(milliseconds) / 1_000) }
    static func invalid() -> CrewdClientError { .init(message:"原待人事项、稍后版本或回执不符。请核对原请求。") }
}

enum CrewdRequirementCreateWire {
    static let lookupMethod = "todo.createLookup"

    struct Original {
        let request: UUID
        let crew: String
        let channel: String
        let text: String

        init(_ params: CrewdValue) throws {
            guard case .object(let fields) = params,
                  Set(fields.keys).isSubset(of:["requestId","crewId","channelId","text"]),
                  let request = params["requestId"].string.flatMap(UUID.init(uuidString:)),
                  let crew = params["crewId"].string, !crew.isEmpty,
                  params["channelId"] == .null || params["channelId"].string?.isEmpty == false,
                  let text = params["text"].string, !CrewdAskResolutionWire.trim(text).isEmpty else { throw invalid() }
            self.request = request; self.crew = crew; channel = params["channelId"].string ?? ""; self.text = text
        }

        func matches(_ other: Original) -> Bool {
            request == other.request && crew.utf8.elementsEqual(other.crew.utf8) &&
                channel.utf8.elementsEqual(other.channel.utf8) && text.utf8.elementsEqual(other.text.utf8)
        }

        var requestID: String { request.uuidString.lowercased() }
        var textSHA: String { SHA256.hash(data:Data(text.utf8)).map { String(format:"%02x",$0) }.joined() }
        var lookup: CrewdValue {
            var fields: [String:CrewdValue] = ["requestId":.string(requestID),"crewId":.string(crew),"textSha256":.string(textSHA)]
            if !channel.isEmpty { fields["channelId"] = .string(channel) }
            return .object(fields)
        }

        func validateSubmission(_ value: CrewdValue) throws {
            guard value["id"] == .string("requirement-request-" + requestID),
                  value["crewId"].string?.utf8.elementsEqual(crew.utf8) == true, value["ledger"] == .string("agent"),
                  value["text"].string?.utf8.elementsEqual(text.utf8) == true,
                  let id = value["requirementId"].string, isRequirementID(id),
                  let created = value["createdAt"].string, !created.isEmpty,
                  let status = value["status"].string, !status.isEmpty else { throw invalid() }
            if status == "closed" {
                guard value["closure"]["note"].string?.isEmpty == false,
                      value["closure"]["at"].string?.isEmpty == false else { throw invalid() }
            }
            if channel.isEmpty {
                guard value["channelAssociation"] == .null else { throw invalid() }
            } else {
                _ = try Association(value["channelAssociation"],channel:channel)
            }
        }
    }

    struct Association: Equatable {
        let linked: Bool
        init(_ value: CrewdValue, channel: String) throws {
            guard case .object(let fields) = value, Set(fields.keys) == ["channelId","linkRevision","linked"],
                  value["channelId"].string?.utf8.elementsEqual(channel.utf8) == true, let raw = value["linkRevision"].string,
                  let revision = Int64(raw), revision >= 0, String(revision) == raw,
                  case .bool(let linked) = value["linked"] else { throw invalid() }
            self.linked = linked
        }
    }

    struct Receipt: Equatable {
        let requestID: UUID
        let requirementID: String
        let currentAssociation: Association?
        fileprivate init(requestID: UUID, requirementID: String, currentAssociation: Association?) {
            self.requestID = requestID; self.requirementID = requirementID; self.currentAssociation = currentAssociation
        }
    }

    static func receipt(_ value: CrewdValue, original: Original) throws -> Receipt? {
        guard case .object(let fields) = value, value["requestId"] == .string(original.requestID) else { throw invalid() }
        if value["status"] == .string("notFound") {
            guard Set(fields.keys) == ["requestId","status"] else { throw invalid() }
            return nil
        }
        let receipt = value["receipt"]
        guard value["status"] == .string("committed"), Set(fields.keys) == ["requestId","status","receipt"],
              case .object(let body) = receipt, receipt["crewId"].string?.utf8.elementsEqual(original.crew.utf8) == true,
              receipt["textSha256"] == .string(original.textSHA),
              let id = receipt["requirementId"].string, isRequirementID(id),
              let created = receipt["createdAt"].string, !created.isEmpty else { throw invalid() }
        let common: Set<String> = ["crewId","requirementId","textSha256","createdAt"]
        if original.channel.isEmpty {
            guard Set(body.keys) == common else { throw invalid() }
            return .init(requestID:original.request,requirementID:id,currentAssociation:nil)
        }
        guard Set(body.keys) == common.union(["sourceChannelId","currentAssociation"]),
              receipt["sourceChannelId"].string?.utf8.elementsEqual(original.channel.utf8) == true else { throw invalid() }
        return try .init(requestID:original.request,requirementID:id,currentAssociation:Association(receipt["currentAssociation"],channel:original.channel))
    }

    private static func isRequirementID(_ id: String) -> Bool {
        guard id.first == "#", let number = Int64(id.dropFirst()), number > 0 else { return false }
        return id == "#\(number)"
    }

    static func invalid() -> CrewdClientError {
        .init(message:"原需求的请求编号、机组、群或原文不符。请保留原记录核对。")
    }
}

enum CrewdRequirementRulingWire {
    static let methods = ["todo.rulingCheck", "todo.ruleClose", "todo.rulingResult"]
    enum State: String { case ready, stale, notNeedsRuling = "not_needs_ruling", incomplete = "review_incomplete" }
    enum DropKind: String { case duplicate, superseded, notNeeded = "not_needed" }
    enum Actor: String { case human, captain }

    struct Review: Equatable {
        struct Work: Equatable, Identifiable {
            let id: String
            let crew: String
            let title: String
            let revision: String
            let drop: DropKind
            let dropNote: String
            let dropNoteTruncated: Bool
        }
        let crew: String
        let id: String
        let state: State
        let revision: String?
        let closeRevision: String?
        let linkedWorkCount: Int
        let work: [Work]
        let nextAfter: String?
        var complete: Bool { state == .ready && closeRevision != nil && nextAfter == nil && work.count == linkedWorkCount }

        init(_ value: CrewdValue, crew: String, id: String, after: String? = nil, expectedRevision: String? = nil) throws {
            guard case .object(let fields) = value,
                  Set(fields.keys).isSubset(of:["crewId","id","state","revision","closeRevision","linkedWorkCount","work","nextAfter"]),
                  identity(value,crew:crew,id:id), let rawState = value["state"].string,
                  let state = State(rawValue:rawState), let count = value["linkedWorkCount"].int, count >= 0,
                  case .array(let rows) = value["work"] else { throw invalid() }
            let revision = fields["revision"]?.string
            let next = fields["nextAfter"]?.string
            let closeRevision = fields["closeRevision"]?.string
            guard state != .stale || after != nil else { throw invalid() }
            if state == .ready {
                guard let revision, digest(revision), expectedRevision.map({ $0 == revision }) ?? true,
                      (1...32).contains(rows.count), rows.count <= count,
                      fields["nextAfter"] == nil || next != nil else { throw invalid() }
                if next == nil {
                    guard let closeRevision, digest(closeRevision), closeRevision != revision else { throw invalid() }
                } else if fields["closeRevision"] != nil { throw invalid() }
            } else {
                guard fields["revision"] == nil, fields["closeRevision"] == nil,
                      fields["nextAfter"] == nil, rows.isEmpty else { throw invalid() }
            }
            let work = try rows.map { row -> Work in
                guard case .object(let fields) = row,
                      Set(fields.keys) == ["id","crewId","title","revision","dropKind","dropNote","dropNoteTruncated"],
                      let id = row["id"].string, publicID(id,prefix:"W"), let title = row["title"].string,
                      title.unicodeScalars.count <= 200,
                      let crew = row["crewId"].string, token(crew),
                      let revision = row["revision"].string, positive(revision),
                      let rawDrop = row["dropKind"].string, let drop = DropKind(rawValue:rawDrop),
                      let note = row["dropNote"].string, note.unicodeScalars.count <= 500,
                      case .bool(let truncated) = row["dropNoteTruncated"] else { throw invalid() }
                return .init(id:id,crew:crew,title:title,revision:revision,drop:drop,dropNote:note,dropNoteTruncated:truncated)
            }
            let ids = Set(work.map(\.id))
            guard ids.count == work.count, next == nil || next == work.last?.id else { throw invalid() }
            if let after, state == .ready {
                guard publicID(after,prefix:"W"), !ids.contains(after) else { throw invalid() }
            } else if state == .ready {
                guard next == nil ? work.count == count : work.count < count else { throw invalid() }
            }
            self.crew = crew; self.id = id; self.state = state; self.revision = revision; self.closeRevision = closeRevision
            linkedWorkCount = count; self.work = work; nextAfter = next
        }

        private init(_ previous: Self, page: Self) {
            crew = previous.crew; id = previous.id; state = previous.state; revision = previous.revision; closeRevision = page.closeRevision
            linkedWorkCount = previous.linkedWorkCount; work = previous.work + page.work; nextAfter = page.nextAfter
        }

        func appending(_ page: Self) throws -> Self {
            guard state == .ready, page.state == .ready, crew == page.crew, id == page.id,
                  revision == page.revision, linkedWorkCount == page.linkedWorkCount, nextAfter != nil,
                  Set(work.map(\.id)).isDisjoint(with:page.work.map(\.id)) else { throw invalid() }
            let combined = Self(self,page:page)
            guard combined.nextAfter == nil ? combined.complete : combined.work.count < linkedWorkCount else { throw invalid() }
            return combined
        }

        func parameters(note: String) throws -> CrewdValue {
            guard complete, let closeRevision else { throw invalid() }
            return try canonical(.object(["crewId":.string(crew),"id":.string(id),
                "expectedRevision":.string(closeRevision),"note":.string(note)]))
        }
    }

    struct Receipt: Equatable {
        let id: String
        let requestID: UUID
        let revision: String
        let note: String
        let closedAtMs: Int
        let eventSequence: String
        let actor: Actor
        let captain: String?

        init(_ value: CrewdValue, crew: String, id: String) throws {
            guard case .object(let fields) = value,
                  Set(fields.keys).isSubset(of:["crewId","id","requestId","expectedRevision","note","closedAtMs","eventSeq","by","captain"]),
                  identity(value,crew:crew,id:id), let request = value["requestId"].string,
                  let uuid = UUID(uuidString:request), request == uuid.uuidString.lowercased(),
                  let revision = value["expectedRevision"].string, digest(revision),
                  let note = value["note"].string, validNote(note), trimmed(note).utf8.elementsEqual(note.utf8),
                  let time = value["closedAtMs"].int, time >= 0,
                  let seq = value["eventSeq"].string, positive(seq),
                  let rawActor = value["by"].string, let actor = Actor(rawValue:rawActor) else { throw invalid() }
            let captain = fields["captain"]?.string
            switch actor {
            case .human: guard fields["captain"] == nil else { throw invalid() }
            case .captain: guard let captain, token(captain) else { throw invalid() }
            }
            self.id = id; requestID = uuid; self.revision = revision; self.note = note
            closedAtMs = time; eventSequence = seq; self.actor = actor; self.captain = captain
        }
    }

    static func result(_ value: CrewdValue, crew: String, id: String) throws -> Receipt? {
        guard case .object(let fields) = value, Set(fields.keys).isSubset(of:["crewId","id","receipt"]),
              identity(value,crew:crew,id:id) else { throw invalid() }
        return try fields["receipt"].map { try Receipt($0,crew:crew,id:id) }
    }

    static func canonical(_ params: CrewdValue) throws -> CrewdValue {
        guard case .object(let fields) = params, Set(fields.keys) == ["crewId","id","expectedRevision","note"],
              let crew = params["crewId"].string, token(crew), let id = params["id"].string, publicID(id,prefix:"#"),
              let revision = params["expectedRevision"].string, digest(revision),
              let note = params["note"].string, validNote(note) else { throw invalid() }
        return params
    }

    static func validateResult(_ value: CrewdValue, params: CrewdValue, request: UUID) throws {
        _ = try canonical(params)
        let receipt = try Receipt(value,crew:params["crewId"].string ?? "",id:params["id"].string ?? "")
        guard receipt.actor == .human, receipt.requestID == request,
              receipt.revision == params["expectedRevision"].string,
              let note = params["note"].string, receipt.note.utf8.elementsEqual(trimmed(note).utf8) else { throw invalid() }
    }

    static func validNote(_ text: String) -> Bool { text.utf8.count <= 4_096 && !trimmed(text).isEmpty }
    static func requiresFreshReview(_ error: Error) -> Bool {
        guard let failure = error as? CrewdClientError else { return false }
        return failure.code == -32009 && failure.refusal == "todo.ruling_review_required"
    }
    static func invalid() -> CrewdClientError { .init(message:"需求、关联计划或原裁决回执与本次读取不符。") }
    private static func trimmed(_ text: String) -> String { CrewdAskResolutionWire.trim(text) }
    private static func token(_ raw: String) -> Bool { !raw.isEmpty && raw.utf8.count <= 4_096 }
    private static func positive(_ raw: String) -> Bool {
        guard let number = UInt64(raw), number > 0 else { return false }
        return String(number) == raw
    }
    private static func publicID(_ raw: String, prefix: Character) -> Bool {
        guard raw.first == prefix, let number = Int64(raw.dropFirst()), number > 0 else { return false }
        return raw == "\(prefix)\(number)"
    }
    private static func digest(_ raw: String) -> Bool {
        raw.utf8.count == 64 && raw.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
    private static func identity(_ value: CrewdValue, crew: String, id: String) -> Bool {
        token(crew) && publicID(id,prefix:"#") && value["crewId"] == .string(crew) && value["id"] == .string(id)
    }
}

enum CrewdWorkDuplicateWire {
    struct Candidate: Equatable, Identifiable {
        let duplicate: String
        let duplicateRevision: String
        let survivor: String
        let survivorRevision: String
        let sources: [String]
        let witness: String
        var id: String { duplicate }

        init(_ value: CrewdValue) throws {
            guard let duplicate = value["duplicateId"].string, let survivor = value["survivorId"].string,
                  pair(duplicate,survivor), let duplicateRevision = value["duplicateRevision"].string,
                  duplicateRevision == "1", let survivorRevision = value["survivorRevision"].string,
                  revision(survivorRevision) != nil, let witness = value["witness"].string, digest(witness),
                  case .array(let rawSources) = value["sourceIds"], (1...8).contains(rawSources.count) else { throw invalid() }
            let sources = rawSources.compactMap(\.string)
            let numbers = sources.compactMap { number($0,prefix:"#") }
            guard numbers.count == rawSources.count, numbers.sorted() == numbers,
                  Set(numbers).count == numbers.count else { throw invalid() }
            self.duplicate = duplicate; self.duplicateRevision = duplicateRevision
            self.survivor = survivor; self.survivorRevision = survivorRevision
            self.sources = sources; self.witness = witness
        }

        func parameters(crew: String, evidenceKind: String, evidence: String, note: String) -> CrewdValue {
            .object(["crewId":.string(crew),"duplicateId":.string(duplicate),"expectedDuplicateRevision":.string(duplicateRevision),
                "survivorId":.string(survivor),"expectedSurvivorRevision":.string(survivorRevision),"witness":.string(witness),
                "evidence":.object(["kind":.string(evidenceKind),"value":.string(evidence)]),"note":.string(note)])
        }
    }

    struct Page {
        let items: [Candidate]
        let next: String?

        init(_ value: CrewdValue, crew: String, after: String?, limit: Int) throws {
            guard value["crewId"] == .string(crew), case .array(let rows) = value["items"],
                  rows.count <= limit, value["nextAfter"] == .null || value["nextAfter"].string != nil else { throw invalid() }
            let previous = try after.map { try cursor($0) }
            items = try rows.map(Candidate.init)
            let ids = items.compactMap { number($0.duplicate,prefix:"W") }
            guard ids.count == items.count, ids.sorted() == ids, Set(ids).count == ids.count,
                  previous.map({ marker in ids.allSatisfy { $0 > marker.number } }) ?? true else { throw invalid() }
            next = value["nextAfter"].string
            if let next {
                let marker = try cursor(next)
                guard items.count == limit, marker.number == ids.last,
                      previous.map({ $0.scope == marker.scope }) ?? true else { throw invalid() }
            }
        }
    }

    static func canonical(_ params: CrewdValue) throws -> CrewdValue {
        guard case .object(let fields) = params,
              Set(fields.keys) == ["crewId","duplicateId","expectedDuplicateRevision","survivorId","expectedSurvivorRevision","witness","evidence","note"],
              let duplicate = params["duplicateId"].string, let survivor = params["survivorId"].string, pair(duplicate,survivor),
              params["expectedDuplicateRevision"] == .string("1"),
              let survivorRevision = params["expectedSurvivorRevision"].string, revision(survivorRevision) != nil,
              let witness = params["witness"].string, digest(witness),
              evidence(params["evidence"]), text(params["note"]) else { throw invalid() }
        guard let crew = params["crewId"].string, !crew.isEmpty, crew.utf8.count <= 4_096 else { throw invalid() }
        return params
    }

    static func validateResult(_ value: CrewdValue, params: CrewdValue) throws {
        _ = try canonical(params)
        try validateClosure(value,crew:params["crewId"],id:params["duplicateId"])
        guard value["previousDuplicateRevision"] == params["expectedDuplicateRevision"],
              value["survivorId"] == params["survivorId"], value["survivorRevision"] == params["expectedSurvivorRevision"],
              value["evidence"] == params["evidence"], value["note"] == params["note"] else { throw invalid() }
    }

    static func validateHistory(_ event: CrewdValue, crew: String, id: String) throws {
        let value = event["duplicateReview"]
        guard value != .null else { return }
        try validateClosure(value,crew:.string(crew),id:.string(id))
        guard value["eventSeq"] == event["seq"], event["actor"]["kind"] == .string("human"),
              event["closure"]["kind"] == .string("duplicate"),
              value["note"] == event["closure"]["note"], value["evidence"] == event["closure"]["evidence"] else {
            throw invalid()
        }
    }

    private static func validateClosure(_ value: CrewdValue, crew: CrewdValue, id: CrewdValue) throws {
        guard case .object(let fields) = value,
              Set(fields.keys) == ["crewId","duplicateId","previousDuplicateRevision","duplicateRevision","survivorId","survivorRevision","eventSeq","closedAtMs","evidence","note"],
              value["crewId"] == crew, value["duplicateId"] == id,
              let duplicate = id.string, let survivor = value["survivorId"].string, pair(duplicate,survivor),
              value["previousDuplicateRevision"] == .string("1"), value["duplicateRevision"] == .string("2"),
              value["survivorRevision"].string.flatMap({ revision($0) }) != nil,
              value["eventSeq"].string.flatMap({ revision($0) }) != nil, (value["closedAtMs"].int ?? -1) >= 0,
              evidence(value["evidence"]), text(value["note"]) else { throw invalid() }
    }

    private static func evidence(_ value: CrewdValue) -> Bool {
        guard case .object(let fields) = value, Set(fields.keys) == ["kind","value"],
              ["commit","url","text"].contains(value["kind"].string ?? "") else { return false }
        return text(value["value"])
    }
    static func text(_ value: CrewdValue) -> Bool {
        guard let text = value.string else { return false }
        return !text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty && text.utf8.count <= 4_096
    }
    static func invalid() -> CrewdClientError {
        .init(message:"重复工作复核的身份、版本或凭据不完整；请读取当前记录核对。")
    }
    private static func revision(_ raw: String) -> UInt64? {
        guard let number = UInt64(raw), number > 0, String(number) == raw else { return nil }
        return number
    }
    private static func number(_ raw: String, prefix: Character) -> Int64? {
        guard raw.first == prefix, let number = Int64(raw.dropFirst()), number > 0, raw == "\(prefix)\(number)" else { return nil }
        return number
    }
    private static func pair(_ duplicate: String, _ survivor: String) -> Bool {
        guard let duplicate = number(duplicate,prefix:"W"), let survivor = number(survivor,prefix:"W") else { return false }
        return survivor < duplicate
    }
    private static func digest(_ raw: String) -> Bool {
        raw.utf8.count == 64 && raw.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
    private static func cursor(_ raw: String) throws -> (scope: String, number: Int64) {
        let parts = raw.split(separator:":",omittingEmptySubsequences:false)
        guard parts.count == 2, digest(String(parts[0])), let number = Int64(parts[1]), number > 0,
              String(number) == parts[1] else { throw invalid() }
        return (String(parts[0]),number)
    }
}
