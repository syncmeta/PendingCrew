import Foundation
@main struct LifecycleEmptyParamsRegression {
    @MainActor static func main() async throws {
        let client = CrewdClient()
        try await client.connect(path:CommandLine.arguments[1])
        var observations = [CrewdValue]()
        for method in ["lifecycle.status","lifecycle.claim","lifecycle.release"] {
            do {
                _ = try await client.call(method, .null)
                preconditionFailure("receiver accepted null for " + method)
            } catch let error as CrewdClientError {
                precondition(error.code == -32602, "real receiver must reject null before any role mutation")
                observations.append(.object(["method":.string(method),"params":.null,"errorCode":.integer(-32602)]))
            }
        }
        for (method, expectedRole) in [("lifecycle.status","observer"),("lifecycle.claim","owner"),("lifecycle.release","observer"),("lifecycle.status","observer")] {
            let value = try await client.call(method, .object([:]))
            precondition(value["role"].string == expectedRole)
            observations.append(.object(["method":.string(method),"params":.object([:]),"result":value]))
        }
        try JSONEncoder().encode(observations).write(to:URL(fileURLWithPath:CommandLine.arguments[2]))
        client.retire()
        print("PASS actual CrewdClient serialization + production receiver: null rejected -32602 for status/claim/release; explicit objects perform status→owner claim→release→observer without execution")
    }
}
