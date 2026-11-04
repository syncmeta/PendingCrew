import Foundation

@main struct ChannelResponsibilityFixture {
    @MainActor static func main() async throws {
        typealias Page = CrewdChannelResponsibilityPage
        let channel = "ch-a", crew = "crew-a"
        let episodeID = "1cf64cbb-ef11-4a73-a52a-52cd5acdc1c1"
        func channelRow(_ id: String, _ owner: String, _ kind: String, archived: Bool = false) -> CrewdValue {
            .object(["id":.string(id),"crewId":.string(owner),"kind":.string(kind),
                     "archivedAt":archived ? .string("2026-11-05T00:00:00Z") : .null])
        }
        let available = [channelRow(channel,crew,"topic"),channelRow("foreign","other","topic"),
                         channelRow("archived",crew,"topic",archived:true),channelRow("dm",crew,"dm")]
        precondition(Page.ownsActiveTopic(available,crew:crew,channel:channel))
        for excluded in ["","foreign","archived","dm","absent"] {
            precondition(!Page.ownsActiveTopic(available,crew:crew,channel:excluded))
        }
        let unqueued = CrewdValue.object([
            "channelId":.string(channel),"episodeId":.string(episodeID),"phase":.string("open"),
            "noticePhase":.string("not_queued"),"observedAtMs":.integer(10),"updatedAtMs":.integer(20),
            "workIds":.array([.string("W2"),.string("W10")])
        ])
        let ownerEvent = CrewdValue.object([
            "workId":.string("W2"),"ownerSessionId":.string("owner"),"eventSeq":.string("9"),"atMs":.integer(19)
        ])
        func field(_ value: CrewdValue, _ key: String, _ next: CrewdValue) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("object required") }
            fields[key] = next
            return .object(fields)
        }
        func rejects(_ action: () throws -> Void) {
            do { try action(); preconditionFailure("invalid responsibility evidence accepted") } catch {}
        }
        func page(_ rows: [CrewdValue], next: String? = nil) -> CrewdValue {
            var fields: [String:CrewdValue] = ["crewId":.string(crew),"items":.array(rows)]
            if let next { fields["nextAfter"] = .string(next) }
            return .object(fields)
        }
        let withWake = field(field(unqueued,"captainSessionId",.string("captain")),"wakeId",.integer(42))
        let delivered = field(withWake,"noticePhase",.string("delivered"))
        let handled = field(field(delivered,"phase",.string("handled")),"handled",ownerEvent)
        let pending = try Page(page([unqueued]),crew:crew,channel:channel,after:nil)
        let sent = try Page(page([delivered]),crew:crew,channel:channel,after:nil)
        let verified = try Page(page([handled]),crew:crew,channel:channel,after:nil)
        precondition(pending.items[0].captain == nil && pending.items[0].notice == .notQueued)
        precondition(sent.items[0].phase == .open && sent.items[0].notice == .delivered && sent.items[0].ownership == nil)
        precondition(verified.items[0].phase == .handled && verified.items[0].ownership?.owner == "owner")
        rejects { _ = try Page(page([unqueued]),crew:"wrong",channel:channel,after:nil) }
        rejects { _ = try Page(page([unqueued]),crew:crew,channel:"wrong",after:nil) }
        rejects { _ = try Page(page([unqueued],next:channel),crew:crew,channel:channel,after:nil) }
        rejects { _ = try Page(page([unqueued]),crew:crew,channel:nil,after:channel) }
        rejects { _ = try Page(page([unqueued,unqueued]),crew:crew,channel:nil,after:nil) }
        rejects { _ = try Page(page([field(delivered,"phase",.string("handled"))]),crew:crew,channel:channel,after:nil) }
        rejects { _ = try Page(page([field(delivered,"handled",ownerEvent)]),crew:crew,channel:channel,after:nil) }
        rejects { _ = try Page(page([field(unqueued,"noticePhase",.string("delivered"))]),crew:crew,channel:channel,after:nil) }
        rejects { _ = try Page(page([field(unqueued,"workIds",.array([.string("W10"),.string("W2")]))]),crew:crew,channel:channel,after:nil) }
        let empty = try Page(page([]),crew:crew,channel:channel,after:nil)
        precondition(empty.items.isEmpty && empty.next == nil)

        let allRows = (1...32).map { number in
            field(field(unqueued,"episodeId",.string(UUID().uuidString.lowercased())),
                  "channelId",.string(String(format:"topic-%02d",number)))
        }
        let full = try Page(page(allRows,next:"topic-32"),crew:crew,channel:nil,after:nil)
        precondition(full.items.count == 32 && full.next == "topic-32")
        rejects { _ = try Page(page(allRows,next:"topic-31"),crew:crew,channel:nil,after:nil) }

        var calls: [String] = []
        var mode = "open"
        var hold: CheckedContinuation<CrewdValue,Error>?
        let store = CrewdWorkStore { method, params in
            calls.append(method)
            switch method {
            case Page.method:
                precondition(params["crewId"] == .string(crew))
                if params["channelId"] == .null {
                    return params["after"] == .null ? page(allRows,next:"topic-32") : page([])
                }
                precondition(params["channelId"] == .string(channel) && params["after"] == .null)
                if mode == "held" { return try await withCheckedThrowingContinuation { hold = $0 } }
                if mode == "stale" { return page([field(unqueued,"phase",.string("stale"))]) }
                return page([delivered])
            case "work.get":
                precondition(params["id"] == .string("W2"))
                return .object(["id":.string("W2"),"crewId":.string(mode == "foreign" ? "other" : crew),
                    "revision":.string("2"),"status":.string("open"),"owner":mode == "taken" ? .string("owner") : .null,
                    "title":.string("Unowned fixture"),"sources":.array([]),"frozen":.bool(false)])
            default: preconditionFailure("responsibility UI sent a mutation or unexpected RPC")
            }
        }
        store.selectCrew(crew)
        do { _ = try await store.channelResponsibility(crew:crew); preconditionFailure("unsupported read") } catch {}
        precondition(calls.isEmpty)
        store.configureCapabilities([Page.method])
        let first = try await store.channelResponsibility(crew:crew)
        let next = try await store.channelResponsibility(crew:crew,after:first.next)
        precondition(first.next == "topic-32" && next.items.isEmpty)
        let exact = try await store.channelResponsibility(crew:crew,channel:channel)
        let id = exact.items[0].work[0]
        let work = try await store.channelResponsibilityWork(id,episode:exact.items[0],crew:crew)
        precondition(work.id == id && work.crew == crew && work.revision == "2")
        for rejected in ["foreign","taken","stale"] {
            mode = rejected
            do { _ = try await store.channelResponsibilityWork(id,episode:exact.items[0],crew:crew); preconditionFailure("invalid work opened") } catch {}
        }
        let before = calls.count
        do { _ = try await store.channelResponsibilityWork(id,episode:verified.items[0],crew:crew); preconditionFailure("historic work became actionable") } catch {}
        precondition(calls.count == before)
        mode = "held"
        let late = Task { try await store.channelResponsibility(crew:crew,channel:channel) }
        while hold == nil { await Task.yield() }
        store.selectCrew("different")
        hold?.resume(returning:page([delivered])); hold = nil
        do { _ = try await late.value; preconditionFailure("late foreign result was accepted") } catch is CancellationError {}
        print("PASS independent responsibility facts, notice receipt and owner evidence, exact Work, bounded pages and stale scope")
    }
}
