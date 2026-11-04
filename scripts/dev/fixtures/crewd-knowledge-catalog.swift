import Foundation

@MainActor private final class CatalogWire {
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:"crew",namespace:"namespace",generation:"1")
    var pages: [CrewdDataObjectListParams] = []
    var beforePageReply: (() -> Void)?
    var duplicatePage = false
    var unauthorized = false
    var state = "ready"

    func id(_ number: Int) -> String { String(format:"00000000-0000-0000-0000-%012d",number) }
    func object(_ number: Int) -> CrewdDataObject {
        let author: CrewdValue = .object(["kind":.string("human")])
        let provenance = CrewdDataProvenance(createdBy:author,updatedBy:author,createdAt:"1",updatedAt:"1",sources:[],attributes:.object([:]))
        let definition: CrewdDataViewDefinition = number > 50 ? .document(placements:[],layout:.object([:])) :
            .web(renderer:"web",rendererVersion:"1",html:"<p>body-only-match</p>",css:"",javascript:"",bindings:[],requestedCapabilities:[],attributes:.object([:]))
        return .view(id:id(number),revision:"1",title:number == 53 ? "Café 月度纪要" : "资料 \(number)",definition:definition,archived:number == 54,provenance:provenance)
    }

    func call(_ method: String, _ params: CrewdValue) throws -> CrewdValue {
        switch method {
        case "data.workspace.get":
            return try CrewdKnowledgeStore.encode(CrewdDataWorkspaceInfo(context:context,state:state,engineVersion:"SQLite",reason:nil))
        case "data.object.list":
            let input = try CrewdKnowledgeStore.decode(params,CrewdDataObjectListParams.self)
            pages.append(input)
            precondition(input.context == context && input.objectType == "view" && input.atRevision == nil)
            if unauthorized { throw CrewdClientError(message:"revoked",code:-32003) }
            let available = (1...(input.includeArchived ? 54:53)).filter { id($0) > (input.after ?? "") }
            var returned = Array(available.prefix(Int(input.limit)))
            let next = available.count > returned.count ? returned.last.map(id) : nil
            if duplicatePage { returned[0] = 1 }
            beforePageReply?()
            return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,items:returned.map(object),next:next))
        case "data.object.get":
            let ref = try CrewdKnowledgeStore.decode(params["reference"],CrewdDataRef.self)
            let item = (1...54).map(object).first { $0.identity == ref.id }!
            return try CrewdKnowledgeStore.encode(CrewdDataObjectDetail(context:context,resolvedContext:context,generation:context.generation,object:item,references:[],affectedLiveViews:[],affectedTruncated:false))
        default: preconditionFailure("catalog unexpectedly invoked \(method)")
        }
    }
}

@main struct CatalogContract {
    @MainActor static func main() async throws {
        if CommandLine.arguments[1] == "public-wire" { try await publicWire(); return }
        let root = URL(fileURLWithPath:"/private/tmp/pcw-knowledge-catalog-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        var origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        let wire = CatalogWire()
        let store = CrewdKnowledgeStore(origin:{origin},rpc:{try wire.call($0,$1)},journal:{journal})
        await store.refreshViewCatalog(includeArchived:false)
        precondition(store.catalogStatus == .ready && store.viewCatalog?.items.count == 12 && store.viewCatalog?.next == wire.id(12))
        precondition(store.viewCatalog?.items.allSatisfy{$0.kind == .web} == true && wire.pages.count == 1)
        precondition(store.detail == nil,"opening catalog must not automatically open or execute a page")
        switch CommandLine.arguments[1] {
        case "progressive":
            await store.openViewCatalogItem(wire.id(53),kind:.document)
            precondition(store.detail == nil,"an undiscovered object must not be opened by a picker action")
            for _ in 0..<4 { await store.loadNextViewCatalogPage() }
            let page = store.viewCatalog!
            precondition(page.items.count == 53 && page.next == nil && wire.pages.count == 5)
            let matches = page.items.filter { $0.kind == .document && CrewdKnowledgeTitleSearch.matches($0.title,query:" CAFE " ) }
            precondition(matches.map(\.id) == [wire.id(53)])
            precondition(page.items.allSatisfy{!CrewdKnowledgeTitleSearch.matches($0.title,query:"body-only-match")})
            precondition(CrewdKnowledgeTitleSearch.matches(nil,query:"  ") && !CrewdKnowledgeTitleSearch.matches(nil,query:"月度"))
            await store.openViewCatalogItem(wire.id(53),kind:.document)
            precondition(store.detail?.object.identity == wire.id(53))
        case "remembered":
            await store.openViewCatalogItem(wire.id(53),kind:.document,previouslyOpenedDocument:true)
            precondition(store.detail?.object.identity == wire.id(53) && wire.pages.count == 1)
            await store.openViewCatalogItem(wire.id(12),kind:.document,previouslyOpenedDocument:true)
            precondition(store.detail?.object.identity == wire.id(53),"remembered native ID must never open a web view")
        case "malformed":
            wire.duplicatePage = true
            await store.loadNextViewCatalogPage()
            precondition(store.catalogStatus == .failed && store.viewCatalog?.items.count == 12 && store.viewCatalog?.next == wire.id(12))
        case "revoked":
            wire.unauthorized = true
            await store.loadNextViewCatalogPage()
            precondition(store.catalogStatus == .failed && store.viewCatalog == nil)
        case "changed-scope":
            wire.beforePageReply = {
                origin = .init(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
                store.synchronizeOrigin()
            }
            await store.loadNextViewCatalogPage()
            precondition(store.catalogStatus == .idle && store.viewCatalog == nil)
            wire.beforePageReply = nil
            wire.context.generation = "2"
            await store.refreshViewCatalog(includeArchived:true)
            precondition(wire.pages.last?.after == nil && wire.pages.last?.includeArchived == true && store.viewCatalog?.items.count == 12)
        case "uninitialized":
            wire.state = "uninitialized"
            await store.refreshViewCatalog(includeArchived:false)
            precondition(store.catalogStatus == .uninitialized && store.viewCatalog == nil && wire.pages.count == 1)
        default: preconditionFailure("unknown fixture mode")
        }
        print("PASS document discovery \(CommandLine.arguments[1])")
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
        let db = config["dbId"].string!, crew = config["crewId"].string!
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:crew)
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        func rpc(_ method: String, _ params: CrewdValue) throws -> CrewdValue {
            try send(.object(["method":.string(method),"params":params]))
            let envelope = try receive()
            if envelope["error"] != .null {
                throw CrewdClientError(message:envelope["error"]["message"].string ?? "refused",code:envelope["error"]["code"].int,data:envelope["error"]["data"])
            }
            return envelope["result"]
        }
        let store = CrewdKnowledgeStore(origin:{origin},rpc:{try rpc($0,$1)},journal:{journal})
        await store.refreshViewCatalog(includeArchived:false)
        precondition(store.viewCatalog?.items.count == 12 && store.viewCatalog?.items.allSatisfy{$0.kind == .web} == true)
        precondition(store.detail == nil,"metadata browsing never opens a page")
        for _ in 0..<4 { await store.loadNextViewCatalogPage() }
        let items = store.viewCatalog!.items
        let id = config["viewId"].string!
        precondition(items.count == 54 && store.viewCatalog?.next == nil)
        precondition(items.prefix(50).allSatisfy{$0.kind == .web})
        precondition(items.filter{ $0.kind == .document && CrewdKnowledgeTitleSearch.matches($0.title,query:" cafe 月度 ") }.map(\.id) == [id])
        await store.openViewCatalogItem(id,kind:.document)
        precondition(store.detail?.object.identity == id)
        let groups = CrewdChannelKnowledgeStore(call:{try rpc($0,$1)})
        for (key,expected) in [("linkedChannel",true),("emptyChannel",false)] {
            groups.selectChannel(crew:crew,channel:config[key].string!)
            try await groups.refresh()
            let found = groups.items.filter{$0.state == "ready" && CrewdKnowledgeTitleSearch.matches($0.title,query:"cafe 月度")}
            precondition(groups.loaded && (expected ? found.map{ $0.source.reference.id } == [id] : found.isEmpty))
        }
        try send(.object(["finished":.bool(true)]))
    }
}
