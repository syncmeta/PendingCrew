import Foundation
@main struct PatrolFixture {
    @MainActor static func main() async throws {
        var calls: [CrewdValue] = []
        var foreign = false
        var malformed = 0
        var held: CheckedContinuation<CrewdValue, Error>?
        var delay = false
        let store = CrewdWorkStore { method, p in
            precondition(method == "work.patrol"); calls.append(p)
            if delay { return try await withCheckedThrowingContinuation { held = $0 } }
            let scan = p["scan"].string!
            var row: [String:CrewdValue] = ["id":.string("W1"),"crewId":.string(foreign ? "other":"a"),"revision":.string("1"),"unowned":.bool(false),"stopEscalated":.bool(false),"longWaitsTruncated":.bool(false),"longWaits":.array([]),"unclaimed":.bool(false),"needsRuling":.bool(false),"archiveCandidate":.bool(false),"blockedBy":.array([.string("future_reason")])]
            if malformed == 4 {row.removeValue(forKey:"id")}
            if malformed == 5 {row.removeValue(forKey:"unowned")}
            if malformed == 6 {row["unowned"] = .string("false")}
            var page:[String:CrewdValue] = ["scan":.string(scan), (scan == "work" ? "items":scan):.array([.object(row)]),"nextAfter":.null]
            if malformed == 1 {page.removeValue(forKey:"nextAfter")}
            if malformed == 2 {page["nextAfter"] = .bool(false)}
            if malformed == 3 {page["nextAfter"] = .string("")}
            if malformed == 7 {page["nextAfter"] = .string("W1")}
            return .object(page)
        }
        store.selectCrew("a")
        for scan in ["work","entries","channels"] { _ = try await store.patrolPage(scan:scan,after:nil) }
        precondition(calls.allSatisfy{$0["crewId"] == .string("a")})
        for mode in 1...7 {
            malformed = mode
            do { _ = try await store.patrolPage(scan:"work",after:mode == 7 ? "W1":nil); fatalError("malformed accepted") } catch {}
        }
        malformed = 0
        foreign=true
        do { _ = try await store.patrolPage(scan:"work",after:nil); fatalError("foreign accepted") } catch {}
        delay=true
        let task=Task {try await store.patrolPage(scan:"work",after:nil)}
        while held == nil {await Task.yield()}
        store.selectCrew("b")
        held!.resume(returning:.null)
        do { _ = try await task.value; fatalError("stale accepted") } catch is CancellationError {}
        var status: CrewdValue = .object(["crewId":.string("a"),"run":.null,"error":.null])
        var waitStatus = false
        var statusHeld: CheckedContinuation<CrewdValue, Error>?
        let statusStore = CrewdWorkStore { method, params in
            precondition(method == "work.patrolStatus" && params["crewId"] == .string("a"))
            if waitStatus { return try await withCheckedThrowingContinuation {statusHeld = $0} }
            return status
        }
        statusStore.selectCrew("a")
        _ = try await statusStore.patrolStatus()
        status = .object(["crewId":.string("other"),"run":.null,"error":.null])
        do { _ = try await statusStore.patrolStatus(); fatalError("foreign status") } catch {}
        status = .object(["crewId":.string("a"),"run":.object([:]),"error":.null])
        do { _ = try await statusStore.patrolStatus(); fatalError("incomplete run") } catch {}
        status = .object(["crewId":.string("a"),"run":.object(["startedAtMs":.integer(1),"completedAtMs":.null,"truncated":.bool(true),"scannedCounts":.object(["work":.integer(3)]),"counts":.object([:])]),"error":.null])
        _ = try await statusStore.patrolStatus()
        waitStatus = true
        let statusRead = Task {try await statusStore.patrolStatus()}
        while statusHeld == nil { await Task.yield() }
        statusStore.invalidateConnection(); statusHeld!.resume(returning:status)
        do { _ = try await statusRead.value; fatalError("stale status") } catch is CancellationError {}
        print("patrol scoped scans, foreign rejection, stale cancellation passed")
    }
}
