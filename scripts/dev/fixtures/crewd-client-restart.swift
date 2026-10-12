import Foundation
@main struct RestartProbe {
    @MainActor static func main() async throws {
        func require(_ value:Bool, _ message:String = "restart invariant failed", file:StaticString = #file, line:UInt = #line) { precondition(value, message, file:file, line:line) }
        let root = URL(fileURLWithPath:CommandLine.arguments[1])
        let logPath = CommandLine.arguments[2]
        let phase = CommandLine.arguments[3]
        let statePath = root.deletingLastPathComponent().appendingPathComponent("restart-state.json")
        let workspace = CrewdWorkspace(dataDirectory:root)
        await workspace.connect()
        require(workspace.client.connected)
        let client = workspace.client
        func waitFor(_ condition: () async throws -> Bool) async throws {
            for _ in 0..<200 {
                if try await condition() { return }
                try await Task.sleep(for:.milliseconds(25))
            }
            throw CrewdClientError(message:"restart probe condition timed out")
        }
        func calls(_ method:String) throws -> [CrewdValue] {
            guard FileManager.default.fileExists(atPath:logPath) else { return [] }
            let log = try String(contentsOfFile:logPath, encoding:.utf8)
            let lines = log.split(separator:"\n").dropLast(log.hasSuffix("\n") ? 0 : 1)
            return try lines.map { try JSONDecoder().decode(CrewdValue.self, from:Data($0.utf8)) }
                .filter { $0["method"].string == method }
        }
        if phase == "before" {
            let crew = try await client.call("crew.create", .object(["title":.string("Restart synthetic crew"),"workingDirectory":.string(root.deletingLastPathComponent().appendingPathComponent("work").path)]))
            let active = try await client.call("session.start", .object(["crewId":crew["id"],"role":.string("captain"),"executor":.string("codex"),"brief":.string("RESTART_INITIAL_ACTIVE"),"isolation":.bool(false)]))
            let stopped = try await client.call("session.start", .object(["crewId":crew["id"],"role":.string("worker"),"executor":.string("codex"),"brief":.string("RESTART_INITIAL_STOPPED"),"isolation":.bool(false)]))
            try await waitFor {
                let row = try await client.call("session.get", .object(["sessionId":active["id"]]))
                let count = try calls("turn/start").count
                return row["agentSessionId"].string != nil && count == 2
            }
            _ = try await client.call("session.stop", .object(["sessionId":stopped["id"],"reason":.string("synthetic human stop")]))
            let row = try await client.call("session.get", .object(["sessionId":active["id"]]))
            let state = CrewdValue.object(["active":active["id"],"stopped":stopped["id"],"thread":row["agentSessionId"]])
            try JSONEncoder().encode(state).write(to:statePath)
            client.disconnect()
            print("PASS: two synthetic sessions created, one explicitly stopped")
            return
        }
        let state = try JSONDecoder().decode(CrewdValue.self, from:Data(contentsOf:statePath))
        try await waitFor { workspace.restartPrompt != .null }
        let prompt = workspace.restartPrompt
        require(prompt["sessionIds"].array == [state["active"]], "only active sessions belong in restart prompt")
        require(try calls("turn/start").count == 2, "backend restart must not auto-run")
        let later = await workspace.perform("restart.answer", .object(["id":prompt["id"],"answer":.string("later")]))
        require(later, workspace.error ?? "later failed")
        await workspace.connect()
        require(workspace.restartPrompt["id"] == prompt["id"], "deferred restart must remain visible after reconnect")
        require(try calls("turn/start").count == 2)
        let resume = await workspace.perform("restart.answer", .object(["id":prompt["id"],"answer":.string("resume")]))
        require(resume, workspace.error ?? "resume failed")
        try await waitFor {
            let count = try calls("thread/resume").count
            return workspace.restartPrompt == .null && count == 1
        }
        let resumed = try calls("thread/resume")
        require(resumed[0]["params"]["threadId"] == state["thread"])
        let stoppedLive = try await client.call("session.live", .object(["sessionId":state["stopped"]]))
        require(stoppedLive["running"] == .bool(false), "human-stopped member must remain stopped")
        for _ in 0..<20 {
            require(try calls("thread/resume").count == 1)
            let turns = try calls("turn/start")
            require(turns.filter { ($0["params"]["input"].array.first?["text"].string)?.contains("RESTART_INITIAL_ACTIVE") == true }.count == 1, "initial task must not replay on resume")
            try await Task.sleep(for:.milliseconds(25))
        }
        _ = try await client.call("session.stop", .object(["sessionId":state["active"],"reason":.string("synthetic restart acceptance complete")]))
        client.disconnect()
        print("PASS: restart prompt, later, explicit resume, original thread, no initial-task replay, human stop preserved")
    }
}
