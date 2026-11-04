import Foundation

@main struct MessageAttributionHistoryFixture {
    @MainActor static func main() async throws {
        typealias Page = CrewdMessageAttributionHistoryPage
        let source = Page.Source(crew:"crew-a",channel:"channel-a",message:"message-a")
        let target: CrewdValue = .object(["kind":.string("work"),"id":.string("W1")])
        let linked: CrewdValue = .object(["action":.string("set"),"target":target,"targetUnavailable":.bool(false)])
        let hidden: CrewdValue = .object(["action":.string("set"),"targetUnavailable":.bool(true)])
        let clear: CrewdValue = .object(["action":.string("clear"),"targetUnavailable":.bool(false)])
        func item(_ action: String, at: Int64, historical: Bool = false, before: CrewdValue? = nil, after: CrewdValue? = nil) -> CrewdValue {
            var row: [String:CrewdValue] = ["action":.string(action),"atMs":.integer(at),"historical":.bool(historical)]
            if let before { row["before"] = before }
            if let after { row["after"] = after }
            return .object(row)
        }
        let newest = item("reset",at:3,before:clear)
        let explicitClear = item("clear",at:3,historical:true,before:hidden,after:clear)
        let original = item("set",at:4,after:linked)
        func page(_ rows: [CrewdValue], next: String? = nil, older: Bool = false) -> CrewdValue {
            var fields: [String:CrewdValue] = ["crewId":.string(source.crew),"channelId":.string(source.channel),
                "messageId":.string(source.message),"items":.array(rows),"olderUnavailable":.bool(older)]
            if let next { fields["nextCursor"] = .string(next) }
            return .object(fields)
        }
        func changed(_ value: CrewdValue, _ key: String, _ next: CrewdValue) -> CrewdValue {
            guard case .object(var fields) = value else { preconditionFailure("object required") }
            fields[key] = next
            return .object(fields)
        }
        func rejects(_ value: CrewdValue, cursor: String? = nil) {
            do { _ = try Page(value,source:source,cursor:cursor); preconditionFailure("bad evidence accepted") }
            catch {}
        }
        let parsed = try Page(page([newest,explicitClear,original]),source:source,cursor:nil)
        precondition(parsed.items.map(\.action) == [.reset,.clear,.set] && parsed.items.map(\.atMs) == [3,3,4])
        precondition(parsed.items[0].after == nil && parsed.items[0].before?.action == .clear)
        precondition(parsed.items[1].historical && parsed.items[1].before?.targetUnavailable == true)
        precondition(parsed.items[1].before?.target == nil && parsed.items[1].after?.action == .clear)
        precondition(parsed.items[2].after?.target?.id == "W1")
        rejects(page([item("set",at:1,after:clear)]))
        rejects(page([item("clear",at:1,after:changed(clear,"target",target))]))
        rejects(page([item("set",at:1,after:changed(hidden,"target",target))]))
        rejects(page([item("reset",at:1,after:clear)]))
        rejects(page([item("reset",at:1,after:.null)]))
        rejects(changed(page([original]),"channelId",.string("wrong-channel")))
        rejects(changed(page([original]),"crewId",.string("wrong-crew")))
        rejects(changed(page([original]),"messageId",.string("wrong-message")))
        rejects(page([original],next:"cursor"))
        rejects(page([original],older:true))
        rejects(page([],older:true),cursor:"continuation")
        rejects(page(Array(repeating:original,count:21)))
        rejects(page(Array(repeating:original,count:20),next:"cursor",older:true),cursor:"prior")
        rejects(page(Array(repeating:original,count:20),next:"cursor"),cursor:"cursor")
        let full = page(Array(repeating:original,count:20),next:"cursor")
        let terminal = page([newest],older:true)

        var calls: [CrewdValue] = []
        var pending: CheckedContinuation<CrewdValue,Error>?
        var hold = false
        let store = CrewdWorkStore { method, params in
            precondition(method == Page.method)
            precondition(params["crewId"] == .string(source.crew) && params["channelId"] == .string(source.channel) &&
                         params["messageId"] == .string(source.message))
            calls.append(params)
            if hold { return try await withCheckedThrowingContinuation { pending = $0 } }
            return params["cursor"] == .null ? full : terminal
        }
        store.selectCrew(source.crew)
        do {
            _ = try await store.messageAttributionHistory(source:source)
            preconditionFailure("older Core must not receive unsupported RPC")
        } catch let error as CrewdClientError { precondition(error.kind == .notSubmitted) }
        precondition(calls.isEmpty)
        store.configureCapabilities([Page.method])
        let first = try await store.messageAttributionHistory(source:source)
        precondition(first.items.count == 20 && first.nextCursor == "cursor")
        let last = try await store.messageAttributionHistory(source:source,cursor:first.nextCursor)
        precondition(last.items[0].action == .reset && last.olderUnavailable && last.nextCursor == nil)

        hold = true
        let stale = Task { @MainActor in try await store.messageAttributionHistory(source:source) }
        while pending == nil { await Task.yield() }
        store.invalidateConnection()
        pending?.resume(returning:full)
        do { _ = try await stale.value; preconditionFailure("stale grant/connection evidence escaped") }
        catch is CancellationError {}
        precondition(calls.count == 3)
        print("PASS exact source, model-independent Human history, redaction, bounded pages and stale connection")
    }
}
