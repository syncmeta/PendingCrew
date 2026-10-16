import Foundation
@main struct RetiredManagementRegression {
    @MainActor static func main() async throws {
        let original = CrewdRetiredManagement.Target(database:"db",instance:"original")
        for mode in [CrewdQuitMode.direct, .safePause] {
            var methods = [String](), closed = false
            try await CrewdRetiredManagement.exit(target:original,mode:mode) {
                .init(target:original,call:{ method, params in
                    methods.append(method)
                    switch method {
                    case "lifecycle.status": return .object(["role":.string("observer"),"ownerPresent":.bool(false)])
                    case "lifecycle.claim": return .object(["role":.string("owner"),"ownerPresent":.bool(true)])
                    case "lifecycle.exit":
                        precondition(params["mode"].string == (mode == .direct ? "direct" : "safePause"))
                        return .object(["status":.string("stopped")])
                    default: preconditionFailure("unexpected command")
                    }
                },close:{closed = true})
            }
            precondition(methods == ["lifecycle.status","lifecycle.claim","lifecycle.exit"] && closed)
        }
        var methods = [String]()
        try await CrewdRetiredManagement.exit(target:original,mode:.safePause) {
            .init(target:original,call:{ method,_ in methods.append(method);return .object(["role":.string("observer"),"ownerPresent":.bool(true)]) },close:{})
        }
        precondition(methods == ["lifecycle.status"],"another owner must remain untouched")
        methods = []
        try await CrewdRetiredManagement.exit(target:original,mode:.direct) {
            .init(target:.init(database:"db",instance:"replacement"),call:{ method,_ in methods.append(method);return .null },close:{})
        }
        precondition(methods.isEmpty,"replacement must receive no lifecycle calls")
        var connected = false
        try await CrewdRetiredManagement.exit(target:original,mode:.background) { connected = true;throw NSError(domain:"unexpected",code:0) }
        precondition(!connected)
        for outcome in [CrewdValue.null,.object(["status":.string("timedOut")])] {
            methods = []
            do {
                try await CrewdRetiredManagement.exit(target:original,mode:.safePause) {
                    .init(target:original,call:{ method,_ in
                        methods.append(method)
                        if method == "lifecycle.status", outcome != .null { return .object(["role":.string("owner"),"ownerPresent":.bool(true)]) }
                        return outcome
                    },close:{})
                }
                preconditionFailure("unknown status/timeout reported success")
            } catch {}
            if outcome == .null { precondition(methods == ["lifecycle.status"]) }
        }
        do {
            try await CrewdRetiredManagement.exit(target:original,mode:.direct) { throw NSError(domain:"authentication unknown",code:1) }
            preconditionFailure("failed fresh authentication admitted")
        } catch {}
        print("PASS actual retired-management exit: direct and safe mode reacquire only same vacant instance; existing owner and replacement untouched; background no connection; malformed status/auth failure/timeout never claim success")
    }
}
