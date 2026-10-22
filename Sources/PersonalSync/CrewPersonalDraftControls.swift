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
    @State private var settings=false
    @State private var scope:CrewPersonalDraftService.EditingScope?
    @State private var recoveredExport:CrewPersonalDraftService.RecoveredExportTicket?
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
                if let key,service.errors[key] != nil{Button("重试保存"){service.checkpoint(payload,key:key,scope:scope)}.font(.caption)}
                Menu {
                    Button("草稿版本"){showing=true}.disabled(key == nil)
                    Button("个人同步与保留原文"){settings=true}
                    Button("导出草稿"){do{exportDocument = .init(bytes:try appleSyncEncode(payload));exportTicket=key.flatMap{service.exportTicket($0,payload:payload,scope:scope)};recoveredExport=key.flatMap{service.staleExportTicket($0,payload:payload,scope:scope)};exporting=true}catch{failure=error.localizedDescription}}
                }label:{Label("草稿",systemImage:"doc.text")}.font(.caption)
            }
            if let issue=key.flatMap({service.errors[$0]}) ?? service.initializationError ?? failure{Text(issue).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
            if key == nil{Text("来源身份尚未核对；原输入仍按本机草稿保留。").font(.caption2).foregroundStyle(.secondary)}
        }
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-草稿"){result in switch result{case .failure(let error):failure=error.localizedDescription;case .success:if let exportTicket{service.acknowledgeExport(exportTicket)};if let recoveredExport{service.acknowledgeRecoveredExport(recoveredExport)}}}
        .sheet(isPresented:$showing){if let key{CrewPersonalVersionPicker(service:service,key:key,payload:$payload)}}
        .sheet(isPresented:$settings){CrewPersonalSyncSettings(service:service)}
        .task(id:service.projectionID){
            let current=service.editingScope();scope=current
            guard let key else{return}
            let wasEmpty=exactlyEmpty
            await service.refresh(key)
            guard service.isCurrent(current),wasEmpty,exactlyEmpty,self.key == key,let own=await service.restoreOwn(key),service.isCurrent(current),exactlyEmpty,self.key == key else{return}
            payload=own
        }
    }
}

private struct CrewPersonalVersionPicker:View {
    @ObservedObject var service:CrewPersonalDraftService
    let key:AppleDraftKey
    @Binding var payload:CrewComposerDraftStore.Draft
    @Environment(\.dismiss) private var dismiss
    @State private var expected:CrewPersonalDraftService.Snapshot?
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
    private var stale:Bool{expected.map{!service.isCurrent($0)} ?? true}
    private var own:Set<UUID>{Set(versions.filter(\.own).map(\.id))}
    var body:some View {
        VStack(alignment:.leading,spacing:12){
            Text("各设备草稿版本").font(.headline)
            Text("每个设备保留自己的版本；选用或合并不会连接电脑、发送消息或启动任务。").font(.caption).foregroundStyle(.secondary)
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
            if stale{Text("账号或原输入已变化，请关闭后重新核对版本。").font(.caption).foregroundStyle(.orange)}
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
            Button("导出合并原文"){do{var value=keepStructure ?? .init();value.text=merged;exportDocument = .init(bytes:try appleSyncEncode(value));exportTicket=service.exportTicket(key,payload:value);exporting=true}catch{failure=error.localizedDescription}}
            HStack{Button("关闭"){dismiss()};Spacer();Button("确认合并/选用"){confirmation=true}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(busy || stale || selected == nil || !own.isSubset(of:retiring))}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:620))
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-合并草稿"){result in switch result{case .failure(let error):failure=error.localizedDescription;case .success:if let exportTicket{service.acknowledgeExport(exportTicket)}}}
        .task{merged=payload.text;expected=service.snapshot(key,payload:payload);await service.refresh(key)}
        .confirmationDialog("替换当前输入并结束已勾选的旧分支？",isPresented:$confirmation,titleVisibility:.visible){
            Button("确认保存合并版本"){apply()}
            Button("取消",role:.cancel){}
        }message:{Text("只处理已勾选分支；当前非空输入不会因收到其它设备版本自动改变。")}
    }
    private func apply(){
        guard let expected else{failure="当前输入已在其它窗口更新，请关闭后重新核对。";return}
        var chosen=keepStructure ?? .init();chosen.text=merged
        busy=true
        Task{do{let applied=try await service.resolve(key,payload:chosen,retiring:retiring,expected:expected);guard service.isCurrent(applied) else{throw AppleSyncFailure.invalid};payload=chosen;dismiss()}catch{failure="合并未能完整保存，原输入与未合并分支保留。"};busy=false}
    }
}

struct CrewPersonalHostDirectory:View {
    @ObservedObject private var service=CrewPersonalDraftService.shared
    @State private var settings=false
    var body:some View {
        VStack(alignment:.leading,spacing:8){
            HStack{Text("个人主机目录").font(.headline);Spacer();Button("个人同步"){settings=true}}
            Text(service.status).font(.caption).foregroundStyle(.secondary)
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
        }.task(id:service.projectionID){await service.loadHosts()}
        .sheet(isPresented:$settings){CrewPersonalSyncSettings(service:service)}
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
            CrewRecoveredPersonalOriginals(service:service,owner:service.editingScope().owner)
            if service.catalog.isEmpty,service.initializationError == nil{Text("尚无个人草稿版本").font(.caption).foregroundStyle(.secondary)}
            if let issue=service.initializationError ?? failure{Text(issue).font(.caption).foregroundStyle(.orange)}
        }
        .task(id:service.projectionID){await service.loadCatalog()}
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
    @State private var scope:CrewPersonalDraftService.EditingScope?
    @State private var recoveredExport:CrewPersonalDraftService.RecoveredExportTicket?
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
            TextEditor(text:Binding(get:{payload.text},set:{editing=true;payload.text=$0;service.checkpoint(payload,key:key,scope:scope)})).frame(minHeight:220).disabled(!loaded)
            Text(scope.map{service.isCurrent($0)} == false ? "账号已变化；此页的编辑仍保留在原账号，可导出后关闭。":"@ 目标与回复引用随原草稿保留；这里只保存个人输入，不向来源电脑提交。").font(.caption).foregroundStyle(.secondary)
            if let issue=service.errors[key] ?? failure{Text(issue).font(.caption).foregroundStyle(.orange)}
            HStack{
                Button("各设备版本"){choosing=true}.disabled(!loaded || scope.map{!service.isCurrent($0)} == true)
                Button("导出"){do{exportDocument = .init(bytes:try appleSyncEncode(payload));exportTicket=service.exportTicket(key,payload:payload,scope:scope);recoveredExport=service.staleExportTicket(key,payload:payload,scope:scope);exporting=true}catch{failure=error.localizedDescription}}.disabled(!loaded)
                Spacer()
                Button("关闭"){dismiss()}
                Button("保存本机草稿"){service.checkpoint(payload,key:key,scope:scope);editing=false}.disabled(!loaded).buttonStyle(CrewdPrimaryButtonStyle())
            }
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:520))
        .task{let current=service.editingScope();scope=current;let original=await service.restoreOwn(key);guard service.isCurrent(current),!editing else{return};payload=original ?? .init();loaded=true}
        .sheet(isPresented:$choosing){CrewPersonalVersionPicker(service:service,key:key,payload:$payload)}
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-离线草稿"){result in switch result{case .failure(let error):failure=error.localizedDescription;case .success:if let exportTicket{service.acknowledgeExport(exportTicket)};if let recoveredExport{service.acknowledgeRecoveredExport(recoveredExport)}}}
    }
}

/// Personal sync has its own explicit consent. It never grants a Core connection.
private struct CrewPersonalSyncSettings:View {
    @ObservedObject var service:CrewPersonalDraftService
    @Environment(\.dismiss) private var dismiss
    @State private var busy=false
    @State private var confirming=false
    @State private var failure:String?
    @State private var selected:RetainedPersonalSelection?
    var body:some View {
        VStack(alignment:.leading,spacing:14){
            HStack{Text("个人同步").font(.headline);Spacer();Button("关闭"){dismiss()}}
            ScrollView{VStack(alignment:.leading,spacing:14){
                Text(service.status).font(.callout)
                Text("仅同步个人草稿的设备版本与公开主机线索。主机目录不含密钥或设备授权；在另一设备仍需独立配对。不会同步工作数据库，也不会自动发送草稿、连接电脑或启动会话。").font(.caption).foregroundStyle(.secondary)
                if service.cloudConfigured {
                    if service.cloudOptedIn {
                        HStack{Button("核对账号并重试"){perform{try await service.enablePersonalSync()}};Button("停用个人同步"){perform{await service.disablePersonalSync()}}}.disabled(busy || service.syncBusy)
                        Text("已同意后，打开 App、回到前台和保存版本时会正常同步。停用会保留原账号与本机数据，不自动互相搬移。").font(.caption).foregroundStyle(.secondary)
                        if service.preferencesQualified {
                            Toggle("同步外观偏好",isOn:Binding(get:{service.preferencesOptedIn},set:{service.setPreferenceSync($0)}))
                        }else{Text("外观偏好保留在本设备。").font(.caption).foregroundStyle(.secondary)}
                    }else{Button("启用个人 iCloud 同步"){confirming=true}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(busy)}
                }else{Text("此版本的草稿仅本机保存；可以照常编辑、选择版本和导出。").font(.caption).foregroundStyle(.secondary)}
                if busy || service.syncBusy{ProgressView("正在核对个人同步").controlSize(.small)}
                if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
                Divider()
                Text("保留的其它来源").font(.subheadline)
                Text("切换账号或停用不会把旧原文带入当前输入。只在这里明确选用或导出；已有输入会再次确认。").font(.caption).foregroundStyle(.secondary)
                ForEach(service.retainedNamespaces.filter{$0 !== service.editingScope().owner},id:\.id){owner in
                    VStack(alignment:.leading,spacing:8){
                        Text(owner.id.account == nil ? "仅本机":"其它 iCloud 来源 · "+String(owner.id.stableName.prefix(8))).font(.caption).foregroundStyle(.secondary)
                        ForEach(Array(owner.catalog).sorted{String(describing:$0)<String(describing:$1)},id:\.self){key in
                            if let ticket=service.retainedExportTicket(owner,key:key){
                                Button{selected = .init(ticket:ticket)}label:{VStack(alignment:.leading,spacing:3){Text(ticket.value.payload.text.isEmpty ? "（空草稿）":ticket.value.payload.text).lineLimit(3);Text(key.crewID+" · "+String(key.databaseID.prefix(12))).font(.caption).foregroundStyle(.secondary)}}.buttonStyle(.plain)
                            }
                        }
                        ForEach(Array(owner.hosts.enumerated()),id:\.offset){_,host in
                            HStack{Text(host.displayName).font(.caption);Spacer();Button("选入当前目录"){guard let ticket=owner.hostTicket(host) else{return};perform{try await service.copyHost(ticket,from:owner)}}.disabled(!service.cloudOptedIn || service.editingScope().owner === owner || busy)}
                        }
                        CrewRecoveredPersonalOriginals(service:service,owner:owner)
                        if owner.catalog.isEmpty && owner.hosts.isEmpty && owner.recovered.isEmpty{Text("此来源暂无已保留内容").font(.caption).foregroundStyle(.secondary)}
                    }.padding(.vertical,6)
                }
            }}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:560))
        .sheet(item:$selected){choice in RetainedPersonalDraftView(service:service,ticket:choice.ticket)}
        .confirmationDialog("启用个人 iCloud 同步？",isPresented:$confirming,titleVisibility:.visible){Button("同意并核对账号"){perform{try await service.enablePersonalSync()}};Button("取消",role:.cancel){}}message:{Text("使用当前系统 iCloud 账号的私有空间。各设备原文保留独立分支，本机旧原文不会自动上传到新账号。")}
    }
    private func perform(_ action:@escaping @MainActor ()async throws->Void){busy=true;Task{do{try await action();failure=nil}catch{failure="个人同步未完成；原文仍保留，可稍后重试或导出。"};busy=false}}
}
private struct RetainedPersonalSelection:Identifiable {
    let id=UUID()
    let ticket:CrewPersonalDraftService.ExportTicket
}
private struct RetainedPersonalDraftView:View {
    @ObservedObject var service:CrewPersonalDraftService
    let ticket:CrewPersonalDraftService.ExportTicket
    @Environment(\.dismiss) private var dismiss
    @State private var target:CrewPersonalDraftService.Snapshot?
    @State private var confirming=false
    @State private var failure:String?
    @State private var exporting=false
    @State private var document=CrewPersonalDraftExport(bytes:Data())
    var body:some View {
        VStack(alignment:.leading,spacing:12){
            Text("保留的原文").font(.headline)
            ScrollView{Text(ticket.value.payload.text.isEmpty ? "（空草稿）":ticket.value.payload.text).frame(maxWidth:.infinity,alignment:.leading).textSelection(.enabled)}
            Text("完整 @ 目标与回复引用随原文保留。选用只复制这一份输入，不发送；来源继续保留。").font(.caption).foregroundStyle(.secondary)
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
            HStack{
                Button("关闭"){dismiss()}
                Button("导出原文"){do{document = .init(bytes:try appleSyncEncode(ticket.value.payload));exporting=true}catch{failure=error.localizedDescription}}
                Spacer()
                Button("选入当前来源"){
                    let current=service.localDraft(ticket.value.key) ?? .init()
                    guard let exact=service.snapshot(ticket.value.key,payload:current) else{failure="当前来源已变化，请重新核对。";return}
                    target=exact;confirming=true
                }.buttonStyle(CrewdPrimaryButtonStyle()).disabled(!service.cloudOptedIn || service.editingScope().owner.id.account == nil || service.editingScope().owner === ticket.owner)
            }
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:460))
        .confirmationDialog("选用这份原文并替换当前输入？",isPresented:$confirming,titleVisibility:.visible){Button("确认选用"){do{try service.copyRetained(ticket,replacing:target);dismiss()}catch{failure="原来源或当前输入已更新；未覆盖，请重新核对。"}};Button("取消",role:.cancel){}}message:{Text("只替换核对时的这一份当前输入；其它窗口若已编辑，本次选择不会覆盖。")}
        .fileExporter(isPresented:$exporting,document:document,contentType:.json,defaultFilename:"PendingCrew-保留原文"){result in switch result{case .success:service.acknowledgeExport(ticket);case .failure(let error):failure=error.localizedDescription}}
    }
}


/// Local recovery includes late stale-editor originals, never cloud branches.
/// Its ticket acknowledges exactly the exported revision, not the current draft.
private struct CrewRecoveredPersonalOriginals:View {
    @ObservedObject var service:CrewPersonalDraftService
    let owner:CrewPersonalDraftService.Namespace
    @State private var ticket:CrewPersonalDraftService.RecoveredExportTicket?
    @State private var document=CrewPersonalDraftExport(bytes:Data())
    @State private var exporting=false
    @State private var failure:String?
    var body:some View {
        VStack(alignment:.leading,spacing:8){
            ForEach(owner.recovered.keys.sorted{$0.uuidString < $1.uuidString},id:\.self){id in
                if let original=owner.recovered[id]{
                    Text("保全原文 · 仅本机").font(.caption).foregroundStyle(.secondary)
                    Text(original.text.isEmpty ? "（空草稿）":original.text).font(.caption).lineLimit(5).textSelection(.enabled)
                    if owner.unsavedRecovered.contains(id){Text("此原文尚未保存，请导出后再退出。").font(.caption).foregroundStyle(.orange)}
                    Button("导出完整原文"){do{guard let value=service.recoveredExportTicket(id,owner:owner) else{throw AppleSyncFailure.invalid};ticket=value;document = .init(bytes:try appleSyncEncode(value.value.payload));exporting=true}catch{failure=error.localizedDescription}}
                }
            }
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
        }.fileExporter(isPresented:$exporting,document:document,contentType:.json,defaultFilename:"PendingCrew-保全原文"){result in switch result{case .failure(let error):failure=error.localizedDescription;case .success:if let ticket{service.acknowledgeRecoveredExport(ticket)}}}
    }
}
