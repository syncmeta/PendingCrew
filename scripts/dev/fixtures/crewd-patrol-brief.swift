import Foundation

@main struct PatrolBriefFixture {
    @MainActor static func main() async throws {
        var run: [String: CrewdValue] = [
            "runId": .string("original-run"),
            "startedAtMs": .integer(1),
            "completedAtMs": .integer(2),
            "truncated": .bool(false),
            "scannedCounts": .object(["work": .integer(4)]),
            "counts": .object([
                "long_waits": .integer(1), "unowned": .integer(2),
                "unclaimed": .integer(1), "incomplete_channels": .integer(1),
                "idle_archive_candidates": .integer(1), "archive_review_incomplete": .integer(1)
            ]),
            "findings": .object([
                "long_waits": .array([.string("W1")]),
                "unowned": .array([.string("W1"), .string("W2")]),
                "unclaimed": .array([.string("#1")]),
                "incomplete_channels": .array([.string("topic-a")]),
                "idle_archive_candidates": .array([.string("topic-b")]),
                "archive_review_incomplete": .array([.string("topic-c")])
            ])
        ]
        let store = CrewdWorkStore { method, params in
            precondition(method == "work.patrolStatus" && params["crewId"] == .string("a"))
            return .object(["crewId": .string("a"), "error": .null, "run": .object(run)])
        }
        store.selectCrew("a")
        let status = try await store.patrolStatus()
        let brief = try CrewdPatrolBrief(run: status["run"])
        precondition(brief.groups.map(\.kind) == [.longWaits, .unclaimed, .unowned, .idleArchiveCandidates, .archiveReviewIncomplete, .incompleteChannels])
        precondition(brief.groups[0].references == ["W1"])
        precondition(brief.groups[1].references == ["#1"])
        precondition(brief.groups[2].references == ["W1", "W2"])
        precondition(brief.groups[3].references == ["topic-b"])
        precondition(brief.groups[4].references == ["topic-c"])
        precondition(brief.hasIncompleteChannels)
        precondition(!brief.hasPartialReferences)

        run["truncated"] = .bool(true)
        run["counts"] = .object(["unowned": .integer(53), "future_kind": .integer(1)])
        run["findings"] = .object([
            "unowned": .array((1...50).map { .string("W\($0)") }),
            "future_kind": .array([.string("opaque-future-reference")])
        ])
        let partial = try CrewdPatrolBrief(run: .object(run))
        precondition(partial.hasPartialReferences && partial.hasUnrecognizedFindings)
        precondition(partial.groups.count == 1 && partial.groups[0].count == 53)
        precondition(partial.groups[0].references.count == 50)

        run["truncated"] = .bool(false)
        run["counts"] = .object(["unowned": .integer(2)])
        run["findings"] = .object(["unowned": .array([.string("W1")])])
        precondition(throwsInvalid(run))
        run["findings"] = .object(["unowned": .array([.string("W1"), .string("W1")])])
        precondition(throwsInvalid(run))
        run["findings"] = .object(["unowned": .array([.string("#1"), .string("W1")])])
        precondition(throwsInvalid(run))

        run["counts"] = .object([:])
        run["findings"] = .object([:])
        let empty = try CrewdPatrolBrief(run: .object(run))
        precondition(empty.groups.isEmpty && !empty.hasIncompleteChannels && !empty.hasUnrecognizedFindings)
        run["completedAtMs"] = .null
        precondition(throwsInvalid(run))
        print("patrol brief: completed source, categories, original references, partial and malformed states passed")
    }

    static func throwsInvalid(_ run: [String: CrewdValue]) -> Bool {
        do { _ = try CrewdPatrolBrief(run: .object(run)); return false }
        catch { return true }
    }
}
