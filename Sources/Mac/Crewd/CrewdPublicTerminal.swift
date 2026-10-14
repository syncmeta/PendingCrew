import Foundation
import Combine

/// One selected original session surface. It owns only its authenticated
/// observation/control socket; retiring it cannot stop the team's daemon.
@MainActor
final class CrewdPublicTerminal: ObservableObject {
    let workspace: CrewdWorkspace
    let sessionID: String
    @Published private(set) var snapshot: CrewdTerminalSnapshot?
    @Published private(set) var failure: String?
    @Published private(set) var leaseID: String?
    @Published private(set) var unresolvedInput = Data()
    @Published private(set) var busy = false
    @Published private(set) var connected = false
    var onSnapshot: ((CrewdTerminalSnapshot) -> Void)?
    var onBytes: ((Data) -> Void)?
    private var client: CrewdClient?
    private var commands: CrewdPublicCommands?
    private var epoch = UUID()
    private var ownerEpoch: UUID?
    private var target: CrewdCommandTarget?
    private var watchID: String?
    private var cursor: String?
    private var awaitingWatch = false
    private var earlyOutput: [CrewdTerminalOutput] = []
    private var earlyOverflow = false
    private var queuedInput = Data()
    private var pumping = false
    private var pumpTask: Task<Void, Never>?
    private var leaseExpiry: Task<Void, Never>?
    private var retired = false
    init(workspace: CrewdWorkspace, sessionID: String) {
        self.workspace = workspace; self.sessionID = sessionID
        workspace.registerTerminalDisplay(self)
    }
    private func accepts(_ captured: UUID) -> Bool {
        guard !retired, epoch == captured, let ownerEpoch,
              workspace.acceptsObservation(ownerEpoch), client?.connected == true else { return false }
        return true
    }
    /// Explicit display activation is bounded to one connection and one watch.
    /// No overflow/EOF/unknown path automatically reconnects or resends input.
    func connect() async {
        guard !retired, !busy, client == nil else { return }
        let captured = epoch
        busy = true; defer { if epoch == captured { busy = false } }
        ownerEpoch = workspace.observationGeneration
        do {
            let connection = try await workspace.openTerminalConnection()
            guard !retired, epoch == captured else { connection.disconnect(); return }
            client = connection; commands = workspace.makeTerminalCommands(for: connection)
            connection.onEvent = { [weak self] name, value in
                guard name == "session.terminal.output", let self, self.accepts(captured) else { return }
                do { self.receive(try CrewdTerminalOutput(value)) }
                catch { self.closeObservation("终端输出契约无效，需要重新读取屏幕；没有重发输入。") }
            }
            connection.onDisconnect = { [weak self] in
                guard let self, self.epoch == captured else { return }
                self.closeObservation("原终端连接已断开；控制权已失效，未知输入不会重发。")
            }
            let view = try await connection.call("session.terminal.inspect", .object(["sessionId": .string(sessionID)]))
            guard accepts(captured) else { throw CancellationError() }
            let initial = try CrewdTerminalSnapshot(view)
            guard initial.target.sessionID == sessionID else { throw CrewdClientError(message: "终端返回其它会话。") }
            target = initial.target
            if initial.kind != .pty {
                snapshot = initial; onSnapshot?(initial)
                closeObservation("该会话没有真实PTY；结构化输出与审批通过原会话投影显示。")
                return
            }
            // Register event handling before watch ACK; a bounded pre-ACK queue
            // prevents an event delivered immediately after ACK from vanishing.
            awaitingWatch = true
            let ack = try await connection.call("session.terminal.watch", .object(["target": initial.target.value]))
            guard accepts(captured) else { throw CancellationError() }
            guard let id = ack["watchId"].string, !id.isEmpty else { throw CrewdClientError(message: "终端观察缺少正式身份。") }
            let confirmedTarget = try CrewdCommandTarget(ack["target"])
            let cut = try CrewdTerminalSnapshot(ack["initialSnapshot"])
            guard cut.target == confirmedTarget, confirmedTarget == initial.target,
                  ack["after"].string == cut.nextCursor, !earlyOverflow else { throw CrewdClientError(message: "终端屏幕与流切点不一致，请重新读取。") }
            watchID = id; cursor = cut.nextCursor; snapshot = cut; onSnapshot?(cut)
            awaitingWatch = false; connected = true; failure = cut.truncated ? "当前屏幕是有限快照，不能视为完整历史。" : nil
            let buffered = earlyOutput; earlyOutput.removeAll()
            for output in buffered { receive(output) }
        } catch {
            if epoch == captured { closeObservation(error.localizedDescription) }
        }
    }
    private func receive(_ event: CrewdTerminalOutput) {
        guard event.target.sessionID == sessionID else { return }
        if awaitingWatch {
            guard earlyOutput.count < 8 else { earlyOverflow = true; return }
            earlyOutput.append(event); return
        }
        guard event.watchID == watchID, let original = snapshot?.target,
              event.target.sessionID == original.sessionID,
              event.target.generation == original.generation else { return }
        if event.truncated { closeObservation("终端流出现缺口，请显式重新读取屏幕；未补造缺失字节。") ; return }
        if event.nextCursor != cursor {
            guard event.cursor == cursor else { closeObservation("终端流顺序已改变，请重新读取屏幕。") ; return }
            cursor = event.nextCursor
            onBytes?(event.bytes)
        }
        // Closed is not physical exit; only the explicit server exited flag is
        // displayed as its observation. It never proves work completion.
        if event.closed { closeObservation(event.exited ? "后台报告该原进程已退出。" : "终端观察已关闭，原进程退出尚未确认。") }
    }
    func claim() async {
        guard !busy, leaseID == nil, connected, snapshot?.manualAvailable == true,
              let target else { return }
        let captured = epoch, began = Date()
        busy = true; defer { if epoch == captured { busy = false } }
        do {
            let receipt = try await command(.terminalClaim, .object(["target": target.value]))
            guard accepts(captured), !receipt.historical, receipt.phase == .confirmed,
                  let id = receipt.value["result"]["leaseId"].string, !id.isEmpty,
                  receipt.value["result"]["manualRevocationPersisted"] == .bool(true),
                  receipt.value["result"]["automaticInputPaused"] == .bool(true),
                  receipt.value["result"]["physicalInputWritten"] == .bool(false),
                  receipt.value["result"]["durationMs"].int == 30000 else { throw CrewdClientError(message: "手工控制结果未确认；不会发送输入。") }
            self.target = try nextTarget(receipt, original: target); leaseID = id; failure = nil
            let remaining = max(0, 30 - Date().timeIntervalSince(began))
            leaseExpiry?.cancel()
            leaseExpiry = Task { [weak self] in
                try? await Task.sleep(for: .seconds(remaining))
                guard !Task.isCancelled, let self, self.epoch == captured, self.leaseID == id else { return }
                self.invalidateLease("手工控制时限已到；请明确重新取得控制权，自动执行不会被本客户端恢复。")
            }
        } catch { if epoch == captured { invalidateLease(error.localizedDescription) } }
    }
    func enqueueInput(_ bytes: Data) {
        guard !bytes.isEmpty else { return }
        guard connected, leaseID != nil, failure == nil else { preserveUnsent(bytes); return }
        guard bytes.count <= 65536 - queuedInput.count else { preserveUnsent(bytes); invalidateLease("输入缓冲已满，未发送的字节保留；不会越过未知结果。") ; return }
        queuedInput.append(bytes)
        beginInputPump()
    }
    private func beginInputPump() {
        guard !pumping, !busy, !queuedInput.isEmpty, leaseID != nil, failure == nil else { return }
        pumping = true
        let captured = epoch
        pumpTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(15)) // bounded batching of Human input only
            guard !Task.isCancelled, let self else { return }
            defer { if self.epoch == captured { self.pumping = false } }
            while self.accepts(captured), let lease = self.leaseID, let current = self.target,
                  !self.queuedInput.isEmpty, self.failure == nil {
                let bytes = self.queuedInput; self.queuedInput.removeAll()
                do {
                    let receipt = try await self.command(.terminalWrite, .object(["target": current.value, "leaseId": .string(lease), "bytesBase64": .string(bytes.base64EncodedString())]))
                    guard self.accepts(captured), self.leaseID == lease, !receipt.historical else { throw CrewdClientError(message: "输入返回时原连接已改变，结果需只读核对。") }
                    let observed = try CrewdTerminalWriteObservation(receipt.value["result"], submittedByteCount: bytes.count)
                    guard receipt.phase == .written, !observed.unknown, observed.writtenBytes == bytes.count else { throw CrewdClientError(message: "终端输入存在部分或未知写入；不会重写剩余字节。") }
                    self.target = try self.nextTarget(receipt, original: current)
                } catch {
                    // Submitted bytes already exist durably in the original
                    // command snapshot. Queued bytes have never been submitted.
                    self.invalidateLease(error.localizedDescription); break
                }
            }
        }
    }
    func resize(columns: Int, rows: Int) async {
        guard connected, !pumping, !busy, let lease = leaseID, let current = target,
              let columns = UInt16(exactly: columns), columns > 0,
              let rows = UInt16(exactly: rows), rows > 0 else { return }
        let captured = epoch; busy = true; defer { if epoch == captured { busy = false; beginInputPump() } }
        do {
            let receipt = try await command(.terminalResize, .object(["target": current.value, "leaseId": .string(lease), "cols": .integer(Int64(columns)), "rows": .integer(Int64(rows))]))
            guard accepts(captured), leaseID == lease, !receipt.historical, receipt.phase == .resized,
                  receipt.value["result"]["applied"] == .bool(true) else { throw CrewdClientError(message: "终端尺寸内核回读尚未确认，不会自动重试。") }
            target = try nextTarget(receipt, original: current)
        } catch { if epoch == captured { invalidateLease(error.localizedDescription) } }
    }
    func release() async {
        guard !busy, !pumping, let lease = leaseID, let current = target else { return }
        let captured = epoch; busy = true; defer { if epoch == captured { busy = false } }
        do {
            let receipt = try await command(.terminalRelease, .object(["target": current.value, "leaseId": .string(lease)]))
            guard accepts(captured), !receipt.historical, receipt.phase == .confirmed else { throw CrewdClientError(message: "释放原控制权的结果尚未确认，不会恢复自动执行。") }
        }
        catch { if epoch == captured { failure = error.localizedDescription } }
        if epoch == captured { invalidateLease(failure ?? "手工控制已结束；不会自动恢复执行。") }
    }
    private func command(_ operation: CrewdPublicOperation, _ params: CrewdValue) async throws -> CrewdPublicReceipt {
        guard let client, let commands, accepts(epoch) else { throw CrewdClientError(message: "原终端连接已关闭，尚未提交。", kind: .notSubmitted) }
        _ = try workspace.publicCommandBinding(for: client)
        let request = try CrewdPublicCommandEnvelope(request: UUID(), binding: client.runtimeIdentity, operation: operation, params: params)
        return try await commands.submit(request, crew: workspace.crewID)
    }
    private func nextTarget(_ receipt: CrewdPublicReceipt, original: CrewdCommandTarget) throws -> CrewdCommandTarget {
        let next = try CrewdCommandTarget(receipt.value["result"]["target"])
        guard next.sessionID == sessionID, next.generation == original.generation else { throw CrewdClientError(message: "原物理代际已改变；不会将控制转到新进程。") }
        return next
    }
    private func preserveUnsent(_ bytes: Data) {
        guard unresolvedInput.count <= 65536, bytes.count <= 65536 - unresolvedInput.count else { failure = "未发送输入超过有限窗口；后续输入被拒，请保留原文。"; return }
        unresolvedInput.append(bytes)
    }
    private func invalidateLease(_ reason: String) {
        leaseExpiry?.cancel(); leaseExpiry = nil; leaseID = nil
        preserveUnsent(queuedInput); queuedInput.removeAll(); failure = reason
    }
    private func closeObservation(_ reason: String) {
        invalidateLease(reason); connected = false; awaitingWatch = false; earlyOutput.removeAll()
        client?.onDisconnect = nil; client?.onEvent = nil; client?.disconnect(); client = nil; commands = nil
        watchID = nil; cursor = nil
    }
    func deactivate() {
        epoch = UUID(); pumpTask?.cancel(); pumpTask = nil
        leaseExpiry?.cancel(); pumping = false; busy = false
        closeObservation("原终端页面已离开；仅本连接观察/控制结束，团队继续运行。")
        ownerEpoch = nil; target = nil; snapshot = nil
    }
    func retire() {
        retired = true; epoch = UUID(); pumpTask?.cancel(); leaseExpiry?.cancel()
        closeObservation("原终端呈现已关闭；仅本连接观察/控制已结束，团队仍由后台管理。")
    }
}
