import Foundation

/// Swift mirror of the SessionProxyDO wire protocol
/// (`apps/edge/src/lib/session-proxy-protocol.ts`, spec v2 §8.2 + §9.6).
///
/// This is the **runner side** of T4.5 cross-device remote control. The
/// PendingCrew Mac host is the single `runner` peer for a crew session: it
/// subscribes as `runner`, publishes `session.state` (its transcript) +
/// `permission.request` to the DO, and receives `session.command` +
/// `permission.decision` back. Viewers (iOS/iPad/Mac PendingBot) live on the
/// other end and are out of scope here.
///
/// Only JSON framing lives in this file — pure value transforms, no
/// networking. Keeping the codec separate from `SessionProxyClient` makes it
/// unit-testable without a live socket.
///
/// Frames are encoded/decoded with `JSONSerialization` rather than Codable,
/// because the protocol's `payload` / `state` fields are deliberately free-form
/// `Record<string, unknown>` on the wire.

public enum ProxyRole: String, Sendable {
    case viewer
    case runner
}

// MARK: - JSONValue (minimal, Equatable free-form JSON)

/// A small Equatable JSON tree, used for the protocol's free-form `payload`
/// fields so inbound frames stay testable by value. Numbers normalize to
/// `Double` (JSON has one number type); integer payloads round-trip exactly
/// within 2^53.
public enum JSONValue: Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case null
    case array([JSONValue])
    case object([String: JSONValue])

    /// Wrap a JSONSerialization-produced value. Returns nil for types that
    /// can't appear in JSON.
    public init?(_ any: Any) {
        switch any {
        case let s as String: self = .string(s)
        case let b as Bool where Self.isBoolNSNumber(any): self = .bool(b)
        case let n as NSNumber: self = .number(n.doubleValue)
        case is NSNull: self = .null
        case let arr as [Any]:
            self = .array(arr.compactMap(JSONValue.init))
        case let obj as [String: Any]:
            var out: [String: JSONValue] = [:]
            for (k, v) in obj {
                guard let jv = JSONValue(v) else { return nil }
                out[k] = jv
            }
            self = .object(out)
        default:
            return nil
        }
    }

    /// Back to a JSONSerialization-compatible value (for encoding).
    public var anyValue: Any {
        switch self {
        case let .string(s): return s
        case let .number(n): return n
        case let .bool(b): return b
        case .null: return NSNull()
        case let .array(a): return a.map(\.anyValue)
        case let .object(o): return o.mapValues(\.anyValue)
        }
    }

    // `NSNumber` boxes Bool and numerics identically; distinguish a real
    // JSON bool by its CFTypeID so `true` doesn't decode as `1.0`.
    private static func isBoolNSNumber(_ any: Any) -> Bool {
        (any as? NSNumber).map { CFGetTypeID($0) == CFBooleanGetTypeID() } ?? false
    }
}

// MARK: - SessionState (runner → viewers payload)

/// The runner's published `session.state`. The wire field is free-form; this
/// is the shape PendingCrew owns. Viewers render best-effort.
///
/// **Phase 1**: the runner no longer publishes this (structured transcript
/// events were ripped out with the one-shot runner; the embedded terminal is
/// the source of truth). The struct + its outbound encode path stay so the
/// SessionProxy codec compiles whole; Phase 2's byte tunnel will repopulate
/// live state. The viewer side decodes via `SessionStateSnapshot`, not this.
public struct SessionState: Equatable, Sendable {
    /// running / completed / cancelled / failed — the run's lifecycle.
    public var status: String
    /// How many transcript events have been produced so far.
    public var eventCount: Int
    /// A short human label for the latest event (drives a one-line "what's
    /// happening" on viewers without shipping the whole transcript).
    public var lastEvent: String?

    public init(status: String, eventCount: Int, lastEvent: String?) {
        self.status = status
        self.eventCount = eventCount
        self.lastEvent = lastEvent
    }

    public func dict() -> [String: Any] {
        var d: [String: Any] = ["status": status, "eventCount": eventCount]
        if let lastEvent { d["lastEvent"] = lastEvent }
        return d
    }
}

// MARK: - SessionStateSnapshot (viewer ← runner, inbound decode)

/// The viewer-side decode of a runner's `session.state` fan-out. The runner
/// publishes `SessionState.dict()` (`{status, eventCount, lastEvent?}`) and the
/// DO broadcasts it verbatim inside `{type:"session.state", state:{…}}`. This
/// is the mirror image of `SessionState`: where that *builds* the wire payload,
/// this *reads* it on a watching client. Sendable so the proxy client can hand
/// it across the actor boundary to the SwiftUI viewer.
public struct SessionStateSnapshot: Equatable, Sendable {
    public var status: String
    public var eventCount: Int
    public var lastEvent: String?

    public init(status: String, eventCount: Int, lastEvent: String?) {
        self.status = status
        self.eventCount = eventCount
        self.lastEvent = lastEvent
    }

    /// Parse one inbound `session.state` frame. Returns nil for any other frame
    /// type / malformed JSON so a caller can cheaply try this on `.unknown`
    /// frames without a separate type check. `eventCount` tolerates the JSON
    /// number being a double.
    public static func parse(_ raw: String) -> SessionStateSnapshot? {
        guard let data = raw.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              obj["type"] as? String == "session.state",
              let state = obj["state"] as? [String: Any] else {
            return nil
        }
        let status = (state["status"] as? String) ?? ""
        let count = (state["eventCount"] as? NSNumber)?.intValue ?? 0
        let last = state["lastEvent"] as? String
        return SessionStateSnapshot(status: status, eventCount: count, lastEvent: last)
    }
}

// MARK: - Outbound (runner → DO)

/// A frame the runner sends to the DO.
public enum SessionProxyOutbound: Sendable {
    /// First frame after the socket opens. Role must match what the HTTP
    /// route authorized; `token` is a defence-in-depth echo.
    case subscribe(role: ProxyRole, token: String?)
    /// Publish current state to all viewers.
    case sessionState(SessionState)
    /// Raise a permission request (manual-mode high-risk action). `id` is the
    /// runner's local correlation id (e.g. the LocalApprovalStore item id) —
    /// the DO echoes it back on `permission.request.ack` together with the
    /// persisted server request id.
    case permissionRequest(id: String?, action: String, payload: [String: JSONValue]?, riskLevel: String?)

    /// The wire dictionary (stable shape; matches the TS interfaces).
    public func wireDict() -> [String: Any] {
        switch self {
        case let .subscribe(role, token):
            var d: [String: Any] = ["type": "subscribe", "role": role.rawValue]
            if let token { d["token"] = token }
            return d
        case let .sessionState(state):
            return ["type": "session.state", "state": state.dict()]
        case let .permissionRequest(id, action, payload, riskLevel):
            var req: [String: Any] = ["action": action]
            if let id { req["id"] = id }
            if let payload { req["payload"] = JSONValue.object(payload).anyValue }
            if let riskLevel { req["riskLevel"] = riskLevel }
            return ["type": "permission.request", "request": req]
        }
    }

    /// Serialize to a UTF-8 JSON frame ready for `WebSocket.send`.
    public func jsonData() throws -> Data {
        try JSONSerialization.data(withJSONObject: wireDict(), options: [.sortedKeys])
    }
}

// MARK: - Outbound (viewer → DO)

/// A frame a **viewer** sends up to the DO — the mirror of `SessionProxyOutbound`
/// for the watching/controlling side. The DO routes `session.command` to the
/// runner (assigning a `commandId` if absent) and persists it; it routes
/// `permission.decision` to the runner + back to other viewers. Shapes match the
/// TS `SessionCommandMsg` / `PermissionDecisionMsg`.
public enum SessionProxyViewerOutbound: Sendable {
    /// Send a command to the session's runner (e.g. `cancel`, `send_prompt`).
    /// `payload` is free-form; the DO assigns the `commandId`.
    case command(kind: String, payload: [String: JSONValue]?)
    /// Approve/reject a pending permission request raised by the runner.
    case permissionDecision(requestId: String, decision: String)

    public func wireDict() -> [String: Any] {
        switch self {
        case let .command(kind, payload):
            var cmd: [String: Any] = ["kind": kind]
            if let payload { cmd["payload"] = JSONValue.object(payload).anyValue }
            return ["type": "session.command", "command": cmd]
        case let .permissionDecision(requestId, decision):
            return ["type": "permission.decision", "requestId": requestId, "decision": decision]
        }
    }

    public func jsonData() throws -> Data {
        try JSONSerialization.data(withJSONObject: wireDict(), options: [.sortedKeys])
    }
}

// MARK: - Inbound (DO → runner)

/// A frame the runner receives from the DO. Only the variants that flow to a
/// runner are modeled; viewer-only frames degrade to `.unknown`.
public enum SessionProxyInbound: Equatable, Sendable {
    /// Ack of subscribe.
    case subscribed(sessionId: String, role: ProxyRole)
    /// A command to execute. `queued` = flushed from the offline queue (the
    /// runner dedupes against its own state). `payload` is free-form.
    case command(commandId: String, kind: String, payload: [String: JSONValue]?, queued: Bool)
    /// A viewer's approve/reject for a pending permission request.
    case permissionDecision(requestId: String, decision: String)
    /// Ack of a runner-raised permission.request: `requestId` is the
    /// DO-persisted server id, `clientRequestId` echoes the runner's own
    /// `request.id` (local approval id) for correlation.
    case permissionRequestAck(clientRequestId: String?, requestId: String)
    /// A protocol/error frame from the DO.
    case error(code: String, message: String)
    /// Valid JSON we don't act on as a runner (e.g. a viewer-targeted frame),
    /// or anything unclassifiable. Carries the raw frame for debugging.
    case unknown(String)

    /// Parse one inbound frame. Never throws — bad frames degrade to
    /// `.unknown` so the receive loop can't be crashed by garbage.
    public static func parse(_ raw: String) -> SessionProxyInbound {
        guard let data = raw.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = obj["type"] as? String else {
            return .unknown(raw)
        }
        switch type {
        case "subscribed":
            let sid = (obj["sessionId"] as? String) ?? ""
            let role = (obj["role"] as? String).flatMap(ProxyRole.init) ?? .runner
            return .subscribed(sessionId: sid, role: role)
        case "session.command":
            guard let commandId = obj["commandId"] as? String,
                  let command = obj["command"] as? [String: Any],
                  let kind = command["kind"] as? String else {
                return .unknown(raw)
            }
            let payload = (command["payload"] as? [String: Any]).flatMap(Self.objectPayload)
            let queued = (obj["queued"] as? Bool) ?? false
            return .command(commandId: commandId, kind: kind, payload: payload, queued: queued)
        case "permission.decision":
            guard let requestId = obj["requestId"] as? String,
                  let decision = obj["decision"] as? String else {
                return .unknown(raw)
            }
            return .permissionDecision(requestId: requestId, decision: decision)
        case "permission.request.ack":
            guard let requestId = obj["requestId"] as? String else {
                return .unknown(raw)
            }
            return .permissionRequestAck(
                clientRequestId: obj["clientRequestId"] as? String, requestId: requestId)
        case "error":
            return .error(code: (obj["code"] as? String) ?? "",
                          message: (obj["message"] as? String) ?? "")
        default:
            // subscribed/command/decision/error are the runner-facing frames;
            // session.state / permission.request are viewer-facing.
            return .unknown(raw)
        }
    }

    private static func objectPayload(_ dict: [String: Any]) -> [String: JSONValue]? {
        guard case let .object(o)? = JSONValue(dict) else { return nil }
        return o
    }
}
