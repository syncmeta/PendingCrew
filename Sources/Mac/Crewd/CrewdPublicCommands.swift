import Foundation
import Combine

/// The server owns one durable receipt ledger. This controller holds bounded
/// display observations and a private presentation prepare journal only.
/// Recovery methods issue lookup, never executor dispatch or mutation replay.
@MainActor
final class CrewdPublicCommands: ObservableObject {
    struct Binding: Equatable {
        let scope: CrewdPresentationJournal.Scope
        let databaseID: String
        let runtimeBindingID: String
        let connectionEpoch: UUID
        init(scope: CrewdPresentationJournal.Scope, databaseID: String, runtimeBindingID: String, connectionEpoch: UUID = UUID()) {
            self.scope = scope; self.databaseID = databaseID; self.runtimeBindingID = runtimeBindingID; self.connectionEpoch = connectionEpoch
        }
    }
    struct Observation: Identifiable {
        let prepared: CrewdPresentationJournal.Prepared
        let envelope: CrewdPublicCommandEnvelope
        var receipt: CrewdPublicReceipt?
        var failure: String?
        var id: UUID { envelope.requestID }
    }
    typealias Call = (String, CrewdValue) async throws -> CrewdValue
    typealias Prepare = (CrewdPublicCommandEnvelope, String) throws -> CrewdPresentationJournal.Prepared
    typealias Finish = (CrewdPresentationJournal.Prepared, String, String?) throws -> Void
    @Published private(set) var observations: [UUID: Observation] = [:]
    @Published private(set) var pending = Set<UUID>()
    private let binding: () throws -> Binding
    private let ownsDaemon: () -> Bool
    private let call: Call
    private let prepare: Prepare
    private let finish: Finish
    private let allowsSubmission: () -> Bool
    private var sent = Set<UUID>()
    private let maximumObservations = 256
    init(binding: @escaping () throws -> Binding, ownsDaemon: @escaping () -> Bool,
         call: @escaping Call, prepare: @escaping Prepare, finish: @escaping Finish, allowsSubmission: @escaping () -> Bool = { true }) {
        self.binding = binding; self.ownsDaemon = ownsDaemon; self.call = call
        self.prepare = prepare; self.finish = finish; self.allowsSubmission = allowsSubmission
    }
    /// One explicit send, only after its immutable original scope/payload/UUID
    /// has been privately persisted. A refused/unknown response never clears it.
    func submit(_ envelope: CrewdPublicCommandEnvelope, crew: String, validateReceipt:((CrewdPublicReceipt)throws->Void)?=nil) async throws -> CrewdPublicReceipt {
        guard allowsSubmission() else { throw notSubmitted("正在停工，不再接受新执行。") }
        let captured = try binding()
        if observations.count >= maximumObservations {
            let settled = observations.values.filter { row in
                guard !pending.contains(row.id), let receipt = row.receipt else { return false }
                return !receipt.historical && [.committed, .launched, .written, .resized, .confirmed, .suppressed, .rejected].contains(receipt.phase)
            }.sorted { $0.prepared.preparedAt < $1.prepared.preparedAt }
            for row in settled.prefix(observations.count - maximumObservations + 1) { observations.removeValue(forKey: row.id) }
        }
        guard captured.databaseID == envelope.databaseID,
              captured.runtimeBindingID == envelope.runtimeBindingID,
              !sent.contains(envelope.requestID), !pending.contains(envelope.requestID),
              observations.count < maximumObservations else { throw notSubmitted("原命令身份已提交或历史窗口已满；请只读核对原请求。") }
        if envelope.operation == .hibernate, !ownsDaemon() {
            throw notSubmitted("尚未核实本App实际拥有的后台进程；关闭请求尚未提交。")
        }
        let prepared: CrewdPresentationJournal.Prepared
        do { prepared = try prepare(envelope, crew) }
        catch { throw notSubmitted("原请求未能耐久保存，尚未发送：" + error.localizedDescription) }
        sent.insert(envelope.requestID); pending.insert(envelope.requestID)
        observations[envelope.requestID] = .init(prepared: prepared, envelope: envelope)
        defer { pending.remove(envelope.requestID) }
        do {
            // Same actual connection/authority is required immediately before
            // send. Path, same UID and a JSON instance never confer admission.
            guard try binding() == captured else { throw notSubmitted("连接身份已经改变；原请求尚未发送。") }
            if envelope.operation == .hibernate, !ownsDaemon() { throw notSubmitted("后台拥有关系已经改变；尚未发送关闭。") }
            let value = try await call("command.submit", envelope.value)
            let receipt = try CrewdPublicReceipt(value, request: envelope.requestID,
                database: envelope.databaseID, originalRuntimeBinding: envelope.runtimeBindingID, originalCommand: envelope.command)
            guard (try? binding()) == captured else {
                throw CrewdClientError(message: "原命令返回时连接已改变；保留原请求并在原数据库只读确认。")
            }
            try validateReceipt?(receipt)
            observations[envelope.requestID]?.receipt = receipt
            // A real response can itself report Unknown. Historical or merely
            // dispatched external work remains unresolved, never auto-replayed.
            let unresolved = receipt.historical || [.reserved, .admitted, .dispatched, .unknown].contains(receipt.phase)
            try finish(prepared, unresolved ? "unknown" : "replyObserved", nil)
            return receipt
        } catch {
            let uncertain = CrewdClientError.isUncertainSubmission(error) || (error as? CrewdClientError)?.code == -32603
            observations[envelope.requestID]?.failure = error.localizedDescription
            // If receipt decoding/presentation persistence failed, the single
            // submitted identity stays unresolved even if SQL may have committed.
            do { try finish(prepared, uncertain ? "unknown" : "refused", error.localizedDescription) }
            catch { observations[envelope.requestID]?.failure = "后台结果与本机观察未能同时确认；原请求仍保留。" }
            throw error
        }
    }
    /// Lookup can cross a daemon restart with the same actual database and
    /// principal. The receipt's original instance/historical flag is preserved.
    func lookup(_ original: Observation) async throws -> CrewdPublicReceipt? {
        let captured = try binding()
        guard captured.databaseID == original.envelope.databaseID,
              !pending.contains(original.id),
              original.prepared.scope == captured.scope else {
            throw notSubmitted("原请求不属于当前可信数据库或前端作用域；未查找或重发。")
        }
        pending.insert(original.id); defer { pending.remove(original.id) }
        let value = try await call("command.lookup", original.envelope.lookup)
        guard try binding() == captured else { throw CancellationError() }
        // Not found is read-only uncertainty, never permission to submit anew.
        if value == .null { return nil }
        let receipt = try CrewdPublicReceipt(value, request: original.id,
            database: original.envelope.databaseID, originalRuntimeBinding: original.envelope.runtimeBindingID, originalCommand: original.envelope.command)
        observations[original.id] = .init(prepared: original.prepared,
            envelope: original.envelope, receipt: receipt)
        return receipt
    }
    /// Rehydrate only durable original command snapshots. No connection/start
    /// hook invokes submit for these records; restoration remains observational.
    func restore(_ records: [CrewdPresentationJournal.Prepared]) throws {
        guard records.count <= maximumObservations else { throw notSubmitted("原命令历史超过当前观察窗口；保留磁盘资料，需分批只读核对。") }
        for record in records where record.method == "command.submit" {
            guard let request = record.params["requestId"].string.flatMap(UUID.init(uuidString:)),
                  let operation = record.params["command"]["operation"].string.flatMap(CrewdPublicOperation.init(rawValue:)) else {
                throw notSubmitted("原公开命令记录缺少可信请求或闭合操作类型；未重放。")
            }
            let envelope = try CrewdPublicCommandEnvelope(request: request,
                binding: .object(["dbId": record.params["dbId"], "runtimeBindingId": record.params["runtimeBindingId"]]),
                operation: operation, params: record.params["command"]["params"])
            guard envelope.value == record.params else { throw notSubmitted("原公开命令记录发生变化；未重放。") }
            if let old = observations[request], old.envelope != envelope { throw notSubmitted("原请求编号对应不同payload；保留冲突记录。") }
            if observations[request] == nil, observations.count >= maximumObservations {
                let settled = observations.values.filter { row in
                    guard !pending.contains(row.id), let receipt = row.receipt else { return false }
                    return !receipt.historical && [.committed, .launched, .written, .resized, .confirmed, .suppressed, .rejected].contains(receipt.phase)
                }.min { $0.prepared.preparedAt < $1.prepared.preparedAt }
                guard let settled else { throw notSubmitted("原观察窗口已满；磁盘原记录仍保留，不能移除未确认请求来核对新项。") }
                observations.removeValue(forKey: settled.id)
            }
            sent.insert(request)
            observations[request] = .init(prepared: record, envelope: envelope)
        }
    }
    private func notSubmitted(_ message: String) -> CrewdClientError { .init(message: message, kind: .notSubmitted) }
}
