import Foundation

@main struct FailureProbe {
    @MainActor static func main() async {
        do { try await run() }
        catch {
            FileHandle.standardError.write(Data(("FAIL: " + String(describing: error) + "\n").utf8))
            exit(1)
        }
    }
    @MainActor static func run() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let mode = CommandLine.arguments[2]
        let workspace = CrewdWorkspace(dataDirectory: root)
        func require(_ ok: Bool, _ message: String) throws {
            if !ok { throw ProbeError.failed(message) }
        }
        await workspace.connect()
        if mode == "startup" {
            try require(!workspace.client.connected && workspace.error != nil, "startup failure must be visible")
            try require(workspace.status == "连接失败", "startup failure status")
            print("PASS: real workspace exposes backend startup failure")
            return
        }
        if mode == "incompatible" {
            try require(!workspace.client.connected, "incompatible hello accepted")
            try require(workspace.error?.contains("不兼容") == true, "protocol incompatibility was replaced: \(workspace.error ?? "nil")")
            print("PASS: real workspace exposes incompatible protocol")
            return
        }
        try require(workspace.client.connected && workspace.error == nil, "initial connection failed")
        let sent = await workspace.perform("message.post", .object(["channelId": .string("synthetic-channel"), "text": .string("DO_NOT_REPLAY")]))
        try require(!sent && workspace.error != nil, "lost mutation result must be unknown, never a success")
        // Real backoff: 1+2+4+8+8 seconds, with up to five seconds per connection.
        // No clock or retry-count seam and no shorter policy is imposed by this test.
        let end = Date().addingTimeInterval(55)
        while Date() < end && workspace.status != "连接已中断" {
            try await Task.sleep(for: .milliseconds(100))
        }
        try require(workspace.status == "连接已中断" && !workspace.client.connected && workspace.error != nil,
                    "retry budget did not end: \(workspace.status); \(workspace.error ?? "nil")")
        try Data("ready".utf8).write(to: root.appendingPathComponent("explicit-reconnect"))
        while !FileManager.default.fileExists(atPath: root.appendingPathComponent("peer-ready").path) {
            try await Task.sleep(for: .milliseconds(20))
        }
        await workspace.connect()
        try require(workspace.client.connected && workspace.error == nil && workspace.status.hasPrefix("已连接"), "explicit reconnect failed")
        try Data("drop".utf8).write(to: root.appendingPathComponent("second-drop"))
        let dropEnd = Date().addingTimeInterval(5)
        while workspace.client.connected && Date() < dropEnd {
            try await Task.sleep(for: .milliseconds(20))
        }
        try require(!workspace.client.connected && workspace.status.contains("重新连接"), "second disconnect did not start a fresh retry")
        try Data("ready".utf8).write(to: root.appendingPathComponent("second-reconnect"))
        let recoveryEnd = Date().addingTimeInterval(12)
        while (!workspace.client.connected || workspace.error != nil) && Date() < recoveryEnd {
            try await Task.sleep(for: .milliseconds(50))
        }
        try require(workspace.client.connected && workspace.error == nil, "fresh retry did not recover after explicit reconnect: connected=\(workspace.client.connected), status=\(workspace.status), error=\(workspace.error ?? "nil")")
        workspace.client.disconnect()
        print("PASS: retries exhausted visibly; explicit reconnect recovered; second disconnect started fresh automatic recovery; peer verifies no mutation replay")
    }
    enum ProbeError: Error { case failed(String) }
}
