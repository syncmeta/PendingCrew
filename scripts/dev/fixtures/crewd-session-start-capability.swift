import Foundation
@main struct SessionStartCapabilityProbe {
    @MainActor static func main() async throws {
        let client = CrewdClient()
        precondition(!client.supportsSessionStart)
        try await client.connect(path: CommandLine.arguments[1])
        let actual = client.capabilities
        precondition(actual.contains("orchestra"), "normal production hello must advertise orchestra")
        precondition(!actual.contains("session.start"), "probe must expose the real old UI mismatch")
        precondition(client.supportsSessionStart, "real hello must enable the actual shared start gate")
        precondition(!CrewdClient.sessionStartSupported(connected: true, capabilities: actual.subtracting(["orchestra"])))
        precondition(!CrewdClient.sessionStartSupported(connected: true, capabilities: ["session.start"]))
        precondition(!CrewdClient.sessionStartSupported(connected: false, capabilities: actual))
        let record = try await client.call("system.hello", .object(["client": .object(["name": .string("PendingCrew gate probe"), "version": .string("1")]), "apiVersion": .integer(3)]))
        try JSONEncoder().encode(record).write(to: URL(fileURLWithPath:CommandLine.arguments[2]))
        client.disconnect()
        precondition(!client.supportsSessionStart)
        print("PASS actual normal hello orchestra capability -> shared start gate; absent group/false method/disconnected refuse")
    }
}
