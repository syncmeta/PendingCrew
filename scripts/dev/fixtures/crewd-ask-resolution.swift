import Foundation

@main struct CrewdAskResolutionFixture {
    @MainActor static func main() async throws {
        let revision = String(repeating:"a",count:64)
        let decomposed = "e\u{301}"
        let rawText = "\u{85} " + decomposed + " \n"
        let originalAsk: CrewdValue = .object([
            "crewId":.string("source"),"ask":.string("H3"),"question":.string("Which branch?"),"askedAt":.string("1970-01-01T00:00:01.000Z"),
            "state":.string("open"),"askedBy":.string("session"),"item":.string("W2"),"messageId":.string("original"),
            "default":.object(["text":.string("hold"),"afterMinutes":.integer(3)])])
        let check: CrewdValue = .object(["ask":originalAsk,"resolutionRevision":.string(revision)])
        func setting(_ value: CrewdValue, _ fields: [String:CrewdValue]) -> CrewdValue {
            guard case .object(var object) = value else { preconditionFailure("fixture requires object") }
            object.merge(fields) { _, new in new }
            return .object(object)
        }
        func rejects(_ action: () throws -> Void) {
            do { try action(); preconditionFailure("invalid Ask data accepted") } catch {}
        }
        let ask = try CrewdAskResolutionWire.Review(check,crew:"source",id:"H3")
        precondition(ask.ask == originalAsk && ask.revision == revision)
        let readonly = try CrewdAskResolutionWire.Review(setting(check,["resolutionRevision":.null]),crew:"source",id:"H3")
        precondition(readonly.revision == nil)
        rejects { _ = try readonly.parameters(.answer,text:"ok") }
        rejects { _ = try CrewdAskResolutionWire.Review(check,crew:"foreign",id:"H3") }
        rejects { _ = try CrewdAskResolutionWire.Review(check,crew:"source",id:"H4") }
        rejects { _ = try CrewdAskResolutionWire.Review(setting(check,["ask":setting(originalAsk,["state":.string("answered")])]),crew:"source",id:"H3") }
        rejects { _ = try ask.parameters(.answer,text:"\u{85}\u{2003}") }
        rejects { _ = try ask.parameters(.answer,text:String(repeating:"é",count:32_769)) }
        let zeroWidth = try ask.parameters(.answer,text:"\u{200b}")
        precondition(zeroWidth["text"] == .string("\u{200b}"))

        func result(_ disposition: CrewdAskResolutionWire.Disposition, text: String) -> CrewdValue {
            let state = disposition == .answer ? "answered" : "withdrawn"
            let field = disposition == .answer ? "answer" : "withdrawReason"
            let resolved = setting(originalAsk,["state":.string(state),"closedAt":.string("1970-01-01T00:00:02.123Z"),field:.string(text)])
            return .object(["crewId":.string("source"),"ask":.string("H3"),"originalResolutionRevision":.string(revision),
                "disposition":.string(disposition.rawValue),"eventSeq":.string("12"),"resolvedAtMs":.integer(2_123),"resolved":resolved])
        }
        let params = try ask.parameters(.answer,text:rawText)
        let withdrawn = try ask.parameters(.withdraw,text:rawText)
        let accepted = result(.answer,text:decomposed)
        precondition(params["text"].string?.utf8.elementsEqual(rawText.utf8) == true)
        try CrewdAskResolutionWire.validateResult(accepted,params:params,review:ask)
        try CrewdAskResolutionWire.validateResult(result(.withdraw,text:decomposed),params:withdrawn,review:ask)
        rejects { try CrewdAskResolutionWire.validateResult(result(.answer,text:"é"),params:params) }
        rejects { try CrewdAskResolutionWire.validateResult(result(.withdraw,text:"é"),params:withdrawn) }
        rejects { try CrewdAskResolutionWire.validateResult(accepted,params:withdrawn) }
        rejects { try CrewdAskResolutionWire.validateResult(setting(accepted,["eventSeq":.string("012")]),params:params) }
        rejects { try CrewdAskResolutionWire.validateResult(setting(accepted,["resolvedAtMs":.integer(2_124)]),params:params) }
        rejects { try CrewdAskResolutionWire.validateResult(setting(accepted,["resolved":setting(accepted["resolved"],["question":.string("wrong")])]),params:params,review:ask) }

        var calls: [String] = []
        var hold = false
        var continuation: CheckedContinuation<CrewdValue,Error>?
        let store = CrewdWorkStore { method,p in
            calls.append(method)
            switch method {
            case "ask.resolveCheck":
                precondition(p == .object(["crewId":.string("source"),"ask":.string("H3")]))
                if hold { return try await withCheckedThrowingContinuation { continuation = $0 } }
                return check
            case "ask.answer":
                precondition(p["ask"] == .string("H3") && p["crewId"] == .string("source"))
                return setting(originalAsk,["state":.string("answered"),"answer":p["text"]])
            case "work.list":
                return .object(["items":.array([]),"nextAfter":.null])
            case "todo.list", "ask.list":
                return .array([])
            default:
                preconditionFailure("unexpected method")
            }
        }
        let id = CrewdWorkID("H3",kind:.human)!
        store.selectCrew("display")
        precondition(!store.offersAskResolution)
        _ = try await store.resolveHuman(id,crew:"source",text:"legacy",withdraw:false)
        store.configureCapabilities(["ask.resolveCheck"])
        precondition(store.offersAskResolution && !store.supportsAskResolution)
        do { _ = try await store.resolveHuman(id,crew:"source",text:"never",withdraw:false); preconditionFailure("legacy bypass") } catch {}
        do { _ = try await store.humanResolutionCheck(id,crew:"source"); preconditionFailure("partial capability accepted") } catch {}
        precondition(calls.filter { $0 == "ask.answer" }.count == 1)
        store.configureCapabilities(["ask.resolveCheck","ask.resolve","command.receipts"])
        let observed = try await store.humanResolutionCheck(id,crew:"source")
        precondition(observed == ask)
        hold = true
        let old = Task { try await store.humanResolutionCheck(id,crew:"source") }
        while continuation == nil { await Task.yield() }
        store.invalidateConnection()
        continuation?.resume(returning:check); continuation = nil
        do { _ = try await old.value; preconditionFailure("old generation accepted") } catch is CancellationError {}

        let dir = URL(fileURLWithPath:"/private/tmp/pcw-ask-resolution-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:dir) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:dir.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        let journal = try CrewdPresentationJournal(root:dir,scope:scope)
        var binding = CrewdPublicCommands.Binding(scope:scope,databaseID:"db",runtimeBindingID:"runtime-a")
        let envelope = try CrewdPublicCommandEnvelope(request:UUID(),binding:.object(["dbId":.string("db"),"runtimeBindingId":.string("runtime-a")]),
            operation:.askResolve,params:params)
        var mode = "missing"
        func commands() -> CrewdPublicCommands {
            .init(binding:{binding},ownsDaemon:{false},call:{method,input in
                calls.append(method)
                if method == "command.submit" {
                    precondition(input == envelope.value)
                    throw CrewdClientError(message:"lost original reply")
                }
                precondition(method == "command.lookup" && input == envelope.lookup)
                if mode == "missing" { return .null }
                return .object(["requestId":envelope.value["requestId"],"dbId":.string("db"),"runtimeBindingId":.string("runtime-a"),
                    "command":envelope.command,"phase":.string("committed"),"historical":.bool(true),"payloadSha256":.string(revision),
                    "createdAt":.string("1970-01-01"),"updatedAt":.string("1970-01-01"),
                    "result":mode == "spoof" ? result(.answer,text:"é") : accepted])
            },prepare:{ envelope,crew in try journal.prepare(method:"command.submit",params:envelope.value,crew:crew) },
              finish:{ record,state,detail in try journal.finish(record,state:state,detail:detail) })
        }
        let initial = commands()
        do { _ = try await initial.submit(envelope,crew:"source"); preconditionFailure("lost reply looked committed") } catch {}
        let unknown = try journal.unknownRecords()
        precondition(unknown.count == 1 && unknown[0].params == envelope.value)
        binding = .init(scope:scope,databaseID:"db",runtimeBindingID:"runtime-b")
        let reopened = commands()
        try reopened.restore(unknown)
        let original = reopened.observations[envelope.requestID]!
        let missing = try await reopened.lookup(original)
        precondition(missing == nil)
        mode = "spoof"
        do { _ = try await reopened.lookup(original); preconditionFailure("spoofed original accepted") } catch {}
        mode = "correct"
        let receipt = try await reopened.lookup(original)
        precondition(receipt?.phase == .committed && receipt?.historical == true && receipt?.requestID == envelope.requestID)
        precondition(calls.filter { $0.hasPrefix("command.") } == ["command.submit","command.lookup","command.lookup","command.lookup"])
        print("PASS exact Ask review, legacy capability fallback, byte-exact answer/withdrawal, and one original UUID across reconnect")
    }
}
