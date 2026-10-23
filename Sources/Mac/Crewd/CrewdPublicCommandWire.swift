import Foundation

/// Authoritative public14 DTOs only. Parsing correlation never grants Human
/// admission, proves owned-process identity, or enables a capability on13.
struct CrewdCommandTarget: Equatable {
    let sessionID: String
    let generation: String
    let controlRevision: String
    let turnID: String?
    init(_ value: CrewdValue) throws {
        guard case .object(let fields) = value,
              Set(fields.keys).isSubset(of: ["sessionId", "generation", "controlRevision", "turnId"]) else { throw CrewdPublicWire.invalid() }
        sessionID = try CrewdPublicWire.token(value["sessionId"])
        generation = try CrewdPublicWire.token(value["generation"])
        controlRevision = try CrewdPublicWire.token(value["controlRevision"])
        guard value["turnId"] == .null || value["turnId"].string != nil else { throw CrewdPublicWire.invalid() }
        turnID = value["turnId"].string
        if let turnID, turnID.isEmpty || turnID.utf8.count > 4096 { throw CrewdPublicWire.invalid() }
    }
    var value: CrewdValue {
        .object(["sessionId": .string(sessionID), "generation": .string(generation),
                 "controlRevision": .string(controlRevision), "turnId": turnID.map(CrewdValue.string) ?? .null])
    }
}

enum CrewdCommandPhase: String, Codable {
    case reserved, committed, admitted, dispatched, launched, written, resized
    case confirmed, unknown, suppressed, rejected
    /// Mechanism-specific result verification is still required. A phase alone
    /// never proves model delivery, application consumption or physical exit.
}
struct CrewdPublicReceipt {
    let value: CrewdValue
    let requestID: UUID
    let databaseID: String
    let runtimeBindingID: String
    let phase: CrewdCommandPhase
    let historical: Bool
    init(_ value: CrewdValue, request: UUID, database: String, originalRuntimeBinding: String, originalCommand: CrewdValue) throws {
        guard let id = value["requestId"].string.flatMap(UUID.init(uuidString:)), id == request,
              value["dbId"].string == database, value["runtimeBindingId"].string == originalRuntimeBinding,
              value["command"] == originalCommand,
              let phase = value["phase"].string.flatMap(CrewdCommandPhase.init(rawValue:)),
              case .bool(let historical) = value["historical"],
              let hash = value["payloadSha256"].string, hash.count == 64,
              hash.allSatisfy({ $0.isASCII && ($0.isNumber || ("a"..."f").contains(String($0))) }) else {
            throw CrewdPublicWire.invalid()
        }
        self.value = value; requestID = id; databaseID = database
        runtimeBindingID = try CrewdPublicWire.token(value["runtimeBindingId"])
        self.phase = phase; self.historical = historical
        _ = try CrewdPublicWire.token(value["createdAt"])
        _ = try CrewdPublicWire.token(value["updatedAt"])
        if originalCommand["operation"].string?.hasPrefix("personalDraft.")==true,phase == .committed{try CrewPersonalCoreWire.validateResult(value["result"],command:originalCommand)}
        if originalCommand["operation"].string?.hasPrefix("session.codex.") == true,phase == .confirmed {
            try CrewdCodexWire.validateResult(value["result"],command:originalCommand)
        }
    }
    // Historical/unknown/reserved/dispatch are preserved for read-only recovery;
    // none of this DTO offers an automatic submit/replay method.
}
struct CrewdTerminalSnapshot {
    enum OutputKind: String { case pty, runner, unavailable }
    let target: CrewdCommandTarget
    let kind: OutputKind
    let oldestCursor: String
    let nextCursor: String
    let bytes: Data?
    let text: String?
    let truncated: Bool
    let exited: Bool
    let manualAvailable: Bool
    let columns: UInt16?
    let rows: UInt16?
    init(_ value: CrewdValue) throws {
        target = try CrewdCommandTarget(value["target"])
        guard let kind = value["outputKind"].string.flatMap(OutputKind.init(rawValue:)) else { throw CrewdPublicWire.invalid() }
        self.kind = kind
        oldestCursor = try CrewdPublicWire.token(value["oldestCursor"])
        nextCursor = try CrewdPublicWire.token(value["nextCursor"])
        truncated = try CrewdPublicWire.boolean(value["truncated"])
        exited = try CrewdPublicWire.boolean(value["exited"])
        manualAvailable = try CrewdPublicWire.boolean(value["manualControlAvailable"])
        columns = try CrewdPublicWire.dimension(value["cols"])
        rows = try CrewdPublicWire.dimension(value["rows"])
        if value["screenBytesBase64"] == .null { bytes = nil }
        else { bytes = try CrewdPublicWire.bytes(value["screenBytesBase64"], maximum: 512 * 1024) }
        guard value["screen"] == .null || value["screen"].string != nil else { throw CrewdPublicWire.invalid() }
        text = value["screen"].string
        guard (text?.utf8.count ?? 0) <= 512 * 1024,
              !manualAvailable || kind == .pty else { throw CrewdPublicWire.invalid() }
    }
}
struct CrewdTerminalOutput {
    let watchID: String
    let target: CrewdCommandTarget
    let cursor: String
    let nextCursor: String
    let bytes: Data
    let truncated: Bool
    let exited: Bool
    let closed: Bool
    init(_ event: CrewdValue) throws {
        watchID = try CrewdPublicWire.token(event["watchId"])
        let output = event["output"]
        target = try CrewdCommandTarget(output["target"])
        cursor = try CrewdPublicWire.token(output["cursor"])
        nextCursor = try CrewdPublicWire.token(output["nextCursor"])
        bytes = try CrewdPublicWire.bytes(output["bytesBase64"], maximum: 64 * 1024)
        truncated = try CrewdPublicWire.boolean(output["truncated"])
        exited = try CrewdPublicWire.boolean(output["exited"])
        closed = try CrewdPublicWire.boolean(event["closed"])
    }
}
enum CrewdPublicWire {
    static func invalid() -> CrewdClientError { .init(message: "公开命令或终端响应契约不完整；保留原提交并核对后台状态。") }
    static func token(_ value: CrewdValue) throws -> String {
        guard let text = value.string, !text.isEmpty, text.utf8.count <= 4096 else { throw invalid() }
        return text
    }
    static func boolean(_ value: CrewdValue) throws -> Bool {
        guard case .bool(let value) = value else { throw invalid() }; return value
    }
    static func dimension(_ value: CrewdValue) throws -> UInt16? {
        if value == .null { return nil }
        guard let number = value.int, let result = UInt16(exactly: number), result > 0 else { throw invalid() }
        return result
    }
    static func bytes(_ value: CrewdValue, maximum: Int) throws -> Data {
        guard let encoded = value.string, encoded.utf8.count <= 4 * ((maximum + 2) / 3),
              let decoded = Data(base64Encoded: encoded), decoded.count <= maximum else { throw invalid() }
        return decoded
    }
}

/// Mirrors the server's closed operation enum. It cannot proxy arbitrary RPCs.
enum CrewdPublicOperation: String, Codable {
    case personalDraftOwner="personalDraft.ownerConfigure",personalDraftMember="personalDraft.memberSet",personalDraftConsent="personalDraft.consentSet",personalDraftCheckpoint="personalDraft.checkpoint",personalDraftResolve="personalDraft.resolve"
    var isPersonalDraft:Bool{rawValue.hasPrefix("personalDraft.")}
    case localClientTrust = "local.clientTrust"
    case localClientTrustRevoke = "local.clientTrustRevoke"
    case hibernate = "crewd.hibernate"
    case crewCreate = "crew.create"
    case firstMessage = "crew.createFirstMessage"
    case messagePost = "message.post"
    case sessionStart = "session.start"
    case sessionStop = "session.stop"
    case sessionResume = "session.resume"
    case sessionInterrupt = "session.interrupt"
    case codexConfigure = "session.codex.configure"
    case codexCompact = "session.codex.compact"
    case codexSteer = "session.codex.steer"
    case codexQueue = "session.codex.queue"
    case codexReview = "session.codex.review"
    case codexTitle = "session.codex.titleSet"
    case codexFork = "session.codex.fork"
    case codexArchive="session.codex.archive",codexUnarchive="session.codex.unarchive",codexDelete="session.codex.delete",codexRevert="session.codex.revert"
    var isCodex:Bool{rawValue.hasPrefix("session.codex.")}
    case sessionAnswer = "session.answer"
    case sessionRespond = "session.respondRunnerRequest"
    case terminalClaim = "session.terminal.claim"
    case terminalRelease = "session.terminal.release"
    case terminalWrite = "session.terminal.write"
    case terminalResize = "session.terminal.resize"
}
struct CrewdPublicCommandEnvelope: Equatable {
    let requestID: UUID
    let databaseID: String
    let runtimeBindingID: String
    let operation: CrewdPublicOperation
    let params: CrewdValue
    /// The caller must obtain binding through its actual qualified hello and
    /// advertised command.receipts. This initializer does not authenticate it.
    init(request: UUID, binding: CrewdValue, operation: CrewdPublicOperation, params: CrewdValue) throws {
        requestID = request
        databaseID = try CrewdPublicWire.token(binding["dbId"])
        runtimeBindingID = try CrewdPublicWire.token(binding["runtimeBindingId"])
        guard case .object = params else { throw CrewdPublicWire.invalid() }
        self.operation = operation
        // Persist the typed Rust serialization shape, including explicit null
        // defaults and integer JSON roundtrip. The same immutable command can
        // then be compared to a real server receipt without hashing a different
        // client-specific encoding or silently adopting a changed payload.
        let canonical = try CrewdPublicParameters.canonical(operation, params)
        self.params = try JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(canonical))
        guard try JSONEncoder().encode(value).count <= 256 * 1024 else { throw CrewdPublicWire.invalid() }
    }
    var command: CrewdValue { .object(["operation": .string(operation.rawValue), "params": params]) }
    var value: CrewdValue {
        .object(["requestId": .string(requestID.uuidString.lowercased()), "dbId": .string(databaseID),
                 "runtimeBindingId": .string(runtimeBindingID), "command": command])
    }
    /// Reconnect uses this read-only lookup, never value with a new instance.
    var lookup: CrewdValue { .object(["requestId": .string(requestID.uuidString.lowercased()), "dbId": .string(databaseID)]) }
}

struct CrewdTerminalWriteObservation {
    let inputBytes: Int
    let writtenBytes: Int?
    let unknown: Bool
    init(_ value: CrewdValue, phase: CrewdCommandPhase, submittedByteCount: Int) throws {
        guard submittedByteCount > 0, submittedByteCount <= 64 * 1024,
              value["inputBytes"].int == submittedByteCount,
              value["applicationConsumed"] == .null || value["applicationConsumed"] == .string("unconfirmed") else { throw CrewdPublicWire.invalid() }
        inputBytes = submittedByteCount
        if value["writtenBytes"] == .null { writtenBytes = nil }
        else {
            guard let written = value["writtenBytes"].int, written >= 0, written <= inputBytes else { throw CrewdPublicWire.invalid() }
            writtenBytes = written
        }
        switch phase {
        case .written:
            guard value["unknown"] == .bool(false), writtenBytes == inputBytes else { throw CrewdPublicWire.invalid() }
            unknown = false
        case .unknown:
            // Expiry/revocation has an authoritative Unknown receipt without a
            // physical byte count. Absence stays nil, never fabricated zero.
            if value["unknown"] == .bool(true) { unknown = true }
            else {
                guard value["unknown"] == .null, writtenBytes == nil,
                      value["physicalResult"] == .string("unconfirmed"),
                      value["replayAllowed"] == .bool(false) else { throw CrewdPublicWire.invalid() }
                unknown = true
            }
        default: throw CrewdPublicWire.invalid()
        }
    }

    // No remaining-bytes retry helper: partial/unknown must remain unresolved.
    // Even a complete kernel write has no applicationConsumed confirmation.
}

private enum CrewdPublicParameters {
    static func canonical(_ operation: CrewdPublicOperation, _ value: CrewdValue) throws -> CrewdValue {
        guard case .object(var fields) = value else { throw CrewdPublicWire.invalid() }
        var required: Set<String> = []
        var defaults: [String: CrewdValue] = [:]
        switch operation {
        case .personalDraftOwner,.personalDraftMember,.personalDraftConsent,.personalDraftCheckpoint,.personalDraftResolve:return try CrewPersonalCoreWire.canonical(operation.rawValue,value)
        case .crewCreate: required = ["title", "workingDirectory"]; defaults = ["parentId": .null]
        case .firstMessage:
            required = ["crew", "text"]
            guard case .object(var crew) = fields["crew"], crew["title"] != nil,
                  Set(crew.keys).isSubset(of: ["title", "workingDirectory", "parentId"]) else { throw CrewdPublicWire.invalid() }
            if crew["workingDirectory"] == nil || crew["workingDirectory"] == .null {
                crew["workingDirectory"] = .null
                if crew["parentId"] == nil { crew["parentId"] = .null }
                fields["crew"] = .object(crew)
            } else { fields["crew"] = try canonical(.crewCreate, .object(crew)) }
        case .messagePost:
            required = ["channelId", "text"]; defaults = ["inReplyTo": .null, "mentions": .array([]), "attachments": .array([])]
            if let mentions = fields["mentions"] {
                guard case .array(let rows) = mentions else { throw CrewdPublicWire.invalid() }
                fields["mentions"] = .array(try rows.map { row in
                    guard case .object(var item) = row, item["kind"]?.string != nil,
                          Set(item.keys).isSubset(of: ["kind", "targetId"]) else { throw CrewdPublicWire.invalid() }
                    if item["targetId"] == .null { item.removeValue(forKey: "targetId") }
                    return .object(item)
                })
            }
            // Current public message contract rejects every nonempty attachment.
            if let attachments = fields["attachments"], attachments != .array([]) { throw CrewdPublicWire.invalid() }
        case .sessionStart:
            required = ["crewId", "role", "executor", "brief", "isolation"]
            defaults = ["title": .null, "model": .null, "effort": .null]
            if let options=fields["codexOptions"] {
                guard fields["executor"] == .string("codex") else{throw CrewdPublicWire.invalid()}
                fields["codexOptions"] = try CrewdCodexWire.settings(options)
                for key in ["model","effort"] where options[key] != .null && fields[key] != nil && fields[key] != .null{throw CrewdPublicWire.invalid()}
                defaults["codexOptions"] = .null
            }
            if let selection=fields["codexInputs"]{guard fields["executor"] == .string("codex") else{throw CrewdPublicWire.invalid()};fields["codexInputs"]=try CrewdCodexInputWire.selection(selection);defaults["codexInputs"] = .null}
        case .codexConfigure:
            required=["target","patch"];fields["patch"]=try CrewdCodexWire.settings(fields["patch"] ?? .null)
        case .codexCompact,.codexArchive,.codexUnarchive:required=["target"]
        case .codexDelete:required=["target","confirmNativeHistoryDelete"];guard fields["confirmNativeHistoryDelete"] == .bool(true) else{throw CrewdPublicWire.invalid()}
        case .codexRevert:required=["target","beforeTurnId"];try CrewdCodexWire.text(fields["beforeTurnId"] ?? .null)
        case .codexSteer,.codexQueue:
            required=["target","text"];try CrewdCodexWire.text(fields["text"] ?? .null)
            if let selection=fields["inputs"]{fields["inputs"]=try CrewdCodexInputWire.selection(selection);defaults["inputs"] = .null}
        case .codexTitle:required=["target","title"];try CrewdCodexWire.text(fields["title"] ?? .null)
        case .codexFork:
            required=["target"];defaults=["lastTurnId":.null,"title":.null]
            for key in defaults.keys where fields[key] != nil && fields[key] != .null{try CrewdCodexWire.text(fields[key]!)}
        case .codexReview:
            required=["target","reviewTarget"];fields["reviewTarget"]=try CrewdCodexWire.review(fields["reviewTarget"] ?? .null)
        case .sessionStop: required = ["target", "reason"]
        case .sessionResume, .sessionInterrupt:
            return try CrewdCommandTarget(value).value
        case .sessionAnswer, .sessionRespond: required = ["target", "fingerprint", "runnerRequestId", "response"]
        case .hibernate: defaults = ["deadlineSecs": .null]
        case .terminalClaim: required = ["target"]; defaults = ["previousLeaseId": .null]
        case .terminalRelease: required = ["target", "leaseId"]
        case .terminalWrite:
            required = ["target", "leaseId", "bytesBase64"]
            let bytes = try CrewdPublicWire.bytes(fields["bytesBase64"] ?? .null, maximum: 64 * 1024)
            guard !bytes.isEmpty else { throw CrewdPublicWire.invalid() }
        case .terminalResize:
            required = ["target", "leaseId", "cols", "rows"]
            guard try CrewdPublicWire.dimension(fields["cols"] ?? .null) != nil,
                  try CrewdPublicWire.dimension(fields["rows"] ?? .null) != nil else { throw CrewdPublicWire.invalid() }
        case .localClientTrust:
            required = ["mode", "expectedCdHash"]; defaults = ["clientPid": .null, "expectedTeam": .null]
            guard ["exactCode", "publisher"].contains(fields["mode"]?.string ?? "") else { throw CrewdPublicWire.invalid() }
        case .localClientTrustRevoke: required = ["trustId"]
        }
        guard required.isSubset(of: Set(fields.keys)), Set(fields.keys).isSubset(of: required.union(defaults.keys)) else { throw CrewdPublicWire.invalid() }
        for (key, fallback) in defaults where fields[key] == nil { fields[key] = fallback }
        if let target = fields["target"] { fields["target"] = try CrewdCommandTarget(target).value }
        return .object(fields)
    }
}


/// Read-only factory metadata. Availability is not a model launch or a proof of
/// native turn association. All execution still uses public command receipts.
struct CrewdExecutorProfile: Identifiable, Equatable, Sendable {
    let id: String
    let executor: String
    let displayName: String
    let backend: String
    let available: Bool
    let initialContext: Bool
    let manualTerminal: Bool
    let structuredDecisions: Bool
    let fullHistoryLoad: Bool
    let unavailableReason: String?
    init(_ value: CrewdValue) throws {
        guard case .object(let fields) = value,
              Set(fields.keys) == ["id", "executor", "displayName", "backend", "available", "features", "unavailableReason"],
              case .object(let features) = value["features"],
              Set(features.keys) == ["initialContext", "manualTerminal", "structuredDecisions", "fullHistoryLoad"] else { throw CrewdPublicWire.invalid() }
        id = try CrewdPublicWire.token(value["id"])
        executor = try CrewdPublicWire.token(value["executor"])
        displayName = try CrewdPublicWire.token(value["displayName"])
        backend = try CrewdPublicWire.token(value["backend"])
        guard ["codex", "existingClaudePty", "nativeAcp"].contains(backend) else { throw CrewdPublicWire.invalid() }
        available = try CrewdPublicWire.boolean(value["available"])
        initialContext = try CrewdPublicWire.boolean(value["features"]["initialContext"])
        manualTerminal = try CrewdPublicWire.boolean(value["features"]["manualTerminal"])
        structuredDecisions = try CrewdPublicWire.boolean(value["features"]["structuredDecisions"])
        fullHistoryLoad = try CrewdPublicWire.boolean(value["features"]["fullHistoryLoad"])
        guard value["unavailableReason"] == .null || value["unavailableReason"].string != nil else { throw CrewdPublicWire.invalid() }
        unavailableReason = value["unavailableReason"].string
        guard unavailableReason.map({ ["notConfigured", "binaryChanged", "nativeVerificationPending", "sourceHistoryIncomplete", "unsupportedPlatform"].contains($0) }) ?? true else { throw CrewdPublicWire.invalid() }
        guard !available || unavailableReason == nil else { throw CrewdPublicWire.invalid() }
    }
    static func list(_ value: CrewdValue) throws -> [Self] {
        guard case .object(let fields) = value, Set(fields.keys) == ["profiles"],
              case .array(let rows) = value["profiles"], rows.count <= 64 else { throw CrewdPublicWire.invalid() }
        let profiles = try rows.map(Self.init)
        guard Set(profiles.map(\.id)).count == profiles.count,
              Set(profiles.map(\.executor)).count == profiles.count else { throw CrewdPublicWire.invalid() }
        return profiles
    }
    var offeredInOriginalUI: Bool {
        (id == "codex" && executor == "codex" && backend == "codex") ||
        (id == "claude" && executor == "claude" && backend == "existingClaudePty") ||
        (id == "claude-managed" && executor == "acp:claude-managed" && backend == "nativeAcp")
    }
    var canStart: Bool { offeredInOriginalUI && available && backend != "nativeAcp" }
    var title: String {
        switch backend {
        case "existingClaudePty": return "Claude 交互终端"
        case "nativeAcp": return "Claude 原生模式（尚未开放）"
        default: return displayName
        }
    }
    var recoveryDescription: String {
        backend == "existingClaudePty"
            ? "完整厂商历史未开放；请在原会话详情核对当前Core的恢复资格与限制。"
            : "只在你明确选择后尝试恢复原会话；完整历史加载未开放，不会补发未知输入。"
    }
    var unavailableDescription: String {
        switch unavailableReason {
        case "notConfigured": return "所连接的后台尚未配置可用程序。"
        case "binaryChanged": return "后台程序已更新，请先重启并重新核对执行能力。"
        case "nativeVerificationPending": return "受管 Claude 的运行资格尚未核实，当前不能启动。"
        case "sourceHistoryIncomplete": return "历史来源不完整，当前能力不可用。"
        case "unsupportedPlatform": return "所连接后台的平台不支持此能力。"
        default: return "当前后台未开放此执行器。"
        }
    }
}
