import SwiftUI

enum CrewdFolderFilter {
    static func includes(_ object:CrewdValue,selection:String,snapshot:CrewdFolderSnapshot?)->Bool {
        if selection == "all" {return true}
        guard let snapshot else{return false}
        let folder=snapshot.folder(for:object)
        return selection == "none" ? folder == nil:folder == selection
    }
    static func object(_ kind:String,_ id:String)->CrewdValue{.object(["kind":.string(kind),"id":.string(id)])}
    static func depth(_ item:CrewdValue,in snapshot:CrewdFolderSnapshot)->Int {
        var parent=item["parentId"].string,depth=0,seen=Set<String>()
        while let id=parent,seen.insert(id).inserted,depth < 32 {depth += 1;parent=snapshot.folders.first(where:{$0["id"] == .string(id)})?["parentId"].string}
        return depth
    }
}
struct CrewdFolderControls:View {
    @ObservedObject var store:CrewdResources
    @Binding var drafts:CrewdResourceDrafts
    @Binding var selection:String
    let view:CrewdResourceView
    var object:CrewdValue?=nil
    var compact=false
    var iconOnly=false
    @State private var managing=false
    @State private var classifying=false
    var body:some View{
        VStack(alignment:.leading,spacing:4){HStack{
            if store.capabilities.contains("viewFolders"){
                if compact {
                    Menu {
                        Picker("本群文件夹",selection:$selection){folderChoices}
                        Divider()
                        Button("管理本群分类",systemImage:"folder.badge.gearshape"){managing=true}
                    }label:{if iconOnly{Image(systemName:"folder")}else{Label(currentFolderTitle,systemImage:"folder")}}
                    .fixedSize().frame(minWidth:iconOnly ? CrewdType.target:0,minHeight:iconOnly ? CrewdType.target:0).help("消息分类："+currentFolderTitle+"；管理分类不会改变原文").accessibilityLabel("消息分类："+currentFolderTitle)
                }else {
                    Picker("文件夹",selection:$selection){folderChoices}.frame(maxWidth:220)
                    Button("管理分类",systemImage:"folder.badge.gearshape"){managing=true}.labelStyle(.iconOnly).help(store.channel.isEmpty ? "管理机组资料库分类":"管理本群分类")
                }
                if object != nil {Button("归入文件夹"){classifying=true}}
            }else{Text("文件夹 · 配套后台待开放").font(.caption).foregroundStyle(.secondary)}
            Spacer()
        };if store.capabilities.contains("viewFolders"){
            if !compact{Text(store.channel.isEmpty ? "机组资料库分类":"本群分类").font(.caption).foregroundStyle(.secondary)}
            if let snapshot=store.folders[view],selection != "all",selection != "none",!snapshot.folders.contains(where:{$0["id"] == .string(selection)}){Text("所选文件夹已不在当前目录，请重新选择。").font(.caption).foregroundStyle(.orange)}
        }}.onChange(of:store.crew+store.channel){_,_ in managing=false;classifying=false}.sheet(isPresented:$managing){CrewdFolderEditor(store:store,drafts:$drafts,view:view)}
        .sheet(isPresented:$classifying){if let object{CrewdFolderEditor(store:store,drafts:$drafts,view:view,object:object)}}
        .task(id:store.crew+store.channel+view.rawValue+store.capabilities.sorted().joined(separator:",")+store.activationEpoch.uuidString){await store.activate(view)}
    }
    private var currentFolderTitle:String {
        if selection == "all"{return "全部分类"}
        if selection == "none"{return "未分类"}
        return store.folders[view]?.folders.first(where:{$0["id"] == .string(selection)})?["name"].string ?? "原分类不可用"
    }
    @ViewBuilder private var folderChoices:some View {
        Text("全部").tag("all");Text("未分类").tag("none")
        ForEach(store.folders[view]?.rows ?? []){row in Text(String(repeating:"　",count:row.depth)+(row.value["name"].string ?? "文件夹")).tag(row.id)}
    }

}
struct CrewdFolderAssignmentButton:View {
    @ObservedObject var store:CrewdResources
    @Binding var drafts:CrewdResourceDrafts
    let view:CrewdResourceView
    let object:CrewdValue
    @State private var editing=false
    var body:some View {if store.capabilities.contains("viewFolders"){Button("分类",systemImage:"folder"){editing=true}.font(.caption).sheet(isPresented:$editing){CrewdFolderEditor(store:store,drafts:$drafts,view:view,object:object)}.onChange(of:store.crew+store.channel){_,_ in editing=false}}}
}
struct CrewdFolderEditor:View {
    @ObservedObject var store:CrewdResources
    @Binding var drafts:CrewdResourceDrafts
    let view:CrewdResourceView
    var object:CrewdValue?=nil
    @Environment(\.dismiss) private var dismiss
    @State private var operation="create"
    @State private var selected=""
    @State private var failure:String?
    @State private var reading=false
    @State private var confirmDelete=false
    @State private var submitting=false
    private var key:CrewdResourceDrafts.Key{.init(crew:store.crew,view:view,object:object?["id"].string ?? (operation == "create" ? "":selected),operation:object == nil ? operation:"assign",domain:.folder,channel:store.channel)}
    private var fields:CrewdResourceDrafts.Fields{drafts.fields(key)}
    private var snapshot:CrewdFolderSnapshot?{store.folders[view]}
    private func bind<T>(_ path:WritableKeyPath<CrewdResourceDrafts.Fields,T>)->Binding<T>{Binding(get:{drafts.fields(key)[keyPath:path]},set:{var f=drafts.fields(key);f[keyPath:path]=$0;drafts.edit(key,f)})}
    var body:some View{
        VStack(alignment:.leading,spacing:12){
            Text(object == nil ? "管理文件夹":"归入文件夹").font(.title2)
            Text(store.channel.isEmpty ? "机组资料库分类，独立于计划层级；仅改变展示位置。":"本群独立分类，只改变此处展示位置；共享原文与状态不变。").font(.caption).foregroundStyle(.secondary)
            if object == nil{
                Picker("操作",selection:$operation){Text("新建").tag("create");Text("修改 / 移动 / 重排").tag("update");Text("解散").tag("delete")}.pickerStyle(.segmented).disabled(store.writing || submitting)
                if operation != "create"{Picker("文件夹",selection:$selected){Text("请选择").tag("");ForEach(snapshot?.rows ?? []){row in Text(String(repeating:"　",count:row.depth)+(row.value["name"].string ?? "")).tag(row.id)}}.disabled(store.writing || submitting)}
            }
            if object != nil{folderPicker("归入",selection:bind(\.folder),rootLabel:"未分类")}
            else if operation != "delete"{TextField("文件夹名称",text:bind(\.title)).disabled(submitting || store.writing);folderPicker("父文件夹",selection:bind(\.parent),rootLabel:"顶层");if operation == "update"{Stepper("同级顺序：\(fields.position+1)",value:bind(\.position),in:0...255).disabled(submitting || store.writing)}}
            else{Text("子文件夹移到上一层，直接内容回到未分类；不删除文档、消息、需求或计划。").foregroundStyle(.secondary)}
            ForEach(drafts.uncertain(key),id:\.version){old in VStack(alignment:.leading,spacing:4){Text("此前操作结果未确认，不会自动重发。").foregroundStyle(.orange);Text(old.fields.title).textSelection(.enabled);if key.operation == "create"{Button("确认这份原始创建请求"){if drafts.confirm(old){send(old,reuse:true)}}.disabled(store.writing || submitting)}}.font(.caption)}
            Text("已核对目录版本："+fields.revision).font(.caption).foregroundStyle(.secondary)
            Button("读取当前目录供核对"){Task{await read()}}.disabled(reading || store.writing || submitting)
            if let snapshot,fields.revision != snapshot.revision{Text("目录已改变，请核对名称、位置与归类后再决定。").foregroundStyle(.orange);Button("我已核对，使用当前目录版本"){var f=fields;f.revision=snapshot.revision;drafts.edit(key,f)}.disabled(submitting || store.writing)}
            if let failure{Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            Spacer()
            HStack{Button("关闭"){dismiss()};Spacer();Button(store.writing ? "提交中":"提交"){
                if key.operation == "delete"{confirmDelete=true}else if let draft=drafts.begin(key){send(draft)}
            }.buttonStyle(CrewdPrimaryButtonStyle()).disabled(store.writing || submitting || reading || fields.revision.isEmpty || (!store.capabilities.contains("viewFolders")) || (object == nil && operation != "create" && selected.isEmpty))}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:560,height:500)).task(id:key){await read()}
        .confirmationDialog("解散这个文件夹？不会删除其中的内容。",isPresented:$confirmDelete){Button("解散文件夹",role:.destructive){if let draft=drafts.begin(key){send(draft)}}}
    }
    private func folderPicker(_ label:String,selection:Binding<String>,rootLabel:String)->some View{
        Picker(label,selection:selection){Text(rootLabel).tag("");ForEach(snapshot?.rows.filter{$0.id != selected} ?? []){row in Text(String(repeating:"　",count:row.depth)+(row.value["name"].string ?? "")).tag(row.id)}}.disabled(store.writing || submitting)
    }
    private func read()async{
        reading=true;defer{reading=false};let original=key
        do{let latest=try await store.refreshFolders(view);guard key == original else{return};failure=nil
            if !drafts.exists(original){var f=CrewdResourceDrafts.Fields();f.revision=latest.revision
                if let object{f.folder=latest.folder(for:object) ?? ""}
                else if operation != "create",let row=latest.folders.first(where:{$0["id"].string == selected}){f.title=row["name"].string ?? "";f.parent=row["parentId"].string ?? "";f.position=row["position"].int ?? 0}
                drafts.edit(original,f)
            }
        }catch{if key == original{failure=describe(error)}}
    }
    private func describe(_ error:Error)->String{guard let e=error as? CrewdClientError else{return error.localizedDescription};return [e.message,e.code.map{"错误 \($0)"},e.refusal,e.data["currentTreeRevision"].string.map{"当前目录版本："+$0}].compactMap{$0}.joined(separator:" · ")}
    private func send(_ draft:CrewdResourceDrafts.Snapshot,reuse:Bool=false){
        guard !submitting else{drafts.finish(draft,accepted:false);return};submitting=true
        Task{
            defer{submitting=false}
            var attempted=false
            do{
                guard store.crew == draft.key.crew && store.channel == draft.key.channel else{throw CancellationError()}
                let latest=try await store.refreshFolders(view)
                guard store.crew == draft.key.crew && store.channel == draft.key.channel else{throw CancellationError()}
                if !reuse {guard latest.revision == draft.fields.revision else{throw CrewdClientError(message:"目录版本已变化，请核对后再决定。",code:-32009,data:.object(["currentTreeRevision":.string(latest.revision)]))}}
                var params:[String:CrewdValue]=["expectedTreeRevision":.string(draft.fields.revision)]
                switch draft.key.operation{
                case "create":params["requestId"] = .string(draft.request.uuidString.lowercased());params["name"] = .string(draft.fields.title);params["parentId"] = draft.fields.parent.isEmpty ? .null:.string(draft.fields.parent)
                case "update":params["id"] = .string(draft.key.object);params["name"] = .string(draft.fields.title);params["parentId"] = draft.fields.parent.isEmpty ? .null:.string(draft.fields.parent);params["position"] = .integer(Int64(draft.fields.position))
                case "delete":params["id"] = .string(draft.key.object)
                case "assign":guard let object else{throw CrewdClientError(message:"尚未选择分类对象。",kind:.notSubmitted)};params["object"] = object;params["folderId"] = draft.fields.folder.isEmpty ? .null:.string(draft.fields.folder)
                default:throw CrewdClientError(message:"未知文件夹操作。",kind:.notSubmitted)
                }
                if ["create","update"].contains(draft.key.operation){guard !draft.fields.title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,draft.fields.title.utf8.count <= 256 else{throw CrewdClientError(message:"文件夹名称不能为空，且最多256字节。",kind:.notSubmitted)}}
                attempted=true
                _ = try await store.changeFolder("viewFolder."+draft.key.operation,view:view,fields:params)
                drafts.finish(draft,accepted:true);if key == draft.key{dismiss()}
            }catch{drafts.finish(draft,accepted:false,uncertain:attempted && CrewdClientError.isUncertainSubmission(error));if store.crew == draft.key.crew && store.channel == draft.key.channel{failure=describe(error)}}
        }
    }
}
