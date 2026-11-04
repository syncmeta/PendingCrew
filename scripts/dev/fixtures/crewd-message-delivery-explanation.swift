import Foundation

@main struct MessageDeliveryExplanationFixture {
    @MainActor static func main() async throws {
        typealias Explanation = CrewdMessageDeliveryExplanation
        let source = Explanation.Source(crew:"crew-a",channel:"channel-a",message:"message-a")
        func row(session: String?, state: String, priority: String = "auto", path: String = "live", reasons: [String]) -> CrewdValue {
            var fields: [String:CrewdValue] = ["state":.string(state),"priority":.string(priority),
                "path":.string(path),"reasons":.array(reasons.map(CrewdValue.string))]
            if let session { fields["sessionId"] = .string(session) }
            return .object(fields)
        }
        func response(_ routing: String, _ rows: [CrewdValue], truncated: Bool = false) -> CrewdValue {
            .object(["crewId":.string(source.crew),"channelId":.string(source.channel),"messageId":.string(source.message),
                     "routing":.string(routing),"items":.array(rows),"truncated":.bool(truncated)])
        }
        func changed(_ value: CrewdValue, _ key: String, _ replacement: CrewdValue) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("expected object") }
            fields[key] = replacement
            return .object(fields)
        }
        func rejects(_ value: CrewdValue) {
            do { _ = try Explanation(value,source:source); preconditionFailure("invalid delivery evidence accepted") }
            catch {}
        }

        let unknown = try Explanation(response("unknown",[]),source:source)
        let noTarget = try Explanation(response("no_eligible_target",[]),source:source)
        precondition(unknown.routing == .unknown && noTarget.routing == .noEligibleTarget)
        let queued = row(session:"member-a",state:"queued",priority:"human",reasons:["explicit_mention","reply_to_sender"])
        let delivering = row(session:"member-b",state:"delivering",path:"jev",reasons:["jev_selected"])
        let delivered = row(session:"member-a",state:"delivered_or_already_read",path:"backlog",reasons:["dm_peer"])
        let dropped = row(session:"member-c",state:"dropped",priority:"system",reasons:["channel_owner_fallback"])
        let redacted = row(session:nil,state:"unknown",reasons:["subscription"])
        let lostRow = row(session:"member-d",state:"unknown",reasons:["directed_unknown"])
        let value = response("targets_recorded",[queued,delivering,delivered,dropped,redacted,lostRow])
        let checked = try Explanation(value,source:source)
        precondition(checked.items.map(\.state) == [.queued,.delivering,.deliveredOrAlreadyRead,.dropped,.unknown,.unknown])
        precondition(checked.items[0].reasons == [.explicitMention,.replyToSender])
        precondition(checked.items[1].path == .jev && checked.items[3].priority == .system)
        precondition(checked.items[0].sessionID == checked.items[2].sessionID && checked.items[4].sessionID == nil)
        precondition(checked.items[5].sessionID == "member-d")
        let truncated = try Explanation(response("targets_recorded",Array(repeating:redacted,count:64),truncated:true),source:source)
        precondition(truncated.truncated && truncated.items.count == 64)

        rejects(response("no_eligible_target",[queued]))
        rejects(response("unknown",[queued]))
        rejects(response("unknown",[],truncated:true))
        rejects(response("targets_recorded",[]))
        rejects(response("targets_recorded",[queued],truncated:true))
        rejects(response("targets_recorded",Array(repeating:queued,count:65)))
        rejects(response("targets_recorded",[row(session:nil,state:"delivering",reasons:["subscription"])]))
        rejects(response("targets_recorded",[row(session:"member-a",state:"queued",reasons:["subscription","subscription"])]))
        rejects(response("targets_recorded",[changed(queued,"reasons",.array([]))]))
        rejects(response("targets_recorded",[changed(queued,"state",.string("member_replied"))]))
        rejects(changed(value,"crewId",.string("wrong-crew")))
        rejects(changed(value,"channelId",.string("wrong-channel")))
        rejects(changed(value,"messageId",.string("wrong-message")))
        rejects(changed(value,"truncated",.integer(0)))

        var calls = 0
        var hold = false
        var responseWaiter: CheckedContinuation<CrewdValue,Error>?
        let store = CrewdWorkStore { method, params in
            precondition(method == Explanation.method)
            precondition(params == .object(["crewId":.string(source.crew),"channelId":.string(source.channel),"messageId":.string(source.message)]))
            calls += 1
            if hold { return try await withCheckedThrowingContinuation { responseWaiter = $0 } }
            return value
        }
        store.selectCrew(source.crew)
        do {
            _ = try await store.messageDeliveryExplanation(source:source)
            preconditionFailure("old Core must not receive unsupported request")
        } catch let error as CrewdClientError { precondition(error.kind == .notSubmitted) }
        precondition(calls == 0)
        store.configureCapabilities([Explanation.method])
        let read = try await store.messageDeliveryExplanation(source:source)
        precondition(read == checked && calls == 1)
        hold = true
        let stale = Task { @MainActor in try await store.messageDeliveryExplanation(source:source) }
        while responseWaiter == nil { await Task.yield() }
        store.invalidateConnection()
        responseWaiter?.resume(returning:value)
        do { _ = try await stale.value; preconditionFailure("old connection result reached new presentation") }
        catch is CancellationError {}
        precondition(calls == 2)
        print("PASS exact delivery source, retained states and causes, explicit unknown vs no target, old capability and stale connection")
    }
}
