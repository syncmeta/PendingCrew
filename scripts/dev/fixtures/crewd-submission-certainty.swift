import Foundation
@MainActor final class HeldWrite {
    var held:CheckedContinuation<CrewdValue,Error>?
    var calls:[String]=[]
    func call(_ method:String,_ params:CrewdValue) async throws->CrewdValue {
        calls.append(method)
        if method=="todo.create" {return try await withCheckedThrowingContinuation{held=$0}}
        if method=="work.list" {return .object(["items":.array([]),"nextAfter":.null])}
        return .array([])
    }
}
@main struct Test {
    @MainActor static func main() async throws {
        var failed:[String]=[]
        // RED uses the current shared UI predicate. GREEN switches only this classifier.
        func uncertain(_ error:Error)->Bool {CrewdClientError.isUncertainSubmission(error)}
        func check(_ ok:Bool,_ message:String){print((ok ? "PASS: ":"FAIL: ")+message);if !ok{failed.append(message)}}
        let c=CrewdClient()
        do {_ = try await c.call("work.update",.object([:]));check(false,"unconnected write rejected")} catch {check(!uncertain(error),"never connected write is certainly not submitted")}
        let h=HeldWrite(),s=CrewdWorkStore(call:{try await h.call($0,$1)})
        do {_ = try await s.createRequirement(requestID:UUID(),text:"no selected crew");check(false,"empty crew rejected")} catch {check(!uncertain(error),"empty-crew validation is certainly not submitted")}
        s.selectCrew("a")
        let first=Task{try await s.createRequirement(requestID:UUID(),text:"one actual attempt")}
        while h.held==nil{await Task.yield()}
        do {_ = try await s.createRequirement(requestID:UUID(),text:"busy second attempt");check(false,"busy write rejected")} catch {check(!uncertain(error),"local busy rejection is certainly not submitted")}
        check(h.calls.filter{$0=="todo.create"}.count==1,"local busy never calls transport twice")
        h.held!.resume(returning:.object(["requirementId":.string("#42"),"crewId":.string("a"),"text":.string("one actual attempt"),"status":.string("unclaimed"),"closure":.null]));h.held=nil
        _ = try await first.value
        check(!uncertain(CrewdClientError(message:"server conflict",code:-32009)),"structured backend refusal is not unknown")
        try await c.connect(path:CommandLine.arguments[1])
        do {_ = try await c.call("work.update",.object([:]),timeout:.milliseconds(100));check(false,"withheld response timed out")} catch {check(uncertain(error),"actual sent write timeout remains uncertain")}
        c.disconnect()
        if !failed.isEmpty{print("Submission certainty failed \(failed.count) checks");exit(1)}
        print("PASS: deterministic local refusal versus actual sent timeout and no duplicate attempt")
    }
}
