// Calls the actual built App module: no replacement workspace or runner.
@testable import PendingCrew
import Foundation
@main struct OriginalSessionFlow {
    @MainActor static func main() async throws {
        let path = CommandLine.arguments[1], scenario = CommandLine.arguments[2]
        let workspace = CrewdWorkspace(authorityRecordID: UUID(), connect: { _ in })
        try await workspace.client.connect(path: path)
        workspace.crewID = "crew-a"; workspace.sessionID = "session-a"
        defer { workspace.retire() }
        switch scenario {
        case "captain":
            let first = await workspace.startCaptainForCreatedCrew("crew-a")
            precondition(first == "session-a")
            let second = await workspace.startCaptainForCreatedCrew("crew-a")
            precondition(second == first)
        case "captain-refused":
            let result = await workspace.startCaptainForCreatedCrew("crew-a")
            precondition(result == nil && workspace.error?.contains("机组已创建") == true)
        case "switch":
            let task = Task { await workspace.restoreOpenedSession("session-a", crew: "crew-a") }
            try await Task.sleep(for: .milliseconds(50))
            workspace.sessionID = "session-b"
            await task.value
        case "double-click":
            let first = Task { await workspace.restoreOpenedSession("session-a", crew: "crew-a") }
            let second = Task { await workspace.restoreOpenedSession("session-a", crew: "crew-a") }
            await first.value; await second.value
        default:
            await workspace.restoreOpenedSession("session-a", crew: "crew-a")
            if scenario == "malformed" { precondition(workspace.error?.contains("未能确认") == true) }
            if scenario == "stopped" { await workspace.restoreOpenedSession("session-a", crew: "crew-a") }
        }
        print("PASS actual App session flow: " + scenario)
    }
}
