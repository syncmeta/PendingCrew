import SwiftUI

/// Product surface: Agent-authored pages inside the original application layout.
/// Reading/reopening this surface never sends a prompt or resumes an Agent.
struct CrewdKnowledgeDocumentsEntry: View {
    @ObservedObject var workspace:CrewdWorkspace
    @State private var library=false
    @State private var collection=CrewdKnowledgeCollection.documents
    var body:some View {
        VStack(spacing:0){
            VStack(alignment:.leading,spacing:8){
                Picker("资料范围",selection:$library){Text("本群引用").tag(false);Text("机组资料库").tag(true)}.pickerStyle(.segmented).frame(maxWidth:300)
                Picker("资料类型",selection:$collection){Text("文档/知识切片").tag(CrewdKnowledgeCollection.documents);Text("Agent页面").tag(CrewdKnowledgeCollection.agentPages)}.pickerStyle(.segmented).frame(maxWidth:360)
            }.frame(maxWidth:.infinity,alignment:.leading).padding(.horizontal,14).padding(.vertical,10)
            Divider()
            if library {
                CrewdKnowledgeWorkspaceView(store:workspace.knowledge,available:workspace.client.capabilities.contains("data"),returnToMessages:{workspace.originalSection="消息"},collection:collection).id(collection)
            }else{CrewdGroupKnowledgeView(workspace:workspace,collection:collection)}
        }
    }
}

/// Native placements and free Agent-authored pages share stored blocks, while
/// their navigation stays separate. The selector never executes page code.
enum CrewdKnowledgeCollection: String {
    case all, documents, agentPages
    func includes(_ object: CrewdDataObject) -> Bool {
        guard case let .view(_, _, _, definition, _, _) = object else { return false }
        switch (self, definition) {
        case (.all, _), (.documents, .document), (.agentPages, .web): return true
        default: return false
        }
    }
}

struct CrewdKnowledgeWorkspaceView: View {
    @ObservedObject var store: CrewdKnowledgeStore
    let available: Bool
    var returnToMessages: (() -> Void)? = nil
    var collection: CrewdKnowledgeCollection = .all
    private var visibleObjects: [CrewdDataObject] { store.objects.filter(collection.includes) }
    @Environment(\.colorScheme) private var scheme
    @State private var selected = ""
    @State private var showSystem = false
    @State private var showArchived = false
    @State private var confirmArchive: CrewdDataObject?
    @State private var previewToken = ""
    @State private var confirmingPreview = false
    var body: some View {
        VStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                HStack { pagePicker; reloadButton; Spacer(); systemButton }
                VStack(alignment: .leading, spacing: 8) {
                    pagePicker
                    HStack { reloadButton; Spacer(); systemButton }
                }
            }.font(.system(size: 12)).padding(.horizontal, 14).padding(.vertical, 10)
                .background(CrewdChatStyle.sidebar(scheme).opacity(0.25))
            Divider().overlay(CrewdChatStyle.line(scheme))
            if !available {
                CrewdEmptySurface(title: "知识数据尚不可用", detail: "连接具备知识权限的后台后，可读取已保存的页面。", symbol: "externaldrive.badge.questionmark")
            } else if let detail = store.detail, collection.includes(detail.object) {
                page(detail)
            } else {
                CrewdEmptySurface(title: "让资料成为可用的页面", detail: "在消息中说明你需要的文档或界面。已保存的页面会显示在这里，打开页面不会启动 Agent。",
                                  symbol: "rectangle.on.rectangle", actionTitle: returnToMessages == nil ? nil : "回到消息", action: returnToMessages)
            }
            HStack(spacing: 6) {
                if store.busy { ProgressView().controlSize(.small) }
                Text(store.status).font(.system(size: 11)).foregroundStyle(CrewdChatStyle.muted(scheme))
                    .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }.padding(.horizontal, 14).padding(.vertical, 8)
                .background(CrewdChatStyle.sidebar(scheme).opacity(0.20))
        }
        .task(id: available) { if available { await reload() } }
        .onDisappear { store.renderer.revoke() }
        .onChange(of: selected) { _, id in
            guard !id.isEmpty, let context = store.workspace?.context else { return }
            UserDefaults.standard.set(id, forKey: pagePreference(context))
            Task { await store.open(.init(namespace: context.namespace, kind: "view", id: id)) }
        }
        .onChange(of: store.workspace?.context) { old, current in
            if old?.authorityId != current?.authorityId || old?.crewId != current?.crewId || old?.namespace != current?.namespace {
                selected = ""
            }
            // Native confirmations bind the state the user actually reviewed.
            confirmArchive = nil; restoreTarget = nil; confirmingPreview = false
            // A new generation/authority invalidates code grants. The page is re-read only.
            if store.detail == nil, !store.busy, available {
                Task { await reload() }
            }
        }
        .sheet(isPresented: $showSystem) {
            #if os(macOS)
            ScrollView { systemControls }.frame(minWidth: 520, minHeight: 360)
            #else
            ScrollView { systemControls }
            #endif
        }
    }
    private var pagePicker: some View {
        Picker("页面", selection: $selected) {
            Text("选择页面").tag("")
            ForEach(visibleObjects, id: \.identity) {
                Text($0.displayTitle).tag($0.identity)
            }
        }.frame(maxWidth: 300)
    }
    private var reloadButton: some View {
        Button { Task { await reload() } } label: { Label("刷新页面", systemImage: "arrow.clockwise") }
            .disabled(store.busy)
    }
    private var systemButton: some View {
        Button { showSystem = true } label: { Label("页面管理", systemImage: "slider.horizontal.3") }
            .help("页面权限、回收站、原请求与恢复控制")
    }
    private func reload() async {
        await store.refresh()
        await store.list(objectType: "view", includeArchived: showArchived)
        // Reopen a persisted page without executing its author code or calling an Agent.
        if let context = store.workspace?.context {
            let preferred = selected.isEmpty ? UserDefaults.standard.string(forKey: pagePreference(context)) ?? "" : selected
            let id = visibleObjects.contains(where: { $0.identity == preferred }) ? preferred : visibleObjects.first?.identity
            if let id { selected = id; await store.open(.init(namespace: context.namespace, kind: "view", id: id)) }
        }
    }
    private func pagePreference(_ context: CrewdDataContext) -> String {
        "pendingcrew.page." + context.authorityId + "." + context.crewId + "." + context.namespace + (collection == .all ? "" : "." + collection.rawValue)
    }
    private func page(_ detail:CrewdDataObjectDetail)->some View {CrewdKnowledgeObjectSurface(store:store,detail:detail)}
    private var systemControls: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack { Text("页面与数据系统控制").font(.headline); Spacer(); Button("完成") { showSystem = false } }
            Text("作者页面不能代替这里的权限、原请求核对与恢复控制。").font(.caption)
            Button("停止页面并撤回写入权限") { store.renderer.revoke() }
            Toggle("显示回收站", isOn: $showArchived)
                .onChange(of: showArchived) { _, _ in Task { await reload() } }
            if let object = store.detail?.object, case .view = object {
                Button(object.isArchived ? "恢复当前页面…" : "移入回收站…") { confirmArchive = object }
            }
            ForEach(store.pending) { intent in
                HStack {
                    Text("待核对操作 " + intent.id.uuidString).font(.caption).textSelection(.enabled)
                    Button("查询原结果") { Task { await store.lookup(intent.id) } }.disabled(store.busy)
                }
            }
            DisclosureGroup("审阅 Agent 提交的危险数据变更") {
                TextField("变更确认 token", text: $previewToken)
                Button("读取待确认变更") { Task { await store.readPreview(previewToken) } }
                    .disabled(previewToken.isEmpty || store.busy)
                if let preview = store.preview {
                    Text("预计影响 \(preview.affectedRows) 行" + (preview.schemaChanged ? "；包含 Schema 变更" : ""))
                    ForEach(Array(preview.statements.enumerated()), id: \.offset) { _, statement in
                        Text(statement.sql).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                        Text((try? CrewdKnowledgeStore.encode(statement.params).pretty) ?? "").font(.caption).textSelection(.enabled)
                    }
                    Button("确认执行所示变更…") { confirmingPreview = true }
                        .disabled(store.busy || store.hasBlockingPending)
                }
            }
            Button("读取恢复快照") { Task { await store.loadSnapshots() } }
            if let history = store.snapshots {
                VStack(alignment: .leading) {
                    ForEach(history.items, id: \.id) { snapshot in
                        HStack {
                            Text(snapshot.createdAt + " · " + snapshot.generation).font(.caption)
                            Spacer()
                            Button("恢复…") { restoreTarget = snapshot.id }
                        }
                    }
                }
            }
            Text(store.status).font(.caption).foregroundStyle(.secondary)
        }.padding()
        .confirmationDialog("执行刚才显示的 SQL 及参数？后台会先保存恢复快照。", isPresented: $confirmingPreview) {
            Button("明确执行", role: .destructive) { Task { await store.confirmPreview(); await reload() } }
            Button("取消", role: .cancel) {}
        }
        .confirmationDialog("更改当前页面的回收状态？", isPresented: Binding(get: { confirmArchive != nil }, set: { if !$0 { confirmArchive = nil } })) {
            if let object = confirmArchive {
                Button("确认") {
                    confirmArchive = nil
                    Task {
                        await store.submitFromNativeHost(.apply(message: object.isArchived ? "Restore page" : "Archive page", edits: [.archiveView(id: object.identity, expectedRevision: object.revision, archived: !object.isArchived)]))
                        await reload()
                    }
                }
            }
            Button("取消", role: .cancel) { confirmArchive = nil }
        }
        .confirmationDialog("恢复知识快照？当前数据会先保留恢复副本，旧请求不会重放。", isPresented: Binding(get: { restoreTarget != nil }, set: { if !$0 { restoreTarget = nil } })) {
            Button("明确恢复", role: .destructive) {
                guard let target = restoreTarget else { return }; restoreTarget = nil
                Task { await store.restoreSnapshot(target); await reload() }
            }
            Button("取消", role: .cancel) { restoreTarget = nil }
        }
    }
    @State private var restoreTarget: String?
}

private struct CrewdKnowledgePlacementView: View {
    @ObservedObject var store:CrewdKnowledgeStore
    let parent:CrewdDataObjectDetail
    let placement:CrewdDataPlacement
    @State private var block:CrewdDataObjectDetail?
    @State private var loaded=false
    var body:some View {
        VStack(alignment:.leading,spacing:6) {
            if let block,case .block(_,_,_,let content,_,_)=block.object {
                Text(content.string ?? content.pretty).textSelection(.enabled)
                Text(placement.atRevision == nil ? "Live 共享引用":"固定版本引用").font(.caption).foregroundStyle(.secondary)
            } else {Text(loaded ? "不可用":"尚未读取").foregroundStyle(.secondary)}
        }.frame(maxWidth:.infinity,alignment:.leading).padding(10).background(.quaternary.opacity(0.25))
        .task(id:parent.object.revision+":"+placement.id) {
            block=nil;loaded=false
            do {block=try await store.resolved(placement.block,anchor:"placement:"+placement.id,atRevision:placement.atRevision,parent:parent)}catch{block=nil}
            loaded=true
        }
    }
}


extension CrewdDataObject {
    var isArchived: Bool {
        switch self { case .block(_,_,_,_,let value,_), .view(_,_,_,_,let value,_), .relation(_,_,_,_,_,_,let value,_): return value }
    }
}

private struct CrewdKnowledgeObjectSurface:View {
    @ObservedObject var store:CrewdKnowledgeStore
    let detail:CrewdDataObjectDetail
    var body:some View {
        switch detail.object {
        case .view(_, _, _, let definition, let archived, _):
            if archived { Text("页面已移入回收站。可在系统控制中恢复。").padding() }
            else {
                switch definition {
                case .web(let renderer, let version, _, _, _, _, _, _):
                    if renderer == "web", version == "1" { CrewdKnowledgeCanvas(runtime: store.renderer) }
                    else { Text("暂不支持此渲染版本；定义仍保留。").padding() }
                case .document(let placements, _):
                    ScrollView { LazyVStack(alignment: .leading) {
                        ForEach(placements, id: \.id) { CrewdKnowledgePlacementView(store: store, parent: detail, placement: $0) }
                    }.padding() }
                }
            }
        case .block(_, _, _, let content, _, _): Text(content.string ?? content.pretty).textSelection(.enabled)
        case .relation: Text("关联对象")
        }
    }
}

private struct CrewdGroupKnowledgeView:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var group:CrewdChannelKnowledgeStore
    @ObservedObject var resources:CrewdResources
    @ObservedObject var content:CrewdKnowledgeStore
    let collection:CrewdKnowledgeCollection
    @State private var owner=UUID()
    @State private var selected:String?
    @State private var linking=false
    @State private var creating=false
    @State private var reading=false
    @State private var failure:String?
    @State private var retry=UUID()
    init(workspace:CrewdWorkspace,collection:CrewdKnowledgeCollection){self.workspace=workspace;self.group=workspace.channelKnowledge;self.resources=workspace.channelResources;self.content=workspace.linkedKnowledge;self.collection=collection}
    private var folder:Binding<String>{Binding(get:{workspace.resourceDrafts.selection(resources.crew,.documents,channel:resources.channel)},set:{workspace.resourceDrafts.select(resources.crew,.documents,$0,channel:resources.channel)})}
    private func folderObject(_ source:CrewdChannelKnowledgeReference)->CrewdValue {
        .object(["kind":.string("knowledgeView"),"id":.string(source.reference.id),"knowledge":(try? CrewdKnowledgeStore.encode(source)) ?? .null])
    }
    private var rows:[CrewdChannelKnowledgeItem]{group.items.filter{item in
        let type=collection == .agentPages ? "web":"document"
        return (item.state != "ready" || item.viewKind == type) && CrewdFolderFilter.includes(folderObject(item.source),selection:folder.wrappedValue,snapshot:resources.folders[.documents])
    }}
    private var chosen:CrewdChannelKnowledgeItem?{rows.first{$0.id == selected}}
    private var readKey:String{(chosen?.id ?? "")+"/"+(chosen?.source.context.generation ?? "")+"/"+(chosen?.revision ?? "")+"/"+retry.uuidString}
    var body:some View {
        VStack(spacing:0){
            HStack{Button("引用已有资料",systemImage:"link"){linking=true};if collection == .documents{Button("新建文档",systemImage:"plus"){creating=true}};Spacer();Button("刷新",systemImage:"arrow.clockwise"){Task{try? await group.refresh();retry=UUID()}}}.font(.caption).padding(12)
            CrewdFolderControls(store:resources,drafts:$workspace.resourceDrafts,selection:folder,view:.documents).padding(.horizontal,12)
            if let error=group.failure ?? failure{Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled).padding(8)}
            if group.loading || reading{ProgressView().controlSize(.small)}
            ViewThatFits(in:.horizontal){
                HStack(alignment:.top,spacing:0){list.frame(width:200);Divider();detail.frame(minWidth:280)}
                VStack(spacing:0){list.frame(maxHeight:180);Divider();detail}
            }
        }
        .onAppear{group.setActive(true,owner:owner)}.onDisappear{group.setActive(false,owner:owner);content.renderer.revoke()}
        .task(id:group.crew+group.channel+group.activationEpoch.uuidString){do{try await group.refresh()}catch{if !(error is CancellationError){failure=error.localizedDescription}}}
        .task(id:readKey){guard let chosen,chosen.state == "ready" else{return};reading=true;defer{reading=false};do{try await workspace.openLinkedKnowledge(chosen);failure=nil}catch{if !(error is CancellationError){failure=error.localizedDescription}}}
        .sheet(isPresented:$linking,onDismiss:{retry=UUID()}){CrewdKnowledgeLinkPicker(workspace:workspace,collection:collection)}
        .sheet(isPresented:$creating){CrewdNativeDocumentCreation(workspace:workspace)}
        .onChange(of:group.crew+group.channel){_,_ in selected=nil;linking=false;creating=false;failure=nil}
        .onChange(of:collection){_,_ in selected=nil;content.renderer.revoke()}
        .onChange(of:chosen?.id){_,id in if id == nil{content.renderer.revoke()}}
    }
    private var list:some View {
        ScrollView{LazyVStack(alignment:.leading,spacing:4){
            if group.loaded,rows.isEmpty{Text("本群尚无此类资料引用").font(.caption).foregroundStyle(.secondary).padding(12)}
            ForEach(rows){item in
                Button{selected=item.id}label:{VStack(alignment:.leading,spacing:4){Text(item.title ?? "原知识引用").font(.system(size:13,weight:.medium));Text(item.state == "ready" ? "原机组："+item.source.context.crewId:item.state == "contextChanged" ? "来源上下文已变化":"原内容当前不可用").font(.caption).foregroundStyle(.secondary)}.padding(10).frame(maxWidth:.infinity,alignment:.leading).background(selected == item.id ? Theme.Palette.surfaceMuted:Color.clear,in:RoundedRectangle(cornerRadius:6))}.buttonStyle(.plain)
            }
        }}
    }
    @ViewBuilder private var detail:some View {
        if let chosen {
            VStack(alignment:.leading,spacing:8){
                HStack{Text(chosen.title ?? "知识引用").font(.headline);Spacer();CrewdFolderAssignmentButton(store:resources,drafts:$workspace.resourceDrafts,view:.documents,object:folderObject(chosen.source))}.padding(.horizontal,12)
                Text("共享原文 · "+chosen.source.context.crewId).font(.caption).foregroundStyle(.secondary).padding(.horizontal,12)
                if chosen.state == "ready",let detail=content.detail,detail.object.identity == chosen.source.reference.id,
                   detail.resolvedContext.crewId == chosen.source.context.crewId {
                    CrewdKnowledgeObjectSurface(store:content,detail:detail)
                }else{CrewdEmptySurface(title:"原内容尚不可读",detail:chosen.state == "contextChanged" ? "原知识上下文已变化，引用保留；不会把历史对象当作当前正文。":"请核对当前权限与源机组状态。",symbol:"doc.questionmark")}
                Button("解除本群引用"){unlink(chosen)}.font(.caption).padding(12).disabled(group.writing)
                ForEach(workspace.knowledgeLinkDrafts.unknown.filter{$0.channel == group.channel && $0.source.identity == chosen.id}){snapshot in
                    Button("查询原引用请求结果"){lookup(snapshot)}.font(.caption).padding(.horizontal,12)
                }
            }
        }else{CrewdEmptySurface(title:"选择一份资料",detail:"本群只显示实际引用；机组资料库中的原文可在多个群复用。",symbol:"doc.on.doc")}
    }
    private func unlink(_ item:CrewdChannelKnowledgeItem){
        guard let revision=group.linkRevision,let snapshot=workspace.knowledgeLinkDrafts.begin(crew:group.crew,channel:group.channel,source:item.source,revision:revision,linked:false) else{return}
        Task{do{_ = try await group.submit(snapshot);workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);if selected == item.id{selected=nil}}catch{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));failure=error.localizedDescription}}
    }
    private func lookup(_ snapshot:CrewdChannelKnowledgeDrafts.Snapshot){Task{do{if try await group.lookup(snapshot) != nil{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);failure=nil}else{failure="原请求当前没有已提交回执；引用和输入保留，不会自动重发。"}}catch{failure=error.localizedDescription}}}
}

private struct CrewdKnowledgeLinkPicker:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var library:CrewdKnowledgeStore
    @ObservedObject var group:CrewdChannelKnowledgeStore
    let collection:CrewdKnowledgeCollection
    @Environment(\.dismiss) private var dismiss
    @State private var sourceCrew=""
    @State private var chosen=""
    @State private var targetCrew=""
    @State private var targetChannel=""
    @State private var failure:String?
    init(workspace:CrewdWorkspace,collection:CrewdKnowledgeCollection){self.workspace=workspace;self.library=workspace.linkedKnowledge;self.group=workspace.channelKnowledge;self.collection=collection}
    private var current:Bool{group.crew == targetCrew && group.channel == targetChannel && !workspace.isRetired}
    var body:some View {
        VStack(alignment:.leading,spacing:14){
            Text("引用已有知识资料").font(.headline)
            Picker("源机组",selection:$sourceCrew){ForEach(workspace.crews.compactMap{$0["id"].string},id:\.self){id in Text(workspace.crews.first(where:{$0["id"] == .string(id)})?["title"].string ?? id).tag(id)}}
            if library.busy{ProgressView()}
            ScrollView{LazyVStack(alignment:.leading,spacing:4){ForEach(library.objects.filter(collection.includes),id:\.identity){object in
                Button{chosen=object.identity}label:{Text(object.displayTitle).padding(10).frame(maxWidth:.infinity,alignment:.leading).background(chosen == object.identity ? Theme.Palette.surfaceMuted:Color.clear,in:RoundedRectangle(cornerRadius:6))}.buttonStyle(.plain)
            }}}
            Text("只加入本群引用；原内容与知识块不复制。").font(.caption).foregroundStyle(.secondary)
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange)}
            ForEach(workspace.knowledgeLinkDrafts.unknown.filter{$0.channel == targetChannel}){snapshot in
                Text("原引用结果未确认，不会重发："+snapshot.source.reference.id).font(.caption).foregroundStyle(.orange)
                Button("查询原引用请求"){Task{do{if try await group.lookup(snapshot) != nil{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);dismiss()}else{failure="原请求当前未找到已提交回执，输入保留。"}}catch{failure=error.localizedDescription}}}
            }
            HStack{Button("关闭"){dismiss()};Spacer();Button("引用到本群"){link()}.disabled(!current || chosen.isEmpty || group.writing || library.busy)}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:560,height:540))
        .onAppear{targetCrew=group.crew;targetChannel=group.channel;sourceCrew=group.crew}
        .task(id:sourceCrew){guard !sourceCrew.isEmpty else{return};chosen="";do{try await workspace.readKnowledgeLibrary(crew:sourceCrew);try await group.refresh();failure=nil}catch{failure=error.localizedDescription}}
    }
    private func link(){
        guard current,let context=library.workspace?.context,library.objects.contains(where:{$0.identity == chosen}),let revision=group.linkRevision else{return}
        let source=CrewdChannelKnowledgeReference(context:context,reference:.init(namespace:context.namespace,kind:"view",id:chosen))
        guard let snapshot=workspace.knowledgeLinkDrafts.begin(crew:targetCrew,channel:targetChannel,source:source,revision:revision,linked:true) else{return}
        Task{do{_ = try await group.submit(snapshot);workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);if current{dismiss()}}catch{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));failure=error.localizedDescription}}
    }
}

private struct CrewdNativeDocumentCreation:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:CrewdKnowledgeStore
    @ObservedObject var group:CrewdChannelKnowledgeStore
    @Environment(\.dismiss) private var dismiss
    @State private var crew=""
    @State private var channel=""
    @State private var saved:CrewdChannelKnowledgeReference?
    @State private var submission:UUID?
    @State private var failure:String?
    init(workspace:CrewdWorkspace){self.workspace=workspace;self.store=workspace.knowledge;self.group=workspace.channelKnowledge}
    private var key:CrewdResourceDrafts.Key{.init(crew:crew,view:.documents,object:"",operation:"knowledge.create",domain:.document,channel:channel)}
    private var fields:CrewdResourceDrafts.Fields{workspace.resourceDrafts.fields(key)}
    private var current:Bool{workspace.crewID == crew && workspace.channelID == channel && !workspace.isRetired}
    private func field(_ path:WritableKeyPath<CrewdResourceDrafts.Fields,String>)->Binding<String>{Binding(get:{fields[keyPath:path]},set:{value in var f=fields;f[keyPath:path]=value;workspace.resourceDrafts.edit(key,f)})}
    var body:some View {
        VStack(alignment:.leading,spacing:14){
            Text("新建文档").font(.headline)
            TextField("标题",text:field(\.title)).disabled(saved != nil || store.busy || submission != nil)
            TextEditor(text:field(\.body)).frame(minHeight:180).disabled(saved != nil || store.busy || submission != nil)
            Text(saved == nil ? "先保存SQLite知识原文，再单独加入本群。":"原文已保存；尚未加入本群。加入失败会保留原文，不重新创建。").font(.caption).foregroundStyle(.secondary)
            Text(store.status).font(.caption).foregroundStyle(.secondary)
            if saved != nil{Button("重新核对已保存原文"){Task{await store.refresh();guard current,let original=saved,let context=store.workspace?.context else{return};let refreshed=CrewdChannelKnowledgeReference(context:context,reference:original.reference);guard refreshed.identity == original.identity else{failure="知识来源已换代，原文引用保留，不自动重新绑定。";return};await store.open(original.reference);if store.detail?.object.identity == original.reference.id{saved=refreshed;failure=nil}}}.disabled(store.busy)}
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
            if let submission,store.pending.contains(where:{$0.id == submission}){Button("查询原保存结果"){Task{await store.lookup(submission);adopt(store.result)}}.disabled(store.busy)}
            ForEach(workspace.knowledgeLinkDrafts.unknown.filter{$0.channel == channel && $0.source.identity == saved?.identity}){snapshot in
                Button("查询原加入请求"){Task{do{if try await group.lookup(snapshot) != nil{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);dismiss()}else{failure="原加入请求当前未找到已提交回执；已保存原文保留。"}}catch{failure=error.localizedDescription}}}
            }
            HStack{Button("关闭"){dismiss()};Spacer();if saved == nil{Button("保存原文"){save()}.disabled(!current || store.busy || store.hasBlockingPending || store.workspace?.state != "ready" || fields.title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || submission != nil)}else{Button("加入本群"){join()}.disabled(!current || group.writing || store.busy)}}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:600,height:560))
        .task{crew=workspace.crewID;channel=workspace.channelID;await store.refresh();try? await group.refresh()}
    }
    private func save(){
        let title=fields.title,body=fields.body,old=Set(store.pending.map(\.id))
        let change=CrewdDataChange.apply(message:"创建文档",edits:[
            .createBlock(alias:"body",contentType:"text/plain",content:.string(body),sources:[],attributes:.object([:])),
            .createView(alias:"document",title:title,definition:.document(placements:[.set(id:nil,alias:nil,block:.created(alias:"body"),atRevision:nil,layout:.object([:]))],layout:.object([:])),sources:[],attributes:.object([:]))])
        Task{let result=await store.submitFromNativeHost(change);guard current else{return};submission=result.flatMap{UUID(uuidString:$0.requestId)} ?? store.pending.first(where:{!old.contains($0.id)})?.id;adopt(result)}
    }
    private func adopt(_ result:CrewdDataChangeResult?){
        guard current,let result,result.phase == .committed,submission == UUID(uuidString:result.requestId),case .object(let view)?=result.created["document"],view.reference.kind == "view" else{return}
        saved = .init(context:result.context,reference:view.reference);failure=nil
    }
    private func join(){
        guard current,let saved,let revision=group.linkRevision,
              let snapshot=workspace.knowledgeLinkDrafts.begin(crew:crew,channel:channel,source:saved,revision:revision,linked:true) else{return}
        Task{do{_ = try await group.submit(snapshot);workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);if current{workspace.resourceDrafts.edit(key,.init());dismiss()}}catch{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));failure="原文已保存，未加入本群："+error.localizedDescription}}
    }
}
