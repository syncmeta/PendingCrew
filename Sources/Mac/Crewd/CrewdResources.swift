#if os(macOS)
import Foundation
import Combine

enum CrewdResourceView:String,CaseIterable,Hashable {case messages,todo,documents,progress}
struct CrewdDocument:Identifiable {
    let id:String
    let summary:CrewdValue
    var body:String?
    var revision:String {summary["revision"].string ?? ""}
    var deleted:Bool {summary["deletedAt"] != .null}
}
struct CrewdFolderRow:Identifiable {let value:CrewdValue;let depth:Int;var id:String{value["id"].string ?? ""}}
struct CrewdFolderSnapshot {
    let value:CrewdValue
    var revision:String {value["treeRevision"].string ?? ""}
    var folders:[CrewdValue] {value["folders"].array}
    var assignments:[CrewdValue] {value["assignments"].array}
    var rows:[CrewdFolderRow] {
        func children(_ parent:String?,_ depth:Int)->[CrewdFolderRow]{
            guard depth < 32 else{return []}
            return folders.filter{$0["parentId"].string == parent}.sorted{a,b in
                let ap=a["position"].int ?? 0,bp=b["position"].int ?? 0
                return ap == bp ? (a["id"].string ?? "") < (b["id"].string ?? ""):ap < bp
            }.flatMap{folder in [CrewdFolderRow(value:folder,depth:depth)]+children(folder["id"].string,depth+1)}
        }
        return children(nil,0)
    }
    func folder(for object:CrewdValue)->String? {assignments.first(where:{$0["object"] == object})?["folderId"].string}
}

/// Read caches only. All content and folder state is owned by the crewd authority.
@MainActor final class CrewdResources:ObservableObject {
    typealias Call=(String,CrewdValue)async throws->CrewdValue
    @Published private(set) var crew=""
    @Published private(set) var capabilities:Set<String>=[]
    @Published private(set) var documents:[String:CrewdDocument]=[:]
    @Published private(set) var documentIDs:[String]=[]
    @Published private(set) var documentListLoaded=false
    @Published private(set) var folders:[CrewdResourceView:CrewdFolderSnapshot]=[:]
    @Published private(set) var failure:CrewdClientError?
    @Published private(set) var writing=false
    private let call:Call
    private var generation=UUID()
    private var listQuery=UUID()
    private var detailQueries:[String:UUID]=[:]
    private var folderQueries:[CrewdResourceView:UUID]=[:]
    private var activeView:CrewdResourceView?
    private var lastActivation:String?
    private var refreshTask:Task<Void,Never>?
    init(call:@escaping Call){self.call=call}
    func selectCrew(_ value:String){guard crew != value else{return};refreshTask?.cancel();generation=UUID();crew=value;documentIDs=[];documentListLoaded=false;lastActivation=nil;folders=[:];failure=nil;detailQueries=[:];folderQueries=[:]}
    func configureCapabilities(_ value:Set<String>){capabilities=value}
    func invalidateConnection(){refreshTask?.cancel();generation=UUID();capabilities=[];documentListLoaded=false;lastActivation=nil;detailQueries=[:];folderQueries=[:]}
    func activate(_ view:CrewdResourceView,force:Bool=false)async {
        activeView=view
        let key=crew+":"+view.rawValue+":"+capabilities.sorted().joined(separator:",")
        if !force,lastActivation == key{return};lastActivation=key;failure=nil
        if capabilities.contains("viewFolders"){do{_ = try await refreshFolders(view)}catch{record(error)}}
        if view == .documents,capabilities.contains("documents"){do{try await refreshDocuments()}catch{record(error)}}
    }
    func handleEvent(_ name:String,_ value:CrewdValue){
        guard value["crewId"] == .string(crew),let view=activeView else{return}
        guard name == "document.changed" || (name == "viewFolder.changed" && value["view"] == .string(view.rawValue)) else{return}
        if name == "document.changed",view != .documents{return}
        refreshTask?.cancel();let token=generation
        refreshTask=Task{[weak self] in
            try? await Task.sleep(for:.milliseconds(150))
            guard let self,!Task.isCancelled,self.generation == token,self.activeView == view else{return}
            await self.activate(view,force:true)
        }
    }
    private func record(_ error:Error){if !(error is CancellationError){failure=(error as? CrewdClientError) ?? CrewdClientError(message:error.localizedDescription)}}
    private func check(_ token:UUID)throws{guard token == generation,!Task.isCancelled else{throw CancellationError()}}
    private func require(_ capability:String)throws{guard capabilities.contains(capability) else{throw CrewdClientError(message:"配套后台尚未开放此功能。",kind:.notSubmitted)};guard !crew.isEmpty else{throw CrewdClientError(message:"请先选择机组。",kind:.notSubmitted)}}
    private func uuid(_ value:String)->Bool{UUID(uuidString:value) != nil}
    private func revision(_ value:CrewdValue,zero:Bool=false)->Bool{guard let s=value.string,!s.isEmpty,s.allSatisfy({$0.isASCII && $0.isNumber}) else{return false};return s == "0" ? zero:s.first != "0"}
    private func actor(_ value:CrewdValue)->Bool {value["kind"] == .string("human") || (value["kind"] == .string("session") && value["sessionId"].string != nil)}
    private func parseDocument(_ value:CrewdValue)throws->CrewdDocument {
        guard let id=value["id"].string,uuid(id),value["crewId"].string != nil,value["title"].string != nil,revision(value["revision"]),let length=value["bodyLength"].int,length >= 0,length <= 1024*1024,value["createdAt"].string != nil,value["updatedAt"].string != nil,actor(value["createdBy"]),actor(value["updatedBy"]),(value["deletedAt"] == .null || value["deletedAt"].string != nil) else{throw CrewdClientError(message:"文档回执缺少完整稳定元数据。")}
        var summary=value;if case .object(var fields)=value {fields.removeValue(forKey:"body");summary = .object(fields)}
        return CrewdDocument(id:id,summary:summary,body:nil)
    }
    private func adopt(_ incoming:CrewdDocument){
        if let old=documents[incoming.id] {
            if old.revision.count > incoming.revision.count || (old.revision.count == incoming.revision.count && old.revision > incoming.revision){return}
            var value=incoming;if incoming.revision == old.revision,incoming.body == nil {value.body=old.body};documents[incoming.id]=value
        }else{documents[incoming.id]=incoming}
    }
    func refreshDocuments()async throws{
        try require("documents");let token=generation,query=UUID(),scope=crew;listQuery=query
        do{
            var ids:[String]=[],cursor:String?,seen=Set<String>(),cursors=Set<String>()
            repeat{
                var params:[String:CrewdValue]=["crewId":.string(scope),"limit":.integer(100)]
                if let cursor{params["after"] = .string(cursor)}
                let page=try await call("document.list",.object(params));try check(token)
                guard listQuery == query else{throw CancellationError()}
                guard case .array = page["items"],page["items"].array.count <= 100,(page["next"] == .null || page["next"].string != nil) else{throw CrewdClientError(message:"文档列表或分页结构无效。")}
                for item in page["items"].array {let doc=try parseDocument(item);guard doc.summary["crewId"] == .string(scope),!doc.deleted,seen.insert(doc.id).inserted else{throw CrewdClientError(message:"文档列表返回了其他机组、已删除或重复对象。")};adopt(doc);ids.append(doc.id)}
                cursor=page["next"].string
                if let cursor {guard uuid(cursor),cursors.insert(cursor).inserted else{throw CrewdClientError(message:"文档分页游标无效。")}}
            }while cursor != nil
            documentIDs=ids;documentListLoaded=true;failure=nil
        }catch{if token == generation,!(error is CancellationError){failure=(error as? CrewdClientError) ?? CrewdClientError(message:error.localizedDescription)};throw error}
    }
    @discardableResult func document(_ id:String)async throws->CrewdDocument{
        try require("documents");guard uuid(id) else{throw CrewdClientError(message:"文档编号无效。",kind:.notSubmitted)}
        let token=generation,query=UUID();detailQueries[id]=query
        let value=try await call("document.get",.object(["id":.string(id)]));try check(token)
        guard detailQueries[id] == query else{throw CancellationError()}
        var doc=try parseDocument(value)
        guard doc.id == id,doc.summary["crewId"] == .string(crew),let body=value["body"].string,body.utf8.count == value["bodyLength"].int else{throw CrewdClientError(message:"文档详情对象或正文长度不符。")}
        doc.body=body;adopt(doc)
        guard let cached=documents[id],cached.revision == doc.revision,cached.body != nil else{throw CrewdClientError(message:"文档在读取时已更新，请重新读取当前正文。",code:-32009)}
        return cached
    }
    private func parseFolders(_ value:CrewdValue,_ view:CrewdResourceView)throws->CrewdFolderSnapshot {
        guard value["crewId"] == .string(crew),value["view"] == .string(view.rawValue),revision(value["treeRevision"],zero:true),case .array(let items)=value["folders"],case .array(let assignments)=value["assignments"],items.count <= 256,assignments.count <= 4096 else{throw CrewdClientError(message:"文件夹快照范围、版本或大小无效。")}
        var byID:[String:CrewdValue]=[:]
        for item in items {guard let id=item["id"].string,uuid(id),byID[id] == nil,let name=item["name"].string,!name.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,name.utf8.count <= 256,let position=item["position"].int,position >= 0,(item["parentId"] == .null || item["parentId"].string != nil) else{throw CrewdClientError(message:"文件夹元数据不完整。")};byID[id]=item}
        for item in items {
            var visited=Set<String>(),current=item["id"].string
            while let id=current {guard visited.insert(id).inserted,visited.count <= 32,let row=byID[id] else{throw CrewdClientError(message:"文件夹存在循环、缺失父级或超深层级。")};current=row["parentId"].string}
        }
        var objects=Set<String>()
        for item in assignments {
            let object=item["object"];guard let kind=object["kind"].string,let id=object["id"].string,let folder=item["folderId"].string,byID[folder] != nil,objects.insert(kind+":"+id).inserted else{throw CrewdClientError(message:"分类引用重复或不存在。")}
            let valid:Bool
            switch view {case .messages:valid=kind == "channel" && !id.isEmpty;case .documents:valid=kind == "document" && uuid(id);case .progress:valid=kind == "work" && CrewdWorkID(id,kind:.plan) != nil;case .todo:valid=(kind == "work" && CrewdWorkID(id,kind:.plan) != nil) || (kind == "requirement" && CrewdWorkID(id,kind:.requirement) != nil) || (kind == "humanTask" && CrewdWorkID(id,kind:.human) != nil)}
            guard valid else{throw CrewdClientError(message:"分类对象类型与当前视图不符。")}
        }
        return CrewdFolderSnapshot(value:value)
    }
    @discardableResult func refreshFolders(_ view:CrewdResourceView)async throws->CrewdFolderSnapshot {
        try require("viewFolders");let token=generation,query=UUID();folderQueries[view]=query
        let value=try await call("viewFolder.list",.object(["crewId":.string(crew),"view":.string(view.rawValue)]));try check(token)
        guard folderQueries[view] == query else{throw CancellationError()}
        let snapshot=try parseFolders(value,view);folders[view]=snapshot;failure=nil;return snapshot
    }
    private func mutate(_ method:String,_ params:CrewdValue)async throws->CrewdValue {
        try require(method.hasPrefix("document.") ? "documents":"viewFolders")
        guard !writing else{throw CrewdClientError(message:"文档或分组操作正在提交。",kind:.notSubmitted)}
        generation=UUID();let token=generation;writing=true;failure=nil
        defer{writing=false}
        do {let value=try await call(method,params);try check(token);return value}
        catch{if token == generation,!(error is CancellationError){failure=(error as? CrewdClientError) ?? CrewdClientError(message:error.localizedDescription)};throw error}
    }
    @discardableResult func createDocument(request:UUID,title:String,body:String)async throws->CrewdValue {
        try validate(title,body);let scope=crew
        let receipt=try await mutate("document.create",.object(["requestId":.string(request.uuidString.lowercased()),"crewId":.string(scope),"title":.string(title),"body":.string(body)]))
        let doc=try parseDocument(receipt["document"])
        guard receipt["requestId"].string.flatMap(UUID.init(uuidString:)) == request,case .bool = receipt["reused"],doc.summary["crewId"] == .string(scope) else{throw CrewdClientError(message:"文档创建回执与原始请求不符。")}
        adopt(doc);try? await refreshDocuments();return receipt
    }
    private func validate(_ title:String,_ body:String)throws{guard !title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,title.utf8.count <= 512,body.utf8.count <= 1024*1024 else{throw CrewdClientError(message:"标题须为1至512字节，正文最多1MiB。",kind:.notSubmitted)}}
    @discardableResult func updateDocument(id:String,revision:String,title:String,body:String)async throws->CrewdDocument {
        try validate(title,body)
        let value=try await mutate("document.update",.object(["id":.string(id),"expectedRevision":.string(revision),"title":.string(title),"body":.string(body)]));let doc=try parseDocument(value)
        guard doc.id == id,doc.summary["crewId"] == .string(crew) else{throw CrewdClientError(message:"文档更新回执对象不符。")};adopt(doc);try? await refreshDocuments();return doc
    }
    @discardableResult func deleteDocument(id:String,revision:String)async throws->CrewdDocument {
        let value=try await mutate("document.delete",.object(["id":.string(id),"expectedRevision":.string(revision)]));let doc=try parseDocument(value)
        guard doc.id == id,doc.deleted,doc.summary["crewId"] == .string(crew) else{throw CrewdClientError(message:"文档删除回执对象或状态不符。")};adopt(doc);try? await refreshDocuments();return doc
    }
    @discardableResult func changeFolder(_ method:String,view:CrewdResourceView,fields:[String:CrewdValue])async throws->CrewdFolderSnapshot {
        guard ["viewFolder.create","viewFolder.update","viewFolder.delete","viewFolder.assign"].contains(method) else{throw CrewdClientError(message:"不支持的分组操作。",kind:.notSubmitted)}
        var params=fields;params["crewId"] = .string(crew);params["view"] = .string(view.rawValue)
        let value=try await mutate(method,.object(params));let snapshot=try parseFolders(value,view);folders[view]=snapshot;failure=nil;return snapshot
    }
}
#endif
