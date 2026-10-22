import SwiftUI
import UniformTypeIdentifiers

struct CrewPersonalDraftExport:FileDocument {
    static var readableContentTypes:[UTType]{[.json]}
    var bytes:Data
    init(bytes:Data){self.bytes=bytes}
    init(configuration:ReadConfiguration)throws{bytes=configuration.file.regularFileContents ?? Data()}
    func fileWrapper(configuration:WriteConfiguration)throws->FileWrapper{FileWrapper(regularFileWithContents:bytes)}
}

/// Same control in the original Mac and mobile composers. Showing it only reads
/// local branches; choosing/retiring versions always requires an explicit action.
struct CrewPersonalDraftControls:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var service=CrewPersonalDraftService.shared
    let context:CrewdDrafts.Context
    @Binding var payload:CrewComposerDraftStore.Draft
    @State private var showing=false
    @State private var exporting=false
    @State private var exportTicket:CrewPersonalDraftService.ExportTicket?
    @State private var exportDocument=CrewPersonalDraftExport(bytes:Data())
    @State private var failure:String?
    private var exactlyEmpty:Bool{payload.text.isEmpty && payload.stagedMentions.isEmpty && payload.replyTarget == nil}
    private var key:AppleDraftKey?{workspace.personalDraftKey(context)}
    var body:some View {
        VStack(alignment:.leading,spacing:4){
            HStack(spacing:8){
                Text(key.map{service.saving.contains($0) ? (service.unsavedLatest.contains($0) ? "原文尚未保存 · 正在保存版本":"原文已本机保全 · 正在保存版本"):service.status} ?? service.status).font(.caption2).foregroundStyle(.secondary)
                Spacer(minLength:0)
                Button("草稿版本"){showing=true}.font(.caption).disabled(key == nil)
                if let key,service.errors[key] != nil{Button("重试保存"){service.checkpoint(payload,key:key)}.font(.caption)}
                Button("导出草稿"){do{exportDocument = .init(bytes:try appleSyncEncode(payload));exportTicket=key.flatMap{service.exportTicket($0,payload:payload)};exporting=true}catch{failure=error.localizedDescription}}.font(.caption)
            }
            if let issue=key.flatMap({service.errors[$0]}) ?? service.initializationError ?? failure{Text(issue).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
            if key == nil{Text("来源身份尚未核对；原输入仍按本机草稿保留。").font(.caption2).foregroundStyle(.secondary)}
        }
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-草稿"){result in switch result{case .failure(let error):failure=error.localizedDescription;case .success:if let exportTicket{service.acknowledgeExport(exportTicket)}}}
        .sheet(isPresented:$showing){if let key{CrewPersonalVersionPicker(service:service,key:key,payload:$payload)}}
        .task(id:key){
            guard let key else{return}
            let wasEmpty=exactlyEmpty
            await service.refresh(key)
            guard wasEmpty,exactlyEmpty,self.key == key,let own=await service.restoreOwn(key),exactlyEmpty,self.key == key else{return}
            payload=own
        }
    }
}

private struct CrewPersonalVersionPicker:View {
    @ObservedObject var service:CrewPersonalDraftService
    let key:AppleDraftKey
    @Binding var payload:CrewComposerDraftStore.Draft
    @Environment(\.dismiss) private var dismiss
    @State private var selected:UUID?
    @State private var retiring=Set<UUID>()
    @State private var merged=""
    @State private var keepStructure:CrewComposerDraftStore.Draft?
    @State private var confirmation=false
    @State private var failure:String?
    @State private var busy=false
    @State private var exporting=false
    @State private var exportDocument=CrewPersonalDraftExport(bytes:Data())
    @State private var exportTicket:CrewPersonalDraftService.ExportTicket?
    private var versions:[CrewPersonalDraftService.Version]{service.versions[key] ?? []}
    private var own:Set<UUID>{Set(versions.filter(\.own).map(\.id))}
    var body:some View {
        VStack(alignment:.leading,spacing:12){
            Text("各设备草稿版本").font(.headline)
            Text("当前草稿保存在本设备，版本选择不会连接或启动电脑。").font(.caption).foregroundStyle(.secondary)
            ScrollView{VStack(alignment:.leading,spacing:10){ForEach(versions){version in
                VStack(alignment:.leading,spacing:6){
                    HStack{Text(version.own ? "本设备":"其它设备 · "+String(version.deviceID.prefix(8))).font(.caption);Spacer();Button("选为编辑起点"){selected=version.id;merged=version.payload.text;keepStructure=version.payload}}
                    Text(version.payload.text.isEmpty ? "（空草稿）":version.payload.text).font(.caption).textSelection(.enabled).lineLimit(5)
                    Toggle("明确结束这个旧分支",isOn:Binding(get:{retiring.contains(version.id)},set:{if $0{retiring.insert(version.id)}else{retiring.remove(version.id)}})).font(.caption)
                }.padding(10).background(.quaternary,in:RoundedRectangle(cornerRadius:8))
            }}}
            Text("合并后的输入（保留所选版本的 @ 与回复引用）").font(.caption)
            TextEditor(text:$merged).frame(minHeight:130)
            Text("本设备已有分支时，保存新合并版本需明确结束该分支。未选中的其它设备版本继续保留。").font(.caption).foregroundStyle(.secondary)
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
            Button("导出合并原文"){do{var value=keepStructure ?? .init();value.text=merged;exportDocument = .init(bytes:try appleSyncEncode(value));exportTicket=service.exportTicket(key,payload:value);exporting=true}catch{failure=error.localizedDescription}}
            HStack{Button("关闭"){dismiss()};Spacer();Button("确认合并/选用"){confirmation=true}.disabled(busy || selected == nil || !own.isSubset(of:retiring))}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:620))
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-合并草稿"){result in switch result{case .failure(let error):failure=error.localizedDescription;case .success:if let exportTicket{service.acknowledgeExport(exportTicket)}}}
        .task{merged=payload.text;await service.refresh(key)}
        .confirmationDialog("替换当前输入并结束已勾选的旧分支？",isPresented:$confirmation,titleVisibility:.visible){
            Button("确认保存合并版本"){apply()}
            Button("取消",role:.cancel){}
        }message:{Text("只处理已勾选分支；当前非空输入不会因收到其它设备版本自动改变。")}
    }
    private func apply(){
        var chosen=keepStructure ?? .init();chosen.text=merged
        busy=true
        Task{do{try await service.resolve(key,payload:chosen,retiring:retiring);payload=chosen;dismiss()}catch{failure="合并未能完整保存，原输入与未合并分支保留。"};busy=false}
    }
}

struct CrewPersonalHostDirectory:View {
    @ObservedObject private var service=CrewPersonalDraftService.shared
    var body:some View {
        VStack(alignment:.leading,spacing:8){
            Text("个人主机目录").font(.headline)
            Text("个人目录仅本机保存").font(.caption).foregroundStyle(.secondary)
            ForEach(Array(service.hosts.enumerated()),id:\.offset){_,host in
                VStack(alignment:.leading,spacing:3){
                    Text(host.displayName)
                    Text((host.endpoint.host ?? "")+":"+String(host.endpoint.port ?? 0)).font(.caption.monospaced())
                    Text(host.verifiedSPKI).font(.caption2.monospaced()).textSelection(.enabled)
                    Text("目录只是公开线索；在本设备添加电脑时仍需一次性配对与身份核对。").font(.caption).foregroundStyle(.secondary)
                }.padding(.vertical,5)
            }
            if service.hosts.isEmpty,service.initializationError == nil{Text("尚无已保存的可访问主机线索").font(.caption).foregroundStyle(.secondary)}
            if let error=service.initializationError{Text(error).font(.caption).foregroundStyle(.orange)}
        }.task{await service.loadHosts()}
    }
}

/// Historical personal inputs remain readable/editable even while a VPS/Mac
/// host is offline. These keys are never promoted into a live Core connection.
struct CrewPersonalDraftArchive:View {
    @ObservedObject private var service=CrewPersonalDraftService.shared
    @State private var selected:AppleDraftKey?
    @State private var exportDocument=CrewPersonalDraftExport(bytes:Data())
    @State private var exporting=false
    @State private var failure:String?
    var body:some View {
        VStack(alignment:.leading,spacing:10){
            Text("本机草稿").font(.headline)
            Text("电脑离线时可编辑和导出；连接恢复不会自动发送或启动会话。").font(.caption).foregroundStyle(.secondary)
            ForEach(service.catalog,id:\.self){key in
                Button{selected=key}label:{VStack(alignment:.leading){Text(key.crewID);Text(label(key)).font(.caption).foregroundStyle(.secondary)}}.buttonStyle(.plain)
            }
            ForEach(service.recovered.keys.sorted(by:{$0.uuidString < $1.uuidString}),id:\.self){id in
                if let value=service.recovered[id]{
                    Text("已结束分支的保全原文").font(.caption).foregroundStyle(.secondary)
                    Text(value.text).font(.caption).textSelection(.enabled)
                    Button("导出保全原文"){do{exportDocument = .init(bytes:try appleSyncEncode(value));exporting=true}catch{failure=error.localizedDescription}}
                }
            }
            if service.catalog.isEmpty,service.initializationError == nil{Text("尚无个人草稿版本").font(.caption).foregroundStyle(.secondary)}
            if let issue=service.initializationError ?? failure{Text(issue).font(.caption).foregroundStyle(.orange)}
        }
        .task{await service.loadCatalog()}
        .sheet(item:Binding(get:{selected.map{PersonalArchiveSelection(key:$0)}},set:{selected=$0?.key})){choice in CrewPersonalArchiveEditor(service:service,key:choice.key)}
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-保全草稿"){result in if case .failure(let error)=result{failure=error.localizedDescription}}
    }
    private func label(_ key:AppleDraftKey)->String{
        let destination:String
        switch key.destination{case .channel(let value),.originalSession(let value):destination=value}
        return destination+" · "+String(key.databaseID.prefix(12))
    }
}
private struct PersonalArchiveSelection:Identifiable {
    let key:AppleDraftKey
    var id:AppleDraftKey{key}
}
private struct CrewPersonalArchiveEditor:View {
    @ObservedObject var service:CrewPersonalDraftService
    let key:AppleDraftKey
    @Environment(\.dismiss) private var dismiss
    @State private var payload=CrewComposerDraftStore.Draft()
    @State private var loaded=false
    @State private var editing=false
    @State private var choosing=false
    @State private var exporting=false
    @State private var exportDocument=CrewPersonalDraftExport(bytes:Data())
    @State private var exportTicket:CrewPersonalDraftService.ExportTicket?
    @State private var failure:String?
    var body:some View{
        VStack(alignment:.leading,spacing:12){
            Text("离线个人草稿").font(.headline)
            Text("历史来源："+key.crewID+" · "+String(key.databaseID.prefix(12))).font(.caption).foregroundStyle(.secondary)
            TextEditor(text:Binding(get:{payload.text},set:{editing=true;payload.text=$0;service.checkpoint(payload,key:key)})).frame(minHeight:220).disabled(!loaded)
            Text("@ 目标与回复引用随原草稿保留；这里只保存个人输入，不向来源电脑提交。").font(.caption).foregroundStyle(.secondary)
            if let issue=service.errors[key] ?? failure{Text(issue).font(.caption).foregroundStyle(.orange)}
            HStack{
                Button("各设备版本"){choosing=true}.disabled(!loaded)
                Button("导出"){do{exportDocument = .init(bytes:try appleSyncEncode(payload));exportTicket=service.exportTicket(key,payload:payload);exporting=true}catch{failure=error.localizedDescription}}.disabled(!loaded)
                Spacer()
                Button("关闭"){dismiss()}
                Button("保存本机草稿"){service.checkpoint(payload,key:key);editing=false}.disabled(!loaded)
            }
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:520))
        .task{let original=await service.restoreOwn(key);guard !editing else{return};payload=original ?? .init();loaded=true}
        .sheet(isPresented:$choosing){CrewPersonalVersionPicker(service:service,key:key,payload:$payload)}
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-离线草稿"){result in switch result{case .failure(let error):failure=error.localizedDescription;case .success:if let exportTicket{service.acknowledgeExport(exportTicket)}}}
    }
}
