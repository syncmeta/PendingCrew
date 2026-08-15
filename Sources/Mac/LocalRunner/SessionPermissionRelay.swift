#if os(macOS)
import Foundation

// #204 permission over WS — runner-side bridge between the local approvals
// store and the SessionProxyDO WebSocket.
//
// The local approval chain (PreToolUse permission hook / codex approval
// provider → LocalApprovalStore JSON → long-poll) is the source of truth that
// actually blocks the agent. This relay makes it cross-device:
//
//   pending local `permission` item ──raise──▶ WS permission.request ─▶ DO
//     (DO persists the T4.3 permission_requests row + fans out to viewers,
//      then acks us with {clientRequestId: localId, requestId: serverId})
//
//   WS permission.decision (viewer's approve/reject, live or flushed from
//   the DO's offline queue as a session.command) ──▶ map server id → local
//   id ──▶ LocalApprovalStore.decide → the blocked hook long-poll unblocks.
//
//   local decision (human answered the Mac card first) ──mirror──▶ HTTP
//   decide endpoint via the injected closure, so the server row doesn't
//   stay pending on every remote viewer forever.
//
// HTTP polling on the viewers stays as the fallback path; this only makes
// the "card appears / card clears" halves real-time.

// MARK: - PermissionRelayLogic (pure state machine; unit tested)

/// Pure decision core: which items to raise, how to correlate acks, what to
/// do with an inbound decision, and which local decisions to mirror back.
/// Owns no I/O — `SessionPermissionRelay` drives it.
struct PermissionRelayLogic {
    struct Mirror: Equatable, Sendable {
        let serverRequestId: String
        /// Server vocabulary: "approve" | "reject".
        let decision: String
        init(serverRequestId: String, decision: String) {
            self.serverRequestId = serverRequestId
            self.decision = decision
        }
    }

    struct Outcome: Equatable {
        /// Local approval items to raise over the WS (send permission.request).
        var raises: [ApprovalItem] = []
        /// Local decisions to mirror to the server's decide endpoint.
        var mirrors: [Mirror] = []
    }

    private let sessionId: String
    private let maxRaiseAttempts: Int

    /// local ids raised and awaiting the DO's ack (in-flight).
    private var awaitingAck: Set<String> = []
    /// raise attempts per local id (caps duplicate server rows when acks are
    /// repeatedly lost — after the cap the item stays local-only).
    private var attempts: [String: Int] = [:]
    private var serverToLocal: [String: String] = [:]
    private var localToServer: [String: String] = [:]
    /// local ids decided BY a remote decision — never mirrored back.
    private var remoteDecided: Set<String> = []
    /// local ids whose local decision we already mirrored.
    private var mirrored: Set<String> = []
    /// local ids seen as already-answered before we ever raised them — dead
    /// to the relay (nothing to raise, nothing to correlate).
    private var ignoredAnswered: Set<String> = []

    init(sessionId: String, maxRaiseAttempts: Int = 3) {
        self.sessionId = sessionId
        self.maxRaiseAttempts = maxRaiseAttempts
    }

    /// Reconcile against the store's current items (this session's
    /// `permission`-kind rows only; anything else is ignored).
    mutating func scan(_ items: [ApprovalItem]) -> Outcome {
        var out = Outcome()
        for item in items where item.kind == "permission" && item.sessionId == sessionId {
            if item.status == "pending" {
                guard localToServer[item.id] == nil,          // already correlated
                      !awaitingAck.contains(item.id),         // raise in flight
                      attempts[item.id, default: 0] < maxRaiseAttempts
                else { continue }
                awaitingAck.insert(item.id)
                attempts[item.id, default: 0] += 1
                out.raises.append(item)
            } else {
                // answered — mirror a *local* decision to the server, once,
                // and only for items we actually correlated.
                if localToServer[item.id] == nil { ignoredAnswered.insert(item.id); continue }
                guard let serverId = localToServer[item.id],
                      !mirrored.contains(item.id),
                      !remoteDecided.contains(item.id)
                else { continue }
                mirrored.insert(item.id)
                out.mirrors.append(Mirror(
                    serverRequestId: serverId,
                    decision: item.decision == "allow" ? "approve" : "reject"))
            }
        }
        return out
    }

    /// The DO acked a raise — record the server id mapping.
    mutating func ack(clientRequestId: String?, serverRequestId: String) {
        guard let localId = clientRequestId else { return }
        awaitingAck.remove(localId)
        serverToLocal[serverRequestId] = localId
        localToServer[localId] = serverRequestId
    }

    /// An inbound viewer decision (live frame or flushed queue command).
    /// Returns the local approval to decide, in the local "allow"/"deny"
    /// vocabulary — or nil for unknown/duplicate ids (idempotent).
    mutating func remoteDecision(
        serverRequestId: String, decision: String
    ) -> (localId: String, localDecision: String)? {
        guard let localId = serverToLocal[serverRequestId],
              !remoteDecided.contains(localId)
        else { return nil }
        remoteDecided.insert(localId)
        return (localId, decision == "approve" ? "allow" : "deny")
    }

    /// Periodic rescue: a raise whose ack never came (sent pre-subscribe, or
    /// the socket dropped) becomes eligible to re-raise on the next scan.
    mutating func retryWindowElapsed() {
        awaitingAck.removeAll()
    }
}

// MARK: - SessionPermissionRelay (wiring)

/// Owns one runner-role `SessionProxyClient` for a logged session and pumps
/// `PermissionRelayLogic` from three sources: the approvals store's change
/// stream, the WS inbound stream, and a slow retry timer. `mirrorDecide` is
/// the HTTP decide call (injected as a closure so this file stays free of the
/// API layer and compiles into the PendingCrewTests bundle).
@MainActor
final class SessionPermissionRelay {
    private let crewId: String
    private let sessionId: String
    private let approvals: LocalApprovalStore
    private let client: SessionProxyClient
    private let mirrorDecide: @Sendable (_ serverRequestId: String, _ decision: String) async -> Void

    private var logic: PermissionRelayLogic
    private var inboundTask: Task<Void, Never>?
    private var watchTask: Task<Void, Never>?
    private var retryTask: Task<Void, Never>?

    private static let retryInterval: Duration = .seconds(20)

    init(
        crewId: String,
        sessionId: String,
        approvals: LocalApprovalStore = .shared,
        client: SessionProxyClient,
        mirrorDecide: @escaping @Sendable (_ serverRequestId: String, _ decision: String) async -> Void
    ) {
        self.crewId = crewId
        self.sessionId = sessionId
        self.approvals = approvals
        self.client = client
        self.mirrorDecide = mirrorDecide
        self.logic = PermissionRelayLogic(sessionId: sessionId)
    }

    func start() {
        guard inboundTask == nil else { return }
        let client = self.client
        Task { await client.connect() }

        inboundTask = Task { [weak self] in
            for await frame in await client.inbound {
                guard let self, !Task.isCancelled else { return }
                self.handle(frame)
            }
        }
        watchTask = Task { [weak self] in
            guard let self else { return }
            // Initial reconcile (a hook may have raised before we attached),
            // then re-scan on every store change (raise + local-decide mirror).
            self.scan()
            for await _ in self.approvals.approvalChanges(crewId: self.crewId) {
                if Task.isCancelled { return }
                self.scan()
            }
        }
        retryTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.retryInterval)
                if Task.isCancelled { return }
                guard let self else { return }
                self.logic.retryWindowElapsed()
                self.scan()
            }
        }
    }

    func stop() {
        inboundTask?.cancel(); inboundTask = nil
        watchTask?.cancel(); watchTask = nil
        retryTask?.cancel(); retryTask = nil
        let client = self.client
        Task { await client.close() }
    }

    // MARK: - pumps

    private func scan() {
        let outcome = logic.scan(approvals.list(crewId: crewId))
        let client = self.client
        for item in outcome.raises {
            Task {
                await client.requestPermission(
                    id: item.id, action: item.summary, payload: nil, riskLevel: nil)
            }
        }
        for mirror in outcome.mirrors {
            let decide = mirrorDecide
            Task { await decide(mirror.serverRequestId, mirror.decision) }
        }
    }

    private func handle(_ frame: SessionProxyInbound) {
        switch frame {
        case let .permissionRequestAck(clientRequestId, requestId):
            logic.ack(clientRequestId: clientRequestId, serverRequestId: requestId)
            // The item may have been decided locally while the ack was in
            // flight — re-scan so the mirror fires now that it's correlated.
            scan()
        case let .permissionDecision(requestId, decision):
            applyRemoteDecision(serverRequestId: requestId, decision: decision)
        case let .command(_, kind, payload, _):
            // A decision queued while this socket was down arrives as a
            // flushed session.command (see SessionProxyDO). Other command
            // kinds (send_prompt / cancel) are delivered durably via the
            // session mailbox — not this relay's job; ignore them here.
            guard kind == "permission.decision",
                  case let .string(requestId)? = payload?["requestId"],
                  case let .string(decision)? = payload?["decision"]
            else { return }
            applyRemoteDecision(serverRequestId: requestId, decision: decision)
        case .subscribed, .error, .unknown:
            break
        }
    }

    private func applyRemoteDecision(serverRequestId: String, decision: String) {
        guard let (localId, localDecision) = logic.remoteDecision(
            serverRequestId: serverRequestId, decision: decision) else { return }
        // decide() writes the store → the blocked hook long-poll returns →
        // the agent resumes; the local inline card clears via the store's
        // change stream. No mirror back (the DO already persisted it).
        approvals.decide(crewId: crewId, id: localId, decision: localDecision)
    }
}
#endif
