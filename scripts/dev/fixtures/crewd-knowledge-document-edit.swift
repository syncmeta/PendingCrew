import Foundation

@MainActor private final class DocumentWire {
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:"crew",namespace:"namespace",generation:"1")
    let view = CrewdDataRef(namespace:"namespace",kind:"view",id:"10000000-0000-0000-0000-000000000001")
    let body = CrewdDataRef(namespace:"namespace",kind:"block",id:"10000000-0000-0000-0000-000000000002")
    let newBody = CrewdDataRef(namespace:"namespace",kind:"block",id:"10000000-0000-0000-0000-000000000003")
    let placement = "10000000-0000-0000-0000-000000000004"
    var receipts: [String:CrewdDataChangeResult] = [:]
    var sends: [CrewdDataSubmitParams] = []
    var lookups: [String] = []
    var loseReply = false
    var malformedReply = false
    var rejectCAS = false
    var untypedConflict = false
    var beforeSubmitReply: (() async throws -> Void)?
    var missingHistoricalCapture = false
    var observedHistoricalReads: [(CrewdDataRef,String)] = []
    var protected = false
    var pinned = false
    var title = "原题"
    var text = "原文🌙"
    var viewRevision = "4"
    var info: CrewdDataWorkspaceInfo { .init(context:context,state:"ready",engineVersion:"SQLite",reason:nil) }
    var provenance: CrewdDataProvenance {
        let human: CrewdValue = .object(["kind":.string(protected ? "session" : "human")])
        return .init(createdBy:human,updatedBy:human,createdAt:"1",updatedAt:"2",sources:[],attributes:.object(["kept":.bool(true)]))
    }

    func detail(_ reference: CrewdDataRef) -> CrewdDataObjectDetail {
        let target = viewRevision == "4" || text == "原文🌙" ? body : newBody
        let object: CrewdDataObject
        var references: [CrewdDataResolvedRef] = []
        if reference == view {
            let visible = CrewdDataRefState.visible(reference:target)
            let position = CrewdDataPlacement(id:placement,block:visible,atRevision:pinned ? "3" : nil,layout:.object(["position":.integer(2)]))
            references = [.init(anchor:"placement:"+placement,target:visible,commit:nil,revision:target == body ? "3":"1",live:!pinned,complete:true)]
            object = .view(id:view.id,revision:viewRevision,title:title,definition:.document(placements:[position],layout:.object(["width":.integer(3)])),archived:false,provenance:provenance)
        } else {
            object = .block(id:reference.id,revision:reference == body ? "3":"1",contentType:"text/plain",content:.string(reference == body ? "原文🌙":text),archived:false,provenance:provenance)
        }
        return .init(context:context,resolvedContext:context,generation:context.generation,object:object,references:references,affectedLiveViews:[view],affectedTruncated:false)
    }

    func historicalView(_ revision: String) -> CrewdDataObjectDetail {
        let visible = CrewdDataRefState.visible(reference:body)
        let position = CrewdDataPlacement(id:placement,block:visible,atRevision:"3",layout:.object([:]))
        let object = CrewdDataObject.view(id:view.id,revision:revision,title:"旧版\(revision)",definition:.document(placements:[position],layout:.object([:])),archived:false,provenance:provenance)
        let references: [CrewdDataResolvedRef] = missingHistoricalCapture ? [] : [
            .init(anchor:"placement:"+placement,target:visible,commit:nil,revision:"3",live:false,complete:true)
        ]
        return .init(context:context,resolvedContext:context,generation:context.generation,object:object,references:references,affectedLiveViews:[],affectedTruncated:false)
    }

    func call(_ method: String, _ value: CrewdValue) async throws -> CrewdValue {
        switch method {
        case "data.workspace.get": return try CrewdKnowledgeStore.encode(info)
        case "data.object.list": return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,items:[detail(view).object],next:nil))
        case "data.object.get":
            let reference = try CrewdKnowledgeStore.decode(value["reference"],CrewdDataRef.self)
            if let revision = value["atRevision"].string {
                observedHistoricalReads.append((reference,revision))
                if reference == view { return try CrewdKnowledgeStore.encode(historicalView(revision)) }
                precondition(reference == body && revision == "3")
            }
            return try CrewdKnowledgeStore.encode(detail(reference))
        case "data.history":
            let input = try CrewdKnowledgeStore.decode(value,CrewdDataHistoryParams.self)
            precondition(input.context == context && input.reference == view)
            let start = (input.after.flatMap(Int.init) ?? 0) + 1
            let end = min(start + Int(input.limit) - 1,Int(viewRevision)!)
            let items = (start...end).map { historicalView(String($0)).object }
            return try CrewdKnowledgeStore.encode(CrewdDataHistory(context:context,items:items,next:end < Int(viewRevision)! ? String(end):nil))
        case "data.submit":
            let attempt = try CrewdKnowledgeStore.decode(value,CrewdDataSubmitParams.self)
            sends.append(attempt)
            try await beforeSubmitReply?()
            if rejectCAS { throw CrewdClientError(message:"stale generation",code:-32009,data:.object(["refusal":.string("data.context_conflict")])) }
            if untypedConflict { throw CrewdClientError(message:"generic storage failure",code:-32009) }
            precondition(attempt.context == context)
            guard case let .apply(_,edits) = attempt.change,
                  case let .updateView(id,revision,nextTitle,.document(positions,layout),sources,attributes)? = edits.last else {
                preconditionFailure("native edit did not target the original view")
            }
            precondition(id == view.id && revision == viewRevision && sources == nil && attributes == provenance.attributes)
            precondition(layout == .object(["width":.integer(3)]))
            var created: [String:CrewdDataCreated] = [:]
            var changed: [CrewdDataObjectVersion] = []
            if edits.count == 2,
               case let .createBlock(alias,type,.string(content),blockSources,blockAttributes) = edits[0] {
                precondition(type == "text/plain" && blockAttributes == provenance.attributes && blockSources.count == 1)
                precondition(blockSources[0].reference == .existing(reference:body) && blockSources[0].atRevision == "3")
                precondition(positions == [.set(id:placement,alias:nil,block:.created(alias:alias),atRevision:nil,layout:.object(["position":.integer(2)]))])
                text = content
                let version = CrewdDataObjectVersion(reference:newBody,revision:"1")
                created[alias] = .object(object:version)
                changed.append(version)
            } else {
                precondition(edits.count == 1 && positions == [.keep(id:placement,layout:.object(["position":.integer(2)]))])
            }
            title = nextTitle
            viewRevision = String(Int(viewRevision)! + 1)
            changed.append(.init(reference:view,revision:viewRevision))
            let before = context.generation
            context.generation = String(Int(before)!+1)
            var result = CrewdDataChangeResult(requestId:attempt.requestId,phase:.committed,context:context,beforeGeneration:before,afterGeneration:context.generation,snapshotId:nil,created:created,changed:changed,conflicts:[],reason:nil,resolvedByRequest:nil)
            receipts[attempt.requestId] = result
            if loseReply { throw NSError(domain:"original reply lost",code:1) }
            if malformedReply { result.changed.removeLast() }
            return try CrewdKnowledgeStore.encode(result)
        case "data.request.get":
            let attempt = try CrewdKnowledgeStore.decode(value,CrewdDataRequestGetParams.self)
            lookups.append(attempt.requestId)
            let result = receipts[attempt.requestId].map { CrewdDataRequestLookup.known(value:$0) } ?? .notFound
            return try CrewdKnowledgeStore.encode(CrewdDataRequestGetResult(workspace:info,lookup:result))
        default: preconditionFailure("unexpected route \(method)")
        }
    }
}

@main struct DocumentEditContract {
    @MainActor static func main() async throws {
        if CommandLine.arguments[1] == "public-wire" {
            try await publicWire()
            return
        }
        let root = URL(fileURLWithPath:"/private/tmp/pcw-document-edit-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"authority",databaseID:"db",grantID:nil,grantRevision:nil)
        let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
        let wire = DocumentWire()
        func newStore() throws -> CrewdKnowledgeStore {
            let journal = try CrewdPresentationJournal(root:root,scope:scope)
            return CrewdKnowledgeStore(origin:{origin},rpc:{try await wire.call($0,$1)},journal:{journal})
        }
        let store = try newStore()
        func read(_ store: CrewdKnowledgeStore) async {
            await store.refresh()
            await store.open(wire.view)
        }
        let mode = CommandLine.arguments[1]
        if mode == "history-paging" { wire.viewRevision = "26" }
        await read(store)
        let original = try await store.prepareNativeDocumentEdit(store.detail!)
        if mode == "history-paging" {
            let target = try store.nativeDocumentHistoryTarget(store.detail!)
            let first = try await store.loadNativeDocumentRevisions(target)
            precondition(first.items.count == 25 && first.next == "25")
            let last = try await store.loadNativeDocumentRevisions(target,after:first.next)
            precondition(last.items.map(\.revision) == ["26"] && last.next == nil && wire.sends.isEmpty)
        } else if mode.hasPrefix("history-") {
            let target = try store.nativeDocumentHistoryTarget(store.detail!)
            let page = try await store.loadNativeDocumentRevisions(target)
            precondition(page.items.map(\.revision) == ["1","2","3","4"] && page.next == nil)
            wire.missingHistoricalCapture = mode == "history-missing"
            if wire.missingHistoricalCapture {
                do {
                    _ = try await store.inspectNativeDocumentRevision(target,revision:page.items[0])
                    preconditionFailure("missing capture cannot fall back to current body")
                } catch {}
                precondition(wire.observedHistoricalReads.count == 1 && wire.sends.isEmpty)
            } else {
                let old = try await store.inspectNativeDocumentRevision(target,revision:page.items[0])
                precondition(old.revision.revision == "1" && old.body == "原文🌙")
                precondition(wire.observedHistoricalReads.map(\.1) == ["1","3"])
                precondition(store.detail?.object.revision == "4" && wire.sends.isEmpty)
                if mode == "history-stale" {
                    store.handleEvent("data.changed",.object(["crewId":.string("crew")]))
                    do {
                        _ = try await store.restoreNativeDocumentRevision(old,replacing:original)
                        preconditionFailure("stale history cannot restore into a changed authority")
                    } catch CrewdKnowledgeStore.Failure.stale {}
                    precondition(wire.sends.isEmpty)
                } else if mode == "history-restore" || mode == "history-lost" {
                    wire.loseReply = mode == "history-lost"
                    let receipt = try await store.restoreNativeDocumentRevision(old,replacing:original)
                    precondition(wire.title == "旧版1" && wire.sends.count == 1)
                    guard case .apply(let message,let edits) = wire.sends[0].change else { preconditionFailure("expected one atomic batch") }
                    precondition(message == "恢复文档版本" && edits.count == 2)
                    if wire.loseReply {
                        precondition(receipt == nil)
                        let reopened = try newStore(); await read(reopened)
                        let id = reopened.pending.first!.id
                        await reopened.lookup(id)
                        precondition(!reopened.hasBlockingPending && reopened.result?.phase == .committed)
                        precondition(wire.lookups == [wire.sends[0].requestId] && wire.sends.count == 1)
                    } else { precondition(receipt?.phase == .committed) }
                }
            }
        } else if mode == "title" {
            let result = try await store.submitNativeDocumentEdit(original,title:"新题",body:original.body)
            precondition(result?.phase == .committed && wire.sends.count == 1 && wire.title == "新题" && wire.text == original.body)
        } else if mode == "stale" {
            wire.rejectCAS = true
            do {
                _ = try await store.submitNativeDocumentEdit(original,title:"并发",body:"不提交")
                preconditionFailure("explicit CAS rejection must remain visible")
            } catch let error as CrewdClientError { precondition(error.code == -32009) }
            precondition(wire.sends.count == 1 && !store.hasBlockingPending && wire.receipts.isEmpty)
        } else if mode == "ambiguous" {
            wire.untypedConflict = true
            let result = try await store.submitNativeDocumentEdit(original,title:"失败",body:"保留草稿")
            precondition(result == nil && store.hasBlockingPending)
            let id = store.pending.first!.id
            await store.lookup(id)
            precondition(store.hasBlockingPending && wire.lookups == [id.uuidString.lowercased()] && wire.sends.count == 1)
        } else if mode == "shared-store" {
            let other = try newStore()
            await read(other)
            let otherEdit = try await other.prepareNativeDocumentEdit(other.detail!)
            precondition(!other.hasBlockingPending)
            wire.loseReply = true
            wire.beforeSubmitReply = {
                do {
                    _ = try await other.submitNativeDocumentEdit(otherEdit,title:"另一个窗口",body:otherEdit.body)
                    preconditionFailure("a preactivated store must not send while the original is in flight")
                } catch CrewdKnowledgeStore.Failure.unresolved {}
                precondition(other.hasBlockingPending && wire.sends.count == 1)
            }
            let originalResult = try await store.submitNativeDocumentEdit(original,title:"已提交",body:original.body)
            precondition(originalResult == nil && store.hasBlockingPending && other.hasBlockingPending)
            let id = store.pending.first!.id
            await other.lookup(id)
            store.synchronizePending()
            precondition(!store.hasBlockingPending && !other.hasBlockingPending && wire.sends.count == 1)
            precondition(wire.lookups == [id.uuidString.lowercased()])
        } else if mode == "rebase" {
            wire.title = "并发标题"; wire.text = "并发正文"; wire.viewRevision = "5"; wire.context.generation = "2"
            let current = try await store.reloadNativeDocumentEdit(original)
            let titleOnly = current.rebasedDraft(from:original,title:"我的标题",body:original.body)
            let bodyOnly = current.rebasedDraft(from:original,title:original.title,body:"我的正文")
            precondition(titleOnly.title == "我的标题" && titleOnly.body == "并发正文")
            precondition(bodyOnly.title == "并发标题" && bodyOnly.body == "我的正文")
            precondition(wire.sends.isEmpty)
        } else if mode == "protected" {
            wire.protected = true; await read(store)
            precondition(!store.canPrepareNativeDocumentEdit(store.detail!))
            wire.protected = false; wire.pinned = true; await read(store)
            precondition(!store.canPrepareNativeDocumentEdit(store.detail!))
            precondition(wire.sends.isEmpty)
        } else {
            wire.loseReply = mode == "lost"
            wire.malformedReply = mode == "malformed"
            let result = try await store.submitNativeDocumentEdit(original,title:"新题",body:"新的正文 e\u{301}🌙")
            precondition(wire.sends.count == 1)
            let id = UUID(uuidString:wire.sends[0].requestId)!
            if wire.loseReply || wire.malformedReply {
                precondition(result == nil)
                let reopened = try newStore()
                await read(reopened)
                precondition(reopened.pending.map(\.id) == [id])
                let newer = try await reopened.prepareNativeDocumentEdit(reopened.detail!)
                do {
                    _ = try await reopened.submitNativeDocumentEdit(newer,title:"do not submit",body:newer.body)
                    preconditionFailure("Unknown original must block new writes")
                } catch {}
                await reopened.lookup(id)
                precondition(!reopened.hasBlockingPending && reopened.result?.requestId == wire.sends[0].requestId)
                precondition(wire.lookups == [wire.sends[0].requestId] && wire.sends.count == 1)
            } else { precondition(result?.phase == .committed) }
            await read(store)
            let updated = try await store.prepareNativeDocumentEdit(store.detail!)
            precondition(updated.title == "新题" && updated.body.utf8.elementsEqual("新的正文 e\u{301}🌙".utf8))
            precondition(wire.detail(wire.body).object.revision == "3")
        }
        print("PASS native document edit \(mode)")
    }

    @MainActor private static func publicWire() async throws {
        let decoder = JSONDecoder()
        let encoder = JSONEncoder()
        func send(_ value: CrewdValue) throws {
            FileHandle.standardOutput.write(try encoder.encode(value) + Data([10]))
        }
        func receive() throws -> CrewdValue {
            guard let line = readLine() else { throw CocoaError(.fileReadUnknown) }
            return try decoder.decode(CrewdValue.self,from:Data(line.utf8))
        }
        let config = try receive()
        let root = URL(fileURLWithPath:config["journalRoot"].string!)
        let db = config["dbId"].string!
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:config["crewId"].string!)
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        let store = CrewdKnowledgeStore(origin:{origin},rpc:{ method, params in
            try send(.object(["method":.string(method),"params":params]))
            let envelope = try receive()
            if envelope["error"] != .null {
                throw CrewdClientError(message:envelope["error"]["message"].string ?? "refused",code:envelope["error"]["code"].int,data:envelope["error"]["data"])
            }
            return envelope["result"]
        },journal:{journal})
        let view = try CrewdKnowledgeStore.decode(config["view"],CrewdDataRef.self)
        await store.refresh()
        await store.open(view)
        guard let parent = store.detail else { throw CrewdKnowledgeStore.Failure.unavailable }
        let original = try await store.prepareNativeDocumentEdit(parent)
        let saved: CrewdDataChangeResult?
        if let revision = config["restoreRevision"].string {
            precondition(original.title == "新题" && original.body.utf8.elementsEqual("新的正文 e\u{301}🌙".utf8))
            let target = try store.nativeDocumentHistoryTarget(parent)
            let history = try await store.loadNativeDocumentRevisions(target)
            guard let selected = history.items.first(where:{$0.revision == revision}) else { preconditionFailure("required original revision missing") }
            let old = try await store.inspectNativeDocumentRevision(target,revision:selected)
            precondition(old.revision.title == "原题" && old.body == "原文🌙")
            saved = try await store.restoreNativeDocumentRevision(old,replacing:original)
        } else {
            precondition(original.title == "原题" && original.body == "原文🌙")
            saved = try await store.submitNativeDocumentEdit(original,title:"新题",body:"新的正文 e\u{301}🌙")
        }
        precondition(saved?.phase == .committed)
        try send(.object(["finished":try CrewdKnowledgeStore.encode(saved!)]))
    }
}
