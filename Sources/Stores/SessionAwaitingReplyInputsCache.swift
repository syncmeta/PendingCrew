import Foundation

/// Point-in-time inputs for the session's "waiting for a reply" indicator.
/// Manual approval cards have been retired; only the last-turn question marker
/// needs a cached disk read on each roster tick.
final class SessionAwaitingReplyInputsCache: @unchecked Sendable {
    struct RunKey: Hashable, Sendable {
        let crewId: String
        let sessionId: String
    }

    struct Inputs: Equatable, Sendable {
        var trailingQuestion: String?
    }

    private let markers: FileFingerprintCache<RunKey, String>

    convenience init(directory: URL) {
        let dirPath = directory.path
        self.init(
            markerFingerprint: { key in
                FileChangeGate.fingerprint(
                    atPath: dirPath + "/" + key.crewId + "." + key.sessionId + ".turn")
            },
            trailingQuestion: { key in
                SessionTurnMarker(directory: directory, crewId: key.crewId,
                                  sessionId: key.sessionId).read().awaitingQuestion
            })
    }

    init(markerFingerprint: @escaping (RunKey) -> FileChangeGate.Fingerprint?,
         trailingQuestion: @escaping (RunKey) -> String?) {
        markers = FileFingerprintCache(fingerprintOf: markerFingerprint,
                                       load: trailingQuestion)
    }

    var markerReadCount: Int { markers.loadCount }

    func refresh(runs: [RunKey]) -> [RunKey: Inputs] {
        let questions = markers.refresh(keys: runs)
        var result: [RunKey: Inputs] = [:]
        result.reserveCapacity(runs.count)
        for run in runs {
            result[run] = Inputs(trailingQuestion: questions[run])
        }
        return result
    }

    func clear() { markers.clear() }
}
