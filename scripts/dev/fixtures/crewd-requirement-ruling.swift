import Foundation

@main struct RequirementRulingFixture {
    @MainActor static func main() async throws {
        typealias Wire = CrewdRequirementRulingWire
        let witness = String(repeating:"a",count:64)
        let closeWitness = String(repeating:"b",count:64)
        let id = CrewdWorkID("#2",kind:.requirement)!
        let scope: [String:CrewdValue] = ["crewId":.string("crew"),"id":.string(id.raw)]
        func changed(_ value: CrewdValue, _ key: String, _ next: CrewdValue) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("fixture object required") }
            fields[key] = next
            return .object(fields)
        }
        func rejects(_ run: () throws -> Void) {
            do { try run(); preconditionFailure("invalid ruling accepted") } catch {}
        }
        func row(_ n: Int) -> CrewdValue {
            .object(["id":.string("W\(n)"),"crewId":.string(n == 33 ? "other" : "crew"),"title":.string("Plan \(n)"),
                "revision":.string("3"),"dropKind":.string("not_needed"),"dropNote":.string("Replaced by later design"),
                "dropNoteTruncated":.bool(false)])
        }
        func page(_ rows: [CrewdValue], next: String? = nil) -> CrewdValue {
            var fields = scope
            fields["state"] = .string("ready"); fields["revision"] = .string(witness)
            fields["linkedWorkCount"] = .integer(33); fields["work"] = .array(rows)
            if let next { fields["nextAfter"] = .string(next) }
            else { fields["closeRevision"] = .string(closeWitness) }
            return .object(fields)
        }
        let first = page((1...32).map(row),next:"W32")
        let last = page([row(33)])
        var staleFields = scope
        staleFields["state"] = .string("stale"); staleFields["linkedWorkCount"] = .integer(33); staleFields["work"] = .array([])
        let stale = CrewdValue.object(staleFields)
        let opened = try Wire.Review(first,crew:"crew",id:id.raw)
        precondition(!opened.complete)
        rejects { _ = try opened.parameters(note:"reason") }
        rejects { _ = try Wire.Review(first,crew:"other",id:id.raw) }
        rejects { _ = try Wire.Review(changed(first,"nextAfter",.string("W31")),crew:"crew",id:id.raw) }
        rejects { _ = try Wire.Review(changed(first,"closeRevision",.string(closeWitness)),crew:"crew",id:id.raw) }
        rejects { _ = try Wire.Review(changed(last,"closeRevision",.string(witness)),crew:"crew",id:id.raw,
                                     after:"W32",expectedRevision:witness) }
        rejects { _ = try Wire.Review(changed(first,"work",.array([row(1),row(1)])),crew:"crew",id:id.raw) }
        rejects { _ = try Wire.Review(page([changed(row(33),"dropNote",.string(String(repeating:"x",count:501)))]),
                                     crew:"crew",id:id.raw,after:"W32",expectedRevision:witness) }
        let continuation = try Wire.Review(last,crew:"crew",id:id.raw,after:"W32",expectedRevision:witness)
        let full = try opened.appending(continuation)
        precondition(full.complete && full.work.last?.crew == "other" && full.work.last?.dropNote == "Replaced by later design")
        // Cursor follows organization book ordering; historical public numbers can differ.
        let alternative = try Wire.Review(page([changed(row(33),"id",.string("W1"))]),crew:"crew",id:id.raw,
                                          after:"W32",expectedRevision:witness)
        rejects { _ = try opened.appending(alternative) }
        let incompleteContinuation = try Wire.Review(changed(last,"linkedWorkCount",.integer(34)),
                                                     crew:"crew",id:id.raw,after:"W32",expectedRevision:witness)
        rejects { _ = try opened.appending(incompleteContinuation) }
        rejects { _ = try Wire.Review(stale,crew:"crew",id:id.raw) }

        let request = UUID()
        let originalText = "\u{0085}Cafe\u{0301}\u{00A0}"
        let params = try full.parameters(note:originalText)
        precondition(params["expectedRevision"] == .string(closeWitness))
        let closure = CrewdValue.object(scope.merging([
            "requestId":.string(request.uuidString.lowercased()),"expectedRevision":.string(closeWitness),
            "note":.string("Cafe\u{0301}"),"closedAtMs":.integer(42),"eventSeq":.string("11"),"by":.string("human")
        ],uniquingKeysWith:{ _,b in b }))
        let result = CrewdValue.object(scope.merging(["receipt":closure],uniquingKeysWith:{ _,b in b }))
        try Wire.validateResult(closure,params:params,request:request)
        rejects { try Wire.validateResult(changed(closure,"note",.string("Café")),params:params,request:request) }
        rejects { try Wire.validateResult(closure,params:params,request:UUID()) }
        rejects { _ = try Wire.result(changed(result,"receipt",.null),crew:"crew",id:id.raw) }
        let captain = changed(changed(closure,"by",.string("captain")),"captain",.string("captain-1"))
        _ = try Wire.result(changed(result,"receipt",captain),crew:"crew",id:id.raw)
        rejects { try Wire.validateResult(captain,params:params,request:request) }

        var mode = "normal"
        var held: CheckedContinuation<CrewdValue,Error>?
        var requests: [String] = []
        let store = CrewdWorkStore { method, input in
            requests.append(method)
            precondition(input["crewId"] == .string("crew") && input["id"] == .string("#2"))
            if mode == "held" { return try await withCheckedThrowingContinuation { held = $0 } }
            switch method {
            case "todo.rulingCheck":
                if input["after"] == .null { return first }
                precondition(input["after"] == .string("W32") && input["expectedRevision"] == .string(witness))
                return mode == "stale" ? stale : last
            case "todo.rulingResult": return result
            default: preconditionFailure("unexpected ruling call")
            }
        }
        store.selectCrew("crew")
        store.configureCapabilities(["todo.rulingCheck"])
        precondition(store.offersRequirementRuling && !store.supportsRequirementRuling)
        do { _ = try await store.requirementRulingCheck(id,crew:"crew"); preconditionFailure("partial capability accepted") } catch {}
        precondition(requests.isEmpty)
        store.configureCapabilities(Set(Wire.methods + ["command.receipts"]))
        let review = try await store.requirementRulingCheck(id,crew:"crew")
        mode = "stale"
        let refreshed = try await store.requirementRulingCheck(id,crew:"crew",continuing:review)
        precondition(refreshed.state == .stale && refreshed.work.isEmpty)
        mode = "normal"
        let complete = try await store.requirementRulingCheck(id,crew:"crew",continuing:review)
        let passive = try await store.requirementRulingResult(id,crew:"crew")
        precondition(complete == full && passive?.requestID == request)
        mode = "held"
        let late = Task { try await store.requirementRulingCheck(id,crew:"crew") }
        while held == nil { await Task.yield() }
        store.selectCrew("other")
        held?.resume(returning:first); held = nil
        do { _ = try await late.value; preconditionFailure("late previous crew read accepted") } catch is CancellationError {}

        let directory = URL(fileURLWithPath:"/private/tmp/pcw-ruling-journal-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:directory) }
        let journalScope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:directory.path,
                                                           authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        let journal = try CrewdPresentationJournal(root:directory,scope:journalScope)
        var binding = CrewdPublicCommands.Binding(scope:journalScope,databaseID:"db",runtimeBindingID:"run-a")
        let envelope = try CrewdPublicCommandEnvelope(request:request,binding:.object(["dbId":.string("db"),
            "runtimeBindingId":.string("run-a")]),operation:.requirementRuleClose,params:params)
        var methods: [String] = []
        var response = "missing"
        func controller() -> CrewdPublicCommands {
            .init(binding:{binding},ownsDaemon:{false},call:{method,input in
                methods.append(method)
                if method == "command.submit" {
                    if response == "staleSubmit" {
                        precondition(input["command"] == envelope.command)
                        throw CrewdClientError(message:"review changed",code:-32009,
                                               data:.object(["refusal":.string("todo.ruling_review_required")]))
                    }
                    precondition(input == envelope.value)
                    throw CrewdClientError(message:"database result unavailable",code:-32009)
                }
                precondition(method == "command.lookup" && input == envelope.lookup)
                if response == "missing" { return .null }
                return .object(["requestId":envelope.value["requestId"],"dbId":.string("db"),"runtimeBindingId":.string("run-a"),
                    "command":envelope.command,"phase":.string("committed"),"historical":.bool(true),"payloadSha256":.string(witness),
                    "createdAt":.string("2026-01-01"),"updatedAt":.string("2026-01-01"),
                    "result":response == "wrong" ? changed(closure,"requestId",.string(UUID().uuidString.lowercased())) : closure])
            },prepare:{ envelope,crew in try journal.prepare(method:"command.submit",params:envelope.value,crew:crew) },
              finish:{ record,state,detail in try journal.finish(record,state:state,detail:detail) })
        }
        let staleSubmit = try CrewdPublicCommandEnvelope(request:UUID(),binding:.object(["dbId":.string("db"),
            "runtimeBindingId":.string("run-a")]),operation:.requirementRuleClose,params:params)
        response = "staleSubmit"
        do { _ = try await controller().submit(staleSubmit,crew:"crew"); preconditionFailure("known refusal accepted") }
        catch { precondition(Wire.requiresFreshReview(error)) }
        let refusedRecords = try journal.unknownRecords()
        precondition(refusedRecords.isEmpty && methods == ["command.submit"])
        methods.removeAll(); response = "missing"
        do { _ = try await controller().submit(envelope,crew:"crew"); preconditionFailure("generic failure looked committed") } catch {}
        let records = try journal.unknownRecords()
        precondition(records.count == 1 && records[0].params == envelope.value)
        binding = .init(scope:journalScope,databaseID:"db",runtimeBindingID:"run-b")
        let reopened = controller()
        try reopened.restore(records)
        let original = reopened.observations[request]!
        let missing = try await reopened.lookup(original)
        precondition(missing == nil)
        response = "wrong"
        do { _ = try await reopened.lookup(original); preconditionFailure("wrong receipt accepted") } catch {}
        response = "valid"
        let recovered = try await reopened.lookup(original)
        precondition(recovered?.historical == true && recovered?.value["result"] == closure)
        precondition(methods == ["command.submit","command.lookup","command.lookup","command.lookup"])
        print("PASS bounded scoped evidence, stale continuation, exact Human/captain receipts and original lookup without replay")
    }
}
