import Foundation
@main struct InspectionFixture {
    @MainActor static func main() async throws {
        var calls: [String] = []
        var mismatched = false
        var held: CheckedContinuation<CrewdValue, Error>?
        var delay = false
        let store = CrewdWorkStore { method, source in
            calls.append(method)
            if delay { return try await withCheckedThrowingContinuation { held = $0 } }
            return .object(["source": mismatched ? .null : source, "record": .null])
        }
        store.selectCrew("a")
        for kind in ["message", "requirement"] {
            let result = try await store.inspectJudgment(crew: "a", kind: kind, id: "source")
            precondition(result == .null)
        }
        mismatched = true
        do { _ = try await store.inspectJudgment(crew: "a", kind: "message", id: "source"); fatalError("accepted foreign source") } catch {}
        delay = true
        let pending = Task { try await store.inspectJudgment(crew: "a", kind: "message", id: "source") }
        while held == nil { await Task.yield() }
        store.invalidateConnection()
        held!.resume(returning: .object(["source": .object(["crewId": .string("a"), "kind": .string("message"), "id": .string("source")]), "record": .null]))
        do { _ = try await pending.value; fatalError("accepted stale connection") } catch is CancellationError {} 
        precondition(calls == Array(repeating: "judgment.inspect", count: 4))
        print("inspection: both kinds, no record, source mismatch, stale connection, read-only calls passed")
    }
}
