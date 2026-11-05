import Foundation

@main struct CrewdFactualWorkCloseFixture {
    @MainActor static func main() async throws {
        let dir = URL(fileURLWithPath:"/private/tmp/pcw-work-close-fixture-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:dir) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:dir.path,
            authorityID:"authority",databaseID:"db",grantID:nil,grantRevision:nil)
        let binding: CrewdValue = .object(["dbId":.string("db"),"runtimeBindingId":.string("old")])
        let note = " e\u{301} \n", evidence = " proof e\u{301}  "
        let params: CrewdValue = .object(["crewId":.string("crew"),"id":.string("W7"),"expectedRevision":.string("9"),
            "kind":.string("superseded"),"evidence":.object(["kind":.string("text"),"value":.string(evidence)]),"note":.string(note)])
        func envelope() throws -> CrewdPublicCommandEnvelope {
            try .init(request:UUID(),binding:binding,operation:.workCloseWithReceipt,params:params)
        }
        let original = try envelope()
        func withFields(_ value: CrewdValue, _ changes: [String:CrewdValue]) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("fixture expects object") }
            fields.merge(changes) { _, new in new }
            return .object(fields)
        }
        let result: CrewdValue = .object(["crewId":params["crewId"],"id":params["id"],"originalRevision":params["expectedRevision"],
            "resultRevision":.string("10"),"eventSeq":.string("11"),"closedAtMs":.integer(1234),
            "kind":params["kind"],"evidence":params["evidence"],"note":params["note"]])
        try CrewdFactualWorkCloseWire.validateResult(result,params:params)
        for bad in [
            withFields(result,["id":.string("W8")]), withFields(result,["crewId":.string("elsewhere")]),
            withFields(result,["resultRevision":.string("11")]), withFields(result,["eventSeq":.string("011")]),
            withFields(result,["note":.string(" é \n")]), withFields(result,["closedAtMs":.string("1234")]),
            withFields(result,["evidence":withFields(params["evidence"],["value":.string(" proof é  ")])])
        ] {
            do { try CrewdFactualWorkCloseWire.validateResult(bad,params:params); preconditionFailure("wrong receipt accepted") } catch {}
        }
        let journal = try CrewdPresentationJournal(root:dir,scope:scope)
        var calls: [String] = [], reply = "missing"
        var direct: CrewdPublicCommandEnvelope?
        var runtime = "old"
        let connection = UUID()
        var submitError = CrewdClientError(message:"untyped SQL fault",code:-32009)
        func controller(_ journal: CrewdPresentationJournal) -> CrewdPublicCommands {
            .init(binding:{.init(scope:scope,databaseID:"db",runtimeBindingID:runtime,connectionEpoch:connection)},ownsDaemon:{false},
                  call:{ method, input in
                      calls.append(method)
                      let submitted = method == "command.submit"
                      if submitted, direct == nil { throw submitError }
                      let command = submitted ? direct! : original
                      precondition(input == (submitted ? command.value : command.lookup))
                      if !submitted, reply == "missing" { return .null }
                      let echoed = !submitted && reply == "wrong-command"
                          ? withFields(command.command,["params":withFields(params,["note":.string(" é \n")])])
                          : command.command
                      return .object(["requestId":command.value["requestId"],"dbId":.string("db"),"runtimeBindingId":.string("old"),
                          "command":echoed,"phase":.string("committed"),"historical":.bool(!submitted),"payloadSha256":.string(String(repeating:"f",count:64)),
                          "createdAt":.string("date"),"updatedAt":.string("date"),"result":result])
                  },prepare:{ envelope,crew in try journal.prepareWorkClose(method:"command.submit",params:envelope.value,crew:crew) },
                  finish:{ record,state,detail in try journal.finish(record,state:state,detail:detail) })
        }
        let first = controller(journal)
        do { _ = try await first.submit(original,crew:"crew"); preconditionFailure("fault appeared successful") } catch {}
        let records = try journal.unknownRecords()
        precondition(records.count == 1 && records[0].params == original.value)
        let reopenedJournal = try CrewdPresentationJournal(root:dir,scope:scope)
        precondition(!CrewdFactualWorkCloseWire.matchesOriginal(method:records[0].method,params:records[0].params,
            journalCrew:records[0].crew,id:"W7",crew:"restored-different-crew"))
        let reusedParams = withFields(params,["crewId":.string("restored-different-crew")])
        let reusedEnvelope = try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:.workCloseWithReceipt,params:reusedParams)
        let reusedTyped = try reopenedJournal.prepareWorkClose(method:"command.submit",params:reusedEnvelope.value,crew:"restored-different-crew")
        try reopenedJournal.finish(reusedTyped,state:"refused")
        do { _ = try reopenedJournal.prepareWorkClose(method:"work.close",params:.object(["id":params["id"]]),crew:"crew"); preconditionFailure("legacy duplicate") } catch {}
        do { _ = try reopenedJournal.prepareWorkClose(method:"command.submit",params:envelope().value,crew:"crew"); preconditionFailure("second prepare") } catch {}
        runtime = "new"
        let reopened = controller(reopenedJournal)
        try reopened.restore(try reopenedJournal.unknownRecords())
        let observation = reopened.observations[original.requestID]!
        let missing = try await reopened.lookup(observation)
        precondition(missing == nil)
        reply = "wrong-command"
        do { _ = try await reopened.lookup(observation); preconditionFailure("canonically equivalent bytes accepted") } catch {}
        reply = "real"
        let receipt = try await reopened.lookup(observation)
        precondition(receipt?.requestID == original.requestID && receipt?.historical == true && receipt?.phase == .committed)
        precondition(calls == ["command.submit","command.lookup","command.lookup","command.lookup"])
        try reopenedJournal.acknowledge(observation.prepared,detail:"original lookup")
        let unresolved = try reopenedJournal.unknownRecords()
        precondition(unresolved.isEmpty)
        runtime = "old"
        submitError = .init(message:"stale",code:-32009,data:.object(["refusal":.string("work.revision_conflict")]))
        do { _ = try await first.submit(envelope(),crew:"crew"); preconditionFailure("stale command committed") } catch {}
        let afterStale = try reopenedJournal.unknownRecords()
        precondition(afterStale.isEmpty)

        let live = try envelope()
        direct = live
        let directReceipt = try await first.submit(live,crew:"crew")
        precondition(directReceipt.phase == .committed && !directReceipt.historical)
        let awaitingPresentation = try journal.unknownRecords()
        precondition(awaitingPresentation.count == 1 && awaitingPresentation[0].params == live.value)
        try journal.acknowledge(awaitingPresentation[0],detail:"observed in the live view")

        let legacy = try journal.prepareWorkClose(method:"work.close",params:.object(["id":.string("W8")]),crew:"crew")
        try journal.finishLegacyWorkClose(legacy,error:submitError)
        let legacyRefused = try journal.unknownRecords()
        precondition(legacyRefused.isEmpty)
        let oldLegacy = try journal.prepare(method:"work.close",params:.object(["id":.string("W9")]),crew:"crew")
        try journal.finish(oldLegacy,state:"refused",detail:"generic old error")
        let legacyAmbiguous = try journal.unknownRecords()
        precondition(legacyAmbiguous.map(\.id) == [oldLegacy.id])
        for (offset, kind) in ["done", "duplicate", "superseded"].enumerated() {
            let raw: CrewdValue = .object(["id":.string("W" + String(20 + offset)),
                "kind":.string(kind),"evidence":params["evidence"],"note":params["note"],"expectedRevision":.string("9")])
            let closed: CrewdValue = .object(["id":raw["id"],"crewId":.string("crew"),"revision":.string("10"),"sources":.array([]),
                "status":.string(kind == "done" ? "done" : "dropped"),"evidence":kind == "done" ? raw["evidence"] : .null,
                "closure":.object(["kind":raw["kind"],"evidence":raw["evidence"],"note":raw["note"]])])
            let attempt = try journal.prepareWorkClose(method:"work.close",params:raw,crew:"crew")
            for invalid in [
                withFields(closed,["revision":.string("11")]),
                withFields(closed,["crewId":.string("other")]),
                withFields(closed,["closure":withFields(closed["closure"],["note":.string(" é \n")])]),
                withFields(closed,["closure":withFields(closed["closure"],["evidence":withFields(raw["evidence"],["value":.string(" proof é  ")])])])
            ] {
                do {
                    try journal.observeLegacyWorkClose(attempt,result:invalid,expectedCrew:"crew")
                    preconditionFailure("wrong legacy work result acknowledged")
                } catch {}
                let stillPending = try journal.unknownRecords()
                precondition(stillPending.contains(where:{ $0.id == attempt.id }))
            }
            try journal.observeLegacyWorkClose(attempt,result:closed,expectedCrew:"crew")
            let afterActual = try journal.unknownRecords()
            precondition(!afterActual.contains(where:{ $0.id == attempt.id }))

            let uncertain = try journal.prepareWorkClose(method:"work.close",params:raw,crew:"crew")
            do {
                try journal.observeLegacyWorkClose(uncertain,result:withFields(closed,["status":.string("active")]),expectedCrew:"crew")
                preconditionFailure("wrong legacy status acknowledged")
            } catch {
                try journal.finishLegacyWorkClose(uncertain,error:error)
            }
            let pendingAfterFailure = try journal.unknownRecords()
            precondition(pendingAfterFailure.contains(where:{ $0.id == uncertain.id }))
            let reused = try journal.prepareWorkClose(method:"work.close",params:raw,crew:"different-crew")
            precondition(reused.id != uncertain.id)
            precondition(!CrewdFactualWorkCloseWire.matchesOriginal(method:uncertain.method,params:uncertain.params,
                journalCrew:uncertain.crew,id:raw["id"].string!,crew:"different-crew"))
        }

        let groupID = CrewdWorkID("W7",kind:.plan)!
        let workRef: CrewdValue = .object(["kind":.string("work"),"id":.string(groupID.raw)])
        let groupRow: CrewdValue = .object(["object":workRef,"crewId":.string("crew"),"label":.string("Review"),
            "labelTruncated":.bool(false),"status":.string("open")])
        var linkRevision = "1", linked = true
        let group = CrewdChannelWorkStore { method, _ in
            precondition(method == "channel.workList")
            return .object(["channelId":.string("topic"),"linkRevision":.string(linkRevision),"sourceRevision":.string("source"),
                "items":.array(linked ? [groupRow] : [])])
        }
        group.configureCapabilities(["channelWork"])
        group.selectChannel(crew:"crew",channel:"topic")
        try await group.refresh()
        precondition(group.hasCurrentPlanLink(groupID,sourceCrew:"crew"))
        precondition(!group.hasCurrentPlanLink(groupID,sourceCrew:"other"))
        group.handleEvent("work.changed",.null)
        precondition(group.hasCurrentPlanLink(groupID,sourceCrew:"crew"))
        group.handleEvent("channel.workChanged",.object(["channelId":.string("topic"),"linkRevision":.string("2")]))
        precondition(!group.hasCurrentPlanLink(groupID,sourceCrew:"crew"))
        linked = false; linkRevision = "2"
        try await group.refresh()
        precondition(!group.hasCurrentPlanLink(groupID,sourceCrew:"crew"))
        linked = true; linkRevision = "3"
        try await group.refresh()
        precondition(group.hasCurrentPlanLink(groupID,sourceCrew:"crew"))
        group.handleEvent("events.lagged",.null)
        precondition(!group.hasCurrentPlanLink(groupID,sourceCrew:"crew"))

        print("PASS factual Work close exact command/result bytes, one original UUID, absent/spoofed lookup, restart observation, typed precommit refusal, raw result validation, and known group link invalidation")
    }
}
