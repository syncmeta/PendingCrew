import SwiftUI
import UniformTypeIdentifiers

/// Explicit source-bound selection, with private local originals and receipts.
struct CrewdCodexInputPicker:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var store:CrewdCodexInputStore
    let session:String?
    let title:String
    let lane:String
    let allowMedia:Bool,allowSkills:Bool
    private let crew:String
    @Binding var choice:CrewdCodexInputChoice
    @State private var selectionRevision:UUID?
    @State private var selectionScope:CrewdPresentationJournal.Scope?
    @State private var exporting=false
    @State private var exportDocument=CrewdCodexOriginalFile(bytes:Data())
    @State private var exportName="原输入"
    @State private var importing=false
    @State private var rows:[CrewdCodexInputStore.Entry]=[]
    @State private var pending:[CrewdPresentationJournal.Prepared]=[]
    @State private var skills:[CrewdValue]=[]
    @State private var error:String?
    @State private var loading=false
    init(workspace:CrewdWorkspace,session:String?,title:String,lane:String="start",choice:Binding<CrewdCodexInputChoice>,allowMedia:Bool=true,allowSkills:Bool=true){self.lane=lane;self.allowMedia=allowMedia;self.allowSkills=allowSkills;crew=workspace.crewID;self.workspace=workspace;self.session=session;self.title=title;_choice=choice;store=workspace.codexInputs}
    private var context:CrewdCodexInputStore.Context{.init(crew:crew,session:session)}
    private var identity:String{workspace.crewID+"/"+(session ?? "new")+"/"+(store.currentOrigin?.connection.uuidString ?? "offline")}
    var body:some View{
        DisclosureGroup(title){VStack(alignment:.leading,spacing:10){
            Text("添加仅保存原输入，不会发送任务。只支持图片、音频和UTF-8文本；PDF/其它二进制暂不支持。输入不能随个人草稿跨设备传递。").font(.caption).foregroundStyle(.secondary)
            Button("选择输入文件"){importing=true}.disabled(store.currentOrigin==nil || !allowMedia)
            ForEach(rows){row in
                VStack(alignment:.leading,spacing:4){
                    if row.state=="prepared"{
                        Toggle(row.record.params["filename"].string ?? "原输入",isOn:Binding(get:{choice.ids.contains(row.id)},set:{on in var next=choice;if on{next.ids.insert(row.id)}else{next.ids.remove(row.id)};save(next)}))
                        Button("丢弃原暂存"){run{try await store.discard(row,context:context)}}.font(.caption)
                    }else{Text((row.record.params["filename"].string ?? "原输入")+" · "+stateLabel(row.state)).font(.caption)}
                    if choice.ids.contains(row.id),row.state != "prepared"{Button("从当前选择移出（原记录保留）"){var next=choice;next.ids.remove(row.id);save(next)}.font(.caption)}
                    Button("导出本机原文件"){do{exportDocument = .init(bytes:try store.original(row));exportName=row.record.params["filename"].string ?? "原输入";exporting=true}catch{self.error="本机原文件未能读取，未修改原记录。"}}.font(.caption)
                }
            }
            ForEach(pending){record in
                VStack(alignment:.leading){Text("原暂存请求："+(record.params["requestId"].string ?? "")).font(.caption.monospaced()).textSelection(.enabled);Button("只读核对原UUID"){run{try await store.lookup(record)}}.font(.caption)}
            }
            ForEach(choice.skills.indices,id:\.self){index in HStack{Text("原选择Skill："+(choice.skills[index]["name"].string ?? "")).font(.caption);Button("移出选择"){var next=choice;next.skills.remove(at:index);save(next)}.font(.caption)}}
            Button("重新读取本机选择"){restoreChoice()}.font(.caption)
            Button("读取当前目录可用Skills"){readSkills()}.disabled(loading || store.currentOrigin==nil || !allowSkills)
            ForEach(skills.indices,id:\.self){i in
                let skill=skills[i],path=skill["path"].string ?? ""
                Toggle(skill["name"].string ?? "Skill",isOn:Binding(get:{choice.skills.contains{$0["path"] == .string(path)}},set:{enabled in var next=choice;next.skills.removeAll{$0["path"] == .string(path)};if enabled{next.skills.append(.object(["name":skill["name"],"path":skill["path"]]))};save(next)}))
                    .disabled(skill["enabled"] != .bool(true) || !allowSkills)
            }
            Text("Skills只选当前原目录实际启用条目；最终任务目录变化会拒绝，不会代换路径。").font(.caption).foregroundStyle(.secondary)
            if let issue=error ?? store.error{Text(issue).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
        }.padding(.vertical,8)}
        .task(id:identity){restoreChoice();reload()}
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.data,defaultFilename:exportName){result in if case .failure=result{self.error="原文件导出未完成，原件保留。"}}
        .onChange(of:store.revision){_,_ in reload()}
        .fileImporter(isPresented:$importing,allowedContentTypes:[.image,.audio,.plainText],allowsMultipleSelection:false){result in
            guard case .success(let urls)=result,let url=urls.first,let origin=store.currentOrigin else{if case .failure=result{self.error="文件未能读取；原任务和输入保留。"};return}
            let context=self.context
            do{
                // Bounded synchronous read + private durable copy before yielding.
                // No bookmark/path is retained or sent to another device.
                let allowed=url.startAccessingSecurityScopedResource();defer{if allowed{url.stopAccessingSecurityScopedResource()}}
                let filename=url.lastPathComponent,ext=url.pathExtension.lowercased()
                let formats:[String:(String,String)]=["png":("image","image/png"),"jpg":("image","image/jpeg"),"jpeg":("image","image/jpeg"),"gif":("image","image/gif"),"webp":("image","image/webp"),"wav":("audio","audio/wav"),"mp3":("audio","audio/mpeg"),"m4a":("audio","audio/mp4"),"mp4":("audio","audio/mp4"),"txt":("text","text/plain")]
                guard let format=formats[ext] else{throw CrewdClientError(message:"此类型未加入；PDF/任意二进制不会上传。请保留原文件。",kind:.notSubmitted)}
                let maximum=format.0=="text" ? 512*1024:4*1024*1024,handle=try FileHandle(forReadingFrom:url);defer{try? handle.close()}
                let data=try handle.read(upToCount:maximum+1) ?? Data();guard data.count<=maximum,store.currentOrigin==origin else{throw CrewdClientError(message:"文件超过上限或原连接变化；尚未加入，原文件未修改。",kind:.notSubmitted)}
                let original=try store.prepareLocal(data:data,filename:filename,mime:format.1,kind:format.0,context:context)
                reload();run{try await store.sendPrepared(original)}
            }catch{self.error=(error as? CrewdClientError)?.message ?? "文件未加入或上传；请保留原文件。";reload()}
        }
    }
    private func stateLabel(_ state:String)->String{switch state{case "unknown":return "结果未知，只查原请求";case "used":return "已被原任务引用，不可复用";case "discarded":return "已请求丢弃，不可选";default:return "未准备成功"}}
    private func reload(){do{rows=try store.entries(context);pending=try store.pending(context);}catch{rows=[];pending=[]}}
    private func restoreChoice(){do{let original=try store.restoreChoice(context,lane:lane);choice=original.0;selectionRevision=original.1;selectionScope=try store.choiceScope();self.error=nil}catch{choice = .init();selectionRevision=nil;selectionScope=nil;self.error="原本机选择尚未读到，未覆盖原记录；可导出本机原文件。"}}
    private func save(_ next:CrewdCodexInputChoice){do{guard let selectionScope else{throw CrewdPublicWire.invalid()};selectionRevision=try store.saveChoice(next,context:context,lane:lane,expected:selectionRevision,scope:selectionScope);choice=next;self.error=nil}catch{self.error=(error as? CrewdClientError)?.message ?? "选择未能保存，尚未采用更改；原输入文件和原选择保留，请重新读取。"}}
    private func run(_ action:@escaping @MainActor ()async throws->Void){Task{do{try await action();self.error=nil;reload()}catch{self.error=(error as? CrewdClientError)?.message ?? "原输入操作未完成；只读核对原请求，不会重传。";reload()}}}
    private func readSkills(){
        guard let origin=store.currentOrigin else{return};let context=self.context;loading=true
        Task{defer{loading=false};do{var p:[String:CrewdValue]=[:];if let session=context.session{p["sessionId"] = .string(session)}else{p["crewId"] = .string(context.crew)}
            let raw=try await workspace.readValue("session.codex.tools",.object(p));guard store.currentOrigin==origin,self.context==context else{return}
            skills=try CrewdCodexInputWire.tools(raw).filter{$0["name"].string != nil && $0["path"].string != nil};self.error=nil
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

struct CrewdCodexOriginalFile:FileDocument {
    static var readableContentTypes:[UTType]{[.data]}
    var bytes:Data
    init(bytes:Data){self.bytes=bytes}
    init(configuration:ReadConfiguration)throws{bytes=configuration.file.regularFileContents ?? Data()}
    func fileWrapper(configuration:WriteConfiguration)throws->FileWrapper{FileWrapper(regularFileWithContents:bytes)}
}
/// Reads only the user's private local original journal, including while offline.
/// No credentials, remote calls, reuse or replay controls are exposed here.
struct CrewdCodexLocalInputs:View {
    let directory:URL?
    @Environment(\.dismiss) private var dismiss
    @State private var archives:[CrewdPresentationJournal.CodexArchive]=[]
    @State private var error:String?
    @State private var exporting=false
    @State private var document=CrewdCodexOriginalFile(bytes:Data())
    @State private var name="原输入"
    var body:some View{
        VStack(alignment:.leading,spacing:12){
            HStack{Text("本机Codex原输入").font(.headline);Spacer();Button("关闭"){dismiss()}}
            Text("仅查看和导出本机已保全文件。原引用仍绑定原电脑/设备授权；这里不会连接、上传、发送或恢复会话。").font(.caption).foregroundStyle(.secondary)
            if let error{Text(error).font(.caption).foregroundStyle(.orange)}
            ScrollView{VStack(alignment:.leading,spacing:14){
                if archives.isEmpty{Text("尚无可读取的本机原输入").foregroundStyle(.secondary)}
                ForEach(archives){archive in
                    DisclosureGroup("原电脑："+(archive.journal.scope.authorityID ?? "未知")+" · "+(archive.journal.scope.databaseID ?? "未知")){
                        ForEach(archive.records){record in HStack{VStack(alignment:.leading){Text(record.params["filename"].string ?? "原文件");Text("原请求："+(record.params["requestId"].string ?? "")).font(.caption.monospaced()).textSelection(.enabled);Text("原机组："+record.crew+" · "+(record.params["sessionId"].string ?? "首轮输入")).font(.caption).foregroundStyle(.secondary)};Spacer();Button("导出原文件"){do{document = .init(bytes:try archive.journal.codexInputOriginal(record));name=record.params["filename"].string ?? "原输入";exporting=true}catch{self.error="原文件未能验证，已保留未覆盖。"}}}}
                    }
                }
            }}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:600))
        .task{do{guard let directory else{return};archives=try CrewdPresentationJournal.codexArchives(root:directory.appendingPathComponent("frontend-presentation/unknown-submissions"))}catch{self.error="本机原输入未能完整读取；未连接或修改任何电脑。"}}
        .fileExporter(isPresented:$exporting,document:document,contentType:.data,defaultFilename:name){result in if case .failure=result{self.error="导出未完成，本机原件保留。"}}
    }
}
