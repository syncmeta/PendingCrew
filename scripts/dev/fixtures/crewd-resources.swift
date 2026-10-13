import Foundation
let documentID="10000000-0000-0000-0000-000000000001",folderID="20000000-0000-0000-0000-000000000001"
func summary(_ revision:String="1",title:String="原始文档",body:String="私有正文🌙",crew:String="a")->CrewdValue {.object(["id":.string(documentID),"crewId":.string(crew),"title":.string(title),"revision":.string(revision),"bodyLength":.integer(Int64(body.utf8.count)),"createdBy":.object(["kind":.string("human")]),"updatedBy":.object(["kind":.string("human")]),"createdAt":.string("2026-10-14T00:00:00Z"),"updatedAt":.string("2026-10-14T00:00:00Z"),"deletedAt":.null])}
func withBody(_ value:CrewdValue,_ body:String)->CrewdValue {guard case .object(var fields)=value else{preconditionFailure()};fields["body"] = .string(body);return .object(fields)}
func tree(_ crew:String="a",_ view:String="documents",revision:String="0",folders:[CrewdValue]=[],assignments:[CrewdValue]=[])->CrewdValue {.object(["crewId":.string(crew),"view":.string(view),"treeRevision":.string(revision),"folders":.array(folders),"assignments":.array(assignments)])}
func folder(_ id:String=folderID,parent:String?=nil)->CrewdValue {.object(["id":.string(id),"name":.string("独立分类"),"parentId":parent.map(CrewdValue.string) ?? .null,"position":.integer(0)])}
@MainActor final class ResourcePeer {
    var calls:[(String,CrewdValue)]=[],row=summary(),body="私有正文🌙",snapshot=tree()
    var error:CrewdClientError?,delay=false,held:CheckedContinuation<CrewdValue,Error>?
    var wrongRequest=false
    func call(_ method:String,_ params:CrewdValue)async throws->CrewdValue {
        calls.append((method,params));if let error{throw error}
        if method == "document.get",delay{delay=false;return try await withCheckedThrowingContinuation{held=$0}}
        switch method {
        case "document.list":return .object(["items":.array([row]),"next":.null])
        case "document.get":return withBody(row,body)
        case "document.create":return .object(["document":row,"requestId":wrongRequest ? .string(UUID().uuidString):params["requestId"],"reused":.bool(true)])
        case "document.update":return row
        case "viewFolder.list","viewFolder.assign","viewFolder.create","viewFolder.update","viewFolder.delete":return snapshot
        default:throw CrewdClientError(message:"unexpected resource method "+method)
        }
    }
}
@MainActor func rejects(_ action:()async throws->Void)async {do{try await action();preconditionFailure("invalid resource accepted")}catch{}}
@main struct ResourcesTest {
    @MainActor static func main()async throws {
        let peer=ResourcePeer(),store=CrewdResources(call:{try await peer.call($0,$1)})
        store.selectCrew("a")
        await rejects{_ = try await store.createDocument(request:UUID(),title:"new",body:"exact")}
        precondition(peer.calls.isEmpty,"missing documents cap issued no call")
        store.configureCapabilities(["documents","viewFolders"])
        try await store.refreshDocuments();precondition(store.documents[documentID]!.body == nil && store.documentIDs == [documentID])
        _ = try await store.document(documentID);precondition(store.documents[documentID]!.body == peer.body,"UTF8 body bytes preserved")
        try await store.refreshDocuments();precondition(store.documents[documentID]!.body == peer.body,"same-revision summary keeps fetched detail")
        peer.row=summary("2",title:"共同编辑后的标题")
        try await store.refreshDocuments();precondition(store.documents[documentID]!.body == nil,"new revision invalidates old body")
        let request=UUID();_ = try await store.createDocument(request:request,title:"original create title",body:"original immutable payload")
        precondition(store.documents[documentID]!.summary["title"] == .string("共同编辑后的标题"),"successful idempotent retry returns current metadata after collaborative edits")
        let create=peer.calls.first{$0.0 == "document.create"}!
        precondition(create.1["requestId"] == .string(request.uuidString.lowercased()) && create.1["body"] == .string("original immutable payload"))
        peer.wrongRequest=true;await rejects{_ = try await store.createDocument(request:request,title:"original create title",body:"original immutable payload")};peer.wrongRequest=false
        peer.body="wrong byte length";await rejects{_ = try await store.document(documentID)};peer.body="私有正文🌙"
        let facts=CrewdValue.object(["currentDocument":summary("3"),"currentTreeRevision":.string("9007199254740993"),"unknown":.bool(true)])
        peer.error=CrewdClientError(message:"CAS conflict",code:-32009,data:facts)
        do {_ = try await store.updateDocument(id:documentID,revision:"2",title:"desired",body:"text");preconditionFailure()}catch let e as CrewdClientError{precondition(e.data == facts && store.failure?.data == facts)};peer.error=nil
        _ = try await store.refreshFolders(.documents);precondition(store.folders[.documents]!.revision == "0","empty scope revision0 valid")
        peer.snapshot=tree("a","documents",revision:"1",folders:[folder()],assignments:[.object(["object":.object(["kind":.string("document"),"id":.string(documentID)]),"folderId":.string(folderID)])])
        _ = try await store.refreshFolders(.documents);precondition(store.folders[.documents]!.folder(for:.object(["kind":.string("document"),"id":.string(documentID)])) == folderID)
        let valid=peer.snapshot
        let duplicateAssignment=CrewdValue.object(["object":.object(["kind":.string("document"),"id":.string(documentID)]),"folderId":.string(folderID)])
        let ids=(0..<257).map{String(format:"30000000-0000-0000-0000-%012d",$0+1)}
        let deep=(0..<33).map{folder(ids[$0],parent:$0==0 ? nil:ids[$0-1])}
        let overflow=ids.map{folder($0)}
        let bounds=[tree("a","documents",revision:"00"),tree("a","documents",folders:deep),tree("a","documents",folders:overflow),tree("a","documents",folders:[folder()],assignments:[duplicateAssignment,duplicateAssignment]),tree("a","documents",folders:[folder()],assignments:Array(repeating:duplicateAssignment,count:4097)),tree("a","documents",folders:[folder(parent:ids[0])])]
        for bad in bounds {peer.snapshot=bad;await rejects{_ = try await store.refreshFolders(.documents)};precondition(store.folders[.documents]!.value == valid,"malformed/deep/overflow snapshot must not replace valid scope")}
        for bad in [tree("b"),tree("a","progress"),tree("a","documents",folders:[folder(parent:folderID)]),tree("a","documents",folders:[folder(),folder()])]{peer.snapshot=bad;await rejects{_ = try await store.refreshFolders(.documents)};precondition(store.folders[.documents]!.value == valid)}
        peer.snapshot=tree("a","documents",folders:[folder()],assignments:[.object(["object":.object(["kind":.string("work"),"id":.string("W1")]),"folderId":.string(folderID)])]);await rejects{_ = try await store.refreshFolders(.documents)}
        peer.delay=true;let pending=Task{try await store.document(documentID)}
        while peer.held == nil {await Task.yield()}
        store.selectCrew("b");peer.held!.resume(returning:withBody(summary("9",title:"late old crew"),"私有正文🌙"));peer.held=nil
        do {_ = try await pending.value;preconditionFailure("late old scope adopted")}catch is CancellationError{}
        precondition(store.documentIDs.isEmpty && store.documents[documentID]!.summary["title"] != .string("late old crew"))
        var drafts=CrewdResourceDrafts();let key=CrewdResourceDrafts.Key(crew:"a",view:.documents,object:"",operation:"create")
        var fields=CrewdResourceDrafts.Fields();fields.title="original";fields.body="exact bytes";drafts.edit(key,fields)
        let old=drafts.begin(key)!;precondition(drafts.begin(key) == nil);drafts.finish(old,accepted:false,uncertain:true)
        fields.body="edited later";drafts.edit(key,fields);let fresh=drafts.begin(key)!;drafts.finish(old,accepted:true)
        precondition(drafts.busy(key) && drafts.fields(key).body == "edited later" && old.request != fresh.request,"old receipt cannot clear edited input or reuse another payload UUID")
        drafts.finish(fresh,accepted:false,uncertain:true);precondition(drafts.uncertain(key).first?.fields.body == "edited later")
        store.selectCrew("a");peer.row=summary("4");peer.snapshot=tree("a","documents",revision:"2",folders:[folder()])
        await store.activate(.documents)
        peer.row=summary("5",title:"after dropped event");peer.snapshot=tree("a","documents",revision:"3",folders:[folder()])
        let beforeLag=peer.calls.count
        store.handleEvent("events.lagged",.object(["dropped":.integer(2)]))
        store.handleEvent("events.lagged",.object(["dropped":.integer(1)]))
        try await Task.sleep(nanoseconds:350_000_000)
        precondition(store.documents[documentID]!.revision == "5" && store.folders[.documents]!.revision == "3","global lagged event with no crewId must restore the current active resource facts")
        precondition(peer.calls.dropFirst(beforeLag).map{$0.0}.sorted() == ["document.list","viewFolder.list"],"two lagged events should coalesce one bounded read-only active view refresh without mutations")
        let count=peer.calls.count;store.invalidateConnection();await rejects{_ = try await store.document(documentID)};precondition(peer.calls.count == count,"disconnect clears capabilities, no implicit reads/writes")
        print("PASS: resource exact bytes/current-metadata idempotency/capability/metadata-CAS/folder boundaries/scope generation/window drafts; synthetic only")
    }
}
