import Foundation
@main struct Probe {
    @MainActor static func main() async throws {
        let root=URL(fileURLWithPath:CommandLine.arguments[1])
        let forbidden = CrewdWorkspace(dataDirectory:root.deletingLastPathComponent().appendingPathComponent("legacy"))
        await forbidden.connect()
        precondition(forbidden.error != nil && !forbidden.client.connected)
        precondition(!FileManager.default.fileExists(atPath:forbidden.dataDirectory.path))
        let workspace=CrewdWorkspace(dataDirectory:root)
        let client=workspace.client
        var events:[String]=[]
        let original=client.onEvent
        client.onEvent={event,data in original?(event,data); events.append(event)}
        await workspace.connect()
        precondition(client.connected)
        let created = await workspace.perform("crew.create",.object(["title":.string("Swift synthetic client"),"workingDirectory":.string(root.deletingLastPathComponent().appendingPathComponent("work").path)]))
        precondition(created)
        precondition(!workspace.crewID.isEmpty && workspace.crewID != "pendingcrew-chief")
        for _ in 0..<100 {
            if !workspace.channelID.isEmpty { break }
            try await Task.sleep(for:.milliseconds(20))
        }
        precondition(!workspace.channelID.isEmpty, "channel.list summaries must provide selectable channels")
        let started = await workspace.perform("session.start",.object(["crewId":.string(workspace.crewID),"role":.string("captain"),"executor":.string("codex"),"brief":.string("SWIFT_NATIVE_REQUEST"),"isolation":.bool(false)]))
        precondition(started)
        precondition(!workspace.sessionID.isEmpty)
        let id=CrewdValue.string(workspace.sessionID)
        func request(_ method:String) async throws -> CrewdValue {
            for _ in 0..<100 {
                let v=try await client.call("session.inspect",.object(["sessionId":id]))
                if v["runnerRequest"]["method"].string==method { return v["runnerRequest"] }
                try await Task.sleep(for:.milliseconds(30))
            }
            throw CrewdClientError(message:"synthetic request missing")
        }
        let approval=try await request("item/commandExecution/requestApproval")
        let approved=try await client.call("session.answer",.object(["sessionId":id,"fingerprint":approval["fingerprint"],"choice":.number(0)]))
        precondition(approved["cleared"] == .bool(true))
        let question=try await request("item/tool/requestUserInput")
        do { _=try await client.call("session.answer",.object(["sessionId":id,"fingerprint":approval["fingerprint"],"choice":.number(0)])); fatalError("stale reply accepted") } catch {}
        let submitted=try await client.call("session.respondRunnerRequest",.object(["sessionId":id,"fingerprint":question["fingerprint"],"response":.object(["answers":.object(["task":.object(["answers":.array([.string("SYNTHETIC_TASK")])])])])]))
        precondition(submitted["cleared"] == .bool(true))
        let channel = CrewdValue.string(workspace.channelID)
        let marker = "SYNTHETIC_CHAT_MESSAGE_" + UUID().uuidString
        let posted = await workspace.perform("message.post", .object(["channelId":channel,"text":.string(marker),"mentions":.array([.object(["kind":.string("session"),"targetId":id])])]))
        precondition(posted)
        let nextApproval = try await request("item/commandExecution/requestApproval")
        precondition(nextApproval["fingerprint"] != approval["fingerprint"])
        _ = try await client.call("session.answer",.object(["sessionId":id,"fingerprint":nextApproval["fingerprint"],"choice":.number(0)]))
        let nextQuestion = try await request("item/tool/requestUserInput")
        _ = try await client.call("session.respondRunnerRequest",.object(["sessionId":id,"fingerprint":nextQuestion["fingerprint"],"response":.object(["answers":.object(["task":.object(["answers":.array([.string("SYNTHETIC_TASK")])])])])]))
        // Keep the runner alive and idle: stopping it first would hide a replayed wake.
        var idle = false
        for _ in 0..<100 {
            let view = try await client.call("session.inspect", .object(["sessionId":id]))
            if view["live"]["running"] == .bool(true), view["live"]["working"] == .bool(false),
               view["screen"].string?.contains("GOT: ") == true,
               view["screen"].string?.contains(marker) == true {
                idle = true; break
            }
            try await Task.sleep(for:.milliseconds(30))
        }
        precondition(idle, "message turn must finish before reconnect")
        func turnStarts() throws -> [CrewdValue] {
            let log = try String(contentsOfFile:CommandLine.arguments[2], encoding:.utf8)
            return try log.split(separator:"\n").map { line in
                try JSONDecoder().decode(CrewdValue.self, from:Data(line.utf8))
            }.filter { $0["method"].string == "turn/start" }
        }
        func assertSingleMessage() async throws {
            let messages = try await client.call("message.list", .object(["channelId":channel]))
            precondition(messages.array.filter { $0["text"].string == marker }.count == 1,
                         "reconnect must not replay the posted message")
        }
        try await assertSingleMessage()
        let turnsBefore = try turnStarts()
        precondition(turnsBefore.count == 2, "probe must exercise the initial and message turns")
        client.disconnect()
        precondition(!client.connected)
        await workspace.connect()
        precondition(client.connected && workspace.error == nil)
        // Observe asynchronous wakes too; a replay starts a turn and blocks on approval.
        for _ in 0..<50 {
            try await assertSingleMessage()
            let turnsAfter = try turnStarts()
            precondition(turnsAfter == turnsBefore, "reconnect must not start another runner turn")
            try await Task.sleep(for:.milliseconds(20))
        }
        print("PASS: reconnect retained one marked message and \(turnsBefore.count) runner turns")
        _=try await client.call("session.stop",.object(["sessionId":id,"reason":.string("synthetic Swift probe complete")]))
        precondition(events.contains("session.changed"))
        FileHandle.standardError.write(Data("stage: hibernate after message and reconnect\n".utf8))
        _=try await client.call("crewd.hibernate")
        client.disconnect()
        print("PASS: native Swift/Unix NDJSON, channels, message wake, events, approval, free input, stale reply, reconnect")
    }
}
