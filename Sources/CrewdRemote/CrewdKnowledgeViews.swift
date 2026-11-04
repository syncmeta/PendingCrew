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
                CrewdKnowledgeWorkspaceView(store:workspace.knowledge,available:workspace.client.capabilities.contains("data"),returnToMessages:{workspace.originalSection="消息"},collection:collection,
                    workStore:workspace.client.capabilities.contains("data.documentWorkLinks") ? workspace.workStore : nil)
                    .id(workspace.crewID + "/" + collection.rawValue)
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
    func includes(_ kind: CrewdKnowledgeStore.ViewCatalogKind) -> Bool {
        self == .all || (self == .documents ? kind == .document : kind == .web)
    }
    func includes(_ object: CrewdDataObject) -> Bool {
        guard case let .view(_, _, _, definition, _, _) = object else { return false }
        switch definition {
        case .document: return includes(.document)
        case .web: return includes(.web)
        }
    }
}

struct CrewdKnowledgeWorkspaceView: View {
    @ObservedObject var store: CrewdKnowledgeStore
    let available: Bool
    var returnToMessages: (() -> Void)? = nil
    var collection: CrewdKnowledgeCollection = .all
    var workStore: CrewdWorkStore? = nil
    private var catalog: CrewdKnowledgeStore.ViewCatalog? {
        guard let catalog = store.viewCatalog, catalog.includeArchived == showArchived,
              store.workspace?.context == catalog.context else { return nil }
        return catalog
    }
    private var visibleObjects: [CrewdKnowledgeStore.ViewCatalogItem] {
        catalog?.items.filter { $0.archived == showArchived && collection.includes($0.kind) && CrewdKnowledgeTitleSearch.matches($0.title,query:query) } ?? []
    }
    private var visibleDetail: CrewdDataObjectDetail? {
        guard let catalog, let detail = store.detail, detail.context == catalog.context,
              selected == detail.object.identity, collection.includes(detail.object),
              detail.object.isArchived == showArchived else { return nil }
        return detail
    }
    @Environment(\.colorScheme) private var scheme
    @State private var selected = ""
    @State private var query = ""
    @State private var showSystem = false
    @State private var showArchived = false
    @State private var confirmArchive: CrewdKnowledgeStore.NativeViewArchiveTarget?
    @State private var lifecycleFeedback: String?
    @State private var editTarget: CrewdDataObjectDetail?
    @State private var historyTarget: CrewdDataObjectDetail?
    @State private var compositionTarget: CrewdDataObjectDetail?
    @State private var documentWorkTarget: CrewdDataObjectDetail?
    @State private var previewToken = ""
    @State private var confirmingPreview = false
    var body: some View {
        VStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                HStack { titleSearch; pagePicker; Spacer(); if store.busy { ProgressView().controlSize(.small) }; recycleBinButton; reloadButton; systemButton }
                VStack(alignment: .leading, spacing: 8) {
                    titleSearch
                    HStack { pagePicker; Spacer(); if store.busy { ProgressView().controlSize(.small) }; recycleBinButton; reloadButton; systemButton }
                }
            }.font(.system(size: 12)).padding(.horizontal, 14).padding(.vertical, 10)
                .background(Theme.Palette.canvas)
            if available { catalogProgress }
            if let lifecycleFeedback {
                Text(lifecycleFeedback).font(CrewdType.caption).foregroundStyle(.secondary)
                    .frame(maxWidth:.infinity,alignment:.leading).padding(.horizontal,16).padding(.bottom,8)
            }
            Divider().overlay(CrewdChatStyle.line(scheme))
            if !available {
                CrewdEmptySurface(title: "知识数据尚不可用", detail: "连接具备知识权限的后台后，可读取已保存的页面。", symbol: "externaldrive.badge.questionmark")
            } else if let detail = visibleDetail {
                page(detail)
            } else {
                VStack(spacing:12) {
                    Text(catalogEmptyText).foregroundStyle(.secondary)
                    if catalog != nil, catalog?.next == nil, visibleObjects.isEmpty, let returnToMessages { Button("回到消息", action:returnToMessages).font(CrewdType.label) }
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
        .onChange(of: store.workspace?.context) { old, current in
            if old?.authorityId != current?.authorityId || old?.crewId != current?.crewId || old?.namespace != current?.namespace {
                selected = ""
                if let current, let editTarget,
                   editTarget.context.authorityId != current.authorityId || editTarget.context.crewId != current.crewId || editTarget.context.namespace != current.namespace {
                    self.editTarget = nil
                }
                if let current, let historyTarget,
                   historyTarget.context.authorityId != current.authorityId || historyTarget.context.crewId != current.crewId || historyTarget.context.namespace != current.namespace {
                    self.historyTarget = nil
                }
            }
            if let current, let target = documentWorkTarget,
               target.context.authorityId != current.authorityId || target.context.dbId != current.dbId ||
                target.context.crewId != current.crewId || target.context.namespace != current.namespace ||
                target.context.restoreEpoch != current.restoreEpoch {
                documentWorkTarget = nil
            }
            // Native confirmations bind the state the user actually reviewed.
            confirmArchive = nil; restoreTarget = nil; confirmingPreview = false
            // A new generation/authority invalidates code grants. The page is re-read only.
            if store.detail == nil, !store.busy, available, documentWorkTarget == nil {
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
        .sheet(isPresented: Binding(get:{historyTarget != nil},set:{if !$0 {historyTarget=nil}})) {
            if let historyTarget {
                CrewdNativeDocumentHistory(store:store,parent:historyTarget,scopeCurrent:{ true },onRestored:{ await reload() })
            }
        }
        .sheet(isPresented: Binding(get:{compositionTarget != nil},set:{if !$0 {compositionTarget=nil}})) {
            if let compositionTarget {
                CrewdDocumentCompositionEditor(store:store,parent:compositionTarget,
                    scopeCurrent:{ self.compositionTarget == compositionTarget && collection == .documents && !showArchived },
                    onChanged:{ await reload() })
            }
        }
        .sheet(isPresented: Binding(get:{documentWorkTarget != nil},set:{if !$0 {documentWorkTarget=nil}}),onDismiss:{ Task { await reload() } }) {
            if let documentWorkTarget, let workStore {
                CrewdDocumentWorkLinksView(store:store,workStore:workStore,parent:documentWorkTarget,
                    scopeCurrent:{ self.documentWorkTarget == documentWorkTarget && collection == .documents })
            }
        }
        .confirmationDialog(confirmArchive?.archived == true ? "恢复这份资料？" : "把这份资料移入回收站？",
                            isPresented:Binding(get:{confirmArchive != nil},set:{if !$0 {confirmArchive=nil}})) {
            if let target = confirmArchive {
                Button(target.archived ? "恢复资料" : "移入回收站",role:target.archived ? nil:.destructive) {
                    confirmArchive=nil; changeArchive(target)
                }
            }
            Button("取消",role:.cancel) {}
        } message: {
            if let target = confirmArchive {
                Text(target.title + (target.archived ? "。原有群引用会继续指向这份资料。" : "。正文和历史版本保留，原有群引用会标记已归档。"))
            }
        }
    }
    private var titleSearch: some View {
        TextField(showArchived ? "按标题搜索回收站" : "按标题搜索机组资料库",text:$query).textFieldStyle(.roundedBorder).frame(idealWidth:200)
    }
    private var pagePicker: some View {
        Menu {
            ForEach(visibleObjects) { item in
                Button(item.title + (item.archived ? "（已归档）" : "")) { Task { await open(item.id,kind:item.kind) } }
            }
        } label: { Label(visibleDetail?.object.displayTitle ?? "选择资料",systemImage:"doc.text").lineLimit(1) }
        .disabled(store.busy || visibleObjects.isEmpty).frame(maxWidth:300)
    }
    private var catalogProgress: some View {
        HStack {
            if let catalog {
                let scope = catalog.next == nil ? "已读完当前范围" : "还有后续资料"
                let result = CrewdKnowledgeTitleSearch.normalized(query).isEmpty ? "当前类型 \(visibleObjects.count) 项" : "已读取的资料中有 \(visibleObjects.count) 个标题匹配"
                Text(store.catalogStatus == .failed ? result + " · 后续读取失败" : result + " · " + scope)
                Spacer()
                if catalog.next != nil {
                    Button(store.catalogStatus == .failed ? "重读后续资料" : "读取后续资料") {
                        Task { await store.loadNextViewCatalogPage() }
                    }.disabled(store.busy)
                }
            }
        }.font(CrewdType.caption).foregroundStyle(.secondary).padding(.horizontal,16).padding(.bottom,8)
    }
    private var catalogEmptyText: String {
        if store.busy && catalog == nil { return "正在读取机组资料库…" }
        if catalog == nil {
            switch store.catalogStatus {
            case .uninitialized: return "机组资料库尚未初始化。"
            case .unavailable: return "当前机组资料库不可用。"
            case .failed: return "资料读取未完成，请刷新。"
            default: return "机组资料库尚未读取。"
            }
        }
        if !visibleObjects.isEmpty { return "选择资料查看内容" }
        let searching = !CrewdKnowledgeTitleSearch.normalized(query).isEmpty
        if catalog?.next != nil {
            return searching ? "已读取的资料没有标题匹配，仍有后续资料。" : "已读取的资料没有当前类型，仍有后续资料。"
        }
        return searching ? "当前范围内没有匹配标题。" : "当前范围内没有这种资料。"
    }
    private var reloadButton: some View {
        Button { Task { await reload() } } label: { Image(systemName: "arrow.clockwise") }
            .buttonStyle(CrewdToolButtonStyle()).help("刷新资料").accessibilityLabel("刷新资料")
            .disabled(store.busy)
    }
    private var recycleBinButton: some View {
        Button(showArchived ? "返回资料库" : "回收站",systemImage:showArchived ? "arrow.left" : "trash") {
            showArchived.toggle(); selected=""; query=""; lifecycleFeedback=nil; confirmArchive=nil
            Task { await reload() }
        }.disabled(store.busy)
    }
    private var systemButton: some View {
        Button { showSystem = true } label: { Image(systemName: "ellipsis") }
            .buttonStyle(CrewdToolButtonStyle()).accessibilityLabel("页面管理")
            .help("页面权限、回收站、原请求与恢复控制")
    }
    private func reload(restorePreferred: Bool = true) async {
        await store.refreshViewCatalog(includeArchived:showArchived)
        // Only an explicitly opened native document is eligible for read-only
        // reopening. Browsing catalog pages never opens an Agent-authored page.
        guard restorePreferred, !showArchived, collection == .documents, visibleDetail == nil, let context = catalog?.context,
              let preferred = UserDefaults.standard.string(forKey:pagePreference(context)) else { return }
        await open(preferred,kind:.document,previouslyOpenedDocument:true)
    }
    private func open(_ id: String, kind: CrewdKnowledgeStore.ViewCatalogKind, previouslyOpenedDocument: Bool = false) async {
        guard let context = catalog?.context else { return }
        await store.openViewCatalogItem(id,kind:kind,previouslyOpenedDocument:previouslyOpenedDocument)
        guard let detail = store.detail, detail.context == context, detail.object.identity == id, catalog?.context == context,
              collection.includes(detail.object), detail.object.isArchived == showArchived else { return }
        selected = id
        if !showArchived { UserDefaults.standard.set(id,forKey:pagePreference(context)) }
    }
    private func pagePreference(_ context: CrewdDataContext) -> String {
        "pendingcrew.page." + context.authorityId + "." + context.crewId + "." + context.namespace + (collection == .all ? "" : "." + collection.rawValue)
    }
    private func page(_ detail:CrewdDataObjectDetail)->some View {
        VStack(alignment:.leading,spacing:0) {
            HStack {
                Text(detail.object.displayTitle).font(CrewdType.title)
                Spacer()
                if collection == .documents, workStore != nil, (try? store.documentWorkLinkAnchor(document:detail)) != nil {
                    Button("关联待办",systemImage:"link") { documentWorkTarget = detail }.disabled(store.busy)
                }
                if collection == .documents, store.canPrepareNativeDocumentEdit(detail) {
                    Button("版本记录",systemImage:"clock.arrow.circlepath") { historyTarget = detail }.disabled(store.busy)
                    Button("编辑文档",systemImage:"pencil") { editTarget = detail }.disabled(store.busy || store.hasBlockingPending)
                }
                if collection == .documents, store.canPrepareNativeDocumentComposition(detail) {
                    Button("编排内容块",systemImage:"square.stack") { compositionTarget = detail }.disabled(store.busy)
                }
                Button(detail.object.isArchived ? "恢复资料…" : "移入回收站…",systemImage:detail.object.isArchived ? "arrow.uturn.backward" : "archivebox") {
                    confirmArchive = try? store.prepareNativeViewArchive(detail)
                }.disabled(store.busy || store.hasBlockingPending)
            }.padding(.horizontal,24).padding(.top,20).padding(.bottom,8)
            CrewdKnowledgeObjectSurface(store:store,detail:detail)
        }.frame(maxWidth:.infinity,maxHeight:.infinity,alignment:.topLeading)
    }
    private var systemControls: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack { Text("页面与数据系统控制").font(.headline); Spacer(); Button("完成") { showSystem = false } }
            Text("作者页面不能代替这里的权限、原请求核对与恢复控制。").font(.caption)
            Button("停止页面并撤回写入权限") { store.renderer.revoke() }
            ForEach(store.pending) { intent in
                HStack {
                    let label = intent.viewArchive.map{$0.archived ? "原归档" : "原恢复"} ?? (intent.composition == nil ? "待核对操作" : "原文档编排")
                    Text(label + " " + intent.id.uuidString).font(.caption).textSelection(.enabled)
                    Button("查询原结果") { Task { await lookup(intent) } }.disabled(store.busy)
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
        .confirmationDialog("恢复知识快照？当前数据会先保留恢复副本，旧请求不会重放。", isPresented: Binding(get: { restoreTarget != nil }, set: { if !$0 { restoreTarget = nil } })) {
            Button("明确恢复", role: .destructive) {
                guard let target = restoreTarget else { return }; restoreTarget = nil
                Task { await store.restoreSnapshot(target); await reload() }
            }
            Button("取消", role: .cancel) { restoreTarget = nil }
        }
    }
    private func changeArchive(_ target: CrewdKnowledgeStore.NativeViewArchiveTarget) {
        guard let context = catalog?.context else { return }
        Task {
            do {
                let result = try await store.submitNativeViewArchive(target)
                if result?.phase == .committed {
                    await finishedArchive(id:target.id,archived:!target.archived,context:context)
                } else {
                    await reload(restorePreferred:false)
                    lifecycleFeedback = store.hasBlockingPending ? "原请求结果待核对。请查询原结果。" : "资料状态未确认，请刷新核对。"
                }
            } catch let error as CrewdClientError { lifecycleFeedback = error.localizedDescription }
            catch { lifecycleFeedback = "当前资料已变化，请重新读取后再确认。" }
        }
    }
    private func lookup(_ intent: CrewdKnowledgeStore.Intent) async {
        await store.lookup(intent.id)
        if let archive = intent.viewArchive, store.result?.phase == .committed,
           store.result.flatMap({UUID(uuidString:$0.requestId)}) == intent.id {
            lifecycleFeedback = archive.archived ? "原归档请求已确认。当前状态以资料库和回收站为准。" : "原恢复请求已确认。当前状态以资料库和回收站为准。"
            await reload(restorePreferred:false)
        }
    }
    private func finishedArchive(id: String, archived: Bool, context: CrewdDataContext) async {
        if archived, UserDefaults.standard.string(forKey:pagePreference(context)) == id {
            UserDefaults.standard.removeObject(forKey:pagePreference(context))
        }
        selected=""
        lifecycleFeedback = archived ? "已移入回收站，群引用保留。" : "已恢复到资料库，原有群引用继续有效。"
        await reload(restorePreferred:false)
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

struct CrewdKnowledgeObjectSurface:View {
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
    @State private var query=""
    @State private var linking=false
    @State private var creating=false
    @State private var reading=false
    @State private var failure:String?
    @State private var retry=UUID()
    private struct DocumentTarget: Identifiable {
        let id = UUID()
        let parent: CrewdDataObjectDetail
        let channel: String
        let link: String
    }
    @State private var editTarget: DocumentTarget?
    @State private var historyTarget: DocumentTarget?
    @State private var compositionTarget: DocumentTarget?
    @State private var documentWorkTarget: DocumentTarget?
    init(workspace:CrewdWorkspace,collection:CrewdKnowledgeCollection){self.workspace=workspace;self.group=workspace.channelKnowledge;self.resources=workspace.channelResources;self.content=workspace.linkedKnowledge;self.library=workspace.knowledge;self.collection=collection}
    private var folder:Binding<String>{Binding(get:{workspace.resourceDrafts.selection(resources.crew,.documents,channel:resources.channel)},set:{workspace.resourceDrafts.select(resources.crew,.documents,$0,channel:resources.channel)})}
    private func folderObject(_ source:CrewdChannelKnowledgeReference)->CrewdValue {
        .object(["kind":.string("knowledgeView"),"id":.string(source.reference.id),"knowledge":(try? CrewdKnowledgeStore.encode(source)) ?? .null])
    }
    private var scopedRows:[CrewdChannelKnowledgeItem]{group.items.filter{item in
        let type=collection == .agentPages ? "web":"document"
        return (item.state != "ready" || item.viewKind == type) && CrewdFolderFilter.includes(folderObject(item.source),selection:folder.wrappedValue,snapshot:resources.folders[.documents])
    }}
    private var rows:[CrewdChannelKnowledgeItem] {
        scopedRows.filter { CrewdKnowledgeTitleSearch.matches($0.state == "ready" ? $0.title : nil,query:query) }
    }
    private var chosen:CrewdChannelKnowledgeItem?{scopedRows.first{$0.id == selected}}
    private var searching:Bool { !CrewdKnowledgeTitleSearch.normalized(query).isEmpty }
    private var groupEmptyText:String { searching ? "本群当前分类的可读资料没有匹配标题。" : "暂无资料" }
    private var pendingLinks: [CrewdPendingKnowledgeLink] {
        CrewdPendingKnowledgeLink.inChannel(workspace.durableUnknown,scope:workspace.presentationScope,crew:group.crew,channel:group.channel)
    }
    private var readKey:String{(chosen?.id ?? "")+"/"+(chosen?.source.context.generation ?? "")+"/"+(chosen?.revision ?? "")+"/"+String(chosen?.isReadableActiveView == true)+"/"+retry.uuidString}
    var body:some View {
        VStack(spacing:0){
            ViewThatFits(in:.horizontal){
                HStack(spacing:8){documentActions;documentFolders.fixedSize(horizontal:true,vertical:false);titleSearch}
                VStack(alignment:.leading,spacing:6){documentActions;documentFolders;titleSearch}
            }.font(CrewdType.label).frame(maxWidth:.infinity,alignment:.leading)
                .padding(.horizontal,16).padding(.bottom,10)
            if searching, group.loaded, !group.loading, group.failure == nil {
                Text("本群当前分类的可读资料中有 \(rows.count) 个标题匹配")
                    .font(CrewdType.caption).foregroundStyle(.secondary)
                    .frame(maxWidth:.infinity,alignment:.leading).padding(.horizontal,16)
            }
            CrewdKnowledgeLinkRecovery(workspace:workspace,group:group,crew:group.crew,channel:group.channel)
                .padding(.horizontal,16)
            if collection == .documents {
                ForEach(library.pendingNativeDocumentEdits(crew:group.crew) +
                        library.pendingDocumentCompositions(crew:group.crew)) { intent in
                    HStack {
                        Text(intent.composition == nil ? "原编辑结果待核对" : "原编排结果待核对").foregroundStyle(.orange)
                        Button("仅查询原请求") {
                            let crew = group.crew, channel = group.channel
                            Task {
                                await library.refresh()
                                guard group.crew == crew, group.channel == channel else { return }
                                await library.lookup(intent.id)
                                guard group.crew == crew, group.channel == channel else { return }
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
            if group.loaded,!group.loading,!reading,group.failure == nil,failure == nil,rows.isEmpty,chosen == nil {
                Text(groupEmptyText).font(.callout).foregroundStyle(.secondary)
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
        .task(id:readKey){guard documentWorkTarget == nil else{return};guard let chosen,chosen.isReadableActiveView else{content.renderer.revoke();return};reading=true;defer{reading=false};do{try await workspace.openLinkedKnowledge(chosen);failure=nil}catch{if !(error is CancellationError){failure=error.localizedDescription}}}
        .sheet(isPresented:$linking,onDismiss:{retry=UUID()}){CrewdKnowledgeLinkPicker(workspace:workspace,collection:collection)}
        .sheet(isPresented:$creating){CrewdNativeDocumentCreation(workspace:workspace)}
        .sheet(item:$editTarget){ target in
            CrewdNativeDocumentEditor(store:content,parent:target.parent,scopeCurrent:{ isCurrent(target) },onSaved:{await refreshAfterEdit()})
        }
        .sheet(item:$historyTarget){ target in
            CrewdNativeDocumentHistory(store:content,parent:target.parent,scopeCurrent:{ isCurrent(target) },onRestored:{await refreshAfterEdit()})
        }
        .sheet(item:$compositionTarget) { target in
            CrewdDocumentCompositionEditor(store:content,parent:target.parent,
                scopeCurrent:{ isCurrent(target) },onChanged:{ await refreshAfterEdit() })
        }
        .sheet(item:$documentWorkTarget,onDismiss:{ Task { await refreshAfterEdit() } }){ target in
            CrewdDocumentWorkLinksView(store:content,workStore:workspace.workStore,parent:target.parent,
                scopeCurrent:{ isCurrent(target) && workspace.client.capabilities.contains("data.documentWorkLinks") })
        }
        .onChange(of:content.pending){_,_ in if collection == .documents {library.synchronizePending()}}
        .onChange(of:library.pending){_,pending in if collection == .documents && pending.isEmpty {library.synchronizePending()}}
        .onChange(of:group.crew+group.channel){_,_ in selected=nil;query="";linking=false;creating=false;editTarget=nil;historyTarget=nil;compositionTarget=nil;documentWorkTarget=nil;failure=nil}
        .onChange(of:collection){_,_ in selected=nil;query="";editTarget=nil;historyTarget=nil;compositionTarget=nil;documentWorkTarget=nil;content.renderer.revoke()}
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
    private var titleSearch: some View {
        TextField("按标题搜索本群引用",text:$query).textFieldStyle(.roundedBorder).frame(idealWidth:180)
            .accessibilityLabel("按标题搜索本群引用")
    }
    private var list:some View {
        ScrollView {
            LazyVStack(alignment:.leading,spacing:4) {
                if rows.isEmpty, group.loaded, !group.loading, group.failure == nil {
                    Text(groupEmptyText).font(CrewdType.caption).foregroundStyle(.secondary).padding(8)
                }
                ForEach(rows) { item in
                    Button { selected=item.id } label: {
                        HStack(alignment:.top,spacing:8) {
                            Image(systemName:collection == .agentPages ? "rectangle.on.rectangle":"doc.text").foregroundStyle(.secondary).frame(width:16)
                            VStack(alignment:.leading,spacing:4) {
                                Text(item.title ?? "未命名资料").font(CrewdType.body.weight(selected == item.id ? .semibold:.regular)).multilineTextAlignment(.leading)
                                if item.isArchivedReference {
                                    Text("已归档 · 引用保留").font(CrewdType.caption).foregroundStyle(.secondary)
                                } else if item.state != "ready" {
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
                if chosen.isArchivedReference {
                    CrewdEmptySurface(title:"原资料已归档",detail:"本群引用仍保留。可从原机组的资料库回收站恢复原资料。",symbol:"archivebox")
                } else if chosen.isReadableActiveView,let detail=content.detail,detail.object.identity == chosen.source.reference.id,
                   detail.resolvedContext.crewId == chosen.source.context.crewId {
                    if collection == .documents, group.crew == chosen.source.context.crewId {
                        HStack {
                            if workspace.client.capabilities.contains("data.documentWorkLinks"),
                               (try? content.documentWorkLinkAnchor(document:detail)) != nil {
                                Button("关联待办",systemImage:"link") { documentWorkTarget = .init(parent:detail,channel:group.channel,link:chosen.id) }
                                    .disabled(content.busy)
                            }
                            if content.canPrepareNativeDocumentEdit(detail) {
                                Button("编辑原文",systemImage:"pencil") { editTarget = .init(parent:detail,channel:group.channel,link:chosen.id) }
                                    .disabled(content.busy || content.hasBlockingPending)
                                Button("版本记录",systemImage:"clock.arrow.circlepath") { historyTarget = .init(parent:detail,channel:group.channel,link:chosen.id) }
                                    .disabled(content.busy)
                            }
                            if content.canPrepareNativeDocumentComposition(detail) {
                                Button("编排内容块",systemImage:"square.stack") {
                                    compositionTarget = .init(parent:detail,channel:group.channel,link:chosen.id)
                                }.disabled(content.busy)
                            }
                        }.padding(.horizontal,24)
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
    private func isCurrent(_ target: DocumentTarget) -> Bool {
        group.crew == target.parent.context.crewId && group.channel == target.channel &&
            chosen?.id == target.link && chosen?.isReadableActiveView == true
    }
    private func unlink(_ item:CrewdChannelKnowledgeItem){
        guard !pendingLinks.contains(where: { $0.source.identity == item.id }),
              let revision=group.linkRevision,let snapshot=workspace.knowledgeLinkDrafts.begin(crew:group.crew,channel:group.channel,source:item.source,revision:revision,linked:false) else{return}
        Task{do{_ = try await group.submit(snapshot);workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true);if selected == item.id{selected=nil}}catch{workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));failure=error.localizedDescription}}
    }
}

private struct CrewdDocumentWorkLinksView: View {
    typealias Anchor = CrewdKnowledgeStore.DocumentWorkLinkAnchor
    typealias Link = CrewdKnowledgeStore.DocumentWorkLink

    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: CrewdKnowledgeStore
    @ObservedObject var workStore: CrewdWorkStore
    let parent: CrewdDataObjectDetail
    let scopeCurrent: @MainActor () -> Bool

    private struct Choice: Identifiable {
        let reference: CrewdDataRef
        let title: String
        var id: String { reference.id }
    }
    @State private var anchor: Anchor?
    @State private var links: [Link] = []
    @State private var next: String?
    @State private var loaded = false
    @State private var working = false
    @State private var query = ""
    @State private var selection: String?
    @State private var confirmation: Link?
    @State private var feedback: String?

    private var isCurrent: Bool {
        guard scopeCurrent(), let anchor else { return false }
        return store.documentWorkLinkScopeIsCurrent(anchor)
    }
    private var candidates: [Choice] {
        guard let anchor, isCurrent, workStore.selectedCrew == anchor.crew, workStore.hasLoaded,
              workStore.lastFailure == nil else { return [] }
        let plans: [Choice] = workStore.planIDs.compactMap { id in
            guard id.kind == .plan, let plan = workStore.plans[id], plan.crew == anchor.crew, plan.id == id else { return nil }
            return .init(reference:.init(namespace:anchor.endpoint.namespace,kind:"plan",id:id.raw),
                         title:plan.value["title"].string ?? "")
        }
        let requirements: [Choice] = workStore.requirementIDs.compactMap { id in
            guard id.kind == .requirement, let row = workStore.requirements[id], row["crewId"].string == anchor.crew,
                  row["requirementId"].string == id.raw, row["deletedAt"].string == nil else { return nil }
            return .init(reference:.init(namespace:anchor.endpoint.namespace,kind:"requirement",id:id.raw),
                         title:row["text"].string ?? "")
        }
        return requirements + plans
    }
    private var canWrite: Bool { isCurrent && loaded && !working && !store.busy && !store.hasBlockingPending }
    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            HStack {
                Text("关联待办").font(.headline)
                Spacer()
                Button("完成") { dismiss() }.disabled(working)
            }
            Text(parent.object.displayTitle).font(CrewdType.label).lineLimit(2)
            if let feedback { Text(feedback).font(CrewdType.caption).foregroundStyle(.secondary) }
            if let anchor, scopeCurrent() {
                HStack {
                    Button("重新读取") { Task { await reload() } }.disabled(working || store.busy)
                    if working { ProgressView().controlSize(.small) }
                }
                ScrollView {
                    VStack(alignment:.leading,spacing:12) {
                        ForEach(store.pendingDocumentWorkLinks(anchor)) { intent in
                            HStack {
                                VStack(alignment:.leading) {
                                    Text("有原请求待核对").foregroundStyle(.orange)
                                    Text(intent.id.uuidString).font(CrewdType.caption).textSelection(.enabled)
                                }
                                Spacer()
                                Button("查询原结果") { Task { await recover(intent.id) } }.disabled(working || store.busy)
                            }
                        }
                        if isCurrent {
                            existingLinks
                            Divider()
                            workPicker
                        } else {
                            Text("文档状态已变化，请重新读取。").foregroundStyle(.secondary)
                        }
                    }
                }
            } else if !working {
                Text("当前文档已不可用。请返回后重新打开。").foregroundStyle(.secondary)
            }
        }
        .padding(20)
        #if os(macOS)
        .frame(minWidth:480,minHeight:420)
        #endif
        .interactiveDismissDisabled(working)
        .task {
            guard scopeCurrent(), let initial = try? store.documentWorkLinkAnchor(document:parent) else { return }
            anchor = initial
            store.synchronizePending()
            await readPage()
        }
        .task(id:workStore.readScope) { if scopeCurrent() { await workStore.ensureLoaded() } }
        .confirmationDialog(confirmation?.archived == true ? "恢复这条关联？" : "归档这条关联？",
                            isPresented:Binding(get:{confirmation != nil},set:{if !$0 {confirmation=nil}})) {
            if let link = confirmation {
                Button(link.archived ? "恢复关联" : "归档关联",role:link.archived ? nil : .destructive) {
                    confirmation = nil
                    Task { await changeArchive(link) }
                }
            }
            Button("取消",role:.cancel) {}
        } message: {
            Text("只修改这条关联。文档和待办的内容、状态都保留。")
        }
    }
    private var existingLinks: some View {
        let choices = candidates
        return VStack(alignment:.leading,spacing:8) {
            Text("现有关联").font(.headline)
            ForEach(links) { link in
                HStack {
                    VStack(alignment:.leading,spacing:3) {
                        let title = choices.first { $0.reference == link.work }?.title
                        Text(link.work.map { work in
                            title.map { $0.isEmpty ? work.id : "\(work.id)  \($0)" } ?? work.id
                        } ?? "原待办不可用").lineLimit(2)
                        Text(link.archived ? "已归档" : "有效").font(CrewdType.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button(link.archived ? "恢复…" : "归档…") { confirmation = link }
                        .disabled(!canWrite || (link.archived && link.work == nil))
                }
            }
            if next != nil {
                Text("当前只显示已读取的关联。").font(CrewdType.caption).foregroundStyle(.secondary)
                Button("读取后续关联") { Task { await readPage(after:next) } }.disabled(working || store.busy)
            } else if loaded && links.isEmpty {
                Text("这份文档还没有待办关联。").foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth:.infinity,alignment:.leading)
    }
    private var workPicker: some View {
        let candidates = candidates
        let choices = candidates.filter {
            CrewdKnowledgeTitleSearch.matches($0.reference.id + " " + $0.title,query:query)
        }
        let selected = candidates.first { $0.id == selection }
        let existing = selected.map { choice in links.contains { $0.work == choice.reference } } ?? false
        return VStack(alignment:.leading,spacing:8) {
            Text("关联本机组的待办").font(.headline)
            if workStore.loading {
                Text("正在读取本机组需求与工作项…").foregroundStyle(.secondary)
            } else if workStore.lastFailure != nil || !workStore.hasLoaded {
                Button("读取本机组需求与工作项") { Task { try? await workStore.refresh() } }
            } else if candidates.isEmpty {
                Text("没有可关联的本机组需求或工作项。").foregroundStyle(.secondary)
            } else {
                TextField("搜索编号或标题",text:$query).textFieldStyle(.roundedBorder)
                ForEach(choices.prefix(20)) { choice in
                    Button {
                        selection = choice.id
                    } label: {
                        Label(choice.id + "  " + choice.title,systemImage:selection == choice.id ? "largecircle.fill.circle" : "circle")
                            .lineLimit(2)
                    }.buttonStyle(.plain)
                }
                if choices.isEmpty { Text("没有匹配的待办。").foregroundStyle(.secondary) }
                if choices.count > 20 { Text("显示前 20 项，可用编号或标题缩小范围。").font(CrewdType.caption).foregroundStyle(.secondary) }
                if let selected {
                    Button(existing ? "已有这条关联" : "关联 \(selected.id)") { Task { await create(selected) } }
                        .disabled(!canWrite || existing)
                }
            }
        }
    }
    private func readPage(after: String? = nil) async {
        guard let anchor, isCurrent, !working else { return }
        working = true
        defer { working = false }
        do { try await adoptPage(anchor,after:after) }
        catch { feedback = "未能读取关联。请重新读取后再操作。" }
    }
    private func adoptPage(_ anchor: Anchor, after: String? = nil) async throws {
        let page = try await store.loadDocumentWorkLinkPage(anchor,after:after)
        guard scopeCurrent(), store.documentWorkLinkScopeIsCurrent(anchor), !Task.isCancelled else { throw CancellationError() }
        if after == nil { links = page.items }
        else { links.append(contentsOf:page.items) }
        next = page.next; loaded = true
    }
    private func reread(_ original: Anchor) async throws {
        loaded = false; links = []; next = nil
        let fresh = try await store.reloadDocumentWorkLinkAnchor(original)
        guard scopeCurrent(), !Task.isCancelled else { throw CancellationError() }
        anchor = fresh
        store.synchronizePending()
        try await adoptPage(fresh)
    }
    private func reload() async {
        guard let anchor, scopeCurrent(), !working else { return }
        working = true; feedback = nil
        defer { working = false }
        do { try await reread(anchor) }
        catch { feedback = "未能重新读取当前文档的关联。" }
    }
    private func create(_ choice: Choice) async {
        guard candidates.contains(where:{ $0.reference == choice.reference }), !links.contains(where:{ $0.work == choice.reference }) else { return }
        await mutate(success:"已建立关联。") { try await store.createDocumentWorkLink($0,work:choice.reference) }
    }
    private func changeArchive(_ link: Link) async {
        guard links.contains(link) else { return }
        await mutate(success:link.archived ? "已恢复关联。" : "已归档关联。") {
            try await store.setDocumentWorkLinkArchived($0,link:link,archived:!link.archived)
        }
    }
    private func mutate(success: String, operation: @MainActor (Anchor) async throws -> CrewdDataChangeResult?) async {
        guard let anchor, canWrite else { return }
        working = true
        defer { working = false }
        do {
            if let result = try await operation(anchor), result.phase == .committed {
                feedback = success; selection = nil
            } else { feedback = store.status }
        } catch { feedback = "当前关联已变化，无法提交这项修改。" }
        do { try await reread(anchor) }
        catch { feedback = (feedback ?? "") + " 当前关联列表尚未重新读取。" }
    }
    private func recover(_ id: UUID) async {
        guard let anchor, scopeCurrent(), !working else { return }
        working = true
        defer { working = false }
        await store.lookup(id)
        feedback = store.status
        do { try await reread(anchor) }
        catch { feedback = (feedback ?? "") + " 当前关联列表尚未重新读取。" }
    }
}

private struct CrewdDocumentCompositionEditor: View {
    private typealias Store = CrewdKnowledgeStore
    private enum Section: String, CaseIterable {
        case arrangement = "编排"
        case blocks = "可用内容块"
        case history = "编排历史"
    }
    private struct Row: Identifiable {
        let id = UUID()
        let position: Store.DocumentCompositionPosition
    }
    private enum Confirmation: String, Identifiable {
        case save, restore
        var id: String { rawValue }
    }

    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: CrewdKnowledgeStore
    let parent: CrewdDataObjectDetail
    let scopeCurrent: @MainActor () -> Bool
    let onChanged: @MainActor () async -> Void
    @State private var composition: Store.DocumentComposition?
    @State private var section = Section.arrangement
    @State private var rows: [Row] = []
    @State private var blocks: [Store.DocumentCompositionBlock] = []
    @State private var revisions: [Store.NativeDocumentRevision] = []
    @State private var preview: Store.DocumentCompositionRevision?
    @State private var nextBlock: String?
    @State private var nextRevision: String?
    @State private var blocksLoaded = false
    @State private var historyLoaded = false
    @State private var working = false
    @State private var confirmation: Confirmation?
    @State private var feedback: String?

    private var positions: [Store.DocumentCompositionPosition] { rows.map(\.position) }
    private var current: Bool {
        scopeCurrent() && store.canPrepareNativeDocumentComposition(parent)
    }
    private var pending: [Store.Intent] {
        guard scopeCurrent() else { return [] }
        return store.pendingDocumentCompositions(viewID:parent.object.identity,crew:parent.context.crewId)
    }

    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            HStack {
                Text("文档内容块编排").font(CrewdType.title)
                Spacer()
                Button("关闭") { dismiss() }.disabled(working)
            }
            if let composition {
                Text("打开时的文档版本 \(composition.target.head)").font(CrewdType.caption)
                if current {
                    Picker("编排视图",selection:$section) {
                        ForEach(Section.allCases,id:\.self) { section in Text(section.rawValue).tag(section) }
                    }.pickerStyle(.segmented)
                    ScrollView {
                        VStack(alignment:.leading,spacing:10) {
                            switch section {
                            case .arrangement: arrangement(composition)
                            case .blocks: catalog(composition)
                            case .history: history(composition)
                            }
                        }.frame(maxWidth:.infinity,alignment:.leading)
                    }.frame(minHeight:180,maxHeight:390)
                } else {
                    Text("文档或连接已变化。关闭后从当前文档重新打开可继续编辑。").font(CrewdType.caption)
                }
                ForEach(pending) { intent in
                    HStack {
                        Text("原编排请求待核对").foregroundStyle(.orange)
                        Text(intent.id.uuidString).textSelection(.enabled)
                        Button("仅查询原请求") { recover(intent) }.disabled(working || store.busy)
                    }.font(CrewdType.caption)
                }
            }
            if let feedback { Text(feedback).font(CrewdType.caption).textSelection(.enabled) }
            if working { ProgressView().controlSize(.small) }
        }.padding(20)
        #if os(macOS)
        .frame(minWidth:540)
        #endif
        .interactiveDismissDisabled(working)
        .task {
            guard scopeCurrent(), composition == nil else { return }
            do {
                let value = try store.nativeDocumentComposition(parent)
                composition = value
                rows = value.placements.map { Row(position:.existing($0.id)) }
                store.synchronizePending()
            } catch { feedback = error.localizedDescription }
        }
        .confirmationDialog(confirmation == .restore ? "恢复选定版本的排列？" : "保存当前排列？",
            isPresented:Binding(get:{confirmation != nil},set:{if !$0 {confirmation=nil}})) {
            if let confirmation {
                Button(confirmation == .restore ? "恢复为新版本" : "保存为新版本") {
                    guard let composition else { return }
                    switch confirmation {
                    case .save:
                        let reviewed = positions
                        submit { try await store.saveDocumentComposition(composition,positions:reviewed) }
                    case .restore:
                        if let preview { submit { try await store.restoreDocumentComposition(preview,replacing:composition) } }
                    }
                }
            }
            Button("取消",role:.cancel) {}
        } message: {
            Text(confirmation == .restore
                ? "历史内容按当时的版本固定引用。当前标题和工作关联保留。"
                : "保存顺序和引用。新增内容块固定到所选版本，源块不会改变。")
        }
    }

    private func arrangement(_ composition: Store.DocumentComposition) -> some View {
        VStack(alignment:.leading,spacing:8) {
            if rows.isEmpty { Text("当前排列没有内容块。").foregroundStyle(.secondary) }
            ForEach(Array(rows.enumerated()),id:\.element.id) { index, row in
                HStack {
                    switch row.position {
                    case .existing(let id):
                        if let position = composition.placements.first(where:{$0.id == id}) {
                            placementLabel(position)
                        }
                    case .pinned(let block):
                        VStack(alignment:.leading) {
                            Text(block.preview).lineLimit(2)
                            Text("固定版本 \(block.revision)").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    Spacer()
                    Button { move(row.id,by:-1) } label: { Image(systemName:"arrow.up") }
                        .accessibilityLabel("上移").disabled(index == 0)
                    Button { move(row.id,by:1) } label: { Image(systemName:"arrow.down") }
                        .accessibilityLabel("下移").disabled(index + 1 == rows.count)
                    Button { rows.removeAll { $0.id == row.id } } label: { Image(systemName:"minus.circle") }
                        .accessibilityLabel("移除引用")
                }.disabled(working)
            }
            Button("保存编排…") { confirmation = .save }
                .disabled(working || store.busy || store.hasBlockingPending ||
                    positions == composition.placements.map { .existing($0.id) })
        }
    }

    private func move(_ id: UUID, by offset: Int) {
        guard let index = rows.firstIndex(where:{$0.id == id}), rows.indices.contains(index + offset) else { return }
        rows.swapAt(index,index + offset)
    }

    private func placementLabel(_ position: CrewdDataPlacement) -> some View {
        VStack(alignment:.leading) {
            if case .visible(let block) = position.block { Text(block.id).lineLimit(1).textSelection(.enabled) }
            else { Text("内容块不可用") }
            Text(position.atRevision.map { "固定版本 \($0)" } ?? "跟随当前内容")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private func catalog(_ composition: Store.DocumentComposition) -> some View {
        VStack(alignment:.leading,spacing:8) {
            Text("新增引用会固定到这里显示的版本，源块更新后仍显示这个版本。").font(CrewdType.caption)
            if !blocksLoaded { Button("读取本机组内容块") { loadBlocks(composition) } }
            else if blocks.isEmpty { Text("本机组没有可用内容块。").foregroundStyle(.secondary) }
            ForEach(blocks) { block in
                HStack {
                    VStack(alignment:.leading) {
                        Text(block.preview.isEmpty ? "空内容" : block.preview).lineLimit(2)
                        Text(block.contentType + " · 版本 " + block.revision + " · " + block.id)
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer()
                    Button("添加固定版本") { rows.append(.init(position:.pinned(block))); section = .arrangement }
                        .disabled(rows.count >= Store.documentCompositionLimit)
                }
            }
            if let nextBlock { Button("读取更多内容块") { loadBlocks(composition,after:nextBlock) } }
        }.disabled(working || store.busy)
    }

    private func history(_ composition: Store.DocumentComposition) -> some View {
        VStack(alignment:.leading,spacing:8) {
            if !historyLoaded { Button("读取编排历史") { loadHistory(composition) } }
            ForEach(revisions) { revision in
                Button("版本 " + revision.revision + " · " + revision.updatedAt) { inspect(composition,revision:revision) }
            }
            if let nextRevision { Button("读取更多版本") { loadHistory(composition,after:nextRevision) } }
            if let preview {
                Divider()
                Text("版本 " + preview.summary.revision + " 的排列").font(CrewdType.label)
                if preview.placements.isEmpty { Text("此版本没有内容块。").foregroundStyle(.secondary) }
                ForEach(Array(preview.placements.enumerated()),id:\.offset) { _, position in placementLabel(position) }
                if preview.canRestore {
                    Button("恢复这份排列…") { confirmation = .restore }.disabled(store.hasBlockingPending)
                } else if preview.summary.revision != composition.target.head {
                    Text("该版本包含不可恢复的引用。").font(.caption).foregroundStyle(.secondary)
                }
            }
        }.disabled(working || store.busy)
    }

    private func run(_ operation: @escaping @MainActor () async throws -> Void) {
        guard !working, scopeCurrent() else { return }
        working = true; feedback = nil
        Task {
            defer { working = false }
            do { try await operation() }
            catch { feedback = (error as? CrewdClientError)?.message ?? "当前文档、引用或连接已变化，请重新读取后再操作。" }
        }
    }
    private func loadBlocks(_ composition: Store.DocumentComposition, after: String? = nil) {
        run {
            let page = try await store.loadDocumentCompositionBlocks(composition,after:after)
            guard current else { return }
            blocks.append(contentsOf:page.items); nextBlock = page.next; blocksLoaded = true
        }
    }
    private func loadHistory(_ composition: Store.DocumentComposition, after: String? = nil) {
        run {
            let page = try await store.loadNativeDocumentRevisions(composition.target,after:after)
            guard current else { return }
            revisions.append(contentsOf:page.items); nextRevision = page.next; historyLoaded = true
        }
    }
    private func inspect(_ composition: Store.DocumentComposition, revision: Store.NativeDocumentRevision) {
        run {
            preview = nil
            let result = try await store.inspectDocumentCompositionRevision(composition,revision:revision)
            if current { preview = result }
        }
    }
    private func submit(_ operation: @escaping @MainActor () async throws -> CrewdDataChangeResult?) {
        run {
            guard current, !store.hasBlockingPending else { return }
            let result = try await operation()
            guard scopeCurrent() else { return }
            if result?.phase == .committed {
                dismiss()
                await onChanged()
            } else {
                store.synchronizePending()
                feedback = store.status
            }
        }
    }
    private func recover(_ intent: Store.Intent) {
        run {
            await store.refresh(loadViews:false)
            guard scopeCurrent() else { return }
            await store.lookup(intent.id)
            guard scopeCurrent() else { return }
            if let result = store.result, UUID(uuidString:result.requestId) == intent.id, result.phase == .committed {
                dismiss()
                await onChanged()
            } else { feedback = store.status }
        }
    }
}

private struct CrewdNativeDocumentHistory: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: CrewdKnowledgeStore
    let parent: CrewdDataObjectDetail
    let scopeCurrent: @MainActor () -> Bool
    let onRestored: @MainActor () async -> Void
    @State private var target: CrewdKnowledgeStore.NativeDocumentHistoryTarget?
    @State private var current: CrewdKnowledgeStore.NativeDocumentEdit?
    @State private var items: [CrewdKnowledgeStore.NativeDocumentRevision] = []
    @State private var next: String?
    @State private var selected: String?
    @State private var preview: CrewdKnowledgeStore.NativeDocumentRevisionSnapshot?
    @State private var working = true
    @State private var confirming = false
    @State private var attempted = false
    @State private var failure: String?

    private var currentScope: Bool { scopeCurrent() && store.canPrepareNativeDocumentEdit(parent) }
    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            HStack { Text("文档版本记录").font(CrewdType.title); Spacer(); Button("关闭") { dismiss() }.disabled(working) }
            if let target, currentScope {
                Text("当前原文版本 \(target.head)。选择版本查看当时保存的正文。").font(CrewdType.caption)
                ScrollView {
                    LazyVStack(alignment:.leading,spacing:6) {
                        ForEach(items) { item in
                            Button { inspect(item,target:target) } label: { revisionRow(item) }
                                .buttonStyle(.plain).accessibilityAddTraits(selected == item.revision ? .isSelected:[])
                        }
                        if let next { Button("读取更多版本") { loadMore(target,after:next) } }
                    }.padding(4)
                }.frame(maxHeight:180).disabled(working || store.busy)
                if let preview {
                    Text("版本 \(preview.revision.revision) · " + preview.revision.title).font(CrewdType.label)
                    ScrollView { Text(preview.body).frame(maxWidth:.infinity,alignment:.leading).textSelection(.enabled) }
                        .frame(minHeight:60,maxHeight:160)
                    if let current {
                        DisclosureGroup("对照当前原文") {
                            Text(current.title).font(CrewdType.label)
                            ScrollView { Text(current.body).frame(maxWidth:.infinity,alignment:.leading).textSelection(.enabled) }.frame(maxHeight:120)
                        }
                        if preview.revision.revision != target.head {
                            Button("恢复为新版本…") { confirming = true }.buttonStyle(.borderedProminent)
                                .disabled(working || attempted || store.busy || store.hasBlockingPending ||
                                          !current.hasChanges(title:preview.revision.title,body:preview.body))
                        }
                    }
                }
            } else if !working && !currentScope {
                Text("当前文档或连接已变化，请关闭并重新打开版本记录。").font(CrewdType.caption).foregroundStyle(.secondary)
            }
            if scopeCurrent() {
                ForEach(store.pendingNativeDocumentEdits(viewID:parent.object.identity)) { intent in
                    Button("查询原文档请求结果") { lookup(intent.id) }.disabled(working || store.busy)
                }
            }
            if let failure { Text(failure).font(CrewdType.caption).foregroundStyle(.orange).textSelection(.enabled) }
            if working { ProgressView().controlSize(.small) }
        }.padding(20)
        #if os(macOS)
        .frame(minWidth:540)
        #endif
        .interactiveDismissDisabled(working)
        .task { await loadInitial() }
        .confirmationDialog("把所选版本的标题与正文保存为当前文档的新版本？",isPresented:$confirming) {
            Button("恢复为新版本") { if let preview, let current { restore(preview,replacing:current) } }
            Button("取消",role:.cancel) {}
        } message: { Text("现有版本和群引用都会保留。") }
    }
    private func revisionRow(_ item: CrewdKnowledgeStore.NativeDocumentRevision) -> some View {
        HStack(alignment:.top) {
            VStack(alignment:.leading,spacing:4) {
                Text("版本 \(item.revision) · " + item.title).font(CrewdType.label)
                Text(item.updatedAt + (item.author == "human" ? " · 人类" : item.author == "session" ? " · Agent" : "") + (item.archived ? " · 当时已归档" : ""))
                    .font(CrewdType.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if selected == item.revision { Image(systemName:"checkmark").accessibilityHidden(true) }
        }.contentShape(Rectangle())
    }
    private func loadInitial() async {
        defer { working = false }
        do {
            guard currentScope else { throw CrewdKnowledgeStore.Failure.stale }
            let source = try store.nativeDocumentHistoryTarget(parent)
            let live = try await store.prepareNativeDocumentEdit(parent)
            let page = try await store.loadNativeDocumentRevisions(source)
            guard currentScope else { throw CrewdKnowledgeStore.Failure.stale }
            target = source; current = live; items = page.items; next = page.next
        } catch { failure = "无法读取当前纯文本文档的版本记录。" }
    }
    private func loadMore(_ target: CrewdKnowledgeStore.NativeDocumentHistoryTarget, after: String) {
        working = true
        Task {
            defer { working = false }
            do {
                let page = try await store.loadNativeDocumentRevisions(target,after:after)
                guard currentScope else { throw CrewdKnowledgeStore.Failure.stale }
                items.append(contentsOf:page.items); next = page.next; failure = nil
            } catch { failure = "后续版本未读取，当前列表仍不完整。" }
        }
    }
    private func inspect(_ item: CrewdKnowledgeStore.NativeDocumentRevision, target: CrewdKnowledgeStore.NativeDocumentHistoryTarget) {
        working = true; selected = item.revision; preview = nil
        Task {
            defer { working = false }
            do {
                let snapshot = try await store.inspectNativeDocumentRevision(target,revision:item)
                guard currentScope else { throw CrewdKnowledgeStore.Failure.stale }
                preview = snapshot; failure = nil
            } catch { failure = "该版本的原正文不可用。" }
        }
    }
    private func restore(_ snapshot: CrewdKnowledgeStore.NativeDocumentRevisionSnapshot, replacing live: CrewdKnowledgeStore.NativeDocumentEdit) {
        working = true; attempted = true
        Task {
            defer { working = false }
            do {
                guard currentScope else { throw CrewdKnowledgeStore.Failure.stale }
                let result = try await store.restoreNativeDocumentRevision(snapshot,replacing:live)
                if result?.phase == .committed { if scopeCurrent() { await onRestored() }; dismiss(); return }
                if result == nil { await store.refresh() }
                failure = store.hasBlockingPending ? "恢复结果待核对，请查询原文档请求。" : "恢复未确认，请重新打开文档核对。"
            } catch let error as CrewdClientError { failure = error.localizedDescription }
            catch { failure = "当前文档已变化，请重新打开后核对版本。" }
        }
    }
    private func lookup(_ id: UUID) {
        working = true
        Task {
            defer { working = false }
            guard scopeCurrent() else { return }
            await store.lookup(id)
            if store.result.flatMap({ UUID(uuidString:$0.requestId) }) == id, store.result?.phase == .committed {
                if scopeCurrent() { await onRestored() }; dismiss()
            } else { failure = store.status }
        }
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
    @ObservedObject var client:CrewdClient
    let collection:CrewdKnowledgeCollection
    @Environment(\.dismiss) private var dismiss
    @State private var owner=UUID()
    @State private var sourceCrew=""
    @State private var chosen=""
    @State private var query=""
    @State private var targetCrew=""
    @State private var targetChannel=""
    @State private var refresh=UUID()
    @State private var submitting=false
    @State private var failure:String?
    init(workspace:CrewdWorkspace,collection:CrewdKnowledgeCollection){
        self.workspace=workspace; self.library=workspace.knowledgeLinkPicker; self.group=workspace.channelKnowledge
        self.client=workspace.client; self.collection=collection
    }
    private var current:Bool {
        !targetChannel.isEmpty && workspace.crewID == targetCrew && workspace.channelID == targetChannel &&
            group.crew == targetCrew && group.channel == targetChannel && !workspace.isRetired
    }
    private var targetReadKey:String {
        [targetCrew,targetChannel,client.connectionEpoch.uuidString,String(client.readyForWork),group.activationEpoch.uuidString].joined(separator:"/")
    }
    private var sourceReadKey:String { targetReadKey + "/" + sourceCrew + "/" + refresh.uuidString }
    private var catalog:CrewdKnowledgeStore.ViewCatalog? { current ? library.linkSourceCatalog(crew:sourceCrew) : nil }
    private var rows:[CrewdKnowledgeStore.ViewCatalogItem] {
        catalog?.items.filter { collection.includes($0.kind) && CrewdKnowledgeTitleSearch.matches($0.title,query:query) } ?? []
    }
    private var chosenSource:CrewdChannelKnowledgeReference? {
        guard current,let item=rows.first(where:{$0.id == chosen}) else { return nil }
        return library.catalogLinkSource(item.id,kind:item.kind,crew:sourceCrew)
    }
    private var linkedViewIDs:Set<String> {
        guard let context=catalog?.context else { return [] }
        return group.linkedViewIDs(in:context)
    }
    private var linkPending:Bool {
        guard let chosenSource else { return false }
        return CrewdPendingKnowledgeLink.inChannel(workspace.durableUnknown,scope:workspace.presentationScope,crew:targetCrew,channel:targetChannel)
            .contains { $0.source.identity == chosenSource.identity }
    }
    private var canLink:Bool {
        current && client.readyForWork && !submitting && !group.writing && !library.busy &&
            group.loaded && !group.loading && group.failure == nil && group.linkRevision != nil &&
            chosenSource != nil && !linkedViewIDs.contains(chosen) && !linkPending
    }
    var body:some View {
        let linked = linkedViewIDs
        VStack(alignment:.leading,spacing:0){
            CrewdPanelHeader(title:"引用已有资料").padding(20)
            Divider()
            ScrollView{VStack(alignment:.leading,spacing:16){
            Picker("源机组",selection:$sourceCrew){ForEach(workspace.crews.compactMap{$0["id"].string},id:\.self){id in Text(workspace.crews.first(where:{$0["id"] == .string(id)})?["title"].string ?? id).tag(id)}}
                .disabled(submitting || group.writing)
            HStack {
                TextField("按标题搜索源机组资料",text:$query).textFieldStyle(.roundedBorder)
                Button("刷新源资料",systemImage:"arrow.clockwise") { refresh=UUID() }.disabled(library.busy || submitting)
            }
            if library.busy { ProgressView("正在读取源资料") }
            if group.loading { ProgressView("正在核对本群引用") }
            sourceStatus
            LazyVStack(alignment:.leading,spacing:4){ForEach(rows){item in
                Button{chosen=item.id}label:{
                    HStack {
                        Text(item.title)
                        Spacer()
                        if linked.contains(item.id) { Text("本群已引用").font(CrewdType.caption).foregroundStyle(.secondary) }
                    }.padding(10).frame(maxWidth:.infinity,alignment:.leading)
                        .background(chosen == item.id ? Theme.Palette.surfaceMuted:Color.clear,in:RoundedRectangle(cornerRadius:6))
                }.buttonStyle(.plain).disabled(submitting || group.writing).accessibilityAddTraits(chosen == item.id ? .isSelected:[])
            }}
            if let error=group.failure {
                CrewdInlineNotice(title:"本群引用尚未确认",detail:error,warning:true)
                Button("刷新本群引用") { Task { if current {try? await group.refresh()} } }.disabled(group.loading || submitting)
            }
            if let failure{CrewdInlineNotice(title:"引用尚未完成",detail:failure,warning:true)}
            CrewdKnowledgeLinkRecovery(workspace:workspace,group:group,crew:targetCrew,channel:targetChannel)
            }.padding(20)}
            Divider()
            HStack{Button("关闭"){dismiss()}.disabled(submitting);Spacer();Button(submitting ? "正在加入…":"引用到本群"){link()}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(!canLink)}.padding(16)
        }.modifier(CrewdAdaptiveFormFrame(width:560,height:580,scrolls:false))
        .onAppear{if targetChannel.isEmpty {targetCrew=group.crew;targetChannel=group.channel;sourceCrew=group.crew}}
        .onDisappear { workspace.closeKnowledgeLinkPicker(owner:owner) }
        .onChange(of:current){_,value in if !value && !targetChannel.isEmpty { dismiss() } }
        .onChange(of:sourceCrew){_,_ in chosen=""; query=""; failure=nil }
        .task(id:sourceReadKey){await loadSource()}
        .task(id:targetReadKey){if current,client.readyForWork {try? await group.refresh()}}
    }
    @ViewBuilder private var sourceStatus:some View {
        if let catalog {
            HStack {
                Text(catalog.next == nil ? "已读完源机组资料，匹配 \(rows.count) 项。" : "已读取的源资料匹配 \(rows.count) 项，仍有后续资料。")
                Spacer()
                if catalog.next != nil {
                    Button(library.catalogStatus == .failed ? "重读后续资料" : "读取后续资料") {
                        Task { await library.loadNextViewCatalogPage() }
                    }.disabled(library.busy || submitting)
                }
            }.font(CrewdType.caption).foregroundStyle(.secondary)
            if library.catalogStatus == .failed { Text("后续资料未能读取。请刷新后再引用。").font(CrewdType.caption).foregroundStyle(.orange) }
        } else if !library.busy {
            let message = library.catalogStatus == .uninitialized ? "源机组资料库尚未初始化。" : "源资料尚未确认。可刷新后继续。"
            Text(message).font(CrewdType.caption).foregroundStyle(.secondary)
        }
    }
    private func loadSource() async {
        guard current,client.readyForWork,!sourceCrew.isEmpty else { return }
        let requested=sourceCrew, key=sourceReadKey
        chosen=""; failure=nil
        do { try await workspace.readKnowledgeLinkSource(owner:owner,crew:targetCrew,channel:targetChannel,source:requested) }
        catch { if current,sourceReadKey == key,!Task.isCancelled { failure=error.localizedDescription } }
    }
    private func link(){
        guard canLink,let source=chosenSource,let revision=group.linkRevision else{return}
        guard let snapshot=workspace.knowledgeLinkDrafts.begin(crew:targetCrew,channel:targetChannel,source:source,revision:revision,linked:true) else{return}
        submitting=true; failure=nil
        Task {
            defer {submitting=false}
            do {
                guard current,client.readyForWork,chosenSource == snapshot.source,group.loaded,!group.loading,
                      group.failure == nil,group.linkRevision == snapshot.revision,!linkedViewIDs.contains(chosen) else {
                    throw CrewdClientError(message:"源资料或原群引用已变化，请重新核对。",kind:.notSubmitted)
                }
                _ = try await group.submit(snapshot)
                workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true)
                if current {dismiss()}
            } catch {
                workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error))
                if current {failure=error.localizedDescription}
            }
        }
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
