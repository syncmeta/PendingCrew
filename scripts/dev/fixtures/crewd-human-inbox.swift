import Foundation

@main struct CrewdHumanInboxFixture {
    @MainActor static func main() async throws {
        let resolution = String(repeating:"a",count:64)
        let inbox = String(repeating:"b",count:64)
        let next = String(repeating:"c",count:64)
        let ask: CrewdValue = .object(["crewId":.string("original"),"ask":.string("H9"),"state":.string("open"),
            "question":.string("Which path?"),"askedAt":.string("1970-01-01T00:00:01.000Z")])
        func changing(_ value: CrewdValue, _ updates: [String:CrewdValue]) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("fixture requires an object") }
            fields.merge(updates) { _,new in new }
            return .object(fields)
        }
        func rejects(_ action: () throws -> Void) {
            do { try action(); preconditionFailure("invalid inbox data accepted") } catch {}
        }
        let oldCheck: CrewdValue = .object(["ask":ask,"resolutionRevision":.string(resolution)])
        let current: CrewdValue = .object(["revision":.string(inbox),"maxUntilMs":.integer(4_000)])
        let check = changing(oldCheck,["inbox":current])
        let review = try CrewdAskResolutionWire.Review(check,crew:"original",id:"H9")
        let old = try CrewdAskResolutionWire.Review(oldCheck,crew:"original",id:"H9")
        let malformed = try CrewdAskResolutionWire.Review(changing(oldCheck,["inbox":.string("invalid")]),crew:"original",id:"H9")
        precondition(old.inbox == nil && malformed.inbox == nil && review.sameResolution(as:old))
        _ = try old.parameters(.answer,text:"ok")
        _ = try malformed.parameters(.withdraw,text:"reason")
        let movingMax = try CrewdAskResolutionWire.Review(changing(check,["inbox":changing(current,["maxUntilMs":.integer(4_001)])]),crew:"original",id:"H9")
        precondition(movingMax.inbox?.sameState(as:review.inbox!) == true && movingMax.sameResolution(as:review))
        let moved = try CrewdAskResolutionWire.Review(changing(check,["inbox":changing(current,["revision":.string(next),"deferredUntilMs":.integer(3_000)])]),crew:"original",id:"H9")
        precondition(moved.inbox?.sameState(as:review.inbox!) == false)
        rejects { _ = try CrewdAskInboxWire.parameters(old,untilMs:3_000,nowMs:2_000) }
        rejects { _ = try CrewdAskInboxWire.parameters(review,untilMs:2_000,nowMs:2_000) }
        rejects { _ = try CrewdAskInboxWire.parameters(review,untilMs:4_001,nowMs:2_000) }
        rejects { _ = try CrewdAskInboxWire.parameters(review,untilMs:nil,nowMs:2_000) }
        _ = try CrewdAskInboxWire.parameters(moved,untilMs:nil,nowMs:3_000)
        rejects { _ = try CrewdAskInboxWire.parameters(moved,untilMs:3_000,nowMs:2_000) }
        let params = try CrewdAskInboxWire.parameters(review,untilMs:3_000,nowMs:2_000)
        let clear = try CrewdAskInboxWire.parameters(moved,untilMs:nil,nowMs:2_000)
        precondition(clear["untilMs"] == .null && clear["expectedInboxRevision"] == .string(next))
        rejects {
            guard case .object(var incomplete) = clear else { preconditionFailure() }
            incomplete.removeValue(forKey:"untilMs")
            _ = try CrewdAskInboxWire.canonical(.object(incomplete))
        }

        let h9 = CrewdWorkID("H9",kind:.human)!, h10 = CrewdWorkID("H10",kind:.human)!, h11 = CrewdWorkID("H11",kind:.human)!
        let ids = [h9,h10,h11]
        let rows: [CrewdWorkID:CrewdValue] = [
            h9:.object(["ask":ask,"deferredUntilMs":.integer(3_000)]),
            h10:.object(["ask":changing(ask,["ask":.string("H10")])]),
            h11:.object(["ask":changing(ask,["ask":.string("H11"),"state":.string("answered")]),"deferredUntilMs":.integer(3_001)])
        ]
        let before = CrewdHumanWaitPresentation.inbox(ids,rows:rows)
        var settled = rows
        settled[h9] = .object(["ask":ask])
        let due = CrewdHumanWaitPresentation.inbox(ids,rows:settled)
        precondition(before.active == [h10,h11] && before.later == [h9] && before.activeOpenCount == 1)
        precondition(due.active == ids && due.later.isEmpty && due.activeOpenCount == 2)
        let invalidHint = changing(rows[h9]!,["deferredUntilMs":.string("3000")])
        precondition(CrewdHumanWaitPresentation.deferredUntil(invalidHint) == nil)

        let store = CrewdWorkStore { method,value in
            precondition(method == "ask.resolveCheck" && value["crewId"] == .string("original") && value["ask"] == .string("H9"))
            return check
        }
        store.selectCrew("shown")
        store.configureCapabilities(["ask.resolve","ask.resolveCheck","command.receipts"])
        precondition(store.supportsAskResolution && !store.supportsAskInboxSchedule)
        store.configureCapabilities(["ask.resolve","ask.resolveCheck","command.receipts","ask.inboxSchedule"])
        let decoded = try await store.humanResolutionCheck(h9,crew:"original")
        precondition(store.supportsAskInboxSchedule && decoded == review)

        let accepted: CrewdValue = .object(["crewId":.string("original"),"ask":.string("H9"),
            "originalInboxRevision":.string(inbox),"inboxRevision":.string(next),"acceptedAtMs":.integer(2_001),"deferredUntilMs":.integer(3_000)])
        try CrewdAskInboxWire.validateResult(accepted,params:params)
        rejects { try CrewdAskInboxWire.validateResult(changing(accepted,["crewId":.string("shown")]),params:params) }
        rejects { try CrewdAskInboxWire.validateResult(changing(accepted,["deferredUntilMs":.integer(3_001)]),params:params) }
        rejects { try CrewdAskInboxWire.validateResult(changing(accepted,["inboxRevision":.string(inbox)]),params:params) }
        rejects { try CrewdAskInboxWire.validateResult(changing(accepted,["acceptedAtMs":.integer(3_000)]),params:params) }
        let clearReceipt: CrewdValue = .object(["crewId":.string("original"),"ask":.string("H9"),
            "originalInboxRevision":.string(next),"inboxRevision":.string(String(repeating:"d",count:64)),"acceptedAtMs":.integer(2_002)])
        try CrewdAskInboxWire.validateResult(clearReceipt,params:clear)
        rejects { try CrewdAskInboxWire.validateResult(changing(clearReceipt,["deferredUntilMs":.integer(3_000)]),params:clear) }

        let dir = URL(fileURLWithPath:"/private/tmp/pcw-human-inbox-fixture-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:dir) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:dir.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        let journal = try CrewdPresentationJournal(root:dir,scope:scope)
        var binding = CrewdPublicCommands.Binding(scope:scope,databaseID:"db",runtimeBindingID:"runtime-a")
        let command = try CrewdPublicCommandEnvelope(request:UUID(),binding:.object(["dbId":.string("db"),"runtimeBindingId":.string("runtime-a")]),
            operation:.askInboxSchedule,params:params)
        var calls: [String] = []
        var mode = "absent"
        func commands() -> CrewdPublicCommands {
            .init(binding:{binding},ownsDaemon:{false},call:{method,input in
                calls.append(method)
                if method == "command.submit" {
                    precondition(input == command.value)
                    throw CrewdClientError(message:"lost original reply")
                }
                precondition(method == "command.lookup" && input == command.lookup)
                if mode == "absent" { return .null }
                return .object(["requestId":command.value["requestId"],"dbId":.string("db"),"runtimeBindingId":.string("runtime-a"),
                    "command":command.command,"phase":.string("committed"),"historical":.bool(true),"payloadSha256":.string(resolution),
                    "createdAt":.string("1970-01-01"),"updatedAt":.string("1970-01-01"),
                    "result":mode == "spoofed" ? changing(accepted,["ask":.string("H10")]) : accepted])
            },prepare:{ proposed,crew in try journal.prepare(method:"command.submit",params:proposed.value,crew:crew) },
              finish:{ record,state,detail in try journal.finish(record,state:state,detail:detail) })
        }
        do { _ = try await commands().submit(command,crew:"original"); preconditionFailure("lost reply committed locally") } catch {}
        let originals = try journal.unknownRecords()
        precondition(originals.count == 1 && originals[0].params == command.value)
        binding = .init(scope:scope,databaseID:"db",runtimeBindingID:"runtime-b")
        let restarted = commands()
        try restarted.restore(originals)
        let observation = restarted.observations[command.requestID]!
        let absent = try await restarted.lookup(observation)
        precondition(absent == nil)
        mode = "spoofed"
        do { _ = try await restarted.lookup(observation); preconditionFailure("wrong original Ask accepted") } catch {}
        mode = "exact"
        let recovered = try await restarted.lookup(observation)
        precondition(recovered?.phase == .committed && recovered?.historical == true && recovered?.requestID == command.requestID)
        try journal.acknowledge(originals[0],detail:"scoped receipt checked")
        let remaining = try journal.unknownRecords()
        precondition(remaining.isEmpty)
        precondition(calls == ["command.submit","command.lookup","command.lookup","command.lookup"])
        print("PASS original inbox order/count/due, legacy Ask, exact schedule/clear DTO, lost receipt across reconnect without replay")
    }
}
