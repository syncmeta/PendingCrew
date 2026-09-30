import Foundation
import Darwin

/// Owns the one-time first-message transaction for an in-memory crew draft.
/// The caller supplies persistence operations; this type owns the directory and
/// the single-flight/rollback boundary, so it can be exercised with temp ledgers.
@MainActor
final class PendingCrewFirstSend {
    struct Outcome {
        let crewId: String
        let workingDirectory: String
    }

    private(set) var sending = false
    private var completed = false

    func commit(message: String, selectedDirectory: String?, crewGround: URL, title: String,
                create: (String) async throws -> String,
                attach: (String) async throws -> Void,
                post: (String, String) async throws -> Void,
                rollback: (String) -> Void) async throws -> Outcome? {
        let text = message.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !sending, !completed else { return nil }
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
            try await post(crewId, text)
            completed = true
            return Outcome(crewId: crewId, workingDirectory: workdir)
        } catch {
            if let createdCrewId { rollback(createdCrewId) }
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

    var errorDescription: String? {
        switch self {
        case .invalidWorkingDirectory: "所选工作目录已不存在或不是目录，请重新选择"
        case .creationFailed: "创建 crew 失败，请重试"
        }
    }
}
