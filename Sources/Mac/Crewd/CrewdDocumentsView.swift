#if os(macOS)
import SwiftUI
struct CrewdDocumentTarget:Identifiable {let key:CrewdResourceDrafts.Key;var id:String{key.crew+":"+key.object+":"+key.operation}}
struct CrewdDocumentsView:View {
    @ObservedObject var store:CrewdResources
    @Binding var drafts:CrewdResourceDrafts
    @State private var selected:String?
    @State private var target:CrewdDocumentTarget?
    @State private var failure:String?
    private var selection:Binding<String>{Binding(get:{drafts.selection(store.crew,.documents)},set:{drafts.select(store.crew,.documents,$0)})}
    private func readDocument(_ id:String)async {let scope=store.crew;do{_ = try await store.document(id);if store.crew == scope{failure=nil}}catch{if store.crew == scope,!(error is CancellationError){failure=error.localizedDescription}}}
    var body:some View {
        VStack(alignment:.leading,spacing:10){
            HStack{Text("文档").font(.headline);Spacer();Button("新建文档"){target = .init(key:.init(crew:store.crew,view:.documents,object:"",operation:"create"))}.disabled(!store.capabilities.contains("documents") || store.writing)}
            CrewdFolderControls(store:store,drafts:$drafts,selection:selection,view:.documents)
            if let error=store.failure{Text(error.localizedDescription).foregroundStyle(.orange).textSelection(.enabled)}
            if !store.capabilities.contains("documents"){Text("配套后台尚未开放文档，当前不能读取或编辑。").foregroundStyle(.secondary)}
            else {
                HStack(alignment:.top,spacing:16){
                    ScrollView{VStack(alignment:.leading,spacing:10){ForEach(store.documentIDs.filter{CrewdFolderFilter.includes(CrewdFolderFilter.object("document",$0),selection:selection.wrappedValue,snapshot:store.folders[.documents])},id:\.self){id in
                        if let doc=store.documents[id]{Button{selected=id;Task{await readDocument(id)}}label:{VStack(alignment:.leading){Text(doc.summary["title"].string ?? "文档");Text(doc.summary["updatedAt"].string ?? "").font(.caption).foregroundStyle(.secondary)}}.buttonStyle(.plain)}
                    }}.frame(maxWidth:.infinity,alignment:.leading)}.frame(width:200)
                    Divider()
                    if let selected,let doc=store.documents[selected]{
                        VStack(alignment:.leading,spacing:10){
                            Text(doc.summary["title"].string ?? "文档").font(.title2)
                            if doc.deleted{Text("已删除；原文和审计记录保留。").font(.caption).foregroundStyle(.secondary)}
                            CrewdFolderAssignmentButton(store:store,drafts:$drafts,view:.documents,object:CrewdFolderFilter.object("document",selected))
                            HStack{Button("读取当前正文"){Task{await readDocument(selected)}};Button("编辑 / 删除"){target = .init(key:.init(crew:store.crew,view:.documents,object:selected,operation:"update"))}.disabled(doc.deleted || store.writing)}
                            if let body=doc.body {ScrollView{Text(body).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading)}}else{Text("正文尚未读取，或版本更新后需要重新读取。").foregroundStyle(.secondary)}
                        }.frame(maxWidth:.infinity,alignment:.leading)
                    }else{Text(!store.documentListLoaded ? "文档列表尚未成功读取":store.documentIDs.isEmpty ? "尚无文档":"选择文档读取正文").foregroundStyle(.secondary).frame(maxWidth:.infinity,alignment:.leading)}
                }
            }
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
        }.padding(16).onChange(of:store.crew){_,_ in selected=nil;target=nil;failure=nil}.task(id:store.crew+String(store.capabilities.contains("documents"))){await store.activate(.documents)}.sheet(item:$target){target in CrewdDocumentEditor(store:store,drafts:$drafts,target:target)}
    }
}
struct CrewdDocumentEditor:View {
    @ObservedObject var store:CrewdResources
    @Binding var drafts:CrewdResourceDrafts
    let target:CrewdDocumentTarget
    @Environment(\.dismiss) private var dismiss
    @State private var current:CrewdDocument?
    @State private var failure:String?
    @State private var reading=false
    @State private var confirmDelete=false
    @State private var submitting=false
    private var key:CrewdResourceDrafts.Key{target.key}
    private var fields:CrewdResourceDrafts.Fields{drafts.fields(key)}
    private var deleteKey:CrewdResourceDrafts.Key{.init(crew:key.crew,view:.documents,object:key.object,operation:"delete")}
    private func bind<T>(_ path:WritableKeyPath<CrewdResourceDrafts.Fields,T>)->Binding<T>{Binding(get:{drafts.fields(key)[keyPath:path]},set:{var f=drafts.fields(key);f[keyPath:path]=$0;drafts.edit(key,f)})}
    var body:some View{
        VStack(alignment:.leading,spacing:12){
            Text(key.object.isEmpty ? "新建文档":"编辑文档").font(.title2)
            Text("本机组共用文档").font(.caption).foregroundStyle(.secondary)
            ForEach(drafts.uncertain(key)+drafts.uncertain(deleteKey),id:\.version){snapshot in
                VStack(alignment:.leading,spacing:4){
                    Text(snapshot.key.operation == "delete" ? "此前删除结果未确认；请读取当前状态核对，不会自动重复删除。":"此前提交结果未确认；原文仍保留，不会自动重发。").foregroundStyle(.orange)
                    Text(snapshot.fields.title).font(.headline);Text(snapshot.fields.body).lineLimit(3).textSelection(.enabled)
                    if key.object.isEmpty{Button("确认这份原始创建请求"){if drafts.confirm(snapshot){send(snapshot,delete:false)}}.disabled(drafts.busy(key) || submitting || store.writing || store.crew != key.crew)}
                }.font(.caption)
            }
            TextField("标题",text:bind(\.title)).disabled(drafts.busy(key) || submitting)
            TextEditor(text:bind(\.body)).font(.body).disabled(drafts.busy(key) || submitting).frame(minHeight:180)
            if !key.object.isEmpty{
                Button("读取当前版本供比较"){Task{await read()}}.disabled(reading || store.writing || submitting)
                if let current,fields.revision != current.revision{
                    Text("文档已更新，请比较当前正文后决定如何保留你的编辑。").foregroundStyle(.orange)
                    DisclosureGroup("后台当前正文"){ScrollView{Text(current.body ?? "").textSelection(.enabled)}.frame(maxHeight:140)}
                    Button("我已比较，保留编辑并使用当前版本提交"){var f=fields;f.revision=current.revision;drafts.edit(key,f)}.disabled(drafts.busy(key) || submitting)
                }
                Button("删除文档"){confirmDelete=true}.disabled(drafts.busy(key) || submitting || store.writing || current == nil)
            }
            if let failure{Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            HStack{Button("关闭"){dismiss()};Spacer();Button(drafts.busy(key) ? "提交中":"保存"){if let snapshot=drafts.begin(key){send(snapshot,delete:false)}}.disabled(drafts.busy(key) || submitting || store.writing || reading || store.crew != key.crew || !store.capabilities.contains("documents") || (!key.object.isEmpty && fields.revision.isEmpty))}
        }.padding(20).frame(width:600,height:620).task{if key.object.isEmpty {if !drafts.exists(key){drafts.edit(key,.init())}}else{await read()}}
        .confirmationDialog("删除文档？原文和审计记录保留，普通列表将不再显示。",isPresented:$confirmDelete){Button("删除",role:.destructive){drafts.edit(deleteKey,fields);if let snapshot=drafts.begin(deleteKey){send(snapshot,delete:true)}}}
    }
    private func describe(_ error:Error)->String{guard let e=error as? CrewdClientError else{return error.localizedDescription};return [e.message,e.code.map{"错误 \($0)"},e.refusal,e.data["currentDocument"]["revision"].string.map{"当前版本："+$0}].compactMap{$0}.joined(separator:" · ")}
    private func read()async{
        reading=true;defer{reading=false}
        do{let value=try await store.document(key.object);guard store.crew == key.crew else{return};current=value;failure=nil
            if !drafts.exists(key){var f=CrewdResourceDrafts.Fields();f.title=value.summary["title"].string ?? "";f.body=value.body ?? "";f.revision=value.revision;drafts.edit(key,f)}
        }catch{if store.crew == key.crew{failure=describe(error)}}
    }
    private func send(_ snapshot:CrewdResourceDrafts.Snapshot,delete:Bool){
        guard !submitting else{drafts.finish(snapshot,accepted:false);return};submitting=true
        Task{
            defer{submitting=false}
            var attempted=false
            do{
                guard store.crew == snapshot.key.crew else{throw CancellationError()}
                if !key.object.isEmpty{
                    let latest=try await store.document(key.object)
                    guard store.crew == snapshot.key.crew else{throw CancellationError()}
                    current=latest
                    guard latest.revision == snapshot.fields.revision,!latest.deleted else{throw CrewdClientError(message:"当前文档版本或状态已变化，请比较后再决定。",code:-32009,data:.object(["currentDocument":latest.summary]))}
                }
                attempted=true
                if key.object.isEmpty{_ = try await store.createDocument(request:snapshot.request,title:snapshot.fields.title,body:snapshot.fields.body)}
                else if delete{_ = try await store.deleteDocument(id:key.object,revision:snapshot.fields.revision)}
                else{_ = try await store.updateDocument(id:key.object,revision:snapshot.fields.revision,title:snapshot.fields.title,body:snapshot.fields.body)}
                drafts.finish(snapshot,accepted:true);if store.crew == key.crew{dismiss()}
            }catch{drafts.finish(snapshot,accepted:false,uncertain:attempted && CrewdClientError.isUncertainSubmission(error));if store.crew == key.crew{failure=describe(error)}}
        }
    }
}
#endif
