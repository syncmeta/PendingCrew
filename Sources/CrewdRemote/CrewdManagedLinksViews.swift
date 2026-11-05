import SwiftUI

struct CrewdManagedLinksEntry:View {
    @ObservedObject var workspace:CrewdWorkspace
    let source:CrewdManagedTableV2Wire.Source
    var current:@MainActor ()->Bool={true}
    @State private var showing=false
    var body:some View {
        if workspace.client.capabilities.contains(CrewdManagedTableV2Wire.linksCapability) {
            Button("关联对象",systemImage:"point.3.connected.trianglepath.dotted"){showing=true}.disabled(!current())
                .sheet(isPresented:$showing){CrewdManagedLinksView(workspace:workspace,store:workspace.managedTablesV2,source:source,current:{showing && current()})}
        }
    }
}
struct CrewdManagedTargetButton:View {
    @ObservedObject var workspace:CrewdWorkspace
    let target:CrewdManagedTableV2Wire.Target
    let summary:String
    var current:@MainActor ()->Bool={true}
    @State private var busy=false
    @State private var visible=true
    @State private var action=UUID()
    @State private var failure:String?
    private var supported:Bool {![CrewdManagedTableV2Wire.TargetKind.document,.knowledge].contains(target.kind) || workspace.client.capabilities.contains(CrewdManagedTableV2Wire.knowledgeCapability)}
    var body:some View {VStack(alignment:.leading,spacing:3) {
        Button {action=UUID();let captured=action;busy=true
            Task{defer{if action==captured{busy=false}}
                do{try await workspace.openManagedBusinessTarget(target,current:{visible && action==captured && current()})}
                catch{if visible && action==captured && current(){failure=error.localizedDescription}}
            }
        } label:{VStack(alignment:.leading,spacing:3){Text(summary.isEmpty ? target.label:summary);Text(target.kind == .message ? "打开所在会话":"打开原对象").font(.caption).foregroundStyle(.secondary)}}
        .buttonStyle(.plain).disabled(busy || !current() || !supported)
        if !supported {Text("当前连接暂不能读取此资料目标；关联仍保留。").font(.caption).foregroundStyle(.secondary)}
        if let failure {Text(failure).font(.caption).foregroundStyle(.orange)}
    }.onAppear{visible=true;action=UUID()}.onDisappear{visible=false;action=UUID();busy=false}}
}

private struct CrewdManagedLinksView:View {
    typealias Wire=CrewdManagedTableV2Wire
    typealias Store=CrewdManagedTableV2Store
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:Store
    let source:Wire.Source
    let current:@MainActor ()->Bool
    @Environment(\.dismiss) private var dismiss
    @State private var scope:Store.Scope?
    @State private var crew=""
    @State private var edges:[Wire.Edge]=[]
    @State private var direction="outgoing"
    @State private var visible=true
    @State private var presentation=UUID()
    @State private var columnNames:[String:String]=[:]
    @State private var hidden=false
    @State private var loaded=false
    @State private var next:String?
    @State private var revision:String?
    @State private var seenCursors:Set<String>=[]
    @State private var pending:[Store.Pending]=[]
    @State private var picker=false
    @State private var draft:Wire.LinkWrite?
    @State private var removing:Wire.Edge?
    @State private var busy=false
    @State private var ticket=UUID()
    @State private var refreshQueued=false
    @State private var notice:String?
    @State private var problem:String?
    private var route:String {[workspace.authorityRecordID.uuidString,workspace.observationGeneration.uuidString,workspace.client.connectionEpoch.uuidString,workspace.crewID,String(workspace.originalJournalReady),String(workspace.client.readyForWork),source.key,direction].joined(separator:"/")}
    private var allowed:Bool {visible && current() && !workspace.isRetired && workspace.originalJournalReady && workspace.client.readyForWork && store.supportsLinks}
    var body:some View {
        NavigationStack {ScrollView {VStack(alignment:.leading,spacing:12) {
            Text(source.label).font(.headline).textSelection(.enabled)
            Picker("关联方向",selection:$direction){Text("此对象关联").tag("outgoing");Text("反向引用").tag("incoming")}.disabled(busy || draft != nil)
            Text("手工关联只记录关系。原计划、需求、问题、决定与风险的实际状态，仍由各自原入口管理。").font(.caption).foregroundStyle(.secondary)
            if let scope,allowed,store.owns(scope) {
                Text(crew.isEmpty ? "关联归属尚未核实":"关联归属机组 "+crew).font(.caption)
                if direction=="outgoing" {Button("选择要关联的对象…",systemImage:"plus"){picker=true}.disabled(busy || revision==nil || activeUnknown(scope))}
                if let draft {Text(draft.target?.label ?? "产物 "+(draft.input?.value ?? ""));Button("确认建立手工关联"){link(draft,scope:scope)}.disabled(busy || activeUnknown(scope));Button("取消这次尚未提交的选择"){self.draft=nil}}
                ForEach(edges){edge in VStack(alignment:.leading,spacing:5) {
                    if direction=="incoming" {CrewdManagedSourceButton(workspace:workspace,source:edge.source,current:{allowed && store.owns(scope)})}
                    else {CrewdManagedTargetButton(workspace:workspace,target:edge.target,summary:edge.summary,current:{allowed && store.owns(scope)})}
                    Text(relationTitle(edge)+" · 归属机组 "+edge.ownerCrewId).font(.caption).foregroundStyle(.secondary)
                    if direction=="outgoing" && edge.editable && edge.origin.origin=="manualRelates" {Button("移除此手工关联",role:.destructive){removing=edge}.disabled(busy || activeUnknown(scope))}
                    else {Text("请在源对象或原业务入口管理此关系。").font(.caption).foregroundStyle(.secondary)}
                }.padding(10).background(.quaternary,in:RoundedRectangle(cornerRadius:8))}
                if direction=="outgoing" && hidden {Text("部分关联当前不可见；这里不显示其目标或数量。").font(.caption).foregroundStyle(.secondary)}
                if let next {Button("更多关联"){Task{await load(after:next)}}.disabled(busy)}
                if loaded && edges.isEmpty && next==nil && !hidden && !busy {Text("本次读取没有可见关联。").foregroundStyle(.secondary)}
                ForEach(pending){original in
                    Text("原请求待核对 · "+original.witness.requestId).font(.caption).textSelection(.enabled)
                    Button("只读查询原回执"){recover(original,scope:scope)}.disabled(busy)
                }
            } else if !busy {Text("原连接或范围已变化，请重新打开关联。").foregroundStyle(.secondary)}
            if let notice {Text(notice).font(.caption).textSelection(.enabled)}
            if let problem {Text(problem).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
            if busy {ProgressView().controlSize(.small)}
        }.padding(20)}.navigationTitle("对象关联").toolbar {
            ToolbarItem(placement:.cancellationAction){Button("关闭"){dismiss()}.disabled(busy)}
            ToolbarItem{Button("刷新"){Task{await load()}}.disabled(busy)}
        }}
        #if os(macOS)
        .frame(minWidth:520,minHeight:480)
        #endif
        .task(id:route){ticket=UUID();busy=false;scope=nil;edges=[];next=nil;revision=nil;pending=[];draft=nil;loaded=false;hidden=false;crew="";columnNames=[:];seenCursors=[];await load()}
        .onChange(of:store.invalidation){_,_ in refreshQueued=true;refreshIfReady()}
        .onChange(of:busy){_,_ in refreshIfReady()}
        .onChange(of:picker){_,_ in refreshIfReady()}
        .onAppear{visible=true;presentation=UUID()}
        .onDisappear{visible=false;presentation=UUID();ticket=UUID()}
        .onChange(of:workspace.managedBusinessNavigation?.id){_,id in if id != nil {dismiss()}}
        .sheet(isPresented:$picker){if let scope {CrewdManagedV2TargetPicker(workspace:workspace,store:store,scope:scope,crew:crew,current:{allowed && picker && store.owns(scope)},choose:{value in if allowed {draft=value;picker=false}})}}
        .confirmationDialog("仅移除这一个手工关联？",isPresented:Binding(get:{removing != nil},set:{if !$0{removing=nil}})) {
            Button("移除此关联",role:.destructive){if let edge=removing,let scope {unlink(edge,scope:scope)}};Button("取消",role:.cancel){removing=nil}
        } message:{Text("不修改原对象内容、其他列或原业务关系。")}
    }
    private func relationTitle(_ edge:Wire.Edge)->String {
        if edge.origin.origin=="column",direction=="outgoing",let id=edge.origin.fieldId,let label=columnNames[id] {return "关联列："+label}
        return edge.origin.title
    }
    private func activeUnknown(_ scope:Store.Scope)->Bool {pending.contains{Wire.exact($0.witness.managedScope,scope.authority)}}
    private func refreshPending(_ scope:Store.Scope) {do{pending=try store.pending(scope).filter{$0.witness.manual.map{Wire.exact($0.operation.source,source)}==true}}catch{problem=error.localizedDescription}}
    private func refreshIfReady(){guard refreshQueued,!busy,!picker,draft==nil,removing==nil,allowed else{return};refreshQueued=false;Task{await load()}}
    private func load(after:String?=nil) async {
        guard allowed,!busy else{return};busy=true;let captured=route,request=UUID();ticket=request
        @MainActor func valid()->Bool {allowed && route==captured && ticket==request}
        defer{if ticket==request{busy=false}}
        do{
            let s=try await store.scope(stillCurrent:valid)
            guard valid() else{return};scope=s;refreshPending(s)
            if after==nil{loaded=false}
            let owner:String
            if source.kind=="artifact" {owner=source.metadataCrewId!}
            else {guard let target=source.target,let value=try await store.resolve(target,scope:s,current:valid).ownerCrewId else {throw Store.Problem.invalid};owner=value}
            if source.kind=="row",let id=source.id,store.supportsTables {let (table,_)=try await store.row(id,scope:s,current:valid);columnNames=Dictionary(uniqueKeysWithValues:table.fields.map{($0.id,$0.name+($0.hidden ? "（已隐藏）":""))})}
            let page:Wire.EdgePage
            if direction=="incoming" {guard let target=source.target else{throw Store.Problem.invalid};page=try await store.links(s,target:target,after:after,current:valid)}
            else {page=try await store.links(s,source:source,after:after,current:valid)}
            guard valid() else{return}
            if after==nil {seenCursors=[]}
            if let after {seenCursors.insert(after)}
            guard page.nextAfter.map({!seenCursors.contains($0)}) ?? true else{throw Store.Problem.invalid}
            if after != nil {guard Set(edges.map(\.id)).isDisjoint(with:page.items.map(\.id)),revision==page.manualLinkRevision else {throw Store.Problem.stale};edges+=page.items}
            else {edges=page.items}
            crew=owner;scope=s;loaded=true;hidden=page.outgoingHiddenSet != nil;next=page.nextAfter;revision=page.manualLinkRevision;problem=nil;refreshPending(s)
        }catch{if valid(){problem=error.localizedDescription;if after==nil{edges=[];revision=nil;next=nil;loaded=false;if let scope,!store.owns(scope){self.scope=nil}}}}
    }
    private func link(_ value:Wire.LinkWrite,scope:Store.Scope){guard allowed,!busy,let revision else{return};mutate(.init(kind:"link",source:source,target:value,expectedManualLinkRevision:revision),scope:scope)}
    private func unlink(_ edge:Wire.Edge,scope:Store.Scope){guard allowed,!busy,edge.editable,edge.origin.origin=="manualRelates",let revision else{return};removing=nil;mutate(.init(kind:"unlink",source:source,expectedManualLinkRevision:revision,expectedEdgeId:edge.id),scope:scope)}
    private func mutate(_ operation:Wire.ManualOperation,scope:Store.Scope){busy=true;let captured=presentation
        @MainActor func live()->Bool {allowed && presentation==captured && store.owns(scope)}
        Task{defer{if presentation==captured{busy=false}}
            do{let observed=try await store.submitLink(operation,crew:crew,scope:scope,current:live);guard live() else{return};draft=nil;notice="原关联请求已确认 · "+observed.receipt.requestId;refreshPending(scope);refreshQueued=true}
            catch{if live(){problem=error.localizedDescription;refreshPending(scope)}}
        }
    }
    private func recover(_ original:Store.Pending,scope:Store.Scope){guard allowed,!busy else{return};busy=true;let captured=presentation
        @MainActor func live()->Bool {allowed && presentation==captured && store.owns(scope)}
        Task{defer{if presentation==captured{busy=false}}
            do{let observed=try await store.lookupLink(original,scope:scope,current:live);guard live() else{return};notice=observed.map{($0.historical ? "原历史关联请求已核对 · ":"原关联请求已核对 · ")+$0.receipt.requestId} ?? "原结果仍未知，原请求保留；没有重新提交。";refreshPending(scope)}
            catch{if live(){problem=error.localizedDescription}}
        }
    }
}

struct CrewdManagedV2TargetPicker:View {
    typealias Wire=CrewdManagedTableV2Wire
    typealias Store=CrewdManagedTableV2Store
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:Store
    let scope:Store.Scope
    let crew:String
    var constraint:Wire.Constraint?=nil
    let current:@MainActor ()->Bool
    let choose:@MainActor (Wire.LinkWrite)->Void
    @Environment(\.dismiss) private var dismiss
    @State private var visible=true
    @State private var previewTicket=UUID()
    @State private var kind=Wire.TargetKind.work
    @State private var identifier=""
    @State private var summary:String?
    @State private var selected:Wire.LinkWrite?
    @State private var busy=false
    @State private var problem:String?
    @State private var artifactMode="existing"
    @State private var artifactItems:[CrewdArtifactWire.Artifact]=[]
    @State private var artifactNext:CrewdArtifactWire.Reference?
    @State private var artifactChoice:CrewdArtifactWire.Reference?
    @State private var artifactKind=CrewdArtifactWire.InputKind.commit
    @State private var artifactValue=""
    @State private var repository=""
    @State private var knowledgeCrew=""
    @State private var reader:CrewdKnowledgeStore?
    @State private var knowledgeChoice:CrewdChannelKnowledgeReference?
    private var available:[Wire.TargetKind] {
        let allowed=Wire.TargetKind.allCases.filter{workspace.client.capabilities.contains(Wire.knowledgeCapability) || ![.document,.knowledge].contains($0)}
        guard let constraint,constraint.kind != "anyReadable" else{return allowed}
        return allowed.filter{$0.rawValue==(constraint.kind=="knowledgeView" ? "knowledge":constraint.kind)}
    }
    private var draftKey:Data {Data([kind.rawValue,identifier,artifactMode,artifactChoice?.id ?? "",artifactKind.rawValue,artifactValue,repository,knowledgeChoice.map{$0.identity+":"+$0.context.generation} ?? ""].map{String($0.utf8.count)+":"+$0}.joined().utf8)}
    var body:some View {
        NavigationStack {ScrollView {VStack(alignment:.leading,spacing:14) {
            Text("先按当前权限核对目标，再加入这份表单。此处不会修改原对象状态。").font(.caption).foregroundStyle(.secondary)
            if available.isEmpty {Text("当前连接尚未提供这类资料目标能力；原关联仍保留。").foregroundStyle(.secondary)}
            Picker("目标类型",selection:$kind){ForEach(available){Text($0.title).tag($0)}}
            if kind == .artifact {
                Picker("产物来源",selection:$artifactMode){Text("选择已登记产物").tag("existing");Text("填写新的自然键").tag("declare")}
                if artifactMode=="existing" {
                    Button("读取已有产物"){readArtifacts(nil)}.disabled(busy)
                    ForEach(artifactItems){item in Button{artifactChoice=item.reference}label:{HStack{VStack(alignment:.leading){Text(item.reference.key);Text(item.reference.location+" · 未核验").font(.caption)};Spacer();if artifactChoice==item.reference{Image(systemName:"checkmark")}}}.buttonStyle(.plain)}
                    if let artifactNext {Button("更多产物"){readArtifacts(artifactNext)}.disabled(busy)}
                } else {
                Picker("指针类型",selection:$artifactKind){ForEach(CrewdArtifactWire.InputKind.allCases){Text($0.title).tag($0)}}
                TextField("产物自然键或链接",text:$artifactValue,axis:.vertical)
                TextField("仓库标识（显式填写，可留空）",text:$repository)
                Text("不扫描磁盘或Git，不猜仓库；短提交、文件和测试未填写仓库时归入该机组未归属指针，仍标为未核验。").font(.caption).foregroundStyle(.secondary)
                }
            } else if [.document,.knowledge].contains(kind) {
                Picker("源机组",selection:$knowledgeCrew){ForEach(workspace.crews.compactMap{$0["id"].string},id:\.self){id in Text(workspace.crews.first{$0["id"].string==id}?["title"].string ?? id).tag(id)}}
                if let reader {CrewdManagedV2KnowledgeChoices(reader:reader,crew:knowledgeCrew,documentOnly:kind == .document,current:current,choose:{knowledgeChoice=$0})}
                else {Text("先读取有权访问的实际源资料。").foregroundStyle(.secondary)}
                Button("读取源资料"){openKnowledge()}.disabled(busy || knowledgeCrew.isEmpty)
            } else if kind != .human {TextField("真实"+kind.title+"编号",text:$identifier)}
            if let summary {Text(summary).textSelection(.enabled)}
            if let problem {Text(problem).foregroundStyle(.orange).textSelection(.enabled)}
            Button("核对当前目标"){preview()}.disabled(busy || !current() || !available.contains(kind))
            Button("使用这个目标"){if let selected,current(){choose(selected);if current(){dismiss()}}}.buttonStyle(.borderedProminent).disabled(busy || selected==nil || !current())
            if busy {ProgressView().controlSize(.small)}
        }.padding(20)}.navigationTitle("选择关联目标").toolbar{ToolbarItem(placement:.cancellationAction){Button("关闭"){dismiss()}}}}
        #if os(macOS)
        .frame(minWidth:500,minHeight:400)
        #endif
        .onAppear{visible=true;previewTicket=UUID();kind=available.first ?? .work;knowledgeCrew=workspace.crewID}
        .onDisappear{visible=false;previewTicket=UUID()}
        .onChange(of:draftKey){_,_ in previewTicket=UUID();selected=nil;summary=nil;problem=nil}
        .onChange(of:knowledgeCrew){_,_ in reader=nil;knowledgeChoice=nil;selected=nil;summary=nil}
        .onChange(of:store.invalidation){_,_ in previewTicket=UUID();selected=nil;summary=nil}
        .onReceive(workspace.authorityEvents){name,data in
            reader?.handleEvent(name,data)
            if name=="data.changed" || name=="events.lagged" {previewTicket=UUID();knowledgeChoice=nil;selected=nil;summary=nil}
        }
    }
    private func readArtifacts(_ after:CrewdArtifactWire.Reference?) {
        guard current(),!busy else{return};busy=true
        Task{defer{busy=false}
            do{let page=try await store.artifacts(scope,after:after,current:current);guard current() else{return}
                if after != nil {guard Set(artifactItems.map(\.id)).isDisjoint(with:page.items.map(\.id)) else{throw Store.Problem.invalid};artifactItems+=page.items}else{artifactItems=page.items}
                artifactNext=page.nextAfter
            }catch{if current(){problem=error.localizedDescription}}
        }
    }
    private func openKnowledge(){guard current(),!busy,workspace.client.capabilities.contains(Wire.knowledgeCapability) else{return};reader=workspace.makeManagedLinkKnowledgeReader(crew:knowledgeCrew);knowledgeChoice=nil}
    private func preview(){guard visible,current(),!busy else{return};busy=true;problem=nil;selected=nil;previewTicket=UUID();let ticket=previewTicket,captured=draftKey
        @MainActor func valid()->Bool{visible && current() && previewTicket==ticket && draftKey==captured}
        Task{defer{busy=false}
            do{
                if kind == .artifact && artifactMode=="existing" {
                    guard let artifactChoice else{throw Store.Problem.invalid}
                    let target=Wire.Target(kind:.artifact,reference:artifactChoice)
                    let value=try await store.resolve(target,scope:scope,current:valid)
                    guard valid() else{return};summary=value.summary+" · "+artifactChoice.location+" · 未核验";selected = .init(kind:"reference",target:value.target)
                } else if kind == .artifact {
                    let input=CrewdArtifactWire.Input(kind:artifactKind,value:artifactValue,repository:repository.isEmpty ? nil:repository)
                    let ref=try await store.previewArtifact(input,crew:crew,scope:scope,current:valid)
                    guard valid() else{return};summary=ref.key+" · "+ref.location+" · 未核验";selected = .init(kind:"declareArtifact",input:input)
                } else {
                    let target:Wire.Target
                    if [.document,.knowledge].contains(kind){guard let knowledgeChoice else{throw Store.Problem.invalid};target = .init(kind:kind,source:knowledgeChoice)}
                    else {target = .init(kind:kind,idValue:kind == .human ? nil:identifier)}
                    let value=try await store.resolve(target,scope:scope,current:valid)
                    if constraint?.kind=="row",let table=constraint?.tableId {let (actual,_)=try await store.row(target.idValue!,scope:scope,current:valid);guard actual.id==table else{throw Store.Problem.invalid}}
                    guard valid() else{return};summary=value.summary;selected = .init(kind:"reference",target:value.target)
                }
            }catch{if valid(){problem=error.localizedDescription}}
        }
    }
}

private struct CrewdManagedV2KnowledgeChoices:View {
    @ObservedObject var reader:CrewdKnowledgeStore
    let crew:String
    let documentOnly:Bool
    let current:@MainActor ()->Bool
    let choose:@MainActor (CrewdChannelKnowledgeReference)->Void
    @State private var selected=""
    var body:some View {VStack(alignment:.leading,spacing:8) {
        if let catalog=reader.linkSourceCatalog(crew:crew) {
            ForEach(catalog.items.filter{!documentOnly || $0.kind == .document}){item in Button {if current(),let reference=reader.catalogLinkSource(item.id,kind:item.kind,crew:crew){selected=item.id;choose(reference)}} label:{HStack{Text(item.title);Spacer();if selected==item.id{Image(systemName:"checkmark")}}}.buttonStyle(.plain)}
            if catalog.next != nil {Text("资料列表尚未读完。").font(.caption);Button("读取更多源资料"){Task{if current(){await reader.loadNextViewCatalogPage()}}}.disabled(reader.busy)}
        } else {Text(reader.status).font(.caption).foregroundStyle(.secondary)}
        if reader.busy {ProgressView().controlSize(.small)}
    }.task{if current(){await reader.refreshViewCatalog(includeArchived:false)}}}
}

/// Reuses the actual document/page renderer with a read-only RPC adapter.
struct CrewdManagedKnowledgeTargetView:View {
    @ObservedObject var workspace:CrewdWorkspace
    let source:CrewdChannelKnowledgeReference
    var documentOnly=false
    let current:@MainActor ()->Bool
    @Environment(\.dismiss) private var dismiss
    @State private var reader:CrewdKnowledgeStore?
    @State private var failure:String?
    var body:some View {
        NavigationStack {Group {
            if let reader {CrewdManagedKnowledgeReadSurface(reader:reader,source:source,documentOnly:documentOnly,current:current)}
            else if let failure {Text(failure).padding()}
            else {ProgressView("正在核对实际资料…").padding()}
        }.navigationTitle("关联资料 · 只读").toolbar{ToolbarItem(placement:.cancellationAction){Button("关闭"){dismiss()}}}}
        #if os(macOS)
        .frame(minWidth:600,minHeight:480)
        #endif
        .task(id:workspace.client.connectionEpoch.uuidString+source.identity){
            guard current(),workspace.client.capabilities.contains(CrewdManagedTableV2Wire.knowledgeCapability) else {failure="当前连接不能读取此资料目标；关联仍保留。";return}
            reader=workspace.makeManagedLinkKnowledgeReader(crew:source.context.crewId)
        }
        .onDisappear{reader?.renderer.revoke();reader=nil}
        .onReceive(workspace.authorityEvents){name,data in reader?.handleEvent(name,data)}
        .onChange(of:workspace.observationGeneration){_,_ in reader?.renderer.revoke();reader=nil;failure="原资料权限或连接已改变，请重新打开。"}
    }
}
private struct CrewdManagedKnowledgeReadSurface:View {
    @ObservedObject var reader:CrewdKnowledgeStore
    let source:CrewdChannelKnowledgeReference
    let documentOnly:Bool
    let current:@MainActor ()->Bool
    @State private var verified=false
    @State private var observedContext:CrewdDataContext?
    @State private var failure:String?
    var body:some View {
        Group {if verified,current(),let detail=reader.detail {CrewdKnowledgeObjectSurface(store:reader,detail:detail)}
            else if let failure {Text(failure).padding()}
            else {ProgressView("正在读取实际源对象…").padding()}}
        .task(id:source.identity){
            await reader.refresh(loadViews:false)
            guard current(),let context=reader.workspace?.context,
                  context.dbId.utf8.elementsEqual(source.context.dbId.utf8),context.authorityId.utf8.elementsEqual(source.context.authorityId.utf8),
                  context.restoreEpoch.utf8.elementsEqual(source.context.restoreEpoch.utf8),context.crewId.utf8.elementsEqual(source.context.crewId.utf8),
                  context.namespace.utf8.elementsEqual(source.context.namespace.utf8) else {failure="资料权属或恢复范围已改变。";return}
            await reader.open(source.reference)
            guard current(),let detail=reader.detail,detail.object.identity==source.reference.id,case .view(_,_,_,let definition,_,_)=detail.object else {failure="原资料当前不可读取。";return}
            if documentOnly {guard case .document=definition else {failure="原文档定义已变化；未改用其他页面。";return}}
            observedContext=reader.workspace?.context;verified=true
        }
        .onChange(of:reader.workspace?.context){_,context in if verified && context != observedContext {verified=false;failure="源资料版本已改变，请重新打开核对。"}}
    }
}

private struct CrewdManagedSourceButton:View {
    @ObservedObject var workspace:CrewdWorkspace
    let source:CrewdManagedTableV2Wire.Source
    let current:@MainActor ()->Bool
    @State private var busy=false
    @State private var failure:String?
    @State private var visible=true
    @State private var action=UUID()
    var body:some View {VStack(alignment:.leading,spacing:4) {
        if let target=source.target {
            if source.kind=="artifact",let crew=source.metadataCrewId {
                Button("在机组 "+crew+" 打开源产物"){action=UUID();let captured=action;busy=true;Task{defer{if action==captured{busy=false}};do{try await workspace.openManagedBusinessTarget(target,metadataCrewId:crew,current:{visible && action==captured && current()})}catch{if visible && action==captured && current(){failure=error.localizedDescription}}}}.disabled(busy || !current())
            }else{CrewdManagedTargetButton(workspace:workspace,target:target,summary:source.label,current:current)}
        }
        if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
    }.onAppear{visible=true;action=UUID()}.onDisappear{visible=false;action=UUID();busy=false}}
}
