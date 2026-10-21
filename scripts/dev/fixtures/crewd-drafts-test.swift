import Foundation
#if QA_REMOTE_FACTORY
final class RemoteFactoryReadProbe: CrewdRemoteCredentialStore {
    var reads = 0
    var failRead = false
    func records() throws -> [UUID] {
        reads += 1
        if failRead { throw CrewdRemoteError.unavailableCredential }
        return []
    }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair { throw CrewdRemoteError.unavailableCredential }
    func save(_ record: CrewdRemoteStoredPair) throws { fatalError("not a credential-write test") }
    func remove(_ id: UUID) throws { fatalError("not a credential-delete test") }
}
#endif

@main struct DraftProbe {
    @MainActor static func main() async throws {
        // A controlled counterexample to the old window-wide @State strings.
        if CommandLine.arguments.contains("--legacy") {
            var windowCompose = ""
            windowCompose = "draft for A"
            let bDraft = windowCompose // selection changes only workspace IDs
            guard bDraft.isEmpty else { throw Failure.message("legacy window draft appears in B") }
            return
        }
        #if QA_RETAINED_WORKSPACES
        // The actual toolbar/sheet projection, using real production workspaces.
        let historical = CrewdWorkspace(authorityRecordID: UUID(), connect: { _ in })
        let selected = CrewdWorkspace(authorityRecordID: UUID(), connect: { _ in })
        defer { historical.retire(); selected.retire() }
        let ca = CrewdDrafts.Context.message(crew: "crew-a", channel: "topic-a", session: "")
        let cb = CrewdDrafts.Context.message(crew: "crew-b", channel: "topic-b", session: "")
        selected.messageDrafts.set("UNSENT A", for: ca)
        selected.messageDrafts.set("UNSENT B", for: cb)
        let rows = CrewdRemoteWindow.inputReviewWorkspaces(retained: [historical], current: selected)
        guard rows.count == 2, rows[0] === historical, rows[1] === selected else {
            throw Failure.message("disconnected selected authority missing from original-input review; only historical local workspace exposed")
        }
        guard rows[1].messageDrafts.text(for: ca) == "UNSENT A", rows[1].messageDrafts.text(for: cb) == "UNSENT B" else {
            throw Failure.message("original crew/channel drafts missing from selected readback")
        }
        let dedup = CrewdRemoteWindow.inputReviewWorkspaces(retained: [historical, selected, historical], current: selected)
        guard dedup.count == 2, dedup[0] === historical, dedup[1] === selected else { throw Failure.message("same workspace duplicated in review") }
        guard CrewdRemoteWindow.inputReviewWorkspaces(retained: [historical], current: nil).count == 1 else { throw Failure.message("historical review lost without selection") }
        if CommandLine.arguments.count > 1 {
            // Existing disposable Unix fixture provides real production hello qualification.
            try await selected.client.connect(path: CommandLine.arguments[1])
            guard selected.client.readyForWork else { throw Failure.message("test connection not qualified") }
            let connected = CrewdRemoteWindow.inputReviewWorkspaces(retained: [historical], current: selected)
            guard connected.count == 1, connected[0] === historical else { throw Failure.message("healthy selected connection incorrectly listed as old input") }
            selected.client.disconnect()
            guard CrewdRemoteWindow.inputReviewWorkspaces(retained: [historical], current: selected).count == 2 else { throw Failure.message("actual disconnect did not expose selected inputs") }
        }
        print("PASS: disconnected selected + historical original-input projection; object dedup; crew/channel drafts; healthy exclusion and actual disconnect when socket supplied")
        #endif
        #if QA_REMOTE_FACTORY
        // Constructing the real persistent factory does not call SecItem APIs.
        // The window is separately injected with a read counter, never a vault.
        let factory = CrewdRemoteCredentials.application()
        #if QA_EXPECT_PERSISTENT
        guard CrewdRemoteCredentials.keychainService(bundleIdentifier: "com.pendingname.pendingcrew") == "com.pendingname.pendingcrew.crewd.remote.v1",
              CrewdRemoteCredentials.keychainService(bundleIdentifier: "com.pendingname.pendingcrew.remote-preview") == "com.pendingname.pendingcrew.remote-preview.crewd.remote.v1",
              CrewdRemoteCredentials.keychainService(bundleIdentifier: nil) == nil,
              CrewdRemoteCredentials.keychainService(bundleIdentifier: "com.pendingname..pendingcrew") == nil,
              CrewdRemoteCredentials.keychainService(bundleIdentifier: " com.pendingname.pendingcrew") == nil else {
            throw Failure.message("credential namespace must bind the exact valid App identity")
        }
        if CrewdRemoteCredentials.keychainService(bundleIdentifier: Bundle.main.bundleIdentifier) != nil {
            guard factory is CrewdRemoteKeychain else { throw Failure.message("valid normal App identity lost persistent credential factory") }
        } else {
            guard factory is CrewdRefusedCredentials else { throw Failure.message("unqualified executable opened production credentials") }
        }
        #elseif QA_EXPECT_MEMORY
        guard factory is CrewdRemoteMemoryCredentials else { throw Failure.message("isolated candidate lost memory credential factory") }
        #else
        guard factory is CrewdRefusedCredentials else { throw Failure.message("default local/unqualified validation unexpectedly opened credential factory") }
        #endif
        let reads = RemoteFactoryReadProbe()
        let window = CrewdRemoteWindow(pairing: CrewdRemotePairing(credentials: reads))
        guard reads.reads == 0, window.computerListState == .notLoaded else { throw Failure.message("creating a window read credentials before an explicit request") }
        NotificationCenter.default.post(name: CrewdRemotePairing.changed, object: nil)
        try await Task.sleep(for: .milliseconds(20))
        guard reads.reads == 0 else { throw Failure.message("notification read credentials before an explicit request") }
        window.refresh()
        #if QA_EXPECT_REMOTE_FLOW
        guard reads.reads == 1, window.computerListState == .loaded, window.computers.isEmpty else { throw Failure.message("successful empty credential list was not marked loaded") }
        reads.failRead = true
        guard !window.refresh(), case .failed = window.computerListState, window.error != nil else { throw Failure.message("credential read failure was hidden as an empty list") }
        let failedReadCount = reads.reads
        NotificationCenter.default.post(name: CrewdRemotePairing.changed, object: nil)
        try await Task.sleep(for: .milliseconds(20))
        guard reads.reads == failedReadCount else { throw Failure.message("notification retried a failed credential read") }
        #else
        guard reads.reads == 0 else { throw Failure.message("default local build read remote credentials") }
        #endif
        window.close()
        print("PASS: actual factory + lazy credential list: zero initialization/notification reads, explicit empty success, failure retained without notification retry; no SecItem or GUI")
        return
        #endif
        var drafts = CrewdDrafts()
        let a = CrewdDrafts.Context.message(crew: "a", channel: "one", session: "worker")
        let otherCrew = CrewdDrafts.Context.message(crew: "b", channel: "one", session: "worker")
        let otherChannel = CrewdDrafts.Context.message(crew: "a", channel: "two", session: "worker")
        let otherSession = CrewdDrafts.Context.message(crew: "a", channel: "one", session: "captain")
        let task = CrewdDrafts.Context.initialTask(crew: "a", session: "worker")
        let otherTask = CrewdDrafts.Context.initialTask(crew: "b", session: "worker")
        let taskSession = CrewdDrafts.Context.initialTask(crew: "a", session: "captain")
        func require(_ ok: Bool, _ message: String) throws { if !ok { throw Failure.message(message) } }
        drafts.set("same text", for: a)
        for key in [otherCrew, otherChannel, otherSession, task, otherTask, taskSession] {
            try require(drafts.text(for: key).isEmpty, "destination or draft-kind leak")
        }
        drafts.set("same text", for: otherCrew)
        drafts.set("channel text", for: otherChannel)
        drafts.set("session text", for: otherSession)
        drafts.set("initial task", for: task)
        drafts.set("other task", for: otherTask)
        drafts.set("session task", for: taskSession)
        let sent = drafts.snapshot(for: a)
        try require(sent.context == a && sent.text == "same text", "send context changed")
        try require(drafts.clearIfUnchanged(sent), "unchanged sent draft not cleared")
        try require(drafts.text(for: otherCrew) == "same text", "old completion erased current crew")
        try require(drafts.text(for: otherChannel) == "channel text" && drafts.text(for: otherSession) == "session text", "picker drafts lost")
        try require(drafts.text(for: task) == "initial task", "message completion erased task")
        let taskSent = drafts.snapshot(for: task)
        drafts.set("new task", for: task)
        try require(!drafts.clearIfUnchanged(taskSent), "completion erased edited task")
        drafts.set("initial task", for: task)
        try require(!drafts.clearIfUnchanged(taskSent), "ABA edit erased by old completion")
        let pending = drafts.snapshot(for: otherCrew)
        drafts.set("edited", for: otherCrew)
        try require(!drafts.clearIfUnchanged(pending), "completion erased new text")
        // Failed operations do not invoke clear, so the same exact snapshot remains retryable.
        let retry = drafts.snapshot(for: otherChannel)
        try require(drafts.text(for: retry.context) == retry.text, "failed request lost draft")
        try require(drafts.clearIfUnchanged(retry), "successful retry not cleared")
        try require(!drafts.clearIfUnchanged(retry), "completion applied twice")
        try require(drafts.text(for: otherTask) == "other task" && drafts.text(for: taskSession) == "session task", "new-task picker contexts lost")
        print("PASS: crew/channel/session and message/task isolation; fixed send snapshot; async same-text, ABA, failed retry, double completion")
    }
    enum Failure: Error { case message(String) }
}
