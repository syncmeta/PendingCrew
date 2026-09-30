import Foundation
import Darwin

/// Owns the one-time first-message transaction for an in-memory crew draft.
/// The caller supplies persistence operations; this type owns the directory and
/// the single-flight/rollback boundary, so it can be exercised with temp ledgers.
@MainActor
final class PendingCrewFirstSend {
    struct Outcome {
        enum Delivery: Equatable { case delivered, pending }
        let crewId: String
        let workingDirectory: String
        let delivery: Delivery
    }

    private(set) var sending = false
    private var completed = false
    private var blockedRollback: LocalCrewStore.DeleteOutcome?

    func commit(message: String, selectedDirectory: String?, crewGround: URL, title: String,
                create: (String) async throws -> String,
                attach: (String) async throws -> Void,
                post: (String, String) async throws -> Void,
                rollback: ((String) -> LocalCrewStore.DeleteOutcome)?) async throws -> Outcome? {
        let text = message.trimmingCharacters(in: .whitespacesAndNewlines)
        if let blockedRollback {
            throw PendingCrewFirstSendError.rollbackUnverified(blockedRollback)
        }
        guard !text.isEmpty, !sending, !completed else { return nil }
        guard let rollback else { throw PendingCrewFirstSendError.rollbackUnavailable }
        sending = true
        defer { sending = false }

        var createdDirectory: URL?
        var createdRoot: URL?
        var createdCrewId: String?
        do {
            let workdir: String
            if let selectedDirectory {
                var isDirectory: ObjCBool = false
                guard FileManager.default.fileExists(atPath: selectedDirectory,
                                                     isDirectory: &isDirectory), isDirectory.boolValue else {
                    throw PendingCrewFirstSendError.invalidWorkingDirectory
                }
                workdir = selectedDirectory
            } else {
                let rootExisted = FileManager.default.fileExists(atPath: crewGround.path)
                try FileManager.default.createDirectory(at: crewGround,
                                                        withIntermediateDirectories: true)
                if !rootExisted { createdRoot = crewGround }
                var target = crewGround.appendingPathComponent(title, isDirectory: true)
                while mkdir(target.path, 0o700) != 0 {
                    guard errno == EEXIST else {
                        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                    }
                    target = crewGround.appendingPathComponent(
                        "Crew-\(UUID().uuidString.prefix(8))", isDirectory: true)
                }
                createdDirectory = target
                workdir = target.path
            }

            let crewId = try await create(workdir)
            guard !crewId.isEmpty else { throw PendingCrewFirstSendError.creationFailed }
            createdCrewId = crewId
            try await attach(crewId)
            do {
                try await post(crewId, text)
            } catch {
                if LocalWhiteboardStore.wasPreservedForRetry(error) {
                    // The outbox owns the only copy now. Keep the crew and never
                    // post a second copy, but do not call it delivered yet.
                    completed = true
                    return Outcome(crewId: crewId, workingDirectory: workdir,
                                   delivery: .pending)
                }
                throw error
            }
            completed = true
            return Outcome(crewId: crewId, workingDirectory: workdir,
                           delivery: .delivered)
        } catch {
            if let createdCrewId {
                let result = rollback(createdCrewId)
                guard result == .deleted else {
                    // The crew may still own its workdir. Keep both the directory
                    // and the one-shot gate until a human can resolve its state.
                    blockedRollback = result
                    throw PendingCrewFirstSendError.rollbackUnverified(result)
                }
            }
            if let createdDirectory,
               (try? FileManager.default.contentsOfDirectory(atPath: createdDirectory.path))?.isEmpty == true {
                try? FileManager.default.removeItem(at: createdDirectory)
            }
            if let createdRoot,
               (try? FileManager.default.contentsOfDirectory(atPath: createdRoot.path))?.isEmpty == true {
                try? FileManager.default.removeItem(at: createdRoot)
            }
            throw error
        }
    }
}

enum PendingCrewFirstSendError: LocalizedError {
    case invalidWorkingDirectory
    case creationFailed
    case rollbackUnavailable
    case rollbackUnverified(LocalCrewStore.DeleteOutcome)

    var errorDescription: String? {
        switch self {
        case .invalidWorkingDirectory: "所选工作目录已不存在或不是目录，请重新选择"
        case .creationFailed: "创建 crew 失败，请重试"
        case .rollbackUnavailable: "此连接不支持安全回滚，请在本机完成创建"
        case .rollbackUnverified(let result):
            "首条消息未发送，回滚创建结果为 \(result)；为避免重复创建，已停止此草稿重试"
        }
    }
}

/// Kept by CrewStore across draft navigation. A disposable send attempt cannot
/// be the only owner of an unverified rollback decision.
struct PendingCrewRollbackGuard {
    private(set) var failure: LocalCrewStore.DeleteOutcome?

    var isBlocked: Bool { failure != nil }

    mutating func record(_ error: Error) {
        guard let error = error as? PendingCrewFirstSendError,
              case .rollbackUnverified(let result) = error else {
            return
        }
        failure = result
    }

    func check() throws {
        if let failure { throw PendingCrewFirstSendError.rollbackUnverified(failure) }
    }
}

/// Durable first-send intent. Written before create so an interrupted create
/// remains visible and cannot silently become another new crew on relaunch.
struct PendingCrewRecoveryStore {
    struct Record: Codable {
        let draftId: String
        let title: String
        let parentCrewId: String?
        let selectedDirectory: String?
        let text: String
        var crewId: String?
        var workingDirectory: String?
    }

    let file: URL

    init(directory: URL = PendingCrewDataRoot.subdirectory("pending-first-send")) {
        file = directory.appendingPathComponent("recovery.json")
    }

    func load() throws -> Record? {
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        return try JSONDecoder().decode(Record.self, from: Data(contentsOf: file))
    }

    func save(_ record: Record) throws {
        try MultiProcessJSONStore.writeStaged(JSONEncoder().encode(record), to: file)
    }

    func clear() throws {
        guard FileManager.default.fileExists(atPath: file.path) else { return }
        try FileManager.default.removeItem(at: file)
    }
}
