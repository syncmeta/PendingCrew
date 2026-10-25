import SwiftUI
import UniformTypeIdentifiers

struct CrewPersonalDraftExport:FileDocument {
    static var readableContentTypes:[UTType]{[.json]}
    var bytes:Data
    init(bytes:Data){self.bytes=bytes}
    init(configuration:ReadConfiguration)throws{bytes=configuration.file.regularFileContents ?? Data()}
    func fileWrapper(configuration:WriteConfiguration)throws->FileWrapper{FileWrapper(regularFileWithContents:bytes)}
}

/// Presentation reads the existing cache; these labels never change draft identity.
private enum PersonalDraftPresentation {
    static func kind(_ key:AppleDraftKey)->String {
        switch key.kind {
        case "initialTask":return "新建机组草稿"
        case "sessionMessage":return "会话消息草稿"
        case "channelMessage":return "聊天消息草稿"
        default:return "个人草稿"
        }
    }
    static func preview(_ draft:CrewComposerDraftStore.Draft)->String {
        if !draft.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty{return draft.text}
        if !draft.stagedMentions.isEmpty || draft.replyTarget != nil{return "正文未填写，已保留 @ 目标或回复引用"}
        return "空草稿"
    }
    @MainActor static func source(_ service:CrewPersonalDraftService)->String {
        if service.editingScope().owner.id == .local{return "仅本机"}
        return service.usingCore ? "已配对电脑":"个人 iCloud"
    }
}
private struct PersonalDraftOrigin:View {
    let key:AppleDraftKey
    var body:some View {
        DisclosureGroup("来源详情") {
            VStack(alignment:.leading,spacing:5) {
                Text("机组："+key.crewID)
                Text("主机身份："+key.authorityID)
                Text("数据库："+key.databaseID)
                switch key.destination {
                case .channel(let value):Text("聊天频道："+value)
                case .originalSession(let value):Text("原会话："+value)
                }
            }.font(.caption.monospaced()).textSelection(.enabled)
        }.font(.caption).foregroundStyle(.secondary)
    }
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
            if let key, service.errors[key] != nil {
                Button("重试保存") { service.checkpoint(payload, key: key, scope: scope) }.font(.caption)
            }
            if let issue=key.flatMap({service.errors[$0]}) ?? service.initializationError ?? failure{Text(issue).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
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
            #if os(macOS)
            ScrollView{versionCards}
            #else
            versionCards
            #endif
            Text("编号仅用于本次列表核对，不代表时间先后。选用后仍需下方确认。").font(.caption).foregroundStyle(.secondary)
            Text("合并后的输入").font(.subheadline.weight(.medium))
            TextEditor(text:$merged).frame(minHeight:130)
            Text("保留所选版本的 @ 与回复引用。需勾选结束本设备的全部旧版本，才能确认；未勾选的其他设备版本继续保留。").font(.caption).foregroundStyle(.secondary)
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
    private var versionCards:some View {
        VStack(alignment:.leading,spacing:10){
                if versions.isEmpty{Text("尚未读到设备版本；当前输入保持不变。").font(.caption).foregroundStyle(.secondary)}
                ForEach(Array(versions.enumerated()),id:\.element.id){index,version in
                VStack(alignment:.leading,spacing:6){
                    HStack(alignment:.firstTextBaseline){
                        Label(version.own ? "本设备":"其他设备",systemImage:version.own ? "pencil":"laptopcomputer.and.iphone").font(.subheadline.weight(.medium))
                        Spacer(minLength:8)
                        Text("版本 \(index+1)").font(.caption).foregroundStyle(.secondary)
                    }
                    Text(PersonalDraftPresentation.preview(version.payload)).font(.body).textSelection(.enabled).lineLimit(5)
                    Button(selected == version.id ? "已选作编辑起点":"选为编辑起点"){selected=version.id;merged=version.payload.text;keepStructure=version.payload}
                    DisclosureGroup("版本详情"){
                        VStack(alignment:.leading,spacing:4){Text("设备："+version.deviceID);Text("版本："+version.id.uuidString)}.font(.caption2.monospaced()).textSelection(.enabled)
                    }.font(.caption).foregroundStyle(.secondary)
                    Toggle("合并后结束此旧版本",isOn:Binding(get:{retiring.contains(version.id)},set:{if $0{retiring.insert(version.id)}else{retiring.remove(version.id)}})).font(.caption)
                }.padding(10).background(.quaternary,in:RoundedRectangle(cornerRadius:8))
            }}
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
            if service.usingCore{Text("当前 Core 模式仅交换个人草稿；这里的主机线索仅本机保存。").font(.caption).foregroundStyle(.secondary)}
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
            Text("离线草稿").font(.headline)
            Text("电脑离线时可编辑和导出；连接恢复不会自动发送或启动会话。").font(.caption).foregroundStyle(.secondary)
            ForEach(service.catalog,id:\.self){key in
                Button{selected=key}label:{
                    VStack(alignment:.leading,spacing:5){
                        HStack{Text(PersonalDraftPresentation.kind(key)).font(.subheadline.weight(.medium));Spacer(minLength:8);Image(systemName:"chevron.right").font(.caption)}
                        Text(preview(key)).font(.body).foregroundStyle(.primary).lineLimit(3).frame(maxWidth:.infinity,alignment:.leading)
                        Text(label(key)).font(.caption).foregroundStyle(.secondary)
                    }.padding(.vertical,6).contentShape(Rectangle())
                }.buttonStyle(.plain)
            }
            CrewRecoveredPersonalOriginals(service:service,owner:service.editingScope().owner)
            if service.catalog.isEmpty,service.initializationError == nil{Text("尚无个人草稿版本").font(.caption).foregroundStyle(.secondary)}
            if let issue=service.initializationError ?? failure{Text(issue).font(.caption).foregroundStyle(.orange)}
        }
        .task(id:service.projectionID){await service.loadCatalog()}
        .sheet(item:Binding(get:{selected.map{PersonalArchiveSelection(key:$0)}},set:{selected=$0?.key})){choice in CrewPersonalArchiveEditor(service:service,key:choice.key)}
        .fileExporter(isPresented:$exporting,document:exportDocument,contentType:.json,defaultFilename:"PendingCrew-保全草稿"){result in if case .failure(let error)=result{failure=error.localizedDescription}}
    }
    private func preview(_ key:AppleDraftKey)->String {
        if let draft=service.localDraft(key){return PersonalDraftPresentation.preview(draft)}
        let versions=service.versions[key] ?? []
        if let version=versions.first(where:{$0.own}) ?? versions.first{return PersonalDraftPresentation.preview(version.payload)}
        return "打开查看草稿正文"
    }
    private func label(_ key:AppleDraftKey)->String {
        let versions=service.versions[key] ?? []
        let origin=service.localDraft(key) != nil || versions.contains(where:{$0.own}) ? "本设备原文":(versions.isEmpty ? "正文尚未载入":"其他设备版本预览 · 尚未选用")
        return PersonalDraftPresentation.source(service)+" · "+origin+(versions.isEmpty ? "":" · \(versions.count) 个设备版本")
    }
}
private struct PersonalArchiveSelection:Identifiable {
    let key:AppleDraftKey
    var id:AppleDraftKey{key}
}
#if os(macOS)
private struct CrewPersonalArchiveEditorScroll:ViewModifier {
    func body(content:Content)->some View {ScrollView{content}}
}
#endif
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
    private var saveStatus:String? {
        guard loaded,let scope else{return nil}
        guard service.isCurrent(scope) else{return "来源已变化；此页不显示新来源的保存结果。"}
        if service.errors[key] != nil || service.initializationError != nil{return "保存遇到问题；请查看下方提示，可重试保存或导出原文。"}
        if service.unsavedLatest.contains(key){return "本机原文尚未保存，请重试保存或导出。"}
        // Memory equality alone is not a durable-save acknowledgement. A latest
        // writer entry or an already-read own branch must also hold this Draft.
        let preserved=scope.owner.latest.readAll()[key] == payload || service.versions[key]?.contains(where:{$0.own && $0.payload == payload}) == true
        if service.saving.contains(key){return preserved ? "原文已本机保全 · 正在保存版本":"正在保存草稿版本；尚未确认本机原文已保全。"}
        return preserved ? "本机原文已保全；是否已同步请查看个人同步状态。":"当前输入尚未确认保存到本机，可再次保存或导出。"
    }
    var body:some View{
        VStack(alignment:.leading,spacing:12){
            Text("离线个人草稿").font(.headline)
            Text(PersonalDraftPresentation.kind(key)+" · "+(scope.map{service.isCurrent($0)} == false ? "原来源（已变化）":PersonalDraftPresentation.source(service))).font(.caption).foregroundStyle(.secondary)
            PersonalDraftOrigin(key:key)
            if !loaded,failure == nil{
                ProgressView("正在读取本设备草稿…").controlSize(.small)
                Text("读取完成前暂不可编辑，避免覆盖尚未读回的原文；可随时关闭。").font(.caption).foregroundStyle(.secondary)
            }
            TextEditor(text:Binding(get:{payload.text},set:{editing=true;payload.text=$0;service.checkpoint(payload,key:key,scope:scope)})).frame(minHeight:220).disabled(!loaded)
            Text("输入时自动保存；也可点下方按钮再次保存。").font(.caption).foregroundStyle(.secondary)
            if let saveStatus{Text(saveStatus).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)}
            Text(scope.map{service.isCurrent($0)} == false ? "来源已变化；此页的编辑仍在原来源保全，可导出后关闭。":"@ 目标与回复引用随原草稿保留；这里只保存个人草稿，不发送消息或执行任务。已获准且选中的电脑来源会同步版本。").font(.caption).foregroundStyle(.secondary)
            if let issue=(scope.map{service.isCurrent($0)} == true ? (service.errors[key] ?? service.initializationError):nil) ?? failure{Text(issue).font(.caption).foregroundStyle(.orange)}
            LazyVGrid(columns:[GridItem(.adaptive(minimum:130),alignment:.leading)],alignment:.leading,spacing:12){
                Button("各设备版本"){choosing=true}.disabled(!loaded || scope.map{!service.isCurrent($0)} == true)
                Button("导出"){do{exportDocument = .init(bytes:try appleSyncEncode(payload));exportTicket=service.exportTicket(key,payload:payload,scope:scope);recoveredExport=service.staleExportTicket(key,payload:payload,scope:scope);exporting=true}catch{failure=error.localizedDescription}}.disabled(!loaded)
                Button("关闭"){dismiss()}
                Button("保存本机草稿"){service.checkpoint(payload,key:key,scope:scope);editing=false}.disabled(!loaded).buttonStyle(CrewdPrimaryButtonStyle())
            }
        }.padding(20)
        #if os(macOS)
        .modifier(CrewPersonalArchiveEditorScroll())
        #endif
        .modifier(CrewdAdaptiveFormFrame(width:620,height:520))
        .task{
            let current=service.editingScope();scope=current
            let original=await service.restoreOwn(key)
            guard !Task.isCancelled else{return}
            guard service.isCurrent(current) else{failure="草稿来源已变化；原文仍保留，请关闭后从当前来源重新打开。";return}
            guard !editing else{return}
            payload=original ?? .init();loaded=true
        }
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
    @State private var confirmingCore=false
    @State private var configuringCore=false
    @State private var pendingGrant:CrewdRemoteGrant?
    @State private var enableGrant=true
    var body:some View {
        VStack(alignment:.leading,spacing:14){
            HStack{Text("个人同步").font(.headline);Spacer();Button("关闭"){dismiss()}}
            ScrollView{VStack(alignment:.leading,spacing:14){
                VStack(alignment:.leading,spacing:6){
                    Text("当前草稿来源："+PersonalDraftPresentation.source(service)).font(.subheadline.weight(.medium))
                    Text(service.status).font(.callout)
                    Text("草稿不会自动发送或启动任务。停用或更换来源后，原文仍可在下方查看和导出。").font(.caption).foregroundStyle(.secondary)
                }
                Divider()
                HStack{Text("个人 iCloud").font(.subheadline.weight(.semibold));Spacer();Text(!service.cloudConfigured ? "未配置":(service.cloudOptedIn ? "已允许":"未启用")).font(.caption).foregroundStyle(.secondary)}
                if service.cloudConfigured {
                    if service.cloudOptedIn {
                        cloudActions
                        Text("已允许不代表每个版本均已同步；请以上方实际状态为准。").font(.caption).foregroundStyle(.secondary)

                        Text("已同意后，打开 App、回到前台和保存版本时会正常同步。停用会保留原账号与本机数据，不自动互相搬移。").font(.caption).foregroundStyle(.secondary)
                        if service.preferencesQualified {
                            Toggle("同步外观偏好",isOn:Binding(get:{service.preferencesOptedIn},set:{service.setPreferenceSync($0)}))
                        }else{Text("外观偏好保留在本设备。").font(.caption).foregroundStyle(.secondary)}
                    }else{Button("启用个人 iCloud 同步"){confirming=true}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(busy)}
                }else{Text("此版本尚未配置 iCloud；离线草稿与已获准的电脑同步不受影响。").font(.caption).foregroundStyle(.secondary)}
                if busy || service.syncBusy{ProgressView("正在核对个人同步").controlSize(.small)}
                if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
                Divider()
                coreSettings
                Button("停用同步，仅在本机保存"){perform{await service.disablePersonalSync()}}.disabled(busy || service.editingScope().owner.id == .local)
                Divider()
                Text("保留的其他来源").font(.subheadline)
                Text("切换账号或停用不会把旧原文带入当前输入。只在这里明确选用或导出；已有输入会再次确认。").font(.caption).foregroundStyle(.secondary)
                ForEach(service.retainedNamespaces.filter{$0 !== service.editingScope().owner},id:\.id){owner in
                    VStack(alignment:.leading,spacing:8){
                        DisclosureGroup(owner.id.core != nil ? "保留的电脑来源":(owner.id.account == nil ? "仅本机":"其他 iCloud 来源")){
                            Text(owner.id.stableName).font(.caption2.monospaced()).textSelection(.enabled)
                        }.font(.caption).foregroundStyle(.secondary)
                        ForEach(Array(owner.catalog).sorted{String(describing:$0)<String(describing:$1)},id:\.self){key in
                            if let ticket=service.retainedExportTicket(owner,key:key){
                                Button{selected = .init(ticket:ticket)}label:{VStack(alignment:.leading,spacing:3){Text(ticket.value.payload.text.isEmpty ? "（空草稿）":ticket.value.payload.text).lineLimit(3);Text(PersonalDraftPresentation.kind(key)).font(.caption).foregroundStyle(.secondary)}}.buttonStyle(.plain)
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
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:560,scrolls:false))
        .sheet(item:$selected){choice in RetainedPersonalDraftView(service:service,ticket:choice.ticket)}
        .confirmationDialog("启用个人 iCloud 同步？",isPresented:$confirming,titleVisibility:.visible){Button("同意并核对账号"){perform{try await service.enablePersonalSync()}};Button("取消",role:.cancel){}}message:{Text("使用当前系统 iCloud 账号的私有空间。各设备原文保留独立分支，本机旧原文不会自动上传到新账号。")}
    }
    private var cloudActions:some View {
        LazyVGrid(columns:[GridItem(.adaptive(minimum:150),alignment:.leading)],alignment:.leading,spacing:10){
            Button("核对账号并重试"){perform{try await service.enablePersonalSync()}}
            Button("停用个人同步"){perform{await service.disablePersonalSync()}}
        }.disabled(busy || service.syncBusy)
    }
    @ViewBuilder private var coreSettings:some View {
        VStack(alignment:.leading,spacing:8){
            Text("通过已配对电脑同步").font(.subheadline)
            Text("需电脑将此设备纳入个人设备，并由本设备同意。与 iCloud 分开选择，不会导入其他来源的旧草稿；断连后原文继续保留。").font(.caption).foregroundStyle(.secondary)
            Button("查看当前电脑的同步资格"){perform{try await service.inspectCore()}}.disabled(busy || service.coreTransport==nil)
            if let current=service.coreStatus{
                Text(coreReason(current)).font(.caption).foregroundStyle(.secondary)
                if current.member?.revoked==false{
                    Button(service.usingCore && current.member?.optedIn==true ? "核对并同步当前版本":"同意在此个人空间同步草稿"){if service.usingCore,current.member?.optedIn==true{perform{try await service.synchronizePersonal()}}else{confirmingCore=true}}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(busy || service.syncBusy)
                }
                if current.member?.optedIn==true{Button("停用此设备的电脑同步"){perform{try await service.disableCoreConsent()}}.disabled(busy || service.syncBusy)}
                if current.owner{
                    DisclosureGroup("个人设备归属"){
                        VStack(alignment:.leading,spacing:8){
                            Text("仅管理已认证的准确设备授权；不改变其机组权限。不把所有已配对设备自动归为本人。").font(.caption).foregroundStyle(.secondary)
                            Button(current.reason == "restoreRequired" ? "确认恢复并创建新的个人空间":"配置本机个人草稿空间"){configuringCore=true}.disabled(busy)
                            Button("读取已认证设备与个人成员"){perform{try await service.loadCoreMembers()}}.disabled(busy || current.space==nil)
                            ForEach(service.coreGrants.filter{$0.revokedAt==nil}){grant in
                                VStack(alignment:.leading,spacing:6){
                                    Text(grant.deviceLabel).font(.subheadline.weight(.medium))
                                    Text(String(grant.id.prefix(12))+" · "+String(grant.clientSpkiSha256.prefix(12))).font(.caption2.monospaced()).foregroundStyle(.secondary)
                                    HStack{Button("纳入个人设备"){enableGrant=true;pendingGrant=grant};Button("移出"){enableGrant=false;pendingGrant=grant}}.font(.caption)
                                }.padding(.vertical,4)
                            }
                            ForEach(service.coreTransport?.members ?? [],id:\.id){member in Text("成员 "+String(member.id.prefix(8))+" · "+(member.revoked ? "已移出":(member.optedIn ? "已同意":"待本设备同意"))).font(.caption2).foregroundStyle(.secondary)}
                        }
                    }
                }
                ForEach(service.coreTransport?.pending ?? []){row in
                    VStack(alignment:.leading,spacing:5){
                        Text("原个人草稿操作待核对").font(.caption)
                        Text(row.params["requestId"].string ?? "").font(.caption2.monospaced()).textSelection(.enabled)
                        LazyVGrid(columns:[GridItem(.adaptive(minimum:150),alignment:.leading)],alignment:.leading,spacing:10){Button("只读查原结果"){perform{try await service.coreTransport?.recover(row,sealMissing:false)}};Button("封闭未提交的原请求"){perform{try await service.coreTransport?.recover(row,sealMissing:true)}}}.font(.caption)
                        Text("封闭只会拒绝尚未提交的同一原请求，不会把原文标为已同步，也不会重发。可明确保存当前内容为新版本。").font(.caption2).foregroundStyle(.secondary)
                    }
                }
                if let message=service.coreTransport?.message{Text(message).font(.caption).foregroundStyle(.secondary)}
            }else if service.coreTransport==nil{Text("尚未连接已核验的电脑；离线草稿仍可编辑和导出。").font(.caption).foregroundStyle(.secondary)}
        }
        .confirmationDialog("通过当前电脑同步个人草稿？",isPresented:$confirmingCore,titleVisibility:.visible){Button("同意并选择此来源"){perform{try await service.selectCore()}};Button("取消",role:.cancel){}}message:{Text("仅传输完整文字、@ 与回复引用。其它设备版本独立保留，不上传附件凭据，不自动连接或发送。原本机/iCloud 内容不会自动导入。")}
        .confirmationDialog("配置新的或现有的个人草稿空间？",isPresented:$configuringCore,titleVisibility:.visible){Button("确认配置本机成员"){perform{guard let adapter=service.coreTransport else{throw AppleSyncFailure.invalid};try await adapter.configureOwner(localEnabled:true,acknowledgeRestore:adapter.status?.reason=="restoreRequired")}};Button("取消",role:.cancel){}}message:{Text("恢复确认会为恢复后的数据库创建新空间；旧空间的原文仍保留，不会沿用旧设备订阅。")}
        .confirmationDialog("更改此准确设备的个人归属？",isPresented:Binding(get:{pendingGrant != nil},set:{if !$0{pendingGrant=nil}}),titleVisibility:.visible){Button(enableGrant ? "确认纳入":"确认移出",role:enableGrant ? nil:.destructive){guard let grant=pendingGrant else{return};pendingGrant=nil;perform{try await service.coreTransport?.setMember(grant:grant,enabled:enableGrant)}};Button("取消",role:.cancel){pendingGrant=nil}}message:{Text("只针对核对的设备授权；移出后该设备不能再读取此私有空间，原配对不变。")}
    }
    private func coreReason(_ current:CrewPersonalCoreWire.Status)->String{
        switch current.reason{case "notConfigured":return "当前电脑尚未配置个人空间。";case "notApproved":return "此设备尚未被主机明确纳入个人空间。";case "optedOut":return "此设备已获准，尚未同意同步。";case "restoreRequired":return "数据库恢复后需主机确认新空间；旧原文仍可导出。";default:return current.available ? "此设备当前具备个人草稿同步资格。":"个人同步当前不可用；原文保留。"}
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
            PersonalDraftOrigin(key:ticket.value.key)
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
                }.buttonStyle(CrewdPrimaryButtonStyle()).disabled((!service.cloudOptedIn && !service.usingCore) || service.editingScope().owner.id == .local || service.editingScope().owner === ticket.owner)
            }
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:460,scrolls:false))
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
