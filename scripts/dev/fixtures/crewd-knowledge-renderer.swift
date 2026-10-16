import Foundation

@MainActor final class Authority: CrewdKnowledgeViewAuthority {
    var origin = CrewdKnowledgeViewOrigin(authority:"authority",database:"db",restoreEpoch:"epoch",crew:"crew",namespace:"namespace",generation:"1",permissionEpoch:"grant1")
    var continuation: CheckedContinuation<CrewdValue, Error>?
    func currentOrigin() -> CrewdKnowledgeViewOrigin? { origin }
    func hasUnresolvedWrite(in origin: CrewdKnowledgeViewOrigin) -> Bool { false }
    func read(context: CrewdKnowledgeViewContext, input: CrewdValue) async throws -> CrewdValue { try await withCheckedThrowingContinuation { continuation = $0 } }
    func prepare(_ intent: CrewdKnowledgeViewWrite) throws {}
    func submit(_ intent: CrewdKnowledgeViewWrite) async throws -> CrewdKnowledgeViewWriteObservation { .unknown }
    func lookup(_ intent: CrewdKnowledgeViewWrite) async throws -> CrewdKnowledgeViewWriteObservation { .unknown }
    func navigate(context: CrewdKnowledgeViewContext, reference: CrewdValue) async throws {}
}
@main struct Regression {
    @MainActor static func main() async throws {
        let authority = Authority()
        // Retain the real authority used by the runtime; no model/process is involved.
        let r = CrewdKnowledgeViewRuntime(authority: authority)
        let c = CrewdDataContext(dbId:"db",authorityId:"authority",restoreEpoch:"epoch",crewId:"crew",namespace:"namespace",generation:"1")
        func bundle(_ revision: String) -> CrewdKnowledgeViewBundle { .init(id:"page",revision:revision,html:"<main>saved</main>",css:"",javascript:"",definitionDigest:revision,assets:[:],declaresKnowledgeWrite:true,readOnlySnapshot:false) }
        try r.activate(bundle("1"),origin:authority.origin,readContext:c,readGeneration:"1")
        precondition(!r.executionEnabled && !r.writeEnabled)
        let old = r.context!
        try r.setExecutionEnabledFromNativeHost(true,context:old); r.renderReady()
        precondition(r.executionEnabled && !r.writeEnabled)
        try r.setWriteEnabledFromNativeHost(true)
        let task = Task { @MainActor in
            try await r.handle(.init(callID:UUID(),operation:.read,input:.object(["binding":.string("saved")])),from:old)
        }
        while authority.continuation == nil { await Task.yield() }
        try r.setExecutionEnabledFromNativeHost(false,context:old)
        authority.continuation!.resume(returning:.string("late")); authority.continuation = nil
        do { _ = try await task.value; preconditionFailure("late callback escaped stopped instance") } catch {}
        try r.activate(bundle("2"),origin:authority.origin,readContext:c,readGeneration:"1")
        do { try r.setExecutionEnabledFromNativeHost(true,context:old); preconditionFailure("old revision enabled replacement") } catch {}
        precondition(!r.executionEnabled && !r.writeEnabled)
        authority.origin = .init(authority:"authority",database:"db",restoreEpoch:"epoch",crew:"crew",namespace:"other",generation:"1",permissionEpoch:"grant2")
        r.synchronizeAuthority(); precondition(r.context == nil)
        do { _ = try CrewdKnowledgeViewRuntime.Request.decode("{\"callID\":\"\(UUID().uuidString)\",\"operation\":\"clipboard\",\"input\":null}"); preconditionFailure("general native bridge admitted") } catch {}
        print("PASS renderer: no automatic execution; independent Apply grant; stopped late callback; revision and namespace revocation; closed bridge operations")
    }
}
