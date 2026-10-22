import SwiftUI
import UniformTypeIdentifiers

struct CrewdCodexInputChoice {
    var ids=Set<UUID>()
    var skills:[CrewdValue]=[]
    @MainActor func value(store:CrewdCodexInputStore,context:CrewdCodexInputStore.Context)throws->CrewdValue?{
        let refs=try store.selected(ids,context:context)
        if refs.isEmpty && skills.isEmpty{return nil}
        var fields:[String:CrewdValue]=[:];if !refs.isEmpty{fields["inputs"] = .array(refs)};if !skills.isEmpty{fields["skills"] = .array(skills)}
        return try CrewdCodexInputWire.selection(.object(fields))
    }
}

/// Explicit source-bound selection; only references/metadata live in the App.
struct CrewdCodexInputPicker:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var store:CrewdCodexInputStore
    let session:String?
    let title:String
    let allowMedia:Bool,allowSkills:Bool
    private let crew:String
    @Binding var choice:CrewdCodexInputChoice
    @State private var importing=false
    @State private var rows:[CrewdCodexInputStore.Entry]=[]
    @State private var pending:[CrewdPresentationJournal.Prepared]=[]
    @State private var skills:[CrewdValue]=[]
    @State private var error:String?
    @State private var loading=false
    init(workspace:CrewdWorkspace,session:String?,title:String,choice:Binding<CrewdCodexInputChoice>,allowMedia:Bool=true,allowSkills:Bool=true){self.allowMedia=allowMedia;self.allowSkills=allowSkills;crew=workspace.crewID;self.workspace=workspace;self.session=session;self.title=title;_choice=choice;store=workspace.codexInputs}
    private var context:CrewdCodexInputStore.Context{.init(crew:crew,session:session)}
    private var identity:String{workspace.crewID+"/"+(session ?? "new")+"/"+(store.currentOrigin?.connection.uuidString ?? "offline")}
    var body:some View{
        DisclosureGroup(title){VStack(alignment:.leading,spacing:10){
            Text("添加仅保存原输入，不会发送任务。只支持图片、音频和UTF-8文本；PDF/其它二进制暂不支持。输入不能随个人草稿跨设备传递。").font(.caption).foregroundStyle(.secondary)
            Button("选择输入文件"){importing=true}.disabled(store.currentOrigin==nil || !allowMedia)
            ForEach(rows){row in
                VStack(alignment:.leading,spacing:4){
                    if row.state=="prepared"{
                        Toggle(row.record.params["filename"].string ?? "原输入",isOn:Binding(get:{choice.ids.contains(row.id)},set:{if $0{choice.ids.insert(row.id)}else{choice.ids.remove(row.id)}}))
                        Button("丢弃原暂存"){run{try await store.discard(row,context:context)}}.font(.caption)
                    }else{Text((row.record.params["filename"].string ?? "原输入")+" · "+stateLabel(row.state)).font(.caption)}
                }
            }
            ForEach(pending){record in
                VStack(alignment:.leading){Text("原暂存请求："+(record.params["requestId"].string ?? "")).font(.caption.monospaced()).textSelection(.enabled);Button("只读核对原UUID"){run{try await store.lookup(record)}}.font(.caption)}
            }
            Button("读取当前目录可用Skills"){readSkills()}.disabled(loading || store.currentOrigin==nil || !allowSkills)
            ForEach(skills.indices,id:\.self){i in
                let skill=skills[i],path=skill["path"].string ?? ""
                Toggle(skill["name"].string ?? "Skill",isOn:Binding(get:{choice.skills.contains{$0["path"] == .string(path)}},set:{enabled in choice.skills.removeAll{$0["path"] == .string(path)};if enabled{choice.skills.append(.object(["name":skill["name"],"path":skill["path"]]))}}))
                    .disabled(skill["enabled"] != .bool(true) || !allowSkills)
            }
            Text("Skills只选当前原目录实际启用条目；最终任务目录变化会拒绝，不会代换路径。").font(.caption).foregroundStyle(.secondary)
            if let issue=error ?? store.error{Text(issue).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
        }.padding(.vertical,8)}
        .task(id:identity){choice = .init();reload()}
        .onChange(of:store.revision){_,_ in reload()}
        .fileImporter(isPresented:$importing,allowedContentTypes:[.image,.audio,.plainText],allowsMultipleSelection:false){result in
            guard case .success(let urls)=result,let url=urls.first,let origin=store.currentOrigin else{if case .failure=result{error="文件未能读取；原任务和输入保留。"};return}
            let context=self.context
            run{
                let tuple=try await Task.detached{()->(Data,String,String,String) in
                    let allowed=url.startAccessingSecurityScopedResource();defer{if allowed{url.stopAccessingSecurityScopedResource()}}
                    let filename=url.lastPathComponent,ext=url.pathExtension.lowercased()
                    let formats:[String:(String,String)]=["png":("image","image/png"),"jpg":("image","image/jpeg"),"jpeg":("image","image/jpeg"),"gif":("image","image/gif"),"webp":("image","image/webp"),"wav":("audio","audio/wav"),"mp3":("audio","audio/mpeg"),"m4a":("audio","audio/mp4"),"mp4":("audio","audio/mp4"),"txt":("text","text/plain")]
                    guard let format=formats[ext] else{throw CrewdClientError(message:"此文件类型未支持；PDF和任意二进制不会上传。",kind:.notSubmitted)}
                    let maximum=format.0=="text" ? 512*1024:4*1024*1024,handle=try FileHandle(forReadingFrom:url);defer{try? handle.close()}
                    let data=try handle.read(upToCount:maximum+1) ?? Data();guard data.count<=maximum else{throw CrewdClientError(message:"文件超过此类型的上传上限；原任务保留。",kind:.notSubmitted)}
                    return(data,filename,format.1,format.0)
                }.value
                guard store.currentOrigin==origin else{throw CrewdPublicWire.invalid()}
                try await store.prepare(data:tuple.0,filename:tuple.1,mime:tuple.2,kind:tuple.3,context:context)
            }
        }
    }
    private func stateLabel(_ state:String)->String{switch state{case "unknown":return "结果未知，只查原请求";case "used":return "已被原任务引用，不可复用";case "discarded":return "已请求丢弃，不可选";default:return "未准备成功"}}
    private func reload(){do{rows=try store.entries(context);pending=try store.pending(context);choice.ids.formIntersection(Set(rows.filter{$0.state=="prepared"}.map(\.id)))}catch{rows=[];pending=[]}}
    private func run(_ action:@escaping @MainActor ()async throws->Void){Task{do{try await action();error=nil;reload()}catch{self.error=(error as? CrewdClientError)?.message ?? "原输入操作未完成；只读核对原请求，不会重传。";reload()}}}
    private func readSkills(){
        guard let origin=store.currentOrigin else{return};let context=self.context;loading=true
        Task{defer{loading=false};do{var p:[String:CrewdValue]=[:];if let session=context.session{p["sessionId"] = .string(session)}else{p["crewId"] = .string(context.crew)}
            let raw=try await workspace.readValue("session.codex.tools",.object(p));guard store.currentOrigin==origin,self.context==context else{return}
            skills=try CrewdCodexInputWire.tools(raw).filter{$0["name"].string != nil && $0["path"].string != nil};error=nil
        }catch{self.error="当前目录Skills未能读取；没有使用其它目录或自由路径。"}}
    }
}

struct CrewdCodexPermissionProfiles:View {
    @ObservedObject var workspace:CrewdWorkspace
    let session:String?
    @Binding var settings:CrewdCodexSettingsDraft
    @State private var rows:[CrewdValue]=[]
    @State private var cursor:String?
    @State private var busy=false
    @State private var failure:String?
    var body:some View{
        VStack(alignment:.leading,spacing:8){
            Button("读取原目录权限Profiles"){read(more:false)}.disabled(busy)
            ForEach(rows.indices,id:\.self){i in let row=rows[i];Button(row["id"].string ?? "Profile"){settings.permissionsEnabled=true;settings.permissionProfile=row["id"].string ?? "";settings.sandboxEnabled=false}.disabled(row["allowed"] != .bool(true));if let detail=row["description"].string{Text(detail).font(.caption).foregroundStyle(.secondary)}}
            if cursor != nil{Button("下一页权限Profiles"){read(more:true)}.disabled(busy)}
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
        }
    }
    private func read(more:Bool){
        guard !busy,let origin=workspace.codexInputs.currentOrigin else{return};let crew=workspace.crewID;var p:[String:CrewdValue]=["limit":.integer(50)]
        if let session{p["sessionId"] = .string(session)}else{p["crewId"] = .string(crew)};if more,let cursor{p["cursor"] = .string(cursor)}
        busy=true;Task{defer{busy=false};do{let value=try await workspace.readValue("session.codex.permissionProfiles",.object(p));guard workspace.codexInputs.currentOrigin==origin,workspace.crewID==crew else{return};let items=try CrewdCodexInputWire.profiles(value);guard items.count<=50 else{throw CrewdPublicWire.invalid()};rows=more ? rows+items:items;cursor=value["nextCursor"].string;failure=nil}catch{failure="当前原目录权限Profiles未能读取；没有修改设置。"}}
    }
}
