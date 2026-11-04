import Foundation

@main struct RequirementCreateLookupFixture {
    @MainActor static func main() async throws {
        typealias Wire = CrewdRequirementCreateWire
        if CommandLine.arguments.dropFirst().first == "--race" {
            try await raceChild(Array(CommandLine.arguments.dropFirst(2)))
            return
        }
        let directory = URL(fileURLWithPath:"/private/tmp/pcw-todo-create-journal-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:directory) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:directory.path,
            authorityID:"test",databaseID:"db",grantID:"grant",grantRevision:"1")
        let before = try CrewdPresentationJournal(root:directory,scope:scope)
        func params(_ text: String, channel: String? = nil, id: UUID = UUID()) -> CrewdValue {
            var fields: [String:CrewdValue] = ["requestId":.string(id.uuidString.lowercased()),"crewId":.string("crew"),"text":.string(text)]
            if let channel { fields["channelId"] = .string(channel) }
            return .object(fields)
        }
        func changed(_ value: CrewdValue, _ key: String, _ replacement: CrewdValue) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("object required") }
            fields[key] = replacement
            return .object(fields)
        }
        func rejects(_ run: () throws -> Void) {
            do { try run(); preconditionFailure("invalid original accepted") } catch {}
        }
        let crewParams = params("Original crew Cafe\u{0301}")
        let groupParams = params("Original group Cafe\u{0301}",channel:"topic")
        let missingParams = params("Never sent")
        let crew = try Wire.Original(crewParams), group = try Wire.Original(groupParams)
        let missing = try Wire.Original(missingParams)
        let first = try before.prepareRequirementCreate(crewParams,original:crew)
        let second = try before.prepareRequirementCreate(groupParams,original:group)
        _ = try before.prepareRequirementCreate(missingParams,original:missing)
        try before.finish(first,state:"refused",detail:"legacy generic SQL conflict")
        try before.finish(second,state:"unknown")
        rejects { _ = try before.prepareRequirementCreate(crewParams,original:crew) }
        // Older native confirmation could produce a second local journal record.
        let alias = try before.prepare(method:"todo.create",params:changed(crewParams,"requestId",.string(crew.request.uuidString)),crew:"crew")
        try before.finish(alias,state:"unknown")
        let notSubmittedParams = params("Proven local unsent")
        let notSubmitted = try Wire.Original(notSubmittedParams)
        let notSent = try before.prepareRequirementCreate(notSubmittedParams,original:notSubmitted)
        try before.finishRequirementCreate(notSent,notSubmitted:true,detail:"known local cancellation")
        let retried = try before.prepareRequirementCreate(notSubmittedParams,original:notSubmitted)
        try before.finish(retried,state:"replyObserved")
        rejects { _ = try before.prepareRequirementCreate(notSubmittedParams,original:notSubmitted) }

        let reopened = try CrewdPresentationJournal(root:directory,scope:scope)
        let tampered = CrewdPresentationJournal.Prepared(format:first.format,id:first.id,scope:first.scope,method:first.method,
            params:changed(first.params,"text",.string("Original crew Café")),crew:first.crew,preparedAt:first.preparedAt)
        rejects { try reopened.acknowledge(tampered,detail:"must not accept normalized text") }
        let unresolved = try reopened.requirementCreateIntents(in:reopened.unknownRecords())
        precondition(unresolved.count == 3)
        precondition(unresolved.first(where:{ $0.original.request == crew.request })?.records.count == 2)
        let differentGrant = CrewdPresentationJournal.Scope(namespace:scope.namespace,localDirectory:scope.localDirectory,
            authorityID:scope.authorityID,databaseID:scope.databaseID,grantID:"different",grantRevision:"1")
        let changedScope = try CrewdPresentationJournal(root:directory,scope:differentGrant)
        let foreign = try changedScope.requirementCreateIntents(in:changedScope.unknownRecords())
        precondition(foreign.isEmpty)

        func receipt(_ original: Wire.Original, id: String) -> CrewdValue {
            var body: [String:CrewdValue] = ["crewId":.string(original.crew),"requirementId":.string(id),
                "textSha256":.string(original.textSHA),"createdAt":.string("2026-11-05T01:00:00Z")]
            if !original.channel.isEmpty {
                body["sourceChannelId"] = .string(original.channel)
                body["currentAssociation"] = .object(["channelId":.string(original.channel),"linkRevision":.string("2"),"linked":.bool(false)])
            }
            return .object(["requestId":.string(original.requestID),"status":.string("committed"),"receipt":.object(body)])
        }
        let originalValue = receipt(group,id:"#4")
        let wrongHash = try Wire.Original(changed(groupParams,"text",.string("Original group Café"))).textSHA
        precondition(group.textSHA != wrongHash)
        rejects { _ = try Wire.receipt(changed(originalValue,"receipt",changed(originalValue["receipt"],"textSha256",.string(wrongHash))),original:group) }
        rejects { _ = try Wire.receipt(changed(originalValue,"receipt",changed(originalValue["receipt"],"sourceChannelId",.string("other"))),original:group) }
        rejects { _ = try Wire.receipt(changed(originalValue,"receipt",changed(originalValue["receipt"],"requirementId",.string("#04"))),original:group) }
        let actualSubmission: CrewdValue = .object(["id":.string("requirement-request-" + group.requestID),
            "crewId":.string(group.crew),"ledger":.string("agent"),"text":.string(group.text),"requirementId":.string("#4"),
            "createdAt":.string("2026-11-05T01:00:00Z"),"status":.string("unclaimed"),
            "channelAssociation":originalValue["receipt"]["currentAssociation"]])
        try group.validateSubmission(actualSubmission)
        rejects { try group.validateSubmission(changed(actualSubmission,"text",.string("Original group Café"))) }

        var called: [String] = []
        var hold: CheckedContinuation<CrewdValue,Error>?
        var delay = false
        let store = CrewdWorkStore { method, input in
            called.append(method)
            precondition(method == Wire.lookupMethod)
            if delay { return try await withCheckedThrowingContinuation { hold = $0 } }
            if input == crew.lookup { return receipt(crew,id:"#3") }
            if input == group.lookup { return originalValue }
            precondition(input == missing.lookup)
            return .object(["requestId":.string(missing.requestID),"status":.string("notFound")])
        }
        store.selectCrew("crew")
        do { _ = try await store.requirementCreateLookup(crew); preconditionFailure("unsupported lookup accepted") } catch {}
        precondition(called.isEmpty)
        store.configureCapabilities([Wire.lookupMethod])
        let groupIntent = unresolved.first { $0.original.matches(group) }!
        var sheetOpen = true
        do {
            _ = try await reopened.lookupRequirementCreate(groupIntent,stillCurrent:{sheetOpen},read:{ original in
                let receipt = try await store.requirementCreateLookup(original)
                sheetOpen = false
                return receipt
            })
            preconditionFailure("dismissed sheet acknowledged original")
        } catch is CancellationError {}
        let afterDismiss = try reopened.unknownRecords()
        precondition(afterDismiss.contains(where:{ $0.id == second.id }))
        for intent in unresolved {
            let observed = try await reopened.lookupRequirementCreate(intent,stillCurrent:{true},read:store.requirementCreateLookup)
            if observed == nil { precondition(intent.original.request == missing.request) }
            if intent.original.request == group.request { precondition(observed?.currentAssociation?.linked == false) }
        }
        rejects { _ = try reopened.prepareRequirementCreate(crewParams,original:crew) }
        rejects { _ = try reopened.prepareRequirementCreate(groupParams,original:group) }
        let pending = try reopened.requirementCreateIntents(in:reopened.unknownRecords())
        precondition(pending.count == 1 && pending[0].original.matches(missing))
        precondition(called == Array(repeating:Wire.lookupMethod,count:4))

        delay = true
        let late = Task { try await store.requirementCreateLookup(missing) }
        while hold == nil { await Task.yield() }
        store.selectCrew("different")
        hold?.resume(returning:.null); hold = nil
        do { _ = try await late.value; preconditionFailure("old scope read escaped") } catch is CancellationError {}
        try await raceTwoProcesses(directory:directory,scope:scope)
        print("PASS exact original lookups, restart/absence/detach/dismissal, and one prepare across two processes")
    }

    @MainActor private static func raceTwoProcesses(directory: URL, scope: CrewdPresentationJournal.Scope) async throws {
        let request = UUID()
        let executable = URL(fileURLWithPath:CommandLine.arguments[0])
        var children: [(Process,Pipe)] = []
        defer { for (process,_) in children where process.isRunning { process.terminate(); process.waitUntilExit() } }
        for lane in ["a","b"] {
            let process = Process(), pipe = Pipe()
            process.executableURL = executable
            process.arguments = ["--race",directory.path,scope.namespace.uuidString,request.uuidString,lane]
            process.standardOutput = pipe
            try process.run()
            children.append((process,pipe))
        }
        try await waitUntil {
            ["a","b"].allSatisfy { FileManager.default.fileExists(atPath:directory.appendingPathComponent("ready-" + $0).path) }
        }
        try Data().write(to:directory.appendingPathComponent("go"),options:.withoutOverwriting)
        try await waitUntil { children.allSatisfy { !$0.0.isRunning } }
        let outcomes = children.map { process, pipe -> String in
            process.waitUntilExit()
            precondition(process.terminationStatus == 0)
            return String(decoding:pipe.fileHandleForReading.readDataToEndOfFile(),as:UTF8.self).trimmingCharacters(in:.whitespacesAndNewlines)
        }
        precondition(outcomes.sorted() == ["blocked","prepared"])
        let journal = try CrewdPresentationJournal(root:directory,scope:scope)
        let intents = try journal.requirementCreateIntents(in:journal.unknownRecords())
        precondition(intents.first(where:{ $0.original.request == request })?.records.count == 1)
    }

    @MainActor private static func raceChild(_ args: [String]) async throws {
        precondition(args.count == 4 && ["a","b"].contains(args[3]))
        let root = URL(fileURLWithPath:args[0]), namespace = UUID(uuidString:args[1])!, request = UUID(uuidString:args[2])!
        let scope = CrewdPresentationJournal.Scope(namespace:namespace,localDirectory:root.path,
            authorityID:"test",databaseID:"db",grantID:"grant",grantRevision:"1")
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        let params: CrewdValue = .object(["requestId":.string(request.uuidString.lowercased()),"crewId":.string("crew"),"text":.string("Same original in two processes")])
        let original = try CrewdRequirementCreateWire.Original(params)
        try Data().write(to:root.appendingPathComponent("ready-" + args[3]),options:.withoutOverwriting)
        try await waitUntil { FileManager.default.fileExists(atPath:root.appendingPathComponent("go").path) }
        do { _ = try journal.prepareRequirementCreate(params,original:original); print("prepared") }
        catch { print("blocked") }
    }

    @MainActor private static func waitUntil(_ satisfied: () -> Bool) async throws {
        for _ in 0..<400 {
            if satisfied() { return }
            try await Task.sleep(for:.milliseconds(10))
        }
        throw CrewdRequirementCreateWire.invalid()
    }
}
