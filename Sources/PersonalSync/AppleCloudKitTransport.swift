import Foundation
import CloudKit

/// Explicit private database adapter. Constructing it never fetches an account.
/// Qualification must ensure the signed entitlement environment matches config.
final class AppleCloudKitTransport: AppleBranchTransport, @unchecked Sendable {
    private let container: CKContainer
    private let configuration: (String, AppleSyncEnvironment)
    private let lock = NSLock()
    private var generation: UInt64 = 0
    private var operations: [UUID: CKDatabaseOperation] = [:]
    private var observer: NSObjectProtocol?
    init(containerIdentifier: String, environment: AppleSyncEnvironment) throws {
        guard containerIdentifier.hasPrefix("iCloud."), containerIdentifier.count > 7 else { throw AppleSyncFailure.invalid }
        container = CKContainer(identifier: containerIdentifier)
        configuration = (containerIdentifier, environment)
        observer = NotificationCenter.default.addObserver(forName: .CKAccountChanged, object: nil, queue: nil) { [weak self] _ in self?.invalidate() }
    }
    deinit { if let observer { NotificationCenter.default.removeObserver(observer) }; invalidate() }
    private func invalidate() {
        let pending = lock.withLock { generation &+= 1; let old = Array(operations.values); operations.removeAll(); return old }
        pending.forEach { $0.cancel() }
    }
    func cancel() async { invalidate() }
    /// Explicit account discovery; caller invokes only after cloud opt-in.
    func accountIdentity() async throws -> AppleSyncAccount {
        let start = epoch()
        guard try await container.accountStatus() == .available else { throw AppleSyncFailure.accountChanged }
        let id = try await container.userRecordID(); try check(start)
        return AppleSyncAccount(container: configuration.0, environment: configuration.1, userRecordID: id.recordName)
    }
    private func epoch() -> UInt64 { lock.withLock { generation } }
    private func check(_ expected: UInt64) throws { guard epoch() == expected else { throw AppleSyncFailure.accountChanged }; try Task.checkCancellation() }
    private func submit(_ operation: CKDatabaseOperation, epoch: UInt64) throws -> UUID {
        try check(epoch)
        let id = UUID()
        try lock.withLock {
            guard generation == epoch else { throw AppleSyncFailure.accountChanged }
            operations[id] = operation
            operation.completionBlock = { [weak self] in self?.finish(id) }
            container.privateCloudDatabase.add(operation)
        }
        return id
    }
    private func finish(_ id: UUID) { _ = lock.withLock { operations.removeValue(forKey: id) } }
    private func identity(_ account: AppleSyncAccount, epoch: UInt64) async throws {
        guard account.container == configuration.0, account.environment == configuration.1 else { throw AppleSyncFailure.accountChanged }
        guard try await container.accountStatus() == .available else { throw AppleSyncFailure.accountChanged }
        let actual = try await container.userRecordID()
        try check(epoch)
        guard actual.recordName == account.userRecordID else { throw AppleSyncFailure.accountChanged }
    }
    private func fetch(_ ids: [CKRecord.ID], epoch: UInt64) async throws -> [CKRecord.ID: CKRecord] {
        if ids.isEmpty { return [:] }
        return try await withCheckedThrowingContinuation { continuation in
            let op = CKFetchRecordsOperation(recordIDs: ids)
            let result = RecordResults()
            op.perRecordResultBlock = { id, value in
                switch value {
                case .success(let record): result.append(record)
                case .failure(let error): if (error as? CKError)?.code != .unknownItem { result.fail(error) }
                }
            }
            op.fetchRecordsResultBlock = { value in
                switch value {
                case .success: result.complete(continuation)
                case .failure(let error):
                    if (error as? CKError)?.code == .partialFailure { result.complete(continuation) }
                    else { continuation.resume(throwing: error) }
                }
            }
            do { _ = try submit(op, epoch: epoch) }
            catch { continuation.resume(throwing: error) }
        }
    }
    private final class RecordResults: @unchecked Sendable {
        private let lock = NSLock()
        private var records: [CKRecord.ID: CKRecord] = [:]
        private var error: (any Error)?
        private var byteCount = 0
        func append(_ record: CKRecord) { lock.withLock {
            byteCount += (record["payload"] as? Data)?.count ?? 0
            if byteCount > 8 * 1024 * 1024 { error = AppleSyncFailure.capacity }
            else { records[record.recordID] = record }
        } }
        func fail(_ value: any Error) { lock.withLock { error = value } }
        func page(_ c: CheckedContinuation<([CKRecord.ID: CKRecord], CKQueryOperation.Cursor?), any Error>, cursor: CKQueryOperation.Cursor?) {
            lock.withLock { if let error { c.resume(throwing: error) } else { c.resume(returning: (records, cursor)) } }
        }
        func complete(_ c: CheckedContinuation<[CKRecord.ID: CKRecord], any Error>) {
            lock.withLock { if let error { c.resume(throwing: error) } else { c.resume(returning: records) } }
        }
    }
    private func query(collection: String, epoch: UInt64) async throws -> [CKRecord.ID: CKRecord] {
        var cursor: CKQueryOperation.Cursor?
        var records: [CKRecord.ID: CKRecord] = [:]
        repeat {
            let next = cursor
            let page: ([CKRecord.ID: CKRecord], CKQueryOperation.Cursor?) = try await withCheckedThrowingContinuation { continuation in
                let op = next.map(CKQueryOperation.init(cursor:)) ?? CKQueryOperation(query: CKQuery(recordType: "PCWPersonalBranch", predicate: NSPredicate(format: "collectionID == %@", collection)))
                let result = RecordResults()
                op.recordMatchedBlock = { _, value in
                    switch value { case .success(let record): result.append(record); case .failure(let error): result.fail(error) }
                }
                op.queryResultBlock = { value in
                    switch value {
                    case .success(let cursor):
                        result.page(continuation, cursor: cursor)
                    case .failure(let error): continuation.resume(throwing: error)
                    }
                }
                do { _ = try submit(op, epoch: epoch) }
                catch { continuation.resume(throwing: error) }
            }
            records.merge(page.0) { _, b in b }; cursor = page.1
            guard records.count <= 10000, records.values.reduce(0, { $0 + (($1["payload"] as? Data)?.count ?? 0) }) <= 8 * 1024 * 1024 else { throw AppleSyncFailure.capacity }
            try check(epoch)
        } while cursor != nil
        return records
    }
    private final class SaveResults: @unchecked Sendable {
        private let lock = NSLock(); private var error: (any Error)?
        func fail(_ value: any Error) { lock.withLock { error = value } }
        func complete(_ c: CheckedContinuation<Void, any Error>) { lock.withLock { if let error { c.resume(throwing: error) } else { c.resume() } } }
    }
    private func save(_ records: [CKRecord], epoch: UInt64) async throws {
        guard !records.isEmpty else { return }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
            let op = CKModifyRecordsOperation(recordsToSave: records, recordIDsToDelete: nil)
            let results = SaveResults()
            op.perRecordSaveBlock = { _, result in if case .failure(let error) = result { results.fail(error) } }
            op.savePolicy = .ifServerRecordUnchanged
            op.isAtomic = false
            op.modifyRecordsResultBlock = { result in
                switch result { case .failure(let error): continuation.resume(throwing: error); case .success: results.complete(continuation) }
            }
            do { _ = try submit(op, epoch: epoch) }
            catch { continuation.resume(throwing: error) }
        }
    }
    func exchange(account: AppleSyncAccount, collection: String, branches: [AppleBranch]) async throws -> [AppleBranch] {
        let start = epoch()
        try await identity(account, epoch: start)
        let partition = appleSyncDigest(try appleSyncEncode(account) + Data(collection.utf8))
        var records = try await query(collection: partition, epoch: start)
        func id(_ row: AppleBranch) -> CKRecord.ID { CKRecord.ID(recordName: partition + "." + row.id.uuidString) }
        // Directly fetch local IDs as well: a query index must not hide an existing tombstone.
        for batch in stride(from: 0, to: branches.count, by: 100) {
            let ids = branches[batch..<min(batch + 100, branches.count)].map(id)
            records.merge(try await fetch(ids, epoch: start)) { _, b in b }
        }
        var merged: [UUID: AppleBranch] = [:]
        for record in records.values {
            guard record["collectionID"] as? String == partition, let data = record["payload"] as? Data else { throw AppleSyncFailure.invalid }
            let row = try JSONDecoder().decode(AppleBranch.self, from: data)
            guard record.recordID == id(row) else { throw AppleSyncFailure.invalid }
            merged[row.id] = row
        }
        var changed: [CKRecord] = []
        for local in branches {
            let row = try merged[local.id].map { try AppleBranch.merge($0, local) } ?? local
            if merged[local.id] != row {
                let record = records[id(row)] ?? CKRecord(recordType: "PCWPersonalBranch", recordID: id(row))
                let data = try appleSyncEncode(row)
                guard data.count <= 512 * 1024 else { throw AppleSyncFailure.capacity }
                record["collectionID"] = partition as CKRecordValue; record["payload"] = data as CKRecordValue
                changed.append(record)
            }
            merged[row.id] = row
        }
        for offset in stride(from: 0, to: changed.count, by: 100) {
            try await identity(account, epoch: start)
            try await save(Array(changed[offset..<min(offset + 100, changed.count)]), epoch: start)
        }
        try await identity(account, epoch: start)
        return Array(merged.values)
    }
}
