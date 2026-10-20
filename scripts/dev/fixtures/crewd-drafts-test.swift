import Foundation

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
