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
        _=try await client.call("session.stop",.object(["sessionId":id,"reason":.string("synthetic Swift probe complete")]))
        precondition(events.contains("session.changed"))
        // A reconnect only resubscribes and reads. It never repeats the approval.
        try await client.connect(path:root.appendingPathComponent("crewd.sock").path)
        _=try await client.call("crew.list")
        _=try await client.call("crewd.hibernate")
        client.disconnect()
        print("PASS: native Swift/Unix NDJSON, events, approval, free input, stale reply, reconnect")
    }
}
