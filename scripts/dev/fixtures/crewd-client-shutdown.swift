import Foundation
@main struct ShutdownProbe {
    @MainActor static func main() async throws {
        let workspace = CrewdWorkspace(dataDirectory:URL(fileURLWithPath:CommandLine.arguments[1]))
        await workspace.connect()
        precondition(workspace.client.connected)
        precondition(workspace.status == "后台已休眠")
        workspace.client.disconnect()
        print("PASS: client received the in-flight response before closing")
    }
}
