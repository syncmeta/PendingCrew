import Foundation

@main struct ManualPatrolCommandFixture {
    static func main() throws {
        let request = UUID()
        let runtime = UUID().uuidString.lowercased()
        let database = "db-patrol"
        let binding: CrewdValue = .object(["dbId": .string(database), "runtimeBindingId": .string(runtime)])
        let params: CrewdValue = .object(["crewId": .string("crew-a")])
        let envelope = try CrewdPublicCommandEnvelope(request: request, binding: binding,
            operation: .patrolStart, params: params)
        var result: [String: CrewdValue] = [
            "crewId": .string("crew-a"), "runId": .string(UUID().uuidString.lowercased()),
            "disposition": .string("joined")
        ]
        func receipt() throws -> CrewdPublicReceipt {
            try CrewdPublicReceipt(.object([
                "requestId": .string(request.uuidString.lowercased()), "dbId": .string(database),
                "runtimeBindingId": .string(runtime), "command": envelope.command, "phase": .string("committed"),
                "historical": .bool(true), "payloadSha256": .string(String(repeating: "a", count: 64)),
                "createdAt": .string("2026-11-04T00:00:00Z"), "updatedAt": .string("2026-11-04T00:00:00Z"),
                "result": .object(result)
            ]), request: request, database: database, originalRuntimeBinding: runtime, originalCommand: envelope.command)
        }
        let historical = try receipt()
        precondition(historical.historical && historical.phase == .committed)
        result["crewId"] = .string("crew-b")
        precondition(refuses(receipt))
        result["crewId"] = .string("crew-a")
        result["disposition"] = .string("completed")
        precondition(refuses(receipt))
    }


    static func refuses(_ receipt: () throws -> CrewdPublicReceipt) -> Bool {
        do { _ = try receipt(); return false } catch { return true }
    }
}
