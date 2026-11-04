import Foundation

@main struct WorkDuplicateReviewFixture {
    @MainActor static func main() async throws {
        let digest = String(repeating:"a",count:64)
        let candidate: CrewdValue = .object([
            "duplicateId":.string("W11"),"duplicateRevision":.string("1"),"survivorId":.string("W9"),
            "survivorRevision":.string("3"),"sourceIds":.array([.string("#2"),.string("#12")]),"witness":.string(digest)])
        let typed = try CrewdWorkDuplicateWire.Candidate(candidate)
        let params = typed.parameters(crew:"crew",evidenceKind:"text",evidence:"source #2 + #12",note:"reviewed both definitions")
        let evidence = params["evidence"], note = params["note"]
        let closure: CrewdValue = .object([
            "crewId":.string("crew"),"duplicateId":.string("W11"),"previousDuplicateRevision":.string("1"),
            "duplicateRevision":.string("2"),"survivorId":.string("W9"),"survivorRevision":.string("3"),
            "eventSeq":.string("44"),"closedAtMs":.integer(88),"evidence":evidence,"note":note])
        let event: CrewdValue = .object([
            "seq":.string("44"),"at":.string("2026-01-01T00:00:00Z"),"actor":.object(["kind":.string("human")]),
            "closure":.object(["kind":.string("duplicate"),"note":note,"evidence":evidence]),"duplicateReview":closure])
        func replacing(_ value: CrewdValue, _ field: String, _ next: CrewdValue) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("test object required") }
            fields[field] = next
            return .object(fields)
        }
        func rejects(_ action: () throws -> Void) {
            do { try action(); preconditionFailure("malformed duplicate review accepted") } catch {}
        }
        let page: CrewdValue = .object(["crewId":.string("crew"),"items":.array([candidate]),"nextAfter":.string(digest+":11")])
        _ = try CrewdWorkDuplicateWire.Page(page,crew:"crew",after:digest+":10",limit:1)
        rejects { _ = try CrewdWorkDuplicateWire.Page(page,crew:"foreign",after:nil,limit:1) }
        rejects { _ = try CrewdWorkDuplicateWire.Page(page,crew:"crew",after:digest+":11",limit:1) }
        rejects { _ = try CrewdWorkDuplicateWire.Page(page,crew:"crew",after:nil,limit:2) }
        rejects { _ = try CrewdWorkDuplicateWire.Candidate(replacing(candidate,"sourceIds",.array([.string("#2"),.string("#2")]))) }
        rejects { _ = try CrewdWorkDuplicateWire.Candidate(replacing(candidate,"survivorId",.string("W12"))) }
        rejects { _ = try CrewdWorkDuplicateWire.canonical(replacing(params,"note",.string("  "))) }
        rejects { _ = try CrewdWorkDuplicateWire.canonical(replacing(params,"force",.bool(true))) }
        try CrewdWorkDuplicateWire.validateResult(closure,params:params)
        rejects { try CrewdWorkDuplicateWire.validateResult(replacing(closure,"survivorId",.string("W8")),params:params) }
        rejects { try CrewdWorkDuplicateWire.validateResult(replacing(closure,"survivorRevision",.string("2")),params:params) }
        rejects { try CrewdWorkDuplicateWire.validateResult(replacing(closure,"duplicateRevision",.string("3")),params:params) }
        rejects { try CrewdWorkDuplicateWire.validateHistory(event,crew:"foreign",id:"W11") }
        rejects { try CrewdWorkDuplicateWire.validateHistory(event,crew:"crew",id:"W8") }
        rejects { try CrewdWorkDuplicateWire.validateHistory(replacing(event,"actor",.object(["kind":.string("system")])),crew:"crew",id:"W11") }
        rejects { try CrewdWorkDuplicateWire.validateHistory(replacing(event,"seq",.string("45")),crew:"crew",id:"W11") }

        var lookupMethods: [String] = []
        var workReadMode = "current"
        var hold: CheckedContinuation<CrewdValue,Error>?
        let workStore = CrewdWorkStore { method, input in
            lookupMethods.append(method)
            switch method {
            case "adminCase.workDuplicates":
                precondition(input["crewId"] == .string("crew") && input["limit"] == .integer(20))
                return replacing(page,"nextAfter",.null)
            case "work.get":
                if workReadMode == "hold" { return try await withCheckedThrowingContinuation { hold = $0 } }
                let id = input["id"].string!, isDuplicate = id == "W11"
                return .object(["id":.string(id),"crewId":.string(workReadMode == "foreign" && !isDuplicate ? "foreign" : "crew"),
                    "revision":.string(isDuplicate ? "1" : (workReadMode == "stale" ? "4" : "3")),"status":.string(isDuplicate ? "open" : "active"),
                    "owner":isDuplicate ? .null : .string("worker"),"frozen":.bool(false),
                    "title":.string("same"),"goal":.string("same goal"),"level":.string("task"),"acceptance":.string("same acceptance"),
                    "sources":candidate["sourceIds"]])
            case "adminCase.workDuplicateHistory":
                precondition(input["crewId"] == .string("crew") && input["id"] == .string("W11"))
                return .object(["items":.array([event]),"nextBeforeSeq":.null])
            case "work.closureHistory":
                precondition(input["crewId"] == .null && input["id"] == .string("W11"))
                return .object(["items":.array([replacing(event,"duplicateReview",.null)]),"nextBeforeSeq":.null])
            default: preconditionFailure("unexpected WorkStore request")
            }
        }
        workStore.selectCrew("crew")
        workStore.configureCapabilities(["adminCase.workDuplicates","adminCase.workDuplicateHistory"])
        let rows = try await workStore.duplicateWorkPage()
        precondition(rows.items == [typed] && rows.next == nil)
        let inspection = try await workStore.inspectDuplicateWork(typed)
        precondition(inspection.duplicate.id.raw == "W11" && inspection.survivor.id.raw == "W9")
        for mode in ["foreign","stale"] {
            workReadMode = mode
            do { _ = try await workStore.inspectDuplicateWork(typed); preconditionFailure("invalid Work facts accepted") } catch {}
        }
        let id = CrewdWorkID("W11",kind:.plan)!
        let actualHistory = try await workStore.closureHistory(id,duplicateReviewCrew:"crew")
        let ordinaryHistory = try await workStore.closureHistory(id,duplicateReviewCrew:"foreign")
        precondition(actualHistory["items"].array == [event] && ordinaryHistory["items"].array.first?["duplicateReview"] == .null)
        workReadMode = "hold"
        let late = Task { try await workStore.inspectDuplicateWork(typed) }
        while hold == nil { await Task.yield() }
        workStore.selectCrew("other")
        hold?.resume(returning:.null); hold = nil
        do { _ = try await late.value; preconditionFailure("late old crew adopted") } catch is CancellationError {}

        let directory = URL(fileURLWithPath:"/private/tmp/pcw-work-duplicate-fixture-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:directory) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:directory.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        let journal = try CrewdPresentationJournal(root:directory,scope:scope)
        var binding = CrewdPublicCommands.Binding(scope:scope,databaseID:"db",runtimeBindingID:"runtime-a")
        let original = try CrewdPublicCommandEnvelope(request:UUID(),binding:.object(["dbId":.string("db"),"runtimeBindingId":.string("runtime-a")]),operation:.workDuplicateClose,params:params)
        var commandMethods: [String] = []
        var lookupMode = "missing"
        func controller() -> CrewdPublicCommands {
            .init(binding:{binding},ownsDaemon:{false},call:{method,input in
                commandMethods.append(method)
                if method == "command.submit" {
                    precondition(input == original.value)
                    throw CrewdClientError(message:"transport lost")
                }
                precondition(method == "command.lookup" && input == original.lookup)
                if lookupMode == "missing" { return .null }
                return .object(["requestId":original.value["requestId"],"dbId":.string("db"),"runtimeBindingId":.string("runtime-a"),
                    "command":original.command,"phase":.string("committed"),"historical":.bool(true),"payloadSha256":.string(digest),
                    "createdAt":.string("2026-01-01"),"updatedAt":.string("2026-01-01"),
                    "result":lookupMode == "wrong" ? replacing(closure,"survivorId",.string("W8")) : closure])
            },prepare:{ envelope,crew in try journal.prepare(method:"command.submit",params:envelope.value,crew:crew) },
              finish:{ record,state,detail in try journal.finish(record,state:state,detail:detail) })
        }
        let prior = controller()
        do { _ = try await prior.submit(original,crew:"crew"); preconditionFailure("transport loss looked committed") } catch {}
        let records = try journal.unknownRecords()
        precondition(records.count == 1 && records[0].params == original.value)
        binding = .init(scope:scope,databaseID:"db",runtimeBindingID:"runtime-b")
        let reopened = controller()
        try reopened.restore(records)
        let observation = reopened.observations[original.requestID]!
        let missing = try await reopened.lookup(observation)
        precondition(missing == nil && commandMethods == ["command.submit","command.lookup"])
        lookupMode = "wrong"
        do { _ = try await reopened.lookup(observation); preconditionFailure("wrong survivor receipt accepted") } catch {}
        lookupMode = "correct"
        let settled = try await reopened.lookup(observation)
        precondition(settled?.historical == true && settled?.phase == .committed && settled?.value["result"] == closure)
        precondition(commandMethods == ["command.submit","command.lookup","command.lookup","command.lookup"])
        precondition(lookupMethods.filter { $0 == "adminCase.workDuplicateHistory" }.count == 1)
        print("PASS native exact duplicate review evidence, scoped reads, Human history, and original UUID only after reconnect")
    }
}
