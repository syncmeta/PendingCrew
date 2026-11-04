import SwiftUI
#if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
import Inject
#endif

/// Product surface: Agent-authored pages inside the original application layout.
/// Reading/reopening this surface never sends a prompt or resumes an Agent.
struct CrewdKnowledgeDocumentsEntry: View {
    #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
    @ObserveInjection private var injection
    #endif
    @ObservedObject var workspace:CrewdWorkspace
    @State private var library=false
    @State private var collection=CrewdKnowledgeCollection.documents
    var body:some View {
        VStack(spacing:0){
            ViewThatFits(in:.horizontal){
                HStack(spacing:12){scopePicker;collectionPicker}
                VStack(alignment:.leading,spacing:8){scopePicker;collectionPicker}
            }
            .font(CrewdType.label).controlSize(.small)
            .frame(maxWidth:.infinity,alignment:.leading)
            .padding(.horizontal,16).padding(.vertical,10)
            .help(library ? "当前机组可访问的共享资料；群内分类不会改写这里的原文。":"仅显示当前群已关联的资料；文档原文与其他群共享，分类属于当前群。")
            if library {
                CrewdKnowledgeWorkspaceView(store:workspace.knowledge,available:workspace.client.capabilities.contains("data"),returnToMessages:{workspace.originalSection="消息"},collection:collection).id(collection)
            }else{CrewdGroupKnowledgeView(workspace:workspace,collection:collection)}
        }
        .tint(.gray)
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }
    private var scopePicker:some View{Picker("范围",selection:$library){Text("本群引用").tag(false);Text("机组资料库").tag(true)}.pickerStyle(.menu).labelsHidden().fixedSize(horizontal:true,vertical:false).accessibilityLabel("资料范围")}
    private var collectionPicker:some View{Picker("类型",selection:$collection){Text("文档").tag(CrewdKnowledgeCollection.documents);Text("Agent 页面").tag(CrewdKnowledgeCollection.agentPages)}.pickerStyle(.menu).labelsHidden().fixedSize(horizontal:true,vertical:false).accessibilityLabel("资料类型")}

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
    @State private var editTarget: CrewdDataObjectDetail?
    @State private var previewToken = ""
    @State private var confirmingPreview = false
    var body: some View {
        VStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                HStack { pagePicker; Spacer(); if store.busy { ProgressView().controlSize(.small) }; reloadButton; systemButton }
                VStack(alignment: .leading, spacing: 8) {
                    pagePicker
                    HStack { if store.busy { ProgressView().controlSize(.small) }; reloadButton; Spacer(); systemButton }
                }
            }.font(.system(size: 12)).padding(.horizontal, 14).padding(.vertical, 10)
                .background(Theme.Palette.canvas)
            Divider().overlay(CrewdChatStyle.line(scheme))
            if !available {
                CrewdEmptySurface(title: "知识数据尚不可用", detail: "连接具备知识权限的后台后，可读取已保存的页面。", symbol: "externaldrive.badge.questionmark")
            } else if let detail = store.detail, collection.includes(detail.object) {
                page(detail)
            } else {
                VStack(spacing:12) {
                    Text(visibleObjects.isEmpty ? "暂无资料" : "选择资料查看内容").foregroundStyle(.secondary)
                    if visibleObjects.isEmpty, let returnToMessages { Button("回到消息", action:returnToMessages).font(CrewdType.label) }
                }.frame(maxWidth:.infinity,maxHeight:.infinity)
            }
            if !store.status.isEmpty,
               !["知识数据尚未读取。", "已读取当前数据视图；未改动数据。", "只读结果已更新。"].contains(store.status) {
                Text(store.status).font(CrewdType.caption).foregroundStyle(.orange)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16).padding(.vertical, 8)
            }
            if store.hasBlockingPending {
                HStack { Text("有提交结果待核对").font(CrewdType.caption).foregroundStyle(.orange); Spacer(); Button("核对原请求") { showSystem=true } }
                    .padding(.horizontal,16).padding(.vertical,8)
            }
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
                if let current, let editTarget,
                   editTarget.context.authorityId != current.authorityId || editTarget.context.crewId != current.crewId || editTarget.context.namespace != current.namespace {
                    self.editTarget = nil
                }
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
        .sheet(isPresented: Binding(get:{editTarget != nil},set:{if !$0 {editTarget=nil}})) {
            if let editTarget {
                CrewdNativeDocumentEditor(store:store,parent:editTarget,scopeCurrent:{ true },onSaved:{ await reload() })
            }
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
        Button { Task { await reload() } } label: { Image(systemName: "arrow.clockwise") }
            .buttonStyle(CrewdToolButtonStyle()).help("刷新资料").accessibilityLabel("刷新资料")
            .disabled(store.busy)
    }
    private var systemButton: some View {
        Button { showSystem = true } label: { Image(systemName: "ellipsis") }
            .buttonStyle(CrewdToolButtonStyle()).accessibilityLabel("页面管理")
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
    private func page(_ detail:CrewdDataObjectDetail)->some View {
        VStack(alignment:.leading,spacing:0) {
            HStack {
                Text(detail.object.displayTitle).font(CrewdType.title)
                Spacer()
                if collection == .documents, store.canPrepareNativeDocumentEdit(detail) {
                    Button("编辑文档",systemImage:"pencil") { editTarget = detail }.disabled(store.busy || store.hasBlockingPending)
                }
            }.padding(.horizontal,24).padding(.top,20).padding(.bottom,8)
            CrewdKnowledgeObjectSurface(store:store,detail:detail)
        }.frame(maxWidth:.infinity,maxHeight:.infinity,alignment:.topLeading)
    }
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
                Text(content.string ?? content.pretty).font(CrewdType.body).lineSpacing(6).textSelection(.enabled)
            } else {Text(loaded ? "不可用":"尚未读取").foregroundStyle(.secondary)}
        }.frame(maxWidth:.infinity,alignment:.leading).padding(.vertical,8)
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
                    ScrollView { LazyVStack(alignment: .leading,spacing:12) {
                        ForEach(placements, id: \.id) { CrewdKnowledgePlacementView(store: store, parent: detail, placement: $0) }
                    }.padding(24).frame(maxWidth:760,alignment:.leading).frame(maxWidth:.infinity,alignment:.center) }
                }
            }
        case .block(_, _, _, let content, _, _): Text(content.string ?? content.pretty).textSelection(.enabled)
        case .relation: Text("关联对象")
        }
    }
}

private struct CrewdGroupKnowledgeView:View {
    #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
    @ObserveInjection private var injection
    #endif
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var group:CrewdChannelKnowledgeStore
    @ObservedObject var resources:CrewdResources
    @ObservedObject var content:CrewdKnowledgeStore
    @ObservedObject var library:CrewdKnowledgeStore
    let collection:CrewdKnowledgeCollection
    @State private var owner=UUID()
    @State private var selected:String?
    @State private var linking=false
    @State private var creating=false
    @State private var reading=false
    @State private var failure:String?
    @State private var retry=UUID()
    private struct EditTarget: Identifiable {
        let id = UUID()
        let parent: CrewdDataObjectDetail
        let channel: String
        let link: String
    }
    @State private var editTarget: EditTarget?
    init(workspace:CrewdWorkspace,collection:CrewdKnowledgeCollection){self.workspace=workspace;self.group=workspace.channelKnowledge;self.resources=workspace.channelResources;self.content=workspace.linkedKnowledge;self.library=workspace.knowledge;self.collection=collection}
    private var folder:Binding<String>{Binding(get:{workspace.resourceDrafts.selection(resources.crew,.documents,channel:resources.channel)},set:{workspace.resourceDrafts.select(resources.crew,.documents,$0,channel:resources.channel)})}
    private func folderObject(_ source:CrewdChannelKnowledgeReference)->CrewdValue {
        .object(["kind":.string("knowledgeView"),"id":.string(source.reference.id),"knowledge":(try? CrewdKnowledgeStore.encode(source)) ?? .null])
    }
    private var rows:[CrewdChannelKnowledgeItem]{group.items.filter{item in
        let type=collection == .agentPages ? "web":"document"
        return (item.state != "ready" || item.viewKind == type) && CrewdFolderFilter.includes(folderObject(item.source),selection:folder.wrappedValue,snapshot:resources.folders[.documents])
    }}
    private var chosen:CrewdChannelKnowledgeItem?{rows.first{$0.id == selected}}
    private var pendingLinks: [CrewdPendingKnowledgeLink] {
        CrewdPendingKnowledgeLink.inChannel(workspace.durableUnknown,scope:workspace.presentationScope,crew:group.crew,channel:group.channel)
    }
    private var readKey:String{(chosen?.id ?? "")+"/"+(chosen?.source.context.generation ?? "")+"/"+(chosen?.revision ?? "")+"/"+retry.uuidString}
    var body:some View {
        VStack(spacing:0){
            ViewThatFits(in:.horizontal){
                HStack(spacing:8){documentActions;documentFolders.fixedSize(horizontal:true,vertical:false)}
                VStack(alignment:.leading,spacing:6){documentActions;documentFolders}
            }.font(CrewdType.label).frame(maxWidth:.infinity,alignment:.leading)
                .padding(.horizontal,16).padding(.bottom,10)
            CrewdKnowledgeLinkRecovery(workspace:workspace,group:group,crew:group.crew,channel:group.channel)
                .padding(.horizontal,16)
            if collection == .documents {
                ForEach(library.pendingNativeDocumentEdits(crew:group.crew)) { intent in
                    HStack {
                        Text("原编辑结果待核对").foregroundStyle(.orange)
                        Button("查询原编辑结果") {
                            Task {
                                await library.refresh()
                                await library.lookup(intent.id)
                                content.synchronizePending()
                                if library.result.flatMap({ UUID(uuidString:$0.requestId) }) == intent.id,
                                   library.result?.phase == .committed { await refreshAfterEdit() }
                                else { failure = library.status }
                            }
                        }.disabled(library.busy)
                    }.font(CrewdType.caption).padding(.horizontal,16)
                }
            }
            if let error=group.failure ?? failure{Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled).padding(8)}
            if group.loading || reading{ProgressView().controlSize(.small)}
            if group.loaded,!group.loading,!reading,group.failure == nil,failure == nil,rows.isEmpty {
                Text("暂无资料").font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth:.infinity,maxHeight:.infinity)
            }else{
                ViewThatFits(in:.horizontal){
                    HStack(alignment:.top,spacing:0){list.frame(width:230);Divider();detail.frame(minWidth:280,maxHeight:.infinity,alignment:.topLeading)}
                    VStack(spacing:0){list.frame(height:min(180,CGFloat(max(1,rows.count))*48+16));Divider();detail}
                }
            }
        }
        .onAppear{group.setActive(true,owner:owner)}.onDisappear{group.setActive(false,owner:owner);content.renderer.revoke()}
        .task(id:group.crew+group.channel+group.activationEpoch.uuidString){do{try await group.refresh()}catch{if !(error is CancellationError){failure=error.localizedDescription}}}
        .task(id:group.crew+collection.rawValue+group.activationEpoch.uuidString){if collection == .documents {library.synchronizePending()}}
        .task(id:readKey){guard let chosen,chosen.state == "ready" else{return};reading=true;defer{reading=false};do{try await workspace.openLinkedKnowledge(chosen);failure=nil}catch{if !(error is CancellationError){failure=error.localizedDescription}}}
        .sheet(isPresented:$linking,onDismiss:{retry=UUID()}){CrewdKnowledgeLinkPicker(workspace:workspace,collection:collection)}
        .sheet(isPresented:$creating){CrewdNativeDocumentCreation(workspace:workspace)}
        .sheet(item:$editTarget){ target in
            CrewdNativeDocumentEditor(store:content,parent:target.parent,scopeCurrent:{
                group.crew == target.parent.context.crewId && group.channel == target.channel &&
                    chosen?.id == target.link && chosen?.state == "ready"
            },onSaved:{await refreshAfterEdit()})
        }
        .onChange(of:content.pending){_,_ in if collection == .documents {library.synchronizePending()}}
        .onChange(of:library.pending){_,pending in if collection == .documents && pending.isEmpty {library.synchronizePending()}}
        .onChange(of:group.crew+group.channel){_,_ in selected=nil;linking=false;creating=false;editTarget=nil;failure=nil}
        .onChange(of:collection){_,_ in selected=nil;editTarget=nil;content.renderer.revoke()}
        .onChange(of:chosen?.id){_,id in if id == nil{content.renderer.revoke()}}
        .controlSize(.small).tint(.gray)
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }
    private var documentActions:some View {
        HStack(spacing:8){
            Button("引用已有资料",systemImage:"link"){linking=true}
            if collection == .documents{Button("新建文档",systemImage:"plus"){creating=true}}
            Button {Task{try? await group.refresh();retry=UUID()}} label: {Image(systemName:"arrow.clockwise")}
                .buttonStyle(CrewdToolButtonStyle()).help("刷新").accessibilityLabel("刷新")
        }.fixedSize(horizontal:true,vertical:false)
    }
    private var documentFolders:some View {
        CrewdFolderControls(store:resources,drafts:$workspace.resourceDrafts,selection:folder,view:.documents,compact:true)
            .help("本群资料分类，只改变展示位置，不改变共享原文")
    }
    private var list:some View {
        ScrollView {
            LazyVStack(alignment:.leading,spacing:4) {
                ForEach(rows) { item in
                    Button { selected=item.id } label: {
                        HStack(alignment:.top,spacing:8) {
                            Image(systemName:collection == .agentPages ? "rectangle.on.rectangle":"doc.text").foregroundStyle(.secondary).frame(width:16)
                            VStack(alignment:.leading,spacing:4) {
                                Text(item.title ?? "未命名资料").font(CrewdType.body.weight(selected == item.id ? .semibold:.regular)).multilineTextAlignment(.leading)
                                if item.state != "ready" {
                                    Text(item.state == "contextChanged" ? "来源已变化":"内容不可用").font(CrewdType.caption).foregroundStyle(.secondary)
                                }
                            }
                            Spacer(minLength:0)
                        }.padding(10).frame(maxWidth:.infinity,alignment:.leading)
                            .background(selected == item.id ? Theme.Palette.accentBg:Color.clear,in:RoundedRectangle(cornerRadius:6))
                            .contentShape(Rectangle())
                    }.buttonStyle(.plain).accessibilityAddTraits(selected == item.id ? .isSelected:[])
                }
            }.padding(8)
        }
    }
    @ViewBuilder private var detail:some View {
        if let chosen {
            VStack(alignment:.leading,spacing:8){
                HStack{Text(chosen.title ?? "未命名资料").font(CrewdType.title);Spacer();CrewdFolderAssignmentButton(store:resources,drafts:$workspace.resourceDrafts,view:.documents,object:folderObject(chosen.source))}.padding(.horizontal,24).padding(.top,16)
                DisclosureGroup("来源信息"){Text("原机组："+chosen.source.context.crewId).textSelection(.enabled);Text(chosen.source.reference.id).textSelection(.enabled)}.font(CrewdType.caption).foregroundStyle(.secondary).padding(.horizontal,24)
                if chosen.state == "ready",let detail=content.detail,detail.object.identity == chosen.source.reference.id,
                   detail.resolvedContext.crewId == chosen.source.context.crewId {
                    if collection == .documents, group.crew == chosen.source.context.crewId, content.canPrepareNativeDocumentEdit(detail) {
                        Button("编辑原文",systemImage:"pencil") { editTarget = .init(parent:detail,channel:group.channel,link:chosen.id) }
                            .disabled(content.busy || content.hasBlockingPending).padding(.horizontal,24)
                    }
                    CrewdKnowledgeObjectSurface(store:content,detail:detail)
                }else{CrewdEmptySurface(title:"原内容尚不可读",detail:chosen.state == "contextChanged" ? "原知识上下文已变化，引用保留；不会把历史对象当作当前正文。":"请核对当前权限与源机组状态。",symbol:"doc.questionmark")}
                Button("解除本群引用"){unlink(chosen)}.font(.caption).foregroundStyle(.secondary).padding(.horizontal,14).padding(.vertical,8)
                    .disabled(group.writing || pendingLinks.contains { $0.source.identity == chosen.id })
            }
        }else{Text("选择资料查看内容").font(.callout).foregroundStyle(.secondary).frame(maxWidth:.infinity,maxHeight:.infinity)}
    }
    private func refreshAfterEdit() async {
        do { try await group.refresh(); retry = UUID() } catch { failure = error.localizedDescription }
    }
    private func unlink(_ item:CrewdChannelKnowledgeItem){
        guard !pendingLinks.contains(where: { $0.source.identity == item.id }),
              let revision=group.linkRevision,let snapshot=workspace.knowledgeLinkDrafts.begin(crew:group.crew,channel:group.channel,source:item.source,revision:revision,linked:false) else{return}
        Task{do{_ = try await group.submit(snapshot);workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);if selected == item.id{selected=nil}}catch{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));failure=error.localizedDescription}}
    }
}

private struct CrewdNativeDocumentEditor: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: CrewdKnowledgeStore
    let parent: CrewdDataObjectDetail
    let scopeCurrent: @MainActor () -> Bool
    let onSaved: @MainActor () async -> Void
    @State private var edit: CrewdKnowledgeStore.NativeDocumentEdit?
    @State private var title = ""
    @State private var bodyText = ""
    @State private var failure: String?
    @State private var working = true
    @State private var needsReview = false
    @State private var refreshedBaseline = false

    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            HStack { Text("编辑文档").font(CrewdType.title); Spacer(); Button("关闭") { dismiss() }.disabled(working) }
            if let edit {
                TextField("标题",text:$title).disabled(working || store.hasBlockingPending)
                TextEditor(text:$bodyText).frame(minHeight:160).accessibilityLabel("文档正文")
                    .disabled(working || store.hasBlockingPending)
                if refreshedBaseline {
                    DisclosureGroup("已重新读取后台版本，保存前可核对") {
                        Text(edit.title).font(CrewdType.label).textSelection(.enabled)
                        ScrollView { Text(edit.body).frame(maxWidth:.infinity,alignment:.leading).textSelection(.enabled) }.frame(maxHeight:120)
                    }
                }
                ForEach(store.pendingNativeDocumentEdits(viewID:parent.object.identity)) { intent in
                    HStack {
                        Text("原编辑待核对").font(CrewdType.caption)
                        Button("查询原编辑结果") { lookup(intent.id) }.disabled(working || store.busy)
                    }
                }
                HStack {
                    if needsReview && !store.hasBlockingPending {
                        Button("读取当前版本以核对") { reread(edit) }.disabled(working || store.busy)
                    }
                    Spacer()
                    Button(refreshedBaseline ? "按当前版本保存草稿" : "保存更改") { save(edit) }
                        .buttonStyle(.borderedProminent)
                        .disabled(working || store.busy || needsReview || store.hasBlockingPending ||
                                  title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || !edit.hasChanges(title:title,body:bodyText))
                }
            }
            if let failure { Text(failure).font(CrewdType.caption).foregroundStyle(.orange).textSelection(.enabled) }
            if working { ProgressView().controlSize(.small) }
        }.padding(20)
        #if os(macOS)
        .frame(minWidth:520)
        #endif
        .interactiveDismissDisabled(working)
        .task {
            defer { working = false }
            do {
                guard scopeCurrent() else { throw CrewdKnowledgeStore.Failure.stale }
                let original = try await store.prepareNativeDocumentEdit(parent)
                guard scopeCurrent() else { throw CrewdKnowledgeStore.Failure.stale }
                edit = original; title = original.title; bodyText = original.body
            } catch { failure = "无法确认可编辑的原文。此入口支持机组内当前的人类单段纯文本文档。" }
        }
    }

    private func save(_ original: CrewdKnowledgeStore.NativeDocumentEdit) {
        working = true
        Task {
            defer { working = false }
            do {
                guard scopeCurrent() else { throw CrewdKnowledgeStore.Failure.stale }
                let result = try await store.submitNativeDocumentEdit(original,title:title,body:bodyText)
                if result?.phase == .committed { if scopeCurrent() { await onSaved() }; dismiss(); return }
                if result == nil { await store.refresh() }
                failure = store.hasBlockingPending ? "编辑结果待核对。请查询原请求，草稿保留。" : "保存尚未确认，请读取当前版本核对。"
            } catch let error as CrewdClientError { failure = error.localizedDescription }
            catch { failure = "原文或连接已变化，或输入超出限制。当前草稿保留。" }
            needsReview = true
        }
    }

    private func lookup(_ id: UUID) {
        working = true
        Task {
            defer { working = false }
            guard scopeCurrent() else { return }
            await store.lookup(id)
            if store.result.flatMap({ UUID(uuidString:$0.requestId) }) == id, store.result?.phase == .committed, scopeCurrent() {
                await onSaved(); dismiss()
            } else { failure = store.status }
        }
    }

    private func reread(_ original: CrewdKnowledgeStore.NativeDocumentEdit) {
        working = true
        Task {
            defer { working = false }
            do {
                guard scopeCurrent() else { throw CrewdKnowledgeStore.Failure.stale }
                let current = try await store.reloadNativeDocumentEdit(original)
                guard scopeCurrent() else { throw CrewdKnowledgeStore.Failure.stale }
                let draft = current.rebasedDraft(from:original,title:title,body:bodyText)
                title = draft.title; bodyText = draft.body
                edit = current; needsReview = false; refreshedBaseline = true; failure = nil
            } catch { failure = "未能核对当前原文，草稿保留。" }
        }
    }
}

/// Recovery is available even if an unlink already removed the selected row.
private struct CrewdKnowledgeLinkRecovery: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var group: CrewdChannelKnowledgeStore
    let crew: String
    let channel: String
    var originalSource: CrewdChannelKnowledgeReference? = nil
    var onCommitted: (CrewdPendingKnowledgeLink) -> Void = { _ in }
    @State private var checking: UUID?
    @State private var message: String?

    private var pending: [CrewdPendingKnowledgeLink] {
        CrewdPendingKnowledgeLink.inChannel(workspace.durableUnknown,scope:workspace.presentationScope,crew:crew,channel:channel)
            .filter { attempt in
                guard let originalSource else { return true }
                return attempt.source.identity == originalSource.identity
            }
    }
    var body: some View {
        VStack(alignment:.leading,spacing:6) {
            if !pending.isEmpty {
                DisclosureGroup("有 \(pending.count) 条原引用结果待核对") {
                    ScrollView {
                        VStack(alignment:.leading,spacing:8) {
                            ForEach(pending) { attempt in
                                HStack {
                                    Text(verbatim:(attempt.linked ? "加入 " : "解除 ") + attempt.source.reference.id)
                                        .textSelection(.enabled)
                                    Spacer()
                                    Button(checking == attempt.id ? "正在核对…" : "查询原请求") { lookup(attempt) }
                                        .disabled(checking != nil || group.writing)
                                }
                            }
                        }
                    }.frame(maxHeight:130)
                }
            }
            if let message { Text(message).foregroundStyle(.secondary).textSelection(.enabled) }
        }.font(.caption)
        .onChange(of:group.readScope) { _,_ in checking=nil;message=nil }
        .onDisappear { checking=nil }
    }
    private func lookup(_ attempt: CrewdPendingKnowledgeLink) {
        guard checking == nil, group.crew == crew, group.channel == channel else { return }
        checking=attempt.id;message=nil
        let readScope=group.readScope, scope=workspace.presentationScope
        Task { @MainActor in
            @MainActor func current() -> Bool {
                group.readScope == readScope && workspace.presentationScope == scope &&
                    !workspace.isRetired && checking == attempt.id
            }
            defer { if current() { checking=nil } }
            do {
                let value=try await group.lookup(attempt.snapshot)
                guard current() else { return }
                guard let value else {
                    message="原请求尚无已提交回执；保留原输入和编号。"
                    return
                }
                try workspace.acknowledgeOriginalResource(attempt.record,receipt:value)
                message=attempt.linked ? "已核实原加入请求。" : "已核实原解除请求。"
                onCommitted(attempt)
                try? await group.refresh()
            } catch {
                if current(), !(error is CancellationError) { message=error.localizedDescription }
            }
        }
    }
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
    private var chosenSource:CrewdChannelKnowledgeReference? {
        guard let context=library.workspace?.context,context.crewId == sourceCrew,
              library.objects.contains(where: { $0.identity == chosen }) else { return nil }
        return .init(context:context,reference:.init(namespace:context.namespace,kind:"view",id:chosen))
    }
    private var linkPending:Bool {
        guard let chosenSource else { return false }
        return CrewdPendingKnowledgeLink.inChannel(workspace.durableUnknown,scope:workspace.presentationScope,crew:targetCrew,channel:targetChannel)
            .contains { $0.source.identity == chosenSource.identity }
    }
    var body:some View {
        VStack(alignment:.leading,spacing:0){
            CrewdPanelHeader(title:"引用已有资料").padding(20)
            Divider()
            ScrollView{VStack(alignment:.leading,spacing:16){
            Picker("源机组",selection:$sourceCrew){ForEach(workspace.crews.compactMap{$0["id"].string},id:\.self){id in Text(workspace.crews.first(where:{$0["id"] == .string(id)})?["title"].string ?? id).tag(id)}}
            if library.busy{ProgressView("正在读取资料")}
            if !library.busy,library.objects.filter(collection.includes).isEmpty{CrewdInlineNotice(title:"这个机组尚无可引用的资料",detail:"请选择其他源机组，或先在原机组保存内容。",symbol:"doc.text.magnifyingglass")}
            LazyVStack(alignment:.leading,spacing:4){ForEach(library.objects.filter(collection.includes),id:\.identity){object in
                Button{chosen=object.identity}label:{Text(object.displayTitle).padding(10).frame(maxWidth:.infinity,alignment:.leading).background(chosen == object.identity ? Theme.Palette.surfaceMuted:Color.clear,in:RoundedRectangle(cornerRadius:6))}.buttonStyle(.plain)
            }}
            if let failure{CrewdInlineNotice(title:"引用尚未完成",detail:failure,warning:true)}
            CrewdKnowledgeLinkRecovery(workspace:workspace,group:group,crew:targetCrew,channel:targetChannel)
            }.padding(20)}
            Divider()
            HStack{Button("关闭"){dismiss()};Spacer();Button(group.writing ? "正在加入…":"引用到本群"){link()}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(!current || chosenSource == nil || group.writing || library.busy || linkPending)}.padding(16)
        }.modifier(CrewdAdaptiveFormFrame(width:560,height:580,scrolls:false))
        .onAppear{targetCrew=group.crew;targetChannel=group.channel;sourceCrew=group.crew}
        .task(id:sourceCrew){guard !sourceCrew.isEmpty else{return};chosen="";do{try await workspace.readKnowledgeLibrary(crew:sourceCrew);try await group.refresh();failure=nil}catch{failure=error.localizedDescription}}
    }
    private func link(){
        guard current,!linkPending,let source=chosenSource,let revision=group.linkRevision else{return}
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
    @State private var stage:CrewdPresentationJournal.DocumentStage?
    @State private var failure:String?
    @State private var refused=false
    init(workspace:CrewdWorkspace){self.workspace=workspace;self.store=workspace.knowledge;self.group=workspace.channelKnowledge}
    private var key:CrewdResourceDrafts.Key{.init(crew:crew,view:.documents,object:"",operation:"knowledge.create",domain:.document,channel:channel)}
    private var fields:CrewdResourceDrafts.Fields{workspace.resourceDrafts.fields(key)}
    private var current:Bool{workspace.crewID == crew && workspace.channelID == channel && !workspace.isRetired}
    private var linkPending:Bool {
        guard let saved else { return false }
        return CrewdPendingKnowledgeLink.inChannel(workspace.durableUnknown,scope:workspace.presentationScope,crew:crew,channel:channel)
            .contains { $0.source.identity == saved.identity }
    }
    private var alreadyLinked:Bool {
        guard let saved, group.loaded, group.crew == crew, group.channel == channel else { return false }
        return group.items.contains { $0.source.identity == saved.identity && $0.state == "ready" }
    }
    private func field(_ path:WritableKeyPath<CrewdResourceDrafts.Fields,String>)->Binding<String>{Binding(get:{if let stage{return path == \CrewdResourceDrafts.Fields.title ? stage.title:stage.body};return fields[keyPath:path]},set:{value in var f=fields;f[keyPath:path]=value;workspace.resourceDrafts.edit(key,f)})}
    var body:some View {
        VStack(alignment:.leading,spacing:0){
            CrewdPanelHeader(title:"新建文档").padding(20)
            Divider()
            ScrollView{VStack(alignment:.leading,spacing:16){
            VStack(alignment:.leading,spacing:16){
                CrewdFormField(title:"标题"){TextField("给文档起一个名字",text:field(\.title)).disabled(stage != nil || store.busy || submission != nil)}
                CrewdFormField(title:"正文"){
                    TextEditor(text:field(\.body)).font(CrewdType.body).frame(minHeight:200).padding(8)
                        .background(Theme.Palette.canvas,in:RoundedRectangle(cornerRadius:8))
                        .overlay(RoundedRectangle(cornerRadius:8).strokeBorder(Theme.Palette.hairline,lineWidth:0.7))
                        .disabled(stage != nil || store.busy || submission != nil)
                }
            }
            if saved != nil {
                Text(alreadyLinked ? "已核对当前群引用，可结束本次新建。" : "原文已保存，加入本群后即可在此阅读。")
                    .font(CrewdType.label).foregroundStyle(.secondary)
            }
            if store.hasBlockingPending || (stage != nil && saved == nil) { Text(store.status).font(CrewdType.caption).foregroundStyle(.orange).textSelection(.enabled) }
            if saved != nil{Button("重新核对已保存原文"){Task{await store.refresh();guard current,let original=saved,let context=store.workspace?.context else{return};let refreshed=CrewdChannelKnowledgeReference(context:context,reference:original.reference);guard refreshed.identity == original.identity else{failure="知识来源已换代，原文引用保留，不自动重新绑定。";return};await store.open(original.reference);if store.detail?.object.identity == original.reference.id{saved=refreshed;failure=nil}}}.disabled(store.busy)}
            if let failure{CrewdInlineNotice(title:"需要核对保存状态",detail:failure,warning:true)}
            if let stage,saved == nil{Button("查询原保存结果"){Task{_ = await store.lookupNativeDocument(stage);restoreStage()}}.disabled(store.busy)}
            if let saved {
                CrewdKnowledgeLinkRecovery(workspace:workspace,group:group,crew:crew,channel:channel,originalSource:saved) { attempt in
                    if current, attempt.linked, attempt.source.identity == self.saved?.identity { finishDraft() }
                }
            }
            if refused{Button("原保存已明确拒绝，修改后另存"){reviseRefusedDraft()}}
            if saved != nil,!alreadyLinked{Button("仅保留已保存原文，结束本次新建"){finishDraft()}.disabled(group.writing || linkPending)}
            }.padding(20)}
            Divider()
            HStack {
                Button("关闭"){dismiss()};Spacer()
                if saved == nil {
                    Button(store.busy ? "正在保存…":"保存原文"){save()}.buttonStyle(CrewdPrimaryButtonStyle())
                        .disabled(!current || store.busy || store.hasBlockingPending || !store.canSaveNativeDocument || fields.title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || submission != nil)
                } else {
                    Button(alreadyLinked ? "完成" : group.writing ? "正在加入…" : "加入本群") {
                        if alreadyLinked { finishDraft() } else { join() }
                    }.buttonStyle(CrewdPrimaryButtonStyle())
                        .disabled(!current || group.writing || group.loading || !group.loaded || store.busy || linkPending)
                }
            }.padding(16)
        }.modifier(CrewdAdaptiveFormFrame(width:600,height:680,scrolls:false))
        .task{crew=workspace.crewID;channel=workspace.channelID;await store.refresh();restoreStage();try? await group.refresh()}
    }
    private func restoreStage(){
        guard current else{return}
        do {
            stage=try store.nativeDocumentStage(channel:channel)
            submission=stage?.id
            if let stage,let result=try store.nativeDocumentResult(stage) {
                refused=[.rejected,.conflicted].contains(result.phase)
                if result.phase == .committed,case .object(let view)?=result.created["document"]{saved = .init(context:result.context,reference:view.reference);failure=nil}
                else if refused{failure="原保存已明确拒绝："+(result.reason ?? "数据或权限已变化；原文保留，未自动重发。")}
            }
        }catch{failure="原文档保存阶段尚未可靠读取，不能再创建正文："+error.localizedDescription;submission=UUID()}
    }
    private func save(){
        let title=fields.title,body=fields.body,origin=workspace.presentationScope
        Task{
            _ = await store.submitNativeDocument(title:title,body:body,channel:channel)
            guard current,workspace.presentationScope == origin else{return}
            // data.changed can invalidate the read model before the original
            // commit reply. Restore that model, then read the same durable stage.
            await store.refresh()
            guard current,workspace.presentationScope == origin else{return}
            restoreStage()
        }
    }
    private func reviseRefusedDraft(){
        guard current,refused,let stage else{return}
        do{
            try store.finishNativeDocument(stage)
            if !workspace.resourceDrafts.exists(key){var original=CrewdResourceDrafts.Fields();original.title=stage.title;original.body=stage.body;workspace.resourceDrafts.edit(key,original)}
            self.stage=nil;submission=nil;saved=nil;refused=false;failure=nil
            Task{await store.refresh()}
        }catch{failure=error.localizedDescription}
    }
    private func finishDraft(){
        guard current,let stage else{return}
        do {
            try store.finishNativeDocument(stage)
            if fields.title == stage.title,fields.body == stage.body{workspace.resourceDrafts.edit(key,.init())}
            dismiss()
        }catch{failure=error.localizedDescription}
    }
    private func join(){
        guard current,!alreadyLinked,!linkPending,group.loaded,!group.loading,let saved,let revision=group.linkRevision,
              let snapshot=workspace.knowledgeLinkDrafts.begin(crew:crew,channel:channel,source:saved,revision:revision,linked:true) else{return}
        Task{do{_ = try await group.submit(snapshot);workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);if current{finishDraft()}}catch{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));failure="原文已保存，未加入本群："+error.localizedDescription}}
    }
}
