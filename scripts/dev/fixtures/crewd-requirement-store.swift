import Foundation

func requirement(status: String = "unclaimed", text: String = "original", closure: CrewdValue = .null) -> CrewdValue {
    .object(["requirementId":.string("#42"),"id":.string("raw-42"),"crewId":.string("a"),"text":.string(text),"status":.string(status),"closure":closure])
}
func ruling(_ note: String = "deliberate ruling") -> CrewdValue {
    .object(["note":.string(note),"at":.string("2026-10-13T00:00:00.000Z")])
}
@MainActor final class Fixture {
    var list = requirement()
    var detail = requirement()
    var create = requirement(text:"WRONG_IMMUTABLE_ORIGINAL")
    var close = requirement(status:"unclaimed")
    var holdAsk = false
    var heldAsk: CheckedContinuation<CrewdValue, Error>?
    var holdGet = false
    var held: CheckedContinuation<CrewdValue, Error>?
    var calls: [String] = []
    func call(_ method:String,_ params:CrewdValue) async throws -> CrewdValue {
        calls.append(method)
        switch method {
        case "work.list": return .object(["items":.array([]),"nextAfter":.null])
        case "todo.list": return .array([list])
        case "ask.list":
            if holdAsk { holdAsk=false; return try await withCheckedThrowingContinuation { heldAsk=$0 } }
            return .array([])
        case "todo.get":
            if holdGet { holdGet=false; return try await withCheckedThrowingContinuation { held=$0 } }
            return detail
        case "todo.create": return create
        case "todo.close": return close
        default: preconditionFailure("unexpected call \(method)")
        }
    }
}
@main struct Test {
    @MainActor static func main() async throws {
        var failures:[String]=[]
        func verify(_ yes: Bool,_ message:String) { if !yes { failures.append(message); print("FAIL: \(message)") } else {print("PASS: \(message)")} }
        let id=CrewdWorkID("#42",kind:.requirement)!
        // The earlier todo.get holds a provider snapshot while newer list facts commit first.
        for knownClosure in [false,true] {
            let f=Fixture(), s=CrewdWorkStore(call:{try await f.call($0,$1)})
            s.selectCrew("a")
            if knownClosure {
                f.detail=requirement(status:"closed",closure:ruling())
                _ = try await s.requirementDetail(id)
            } else {try await s.refresh()}
            f.holdGet=true
            let earlier=Task {try await s.requirementDetail(id)}
            while f.held==nil {await Task.yield()}
            f.list=requirement(status:"closed") // todo.list intentionally omits full closure.
            try await s.refresh()
            let held=f.held!;f.held=nil
            held.resume(returning:requirement(status:"unclaimed"))
            do {_ = try await earlier.value} catch is CancellationError {} // Rejecting superseded detail is valid.
            verify(s.requirements[id]?["status"] == .string("closed"),"late old detail cannot reopen newer closed requirement (knownClosure=\(knownClosure))")
            if knownClosure {verify(s.requirements[id]?["closure"]["note"] == .string("deliberate ruling"),"late old detail cannot erase immutable closure")}
        }
        // Reverse ordering: an older list is already fetched while a newer detail commits.
        do {
            let f=Fixture(),s=CrewdWorkStore(call:{try await f.call($0,$1)})
            s.selectCrew("a");f.holdAsk=true
            let earlier=Task {try await s.refresh()}
            while f.heldAsk==nil {await Task.yield()}
            f.detail=requirement(status:"closed",closure:ruling())
            _ = try await s.requirementDetail(id)
            let held=f.heldAsk!;f.heldAsk=nil;held.resume(returning:.array([]))
            do {try await earlier.value} catch is CancellationError {}
            verify(s.requirements[id]?["status"] == .string("closed"),"late old list cannot reopen newer detail fact")
            verify(s.requirements[id]?["closure"]["note"] == .string("deliberate ruling"),"late old list cannot erase newer detail closure")
            f.list=requirement(status:"closed")
            try await s.refresh()
            verify(s.requirements[id]?["status"] == .string("closed") && s.requirements[id]?["closure"]["note"] == .string("deliberate ruling"),"fresh refresh still succeeds after superseded list")
        }
        // A right-ID receipt is insufficient evidence that a human ruling committed.
        for (name,bad) in [
            ("nonclosed",requirement(status:"unclaimed")),
            ("missing closure",requirement(status:"closed")),
            ("wrong note",requirement(status:"closed",closure:ruling("different ruling")))
        ] {
            let f=Fixture(),s=CrewdWorkStore(call:{try await f.call($0,$1)})
            s.selectCrew("a");f.close=bad
            var rejected=false
            do {_ = try await s.closeRequirement(id,expectedStatus:"unclaimed",note:"  deliberate ruling  ")} catch {rejected=true}
            verify(rejected,"reject fake close receipt: \(name)")
            verify(f.calls.filter{$0=="todo.close"}.count==1,"fake close is never automatically retried: \(name)")
        }
        let f=Fixture(),s=CrewdWorkStore(call:{try await f.call($0,$1)})
        s.selectCrew("a")
        var rejected=false
        do {_ = try await s.createRequirement(requestID:UUID(),text:"  exact original\n")} catch {rejected=true}
        verify(rejected,"same-crew create receipt must retain exact submitted original bytes")
        verify(f.calls.filter{$0=="todo.create"}.count==1,"invalid create receipt never replays write")
        // The core trims closure reasons; validating against untrimmed UI text would reject valid facts.
        let good=Fixture(),accepted=CrewdWorkStore(call:{try await good.call($0,$1)})
        accepted.selectCrew("a");good.close=requirement(status:"closed",closure:ruling());good.list=requirement(status:"closed")
        do {_ = try await accepted.closeRequirement(id,expectedStatus:"unclaimed",note:"  deliberate ruling  ");verify(accepted.requirements[id]?["closure"]["note"] == .string("deliberate ruling"),"valid trimmed ruling survives list reconciliation")} catch {verify(false,"valid trimmed ruling was rejected")}
        if !failures.isEmpty {print("Requirement store failed \(failures.count) checks");exit(1)}
        print("PASS: requirement delayed-read, receipt proof, exact source bytes and trimmed immutable ruling")
    }
}
