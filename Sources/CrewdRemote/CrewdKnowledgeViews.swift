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
    @State private var managedTarget:String?
    @State private var artifactTarget:CrewdArtifactWire.Reference?
    @State private var knowledgeTarget:CrewdManagedTableV2Wire.Target?
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
            if collection == .managedDirectory {
                CrewdManagedDirectoryView(workspace:workspace,store:workspace.managedDirectory,initialObjectID:managedTarget).id(managedTarget ?? "catalog")
            } else if collection == .tables {
                if workspace.client.capabilities.contains("data.table.keyset.v1") {
                    CrewdNativeTablesView(workspace:workspace,store:workspace.knowledge)
                } else {
                    CrewdEmptySurface(title:"表格暂不可用",detail:"当前连接未提供带主键的表格读取。",symbol:"tablecells")
                }
            } else if library {
                CrewdKnowledgeWorkspaceView(workspace:workspace,store:workspace.knowledge,available:workspace.client.capabilities.contains("data"),returnToMessages:{workspace.originalSection="消息"},collection:collection,
                    workStore:workspace.client.capabilities.contains("data.documentWorkLinks") ? workspace.workStore : nil)
                    .id(workspace.crewID + "/" + collection.rawValue)
            }else{CrewdGroupKnowledgeView(workspace:workspace,collection:collection)}
        }
        .tint(.gray)
        .task(id:workspace.currentManagedBusinessNavigation?.id) {
            guard let navigation=workspace.currentManagedBusinessNavigation else {return}
            switch navigation.target.kind {
            case .table,.row,.decision,.risk,.lesson:managedTarget=navigation.target.idValue;collection = .managedDirectory
            case .artifact:artifactTarget=navigation.target.reference
            case .document,.knowledge:knowledgeTarget=navigation.target
            default:return
            }
            workspace.consumeManagedBusinessNavigation(navigation.id)
        }
        .sheet(item:$artifactTarget) {reference in CrewdArtifactPanel(workspace:workspace,initialReference:reference,current:{artifactTarget==reference})}
        .sheet(isPresented:Binding(get:{knowledgeTarget != nil},set:{if !$0{knowledgeTarget=nil}})) {if let knowledgeTarget,let source=knowledgeTarget.source {CrewdManagedKnowledgeTargetView(workspace:workspace,source:source,documentOnly:knowledgeTarget.kind == .document,current:{self.knowledgeTarget?.id==knowledgeTarget.id})}}
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }
    @ViewBuilder private var scopePicker:some View {
        if collection == .tables { Text("机组资料库").foregroundStyle(.secondary) }
        else if collection == .managedDirectory { Text("组织共享 · 本机组可维护").foregroundStyle(.secondary) }
        else { Picker("范围",selection:$library){Text("本群引用").tag(false);Text("机组资料库").tag(true)}.pickerStyle(.menu).labelsHidden().fixedSize(horizontal:true,vertical:false).accessibilityLabel("资料范围") }
    }
    private var collectionPicker:some View{Picker("类型",selection:$collection){Text("文档").tag(CrewdKnowledgeCollection.documents);Text("Agent 页面").tag(CrewdKnowledgeCollection.agentPages);Text("本机组表格").tag(CrewdKnowledgeCollection.tables);Text("组织目录").tag(CrewdKnowledgeCollection.managedDirectory)}.pickerStyle(.menu).labelsHidden().fixedSize(horizontal:true,vertical:false).accessibilityLabel("资料类型")}

}

/// Native placements and free Agent-authored pages share stored blocks, while
/// their navigation stays separate. The selector never executes page code.
enum CrewdKnowledgeCollection: String {
    case all, documents, agentPages, tables, managedDirectory
    func includes(_ kind: CrewdKnowledgeStore.ViewCatalogKind) -> Bool {
        self == .all || (self == .documents && kind == .document) || (self == .agentPages && kind == .web)
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
    @ObservedObject var workspace: CrewdWorkspace
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
    private struct DocumentEditTarget: Identifiable {
        let id = UUID()
        let parent: CrewdDataObjectDetail
    }
    @State private var editTarget: DocumentEditTarget?
    @State private var historyTarget: CrewdDataObjectDetail?
    @State private var compositionTarget: CrewdDataObjectDetail?
    @State private var documentWorkTarget: CrewdDataObjectDetail?
    @State private var creationTarget: CrewdNativeDocumentCreationTarget?
    @State private var previewToken = ""
    @State private var confirmingPreview = false
    var body: some View {
        VStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                HStack { titleSearch; pagePicker; Spacer(); if store.busy { ProgressView().controlSize(.small) }; createDocumentButton; recycleBinButton; reloadButton; systemButton }
                VStack(alignment: .leading, spacing: 8) {
                    titleSearch
                    HStack { pagePicker; Spacer(); if store.busy { ProgressView().controlSize(.small) }; createDocumentButton; recycleBinButton; reloadButton; systemButton }
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
                if let current, let historyTarget,
                   historyTarget.context.authorityId != current.authorityId || historyTarget.context.crewId != current.crewId || historyTarget.context.namespace != current.namespace {
                    self.historyTarget = nil
                }
            }
            if let current, let target = editTarget?.parent,
               target.context.authorityId != current.authorityId || target.context.dbId != current.dbId ||
                target.context.crewId != current.crewId || target.context.namespace != current.namespace ||
                target.context.restoreEpoch != current.restoreEpoch {
                editTarget = nil
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
        .sheet(item:$creationTarget) { target in
            CrewdNativeDocumentCreation(workspace:workspace,target:target,
                presentationCurrent:{ creationTarget?.id == target.id && collection == .documents && !showArchived },
                openSaved:{ source in
                    guard creationTarget?.id == target.id, target.isCurrent(workspace) else { return false }
                    await reload(restorePreferred:false)
                    guard creationTarget?.id == target.id, target.isCurrent(workspace), let context = catalog?.context,
                          CrewdChannelKnowledgeReference(context:context,reference:source.reference).identity == source.identity else { return false }
                    await open(source.reference.id,kind:.document,previouslyOpenedDocument:true)
                    return creationTarget?.id == target.id && target.isCurrent(workspace) && visibleDetail?.object.identity == source.reference.id
                })
        }
        .sheet(isPresented: $showSystem) {
            #if os(macOS)
            ScrollView { systemControls }.frame(minWidth: 520, minHeight: 360)
            #else
            ScrollView { systemControls }
            #endif
        }
        .sheet(item:$editTarget) { target in
            CrewdNativeDocumentEditor(store:store,parent:target.parent,scopeCurrent:{
                editTarget?.id == target.id && (selected.isEmpty || selected == target.parent.object.identity) &&
                    collection == .documents && !showArchived
            },onSaved:{ await reload() })
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
    @ViewBuilder private var createDocumentButton: some View {
        if collection == .documents && !showArchived {
            Button("新建文档",systemImage:"plus") { creationTarget = .init(workspace:workspace,channel:nil) }
                .disabled(!available || !workspace.client.readyForWork || workspace.presentationScope == nil)
        }
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
                if collection == .documents {
                    if store.canPrepareNativeDocumentEdit(detail) {
                        Button("版本记录",systemImage:"clock.arrow.circlepath") { historyTarget = detail }.disabled(store.busy)
                    }
                    if !store.editableDocumentPlacements(detail).isEmpty {
                        Button("编辑文档",systemImage:"pencil") { editTarget = .init(parent:detail) }.disabled(store.busy || store.hasBlockingPending)
                    }
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
            if store.pending.contains(where:{ $0.nativeTable != nil }) {
                Text("有表格保存待核对。请在资料类型中选择表格，查询原回执。").font(.caption)
            }
            ForEach(store.pending.filter { $0.nativeTable == nil }) { intent in
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
    @State private var creationTarget: CrewdNativeDocumentCreationTarget?
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
        .sheet(item:$creationTarget) { target in
            CrewdNativeDocumentCreation(workspace:workspace,target:target,presentationCurrent:{
                creationTarget?.id == target.id && collection == .documents && group.crew == target.crew && group.channel == target.channel
            })
        }
        .sheet(item:$editTarget){ target in
            CrewdNativeDocumentEditor(store:content,parent:target.parent,scopeCurrent:{
                editTarget?.id == target.id && isCurrent(target)
            },onSaved:{await refreshAfterEdit()})
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
        .onChange(of:group.crew+group.channel){_,_ in selected=nil;query="";linking=false;creationTarget=nil;editTarget=nil;historyTarget=nil;compositionTarget=nil;documentWorkTarget=nil;failure=nil}
        .onChange(of:collection){_,_ in selected=nil;query="";creationTarget=nil;editTarget=nil;historyTarget=nil;compositionTarget=nil;documentWorkTarget=nil;content.renderer.revoke()}
        .onChange(of:chosen?.id){_,id in if id == nil{content.renderer.revoke()}}
        .controlSize(.small).tint(.gray)
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }
    private var documentActions:some View {
        HStack(spacing:8){
            Button("引用已有资料",systemImage:"link"){linking=true}
            if collection == .documents {
                Button("新建文档",systemImage:"plus") { creationTarget = .init(workspace:workspace,channel:group.channel) }
                    .disabled(!workspace.client.readyForWork || workspace.presentationScope == nil || group.channel.isEmpty)
            }
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
                            if !content.editableDocumentPlacements(detail).isEmpty {
                                Button("编辑原文",systemImage:"pencil") { editTarget = .init(parent:detail,channel:group.channel,link:chosen.id) }
                                    .disabled(content.busy || content.hasBlockingPending)
                            }
                            if content.canPrepareNativeDocumentEdit(detail) {
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
                if let selected = edit.selectedPlacement {
                    Text("内容块 \(selected.position)").font(CrewdType.label)
                    if let revision = selected.pinnedRevision {
                        Text("修改正文后会从固定版本 \(revision) 保存一份此文档的独立内容。原固定版本仍可引用。")
                            .font(CrewdType.caption).foregroundStyle(.secondary)
                    }
                }
                TextField("标题",text:$title).disabled(working || store.hasBlockingPending)
                TextEditor(text:$bodyText).frame(minHeight:160)
                    .accessibilityLabel(edit.selectedPlacement == nil ? "文档正文" : "所选内容块正文")
                    .disabled(working || store.hasBlockingPending)
                if refreshedBaseline {
                    DisclosureGroup("已重新读取后台版本，保存前可核对") {
                        Text(edit.title).font(CrewdType.label).textSelection(.enabled)
                        ScrollView { Text(edit.body).frame(maxWidth:.infinity,alignment:.leading).textSelection(.enabled) }.frame(maxHeight:120)
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
            } else if !working && scopeCurrent() {
                let placements = store.editableDocumentPlacements(parent)
                if !placements.isEmpty {
                    Text("选择要编辑的内容块。").font(CrewdType.caption)
                    ScrollView {
                        LazyVStack(alignment:.leading,spacing:8) {
                            ForEach(placements) { placement in
                                Button {
                                    working = true
                                    Task { await load(placement.id) }
                                } label: {
                                    if let revision = placement.pinnedRevision {
                                        Text("内容块 \(placement.position) · 固定版本 \(revision)")
                                    } else {
                                        Text("内容块 \(placement.position)")
                                    }
                                }.disabled(store.busy || store.hasBlockingPending)
                            }
                        }
                    }.frame(maxHeight:220)
                }
            }
            if scopeCurrent() {
                ForEach(store.pendingNativeDocumentEdits(viewID:parent.object.identity)) { intent in
                    HStack {
                        Text("原编辑待核对").font(CrewdType.caption)
                        Button("查询原编辑结果") { lookup(intent.id) }.disabled(working || store.busy)
                    }
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
            if store.canPrepareNativeDocumentEdit(parent) { await load(nil) }
            else { working = false }
        }
    }

    private func load(_ placementID: String?) async {
        defer { working = false }
        do {
            guard scopeCurrent() else { throw CrewdKnowledgeStore.Failure.stale }
            let original = try await store.prepareNativeDocumentEdit(parent,placementID:placementID)
            guard scopeCurrent(), !Task.isCancelled else { throw CrewdKnowledgeStore.Failure.stale }
            edit = original; title = original.title; bodyText = original.body; failure = nil
        } catch { failure = "无法确认此内容块的当前版本。请选择仍可读取的机组内人类纯文本内容块。" }
    }

    private func save(_ original: CrewdKnowledgeStore.NativeDocumentEdit) {
        working = true
        Task {
            defer { working = false }
            do {
                guard scopeCurrent() else { throw CrewdKnowledgeStore.Failure.stale }
                let result = try await store.submitNativeDocumentEdit(original,title:title,body:bodyText)
                if result?.phase == .committed {
                    if scopeCurrent() { await onSaved(); if scopeCurrent() { dismiss() } }
                    return
                }
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
                await onSaved()
                if scopeCurrent() { dismiss() }
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

private struct CrewdNativeDocumentCreationTarget: Identifiable {
    let id = UUID()
    let crew: String
    let channel: String?
    let scope: CrewdPresentationJournal.Scope
    let connection: UUID

    @MainActor init?(workspace:CrewdWorkspace,channel:String?) {
        guard !workspace.crewID.isEmpty, let scope = workspace.presentationScope, workspace.client.readyForWork,
              channel == nil || (!workspace.channelID.isEmpty && workspace.channelID == channel) else { return nil }
        crew = workspace.crewID; self.channel = channel; self.scope = scope; connection = workspace.client.connectionEpoch
    }
    @MainActor func isCurrent(_ workspace:CrewdWorkspace) -> Bool {
        !workspace.isRetired && workspace.client.readyForWork && workspace.crewID == crew &&
        workspace.presentationScope == scope && workspace.client.connectionEpoch == connection &&
        (channel == nil || workspace.channelID == channel)
    }
}

private struct CrewdNativeDocumentCreation:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:CrewdKnowledgeStore
    @ObservedObject var group:CrewdChannelKnowledgeStore
    @Environment(\.dismiss) private var dismiss
    let target: CrewdNativeDocumentCreationTarget
    let presentationCurrent: @MainActor () -> Bool
    var openSaved: (@MainActor (CrewdChannelKnowledgeReference) async -> Bool)?
    @State private var saved:CrewdChannelKnowledgeReference?
    @State private var stage:CrewdPresentationJournal.DocumentStage?
    @State private var expectedContext:CrewdDataContext?
    @State private var failure:String?
    @State private var refused=false
    @State private var attempted:Bool?
    @State private var working=true
    @State private var visible=true

    init(workspace:CrewdWorkspace,target:CrewdNativeDocumentCreationTarget,presentationCurrent:@escaping @MainActor () -> Bool,
         openSaved:(@MainActor (CrewdChannelKnowledgeReference) async -> Bool)? = nil) {
        self.workspace=workspace; store=workspace.knowledge; group=workspace.channelKnowledge
        self.target=target;self.presentationCurrent=presentationCurrent;self.openSaved=openSaved
    }

    private var crew:String { target.crew }
    private var channel:String? { target.channel }
    private var key:CrewdResourceDrafts.Key {
        let libraryScope = expectedContext.map { context in
            [context.authorityId,context.dbId,context.namespace,context.restoreEpoch].map { "\($0.utf8.count):\($0)" }.joined()
        }
        return .init(crew:crew,view:.documents,object:channel == nil ? libraryScope ?? "":"",operation:channel == nil ? "knowledge.libraryCreate":"knowledge.create",
                     domain:.document,channel:channel ?? "")
    }
    private var fields:CrewdResourceDrafts.Fields{workspace.resourceDrafts.fields(key)}
    private var current:Bool{visible && presentationCurrent() && target.isCurrent(workspace)}
    private var linkPending:Bool {
        guard let saved,let channel else { return false }
        return CrewdPendingKnowledgeLink.inChannel(workspace.durableUnknown,scope:target.scope,crew:crew,channel:channel)
            .contains { $0.source.identity == saved.identity }
    }
    private var alreadyLinked:Bool {
        guard let saved, let channel, group.loaded, group.crew == crew, group.channel == channel else { return false }
        return group.items.contains { $0.source.identity == saved.identity && $0.state == "ready" }
    }
    private func field(_ path:WritableKeyPath<CrewdResourceDrafts.Fields,String>)->Binding<String>{
        Binding(get:{if let stage{return path == \CrewdResourceDrafts.Fields.title ? stage.title:stage.body};return fields[keyPath:path]},
                set:{value in guard current, stage == nil, !working else { return };var f=fields;f[keyPath:path]=value;workspace.resourceDrafts.edit(key,f,preserveTextBytes:channel == nil)})
    }
    var body:some View {
        VStack(alignment:.leading,spacing:0){
            CrewdPanelHeader(title:channel == nil ? "新建资料库文档":"新建文档").padding(20)
            Divider()
            if current {
                ScrollView { VStack(alignment:.leading,spacing:16) {
                    CrewdFormField(title:"标题"){TextField("给文档起一个名字",text:field(\.title)).disabled(stage != nil || working || store.busy)}
                    CrewdFormField(title:"正文"){
                        TextEditor(text:field(\.body)).font(CrewdType.body).frame(minHeight:200).padding(8)
                            .background(Theme.Palette.canvas,in:RoundedRectangle(cornerRadius:8))
                            .overlay(RoundedRectangle(cornerRadius:8).strokeBorder(Theme.Palette.hairline,lineWidth:0.7))
                            .disabled(stage != nil || working || store.busy)
                    }
                    evidence
                }.padding(20)}
            } else {
                Text("当前机组、群或连接已变化，请从当前页面重新打开。").padding(20).foregroundStyle(.secondary)
            }
            Divider()
            HStack {
                Button("关闭"){visible=false;dismiss()}.disabled(working)
                Spacer()
                if current { primaryAction }
            }.padding(16)
        }.modifier(CrewdAdaptiveFormFrame(width:600,height:680,scrolls:false))
        .task { await reloadStage(refreshGroup:channel != nil);working=false }
        .onDisappear { visible=false }
    }

    @ViewBuilder private var evidence: some View {
        if saved != nil {
            Text(channel == nil ? "原文已保存到机组资料库。" :
                    alreadyLinked ? "已核对当前群引用，可结束本次新建。" : "原文已保存，加入本群后即可在此阅读。")
                .font(CrewdType.label).foregroundStyle(.secondary)
        }
        if let failure { CrewdInlineNotice(title:"需要核对保存状态",detail:failure,warning:true) }
        if let stage, saved == nil, attempted == true, !refused {
            Text("原保存结果待核对。").font(CrewdType.caption).foregroundStyle(.orange)
            Button("查询原保存结果"){run{
                let result=await store.lookupNativeDocument(stage)
                guard current else { return }
                if result == nil {failure=store.status}
                await reloadStage()
            }}.disabled(working || store.busy)
        }
        if let stage, attempted == false {
            Text("原保存尚未送交。可以继续保存，或保留草稿重新准备。").foregroundStyle(.secondary)
            Button("保留草稿重新准备"){revise(stage,discardUnsent:true)}.disabled(working || store.busy)
        }
        if let stage, refused { Button("原保存已明确拒绝，修改后另存"){revise(stage)}.disabled(working || store.busy) }
        if let saved, let channel {
            Button("重新核对已保存原文"){run{await reloadStage()}}.disabled(working || store.busy)
            CrewdKnowledgeLinkRecovery(workspace:workspace,group:group,crew:crew,channel:channel,originalSource:saved) { attempt in
                if current, attempt.linked, attempt.source.identity == self.saved?.identity { _ = finishDraft() }
            }
            if !alreadyLinked {
                Button("仅保留已保存原文，结束本次新建"){_ = finishDraft()}.disabled(working || group.writing || linkPending)
            }
        }
        if saved != nil, channel == nil, failure != nil {
            Button("完成并返回资料库"){_ = finishDraft()}.disabled(working || store.busy)
        }
    }

    @ViewBuilder private var primaryAction: some View {
        if let saved {
            if channel == nil {
                Button("打开已保存文档"){run{await openDocument(saved)}}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(working || store.busy)
            } else {
                Button(alreadyLinked ? "完成":"加入本群"){if alreadyLinked {_ = finishDraft()} else {join()}}
                    .buttonStyle(CrewdPrimaryButtonStyle()).disabled(working || group.writing || group.loading || !group.loaded || store.busy || linkPending)
            }
        } else if stage == nil || attempted == false {
            Button(working ? "正在处理…" : stage == nil ? "保存原文" : "继续保存原文"){save()}
                .buttonStyle(CrewdPrimaryButtonStyle())
                .disabled(working || store.busy || store.hasBlockingPending || !store.canSaveNativeDocument ||
                          (stage?.title ?? fields.title).trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || expectedContext == nil)
        }
    }

    private func run(_ action:@escaping @MainActor () async -> Void) {
        guard current, !working else { return }
        working=true
        Task { @MainActor in
            defer {working=false}
            guard current, !Task.isCancelled else { return }
            await action()
        }
    }
    private func reloadStage(refreshGroup:Bool = false) async {
        guard current else { return }
        await store.refresh()
        guard current, !Task.isCancelled else { return }
        expectedContext=store.workspace?.context
        do {
            stage=try store.nativeDocumentStage(channel:channel)
            saved=nil;refused=false;attempted=nil
            if let stage {
                attempted=try store.nativeDocumentWasAttempted(stage)
                refused=try store.nativeDocumentWasRefused(stage)
                if let result=try store.nativeDocumentResult(stage) {
                    refused=[.rejected,.conflicted].contains(result.phase)
                    if result.phase == .committed,case .object(let view)?=result.created["document"] {
                        saved = .init(context:result.context,reference:view.reference);failure=nil
                    } else if refused { failure=result.reason ?? "后台已明确拒绝此次保存。" }
                } else if refused { failure="后台已明确拒绝此次保存，请重新核对。" }
            }
        } catch {
            expectedContext=nil;saved=nil;attempted=nil;refused=false
            failure="未能核对原文档保存记录。原草稿已保留。"
        }
        if refreshGroup, current { try? await group.refresh() }
    }

    private func save(){
        let existing=stage
        let draft=workspace.resourceDrafts.snapshot(key)
        let title=existing?.title ?? fields.title,body=existing?.body ?? fields.body
        guard let expectedContext,channel != nil || existing != nil || draft != nil else { return }
        let requestID=channel == nil ? existing?.id ?? draft?.request:nil
        run {
            _ = await store.submitNativeDocument(title:title,body:body,channel:channel,requestID:requestID,expectedContext:expectedContext)
            guard current else { return }
            if store.status.contains("原输入保留") { failure=store.status }
            await reloadStage()
        }
    }
    private func revise(_ stage:CrewdPresentationJournal.DocumentStage,discardUnsent:Bool = false){
        guard current else{return}
        do{
            try store.finishNativeDocument(stage,discardUnsent:discardUnsent)
            if !workspace.resourceDrafts.exists(key){
                var original=CrewdResourceDrafts.Fields();original.title=stage.title;original.body=stage.body;workspace.resourceDrafts.edit(key,original,preserveTextBytes:channel == nil)
            } else {
                workspace.resourceDrafts.renew(key,request:stage.id)
            }
            self.stage=nil;saved=nil;refused=false;attempted=nil;failure=nil
            run { await reloadStage() }
        }catch{failure=error.localizedDescription}
    }
    private func finishDraft() -> Bool {
        guard current,let stage else{return false}
        do {
            try store.finishNativeDocument(stage)
            if let draft=workspace.resourceDrafts.snapshot(key),
               draft.fields.title.utf8.elementsEqual(stage.title.utf8), draft.fields.body.utf8.elementsEqual(stage.body.utf8),
               channel != nil || draft.request == stage.id {
                workspace.resourceDrafts.finish(draft,accepted:true)
            }
            visible=false;dismiss();return true
        }catch{failure=error.localizedDescription;return false}
    }
    private func openDocument(_ saved:CrewdChannelKnowledgeReference) async {
        guard current,let openSaved else { return }
        let opened=await openSaved(saved)
        guard current else { return }
        if opened {_ = finishDraft()}
        else {failure="原保存已确认，当前无法打开同一文档。"}
    }
    private func join(){
        guard current,let channel,!alreadyLinked,!linkPending,group.loaded,!group.loading,let saved,let revision=group.linkRevision,
              let snapshot=workspace.knowledgeLinkDrafts.begin(crew:crew,channel:channel,source:saved,revision:revision,linked:true) else{return}
        run {
            do {
                _ = try await group.submit(snapshot)
                workspace.knowledgeLinkDrafts.finish(snapshot,accepted:true)
                if current {_ = finishDraft()}
            } catch {
                workspace.knowledgeLinkDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error))
                if current { failure="原文已保存，未加入本群："+error.localizedDescription }
            }
        }
    }
}

/// The table surface addresses the current crew's real SQLite namespace. Its
/// editor only generates bounded, typed statements and never hosts Agent code.
private struct CrewdNativeTablesView: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var store: CrewdKnowledgeStore
    @State private var catalog: CrewdDataSchemaResult?
    @State private var table: CrewdDataSchemaObject?
    @State private var page: CrewdDataTablePage?
    @State private var firstCatalogPage = true
    @State private var firstRowPage = true
    @State private var request = UUID()
    @State private var editor: EditTarget?
    @State private var feedback: String?
    @State private var loading = false

    private struct Route: Hashable { let crew: String; let observation: UUID; let connection: UUID }
    private struct EditTarget: Identifiable {
        enum Mode {
            case create
            case row(CrewdNativeTable.Model, original: [CrewdDataCell]?)
        }
        let id = UUID()
        let mode: Mode
        let context: CrewdDataContext
    }
    private var route: Route { .init(crew:workspace.crewID,observation:workspace.observationGeneration,connection:workspace.client.connectionEpoch) }
    private var pending: [CrewdKnowledgeStore.Intent] {
        store.pending.filter { $0.nativeTable != nil && $0.context.crewId == workspace.crewID }
    }
    private var selectedModel: CrewdNativeTable.Model? {
        guard let table, let page, page.stableOrder else { return nil }
        guard let model = CrewdNativeTable.Model(table),
              model.columns.map({Array($0.name.utf8)}) == page.columns.map({Array($0.utf8)}) else { return nil }
        return model
    }

    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            HStack {
                Text("表格").font(.headline)
                Spacer()
                if loading || store.busy { ProgressView().controlSize(.small) }
                Button("刷新",systemImage:"arrow.clockwise") { Task { await reload() } }.disabled(loading || store.busy)
                Button("新建表格",systemImage:"plus") {
                    if let context = catalog?.context { editor = .init(mode:.create,context:context) }
                }.disabled(loading || store.busy || store.hasBlockingPending || catalog == nil)
            }
            if let feedback { Text(feedback).font(.caption).foregroundStyle(.secondary).textSelection(.enabled) }
            ForEach(pending) { intent in
                HStack {
                    Text("原表格请求待核对 · " + (intent.nativeTable?.table ?? "")).font(.caption)
                    Spacer()
                    Button("查询原回执") { recover(intent) }.disabled(loading || store.busy)
                }
            }
            if let table {
                selected(table)
            } else if let catalog {
                list(catalog)
            } else if !loading {
                Text("表格尚未读取。").foregroundStyle(.secondary)
            }
            Spacer(minLength:0)
        }
        .padding(16)
        .task(id:route) { await reload() }
        .onChange(of:store.workspace?.context) { old, new in
            if old != nil, new == nil, editor == nil, !loading { Task { await reload() } }
        }
        .sheet(item:$editor,onDismiss:{
            Task { await reload(keepingFeedback:true) }
        }) { target in
            let captured = route
            TableEditor(store:store,target:target,scopeIsCurrent:{
                route == captured && editor?.id == target.id
            }, completed:{ result in
                feedback = result == .exact ? "已核实原回执和当前表格数据。" : "原提交已有回执。当前表格数据已经变化。"
                editor = nil
            })
        }
    }

    private func list(_ catalog: CrewdDataSchemaResult) -> some View {
        let tables = catalog.objects.filter { ["table","view"].contains($0.kind) && (try? CrewdNativeTable.identifier($0.name)) != nil }
        return VStack(alignment:.leading,spacing:8) {
            if tables.isEmpty {
                Text(catalog.next == nil ? "本页没有可读的表格。" : "本页没有表格，可继续查看。").foregroundStyle(.secondary)
            }
            ScrollView {
                LazyVStack(alignment:.leading,spacing:10) {
                    ForEach(Array(tables.enumerated()),id:\.offset) { _, table in
                        Button {
                            self.table = table; self.page = nil; firstRowPage = true; feedback = nil
                            Task { await loadRows(table,context:catalog.context) }
                        } label: {
                            HStack {
                                Image(systemName:table.kind == "view" ? "eye" : "tablecells")
                                Text(table.name); Spacer(); Image(systemName:"chevron.right").font(.caption)
                            }.contentShape(Rectangle())
                        }.buttonStyle(.plain).disabled(loading || store.busy)
                    }
                }
            }
            HStack {
                if !firstCatalogPage { Button("表格首页") { Task { await loadCatalog() } }.disabled(loading || store.busy) }
                if let next = catalog.next { Button("下一页表格") { Task { await loadCatalog(after:next) } }.disabled(loading || store.busy) }
            }
        }
    }

    @ViewBuilder private func selected(_ table: CrewdDataSchemaObject) -> some View {
        HStack {
            Button("表格列表",systemImage:"chevron.left") { request = UUID(); self.table = nil; self.page = nil; loading = false; feedback = nil }
            Spacer()
            Text(table.name).font(.subheadline.weight(.semibold))
            if let model = selectedModel, let page {
                Button("新增行",systemImage:"plus") { editor = .init(mode:.row(model,original:nil),context:page.context) }
                    .disabled(loading || store.busy || store.hasBlockingPending)
            }
        }
        if let page {
            if selectedModel == nil {
                Text(page.truncated && !page.stableOrder ? "只读。此表没有可续页的稳定主键，当前仅显示前一批结果。" :
                        "只读。此表的结构超出当前表格编辑范围。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            ScrollView([.horizontal,.vertical]) {
                LazyVStack(alignment:.leading,spacing:8) {
                    HStack {
                        if selectedModel != nil { Text("操作").font(.caption.weight(.semibold)).frame(width:50,alignment:.leading) }
                        ForEach(Array(page.columns.enumerated()),id:\.offset) { _, name in
                            Text(name).font(.caption.weight(.semibold)).frame(width:128,alignment:.leading)
                        }
                    }
                    Divider()
                    if page.rows.isEmpty { Text("没有数据行。").foregroundStyle(.secondary) }
                    ForEach(Array(page.rows.enumerated()),id:\.offset) { _, row in
                        HStack(alignment:.top) {
                            if let model = selectedModel {
                                if zip(row,model.columns).allSatisfy({ CrewdNativeTable.accepts($0.0,column:$0.1) }) {
                                    Button("编辑") { editor = .init(mode:.row(model,original:row),context:page.context) }
                                        .disabled(loading || store.busy || store.hasBlockingPending).frame(width:50,alignment:.leading)
                                } else { Text("只读").foregroundStyle(.secondary).frame(width:50,alignment:.leading) }
                            }
                            ForEach(Array(row.enumerated()),id:\.offset) { _, cell in
                                Text(CrewdNativeTable.display(cell)).lineLimit(3).frame(width:128,alignment:.leading)
                                    .textSelection(.enabled)
                            }
                        }
                    }
                }.font(.caption)
            }
            HStack {
                if !firstRowPage { Button("数据首页") { Task { await loadRows(table,context:page.context) } }.disabled(loading || store.busy) }
                if let next = page.nextKey {
                    Button("下一页数据") { Task { await loadRows(table,context:page.context,after:next) } }
                        .disabled(loading || store.busy)
                }
            }
        }
    }

    private func reload(keepingFeedback: Bool = false) async {
        let captured = route, token = UUID()
        request = token; loading = true; catalog = nil; table = nil; page = nil
        if !keepingFeedback { feedback = nil }
        await store.refresh(loadViews:false)
        guard !Task.isCancelled, route == captured, request == token else { return }
        loading = false
        await loadCatalog()
    }

    private func loadCatalog(after: String? = nil) async {
        guard !loading else { return }
        let captured = route, token = UUID()
        request = token; loading = true
        defer { if request == token { loading = false } }
        do {
            let result = try await store.nativeTableCatalog(after:after)
            guard !Task.isCancelled, route == captured, request == token else { return }
            catalog = result; firstCatalogPage = after == nil
        } catch {
            if route == captured, request == token { feedback = "无法读取当前表格列表。请刷新后再试。" }
        }
    }

    private func loadRows(_ target: CrewdDataSchemaObject, context: CrewdDataContext, after: [CrewdDataCell]? = nil) async {
        guard !loading else { return }
        let captured = route, token = UUID()
        request = token; loading = true
        defer { if request == token { loading = false } }
        do {
            let result = try await store.nativeTablePage(target,context:context,after:after)
            guard !Task.isCancelled, route == captured, request == token, table?.name.utf8.elementsEqual(target.name.utf8) == true else { return }
            page = result; firstRowPage = after == nil
        } catch {
            if route == captured, request == token {
                page = nil
                feedback = "无法读取当前表格数据。请刷新后重新打开。"
            }
        }
    }

    private func recover(_ intent: CrewdKnowledgeStore.Intent) {
        let captured = route, token = UUID()
        request = token; loading = true; feedback = nil
        Task {
            defer { if request == token { loading = false } }
            do {
                let result = try await store.lookupNativeTable(intent.id,stillCurrent:{ route == captured && request == token })
                guard !Task.isCancelled, route == captured, request == token else { return }
                switch result {
                case .exact: feedback = "已核实原回执和当前表格数据。"
                case .currentDiffers: feedback = "原提交已有回执。当前表格数据已经变化。"
                case nil: feedback = "原请求尚未获得可核实回执。"
                }
            } catch {
                if route == captured, request == token { feedback = "原请求尚未获得完整核对结果。" }
            }
        }
    }

    private struct TableEditor: View {
        @ObservedObject var store: CrewdKnowledgeStore
        let target: EditTarget
        let scopeIsCurrent: @MainActor () -> Bool
        let completed: @MainActor (CrewdKnowledgeStore.NativeTableReadback) -> Void
        @Environment(\.dismiss) private var dismiss
        @State private var tableName = ""
        @State private var columns: [ColumnInput] = [.init(name:"编号",notNull:true),.init(name:"内容")]
        @State private var cells: [CellInput] = []
        @State private var live = false
        @State private var busy = false
        @State private var attempted = false
        @State private var preview: CrewdKnowledgeStore.NativeTablePreview?
        @State private var feedback: String?
        private struct ColumnInput: Identifiable {
            let id = UUID()
            var name: String
            var type = CrewdNativeTable.ColumnType.text
            var notNull = false
        }
        private struct CellInput {
            var value: String
            var null: Bool
        }
        private var title: String {
            switch target.mode {
            case .create: "新建表格"
            case .row(_, let original): original == nil ? "新增数据行" : "编辑数据行"
            }
        }
        var body: some View {
            NavigationStack {
                ScrollView {
                    VStack(alignment:.leading,spacing:14) {
                        if let preview {
                            Text(preview.change.witness.table).font(.headline)
                            Text(preview.preview.schemaChanged ? "将创建表格。" : "将更改一行数据。")
                            if let row = preview.change.row {
                                ForEach(Array(row.enumerated()),id:\.offset) { index, cell in
                                    VStack(alignment:.leading,spacing:2) {
                                        Text(preview.change.witness.columns[index]).font(.caption.weight(.semibold))
                                        if cell == .null { Text("NULL").foregroundStyle(.secondary) }
                                        else { Text(CrewdNativeTable.display(cell)).textSelection(.enabled) }
                                    }
                                }
                            } else {
                                Text(preview.change.statement.sql).font(.system(.caption,design:.monospaced)).textSelection(.enabled)
                            }
                            Text("确认时会保存恢复快照。").foregroundStyle(.secondary)
                            Button("确认保存") { confirm(preview) }.disabled(busy || attempted)
                            if !attempted { Button("返回修改") { self.preview = nil; feedback = nil }.disabled(busy) }
                        } else {
                            editorFields.disabled(busy || attempted)
                            Button("核对变更") { makePreview() }.disabled(busy || attempted)
                        }
                        if busy { ProgressView().controlSize(.small) }
                        if let feedback { Text(feedback).foregroundStyle(.orange).font(.caption) }
                    }.padding(20).frame(maxWidth:560)
                }
                .navigationTitle(title)
                .toolbar { Button("关闭") { dismiss() } }
            }
            #if os(macOS)
            .frame(minWidth:480,minHeight:360)
            #endif
            .onAppear {
                live = true
                if case let .row(model,original) = target.mode, cells.isEmpty {
                    cells = model.columns.enumerated().map { index, column in
                        let old = original?[index]
                        return .init(value:old == .null ? "" : old.map(CrewdNativeTable.display) ?? "",null:old == .null)
                    }
                }
            }
            .onDisappear { live = false }
        }

        @ViewBuilder private var editorFields: some View {
            switch target.mode {
            case .create:
                TextField("表格名称",text:$tableName)
                ForEach(Array(columns.indices),id:\.self) { index in
                    VStack(alignment:.leading,spacing:6) {
                        TextField(index == 0 ? "主键列名称" : "列名称",text:$columns[index].name)
                        Picker("数据类型",selection:$columns[index].type) {
                            ForEach(CrewdNativeTable.ColumnType.allCases.filter { index > 0 || $0 != .real }) { type in
                                Text(typeName(type)).tag(type)
                            }
                        }
                        if index == 0 { Text("主键唯一，保存后不能从表格编辑器修改。").font(.caption) }
                        else {
                            Toggle("必填",isOn:$columns[index].notNull)
                            Button("移除此列",role:.destructive) { columns.remove(at:index) }
                        }
                    }
                }
                Button("添加列") { columns.append(.init(name:"")) }.disabled(columns.count >= 12)
            case .row(let model, let original):
                if cells.count == model.columns.count {
                    ForEach(Array(model.columns.indices),id:\.self) { index in
                        let column = model.columns[index]
                        VStack(alignment:.leading,spacing:5) {
                            Text(column.name + " · " + typeName(column.type)).font(.caption)
                            TextField(column.name,text:$cells[index].value,axis:.vertical)
                                .disabled(cells[index].null || (index == model.keyIndex && original != nil))
                            if !column.notNull { Toggle("空值（NULL）",isOn:$cells[index].null) }
                        }
                    }
                }
            }
        }
        private func typeName(_ type: CrewdNativeTable.ColumnType) -> String {
            switch type { case .text: "文字"; case .integer: "整数"; case .real: "小数" }
        }
        @MainActor private func current() -> Bool { live && scopeIsCurrent() && !Task.isCancelled }
        private func makePreview() {
            guard current(), !busy else { return }
            busy = true; feedback = nil
            Task {
                defer { busy = false }
                do {
                    let change: CrewdKnowledgeStore.NativeTableChange
                    switch target.mode {
                    case .create:
                        change = try store.newNativeTable(.init(name:tableName,
                            columns:columns.map { .init(name:$0.name,type:$0.type,notNull:$0.notNull) }),context:target.context)
                    case .row(let model, let original):
                        guard cells.count == model.columns.count else { return }
                        let row = try zip(cells,model.columns).map { try CrewdNativeTable.cell($0.0.value,null:$0.0.null,column:$0.1) }
                        change = try store.saveNativeTableRow(model,row:row,original:original,context:target.context)
                    }
                    let value = try await store.previewNativeTable(change,stillCurrent:current)
                    if current() { preview = value }
                } catch {
                    if current() { feedback = "无法核对此次变更。请检查字段、主键和当前表格状态。" }
                }
            }
        }
        private func confirm(_ staged: CrewdKnowledgeStore.NativeTablePreview) {
            guard current(), !busy, !attempted else { return }
            busy = true; attempted = true; feedback = nil
            Task {
                defer { busy = false }
                do {
                    let result = try await store.confirmNativeTable(staged,stillCurrent:current)
                    guard current() else { return }
                    if let result { completed(result) }
                    else { feedback = "确认结果尚未核实。请关闭并查询原请求。" }
                } catch {
                    if current() { feedback = "此次保存尚未核实。请关闭并查看原请求或刷新表格。" }
                }
            }
        }
    }
}

/// Open the exact D that the committed structured H captured, using the
/// same directory scope and detail renderer as ordinary navigation.
struct CrewdAskCapturedDecisionLink: View {
    @ObservedObject var workspace:CrewdWorkspace
    let source:CrewdStructuredAskWire.Review
    @State private var showing=false
    var body:some View {
        if let capture=source.capture {
            Button("查看决定 " + capture.decisionId,systemImage:"arrow.up.right.square") { showing=true }
                .sheet(isPresented:$showing) {
                    NavigationStack {
                        CrewdManagedDirectoryView(workspace:workspace,store:workspace.managedDirectory,fromAsk:source)
                            .navigationTitle("组织决定 " + capture.decisionId)
                            .toolbar {
                                ToolbarItem(placement:.cancellationAction) { Button("关闭") { showing=false } }
                            }
                    }
                }
        }
    }
}

struct CrewdManagedDirectoryView: View {
    typealias Wire = CrewdManagedDirectoryWire
    typealias Store = CrewdManagedDirectoryStore
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject private var client: CrewdClient
    @ObservedObject var store: Store
    private let linkedAsk: CrewdStructuredAskWire.Review?
    private let initialObjectID:String?

    init(workspace:CrewdWorkspace,store:Store,fromAsk:CrewdStructuredAskWire.Review? = nil,initialObjectID:String?=nil) {
        self.workspace=workspace
        self.client=workspace.client
        self.store=store
        self.linkedAsk=fromAsk;self.initialObjectID=initialObjectID
        let section:SectionMode
        switch initialObjectID?.first {case "D":section = .decisions;case "R":section = .risks;case "L":section = .lessons;default:section=fromAsk == nil ? .tables:.decisions}
        self._section=State(initialValue:section)
        self._initialDecision=State(initialValue:initialObjectID ?? fromAsk?.capture?.decisionId)
    }
    enum SectionMode: Hashable {
        case tables, decisions, risks, lessons
        var riskLesson: Bool { self == .risks || self == .lessons }
    }
    @State private var showArtifacts=false
    @State private var section: SectionMode = .tables
    @State private var shownSection: SectionMode?
    @State private var scope: Store.BrowseScope?
    @State private var decisions: [Wire.Decision] = []
    @State private var decision: Wire.Decision?
    @State private var decisionSource: CrewdStructuredAskWire.Review?
    @State private var initialDecision: String?
    @State private var nextDecision: String?
    @State private var riskLessonItems: [Wire.RiskLessonEntry] = []
    @State private var riskLessonDetail: Wire.RiskLessonEntry?
    @State private var nextRiskLesson: String?
    @State private var linkedRiskLesson: RiskLessonNavigation?
    @State private var tables: [Wire.Table] = []
    @State private var table: Wire.Table?
    @State private var rows: [Wire.Row] = []
    @State private var nextTable: String?
    @State private var nextRow: String?
    @State private var pending: [Store.Pending] = []
    @State private var editor: EditorTarget?
    @State private var ticket = UUID()
    @State private var busy = false
    @State private var problem: String?
    @State private var notice: String?
    @State private var presentationRoute: PresentationRoute?
    @State private var queuedInvalidation: Store.Invalidation?
    @State private var refreshWake: UUID?

    struct PresentationRoute: Equatable {
        let authority: UUID
        let crew: String
        let section: SectionMode
    }
    enum ReloadKind: Equatable { case ordinary, reconnect, invalidation }
    struct Route: Equatable, Hashable {
        let crew: String
        let observation: UUID
        let connection: UUID
        let journalReady: Bool
        let section: SectionMode
        let authority: UUID
        var presentation: PresentationRoute { .init(authority:authority,crew:crew,section:section) }
    }
    struct RiskLessonNavigation {
        let id: String
        let origin: Route
        let section: SectionMode
    }
    private var route: Route {
        .init(crew:workspace.crewID,observation:workspace.observationGeneration,
              connection:workspace.client.connectionEpoch,
              journalReady:workspace.originalJournalReady && workspace.client.readyForWork,
              section:visibleSection,authority:workspace.authorityRecordID)
    }
    private var decisionsAvailable: Bool { client.capabilities.contains(Wire.decisionsCapability) }
    private var riskLessonsAvailable: Bool { client.capabilities.contains(Wire.riskLessonsCapability) }
    private var visibleSection: SectionMode {
        if !client.readyForWork { return section }
        if section.riskLesson && !riskLessonsAvailable { return .tables }
        if section == .decisions && !decisionsAvailable { return .tables }
        return section
    }
    private var selectedID: String? {
        if route.section.riskLesson { return riskLessonDetail?.id }
        return route.section == .decisions ? decision?.id : table?.id
    }
    private var presentsCurrentScope: Bool {
        guard let active = scope?.origin else { return false }
        return route.journalReady && presentationRoute == route.presentation && shownSection == route.section &&
            active.observation == route.observation &&
            active.connection == route.connection && Wire.exact(active.crew,route.crew)
    }
    enum RiskLessonMode {
        case registerRisk, updateRisk(Wire.Risk), assignRisk(Wire.Risk), reviewRisk(Wire.Risk)
        case riskStatus(Wire.Risk,Wire.RiskStatus), occurrence(Wire.Risk)
        case recordLesson, confirmLesson(Wire.Lesson), staleLesson(Wire.Lesson)

        var title: String {
            switch self {
            case .registerRisk: "登记风险"
            case .updateRisk(let item): "编辑 " + item.id
            case .assignRisk(let item): "转交 " + item.id
            case .reviewRisk(let item): "复核 " + item.id
            case .riskStatus(let item,let status): status.label + " " + item.id
            case .occurrence(let item): "登记 " + item.id + " 已发生"
            case .recordLesson: "记录教训"
            case .confirmLesson(let item): "确认 " + item.id + " 仍适用"
            case .staleLesson(let item): "将 " + item.id + " 标为过时"
            }
        }
        var confirmLesson: Bool {
            if case .confirmLesson = self { return true }
            return false
        }
    }
    struct EditorTarget: Identifiable {
        enum Mode {
            case create, add(Wire.Table), edit(Wire.Table, Wire.Row)
            case recordDecision, replaceDecision(Wire.Decision), withdrawDecision(Wire.Decision)
            case riskLesson(RiskLessonMode)
        }
        let id = UUID()
        let mode: Mode
        let scope: Store.BrowseScope
        let route: Route
        var title: String {
            switch mode {
            case .create: "新建组织表"
            case .add: "新增记录"
            case .edit: "编辑记录"
            case .recordDecision: "记录决定"
            case .replaceDecision(let old): "取代 " + old.id
            case .withdrawDecision(let old): "撤回 " + old.id
            case .riskLesson(let operation): operation.title
            }
        }
        var riskLessonMode: RiskLessonMode? {
            if case .riskLesson(let operation) = mode { return operation }
            return nil
        }
        var isWithdrawal: Bool {
            if case .withdrawDecision = mode { return true }
            return false
        }
        var isDecision: Bool {
            switch mode {
            case .recordDecision, .replaceDecision, .withdrawDecision: true
            case .create, .add, .edit, .riskLesson: false
            }
        }
    }

    var body: some View {
        Group {
            if route.section == .tables && client.capabilities.contains(CrewdManagedTableV3Wire.capability) {
                VStack(alignment:.leading,spacing:12){header.padding(.horizontal,16);CrewdManagedTableV3View(workspace:workspace,store:workspace.managedTablesV3,initialID:initialObjectID)}
            } else if client.readyForWork && !client.capabilities.contains(Wire.capability) {
                CrewdEmptySurface(title:"组织目录暂不可用",detail:"当前连接尚未提供组织目录。",symbol:"list.bullet.rectangle")
            } else if route.section == .tables && client.capabilities.contains(CrewdManagedTableV2Wire.capability) {
                VStack(alignment:.leading,spacing:12) {header.padding(.horizontal,16);CrewdManagedTableV2View(workspace:workspace,store:workspace.managedTablesV2,initialID:initialObjectID)}
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        header
                        if let problem { Text(problem).foregroundStyle(.red).textSelection(.enabled) }
                        if presentsCurrentScope {
                            if let notice { Text(notice).foregroundStyle(.secondary).textSelection(.enabled) }
                            if !pending.isEmpty { pendingPanel }
                            if let scope,store.supportsMessageSources {
                                CrewdManagedMessageHistoryView(workspace:workspace,store:store,scope:scope,current:{presentsCurrentScope})
                            }
                            if route.section.riskLesson {
                                if let riskLessonDetail { riskLessonObject(riskLessonDetail) } else { riskLessonCatalog }
                            } else if route.section == .decisions {
                                if let decision { decisionDetail(decision) } else { decisionLog }
                            } else if let table { records(table) } else { catalog }
                        }
                    }
                    .frame(maxWidth:760,alignment:.leading)
                    .padding(16)
                    .frame(maxWidth:.infinity)
                }
            }
        }
        .task(id:route) {
            let captured = route
            if captured.section == .tables && (client.capabilities.contains(CrewdManagedTableV3Wire.capability) || client.capabilities.contains(CrewdManagedTableV2Wire.capability)) {
                presentationRoute=captured.presentation;shownSection=captured.section;busy=false
                do {
                    let active:Store.BrowseScope
                    if client.capabilities.contains(CrewdManagedTableV3Wire.capability){active=try await workspace.managedTablesV3.scope(stillCurrent:{route==captured})}
                    else{active=try await store.scope(stillCurrent:{route==captured})}
                    if route==captured{scope=active}
                }catch{if route==captured{scope=nil;problem=error.localizedDescription}}
                return
            }
            let link = linkedRiskLesson
            linkedRiskLesson = nil
            let preserve = presentationRoute == captured.presentation
            presentationRoute = captured.presentation
            let matches = link.map {
                $0.section == captured.section && $0.origin.authority == captured.authority &&
                    $0.origin.observation == captured.observation && $0.origin.connection == captured.connection &&
                    Wire.exact($0.origin.crew,captured.crew)
            } ?? false
            if preserve && !captured.journalReady {
                ticket = UUID()
                busy = false
                return
            }
            let entry = captured.journalReady ? initialDecision:nil
            await reload(captured,target:entry ?? (matches ? link?.id : (preserve ? selectedID:nil)),
                         reset:!preserve,kind:preserve ? .reconnect:.ordinary)
        }
        .onChange(of:store.invalidation) { _, change in
            guard let change, store.owns(change) else { return }
            queuedInvalidation = change
            scheduleInvalidation()
        }
        .onChange(of:editor?.id) { _,_ in scheduleInvalidation() }
        .onChange(of:busy) { _,_ in scheduleInvalidation() }
        .onDisappear { ticket = UUID(); editor = nil; refreshWake = nil; queuedInvalidation = nil }
        .sheet(isPresented:$showArtifacts) {
            CrewdArtifactPanel(workspace:workspace,current:{showArtifacts && presentsCurrentScope})
        }
        .sheet(item: $editor) { target in
            Editor(mode:target.mode,scope:target.scope,heading:target.title, allowRiskReferences:riskLessonsAvailable, riskOwners:workspace.sessions,
                   current: { editor?.id == target.id && route == target.route && scope == target.scope &&
                      (!currentCrewFrozen || target.riskLessonMode?.confirmLesson == true) },
                   submit: { mutation, current in
                try await store.submit(mutation, scope: target.scope, stillCurrent: current)
            }, complete: { result in
                guard editor?.id == target.id && route == target.route else { return }
                editor = nil
                if let occurrence = result.receipt.occurrence {
                    notice = "已登记 " + occurrence.riskId + " 发生，并记录教训 " + occurrence.lessonId + "。"
                } else {
                    notice = "已保存 " + (result.receipt.object.id ?? "组织对象") + "。"
                }
                if let item = result.receipt.object.riskLessonEntry {
                    showRiskLesson(item.id)
                } else {
                    let selected = target.isDecision ? result.receipt.object.decision?.id : table?.id
                    Task { await reload(target.route,target:selected,reset:false) }
                }
            }, refreshPending: {
                guard route == target.route && scope == target.scope else { return }
                refreshPending(target.scope)
            })
        }
    }
    private var header: some View {
        VStack(alignment:.leading,spacing:6) {
            HStack {
                Text("组织目录").font(.headline)
                if client.capabilities.contains(CrewdArtifactWire.capability) {
                    Button("产物目录",systemImage:"link") {showArtifacts=true}.disabled(!presentsCurrentScope)
                }
                if busy { ProgressView().controlSize(.small) }
                Spacer()
                Button("刷新",systemImage:"arrow.clockwise") {
                    let captured = route, selected = selectedID
                    Task { await reload(captured, target: selected, reset: false) }
                }.disabled(busy)
            }
            if decisionsAvailable || riskLessonsAvailable {
                Picker("组织目录内容",selection:$section) {
                    Text("组织表").tag(SectionMode.tables)
                    if decisionsAvailable { Text("决定日志").tag(SectionMode.decisions) }
                    if riskLessonsAvailable {
                        Text("风险板").tag(SectionMode.risks)
                        Text("教训库").tag(SectionMode.lessons)
                    }
                }.pickerStyle(.segmented).disabled(editor != nil)
            }
            Text(directoryDescription)
                .font(.caption).foregroundStyle(.secondary)
        }
    }
    private var directoryDescription: String {
        switch route.section {
        case .tables: "其他机组的组织表可阅读。本机组维护自己的表和记录。"
        case .decisions: "全组织可读。本机组可以记录、取代或撤回自己的决定。"
        case .risks: "全组织可读。本机组维护自己的风险。复核到期会在这里显示。"
        case .lessons: "全组织可读。可以确认仍然适用的教训。"
        }
    }
    private var catalog: some View {
        VStack(alignment:.leading,spacing:10) {
            Button("新建组织表",systemImage:"plus") { open(.create) }
                .disabled(busy || scope == nil || currentCrewFrozen || blocked("createTable"))
            if tables.isEmpty && !busy && scope != nil {
                Text("组织目录还没有表。").foregroundStyle(.secondary)
            }
            ForEach(tables) { item in
                Button {
                    let captured = route
                    Task { await reload(captured, target: item.id, reset: false) }
                } label: {
                    HStack {
                        VStack(alignment:.leading,spacing:3) {
                            Text(item.title).foregroundStyle(.primary)
                            Text(item.id + " · " + owner(item.crewId) + (item.frozen ? " · 已归档" : ""))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Image(systemName:"chevron.right").font(.caption)
                    }.contentShape(Rectangle())
                }
                .buttonStyle(.plain).disabled(busy)
                Divider()
            }
            if let nextTable {
                Button("更多组织表") { loadMore(after: nextTable, table: nil) }.disabled(busy)
            }
        }
    }
    private func records(_ selected: Wire.Table) -> some View {
        VStack(alignment:.leading,spacing:10) {
            artifactSubject(.table,selected.id,selected.crewId,selected.frozen)
            Button("返回组织目录",systemImage:"chevron.backward") {
                let captured = route
                Task { await reload(captured, target: nil, reset: false) }
            }.disabled(busy)
            Text(selected.title).font(.title3)
            Text(selected.id + " · " + owner(selected.crewId) + (selected.frozen ? " · 已归档，只读" : ""))
                .font(.caption).foregroundStyle(.secondary)
            if canWrite(selected) {
                Button("新增记录",systemImage:"plus") { open(.add(selected)) }
                    .disabled(busy || blocked(selected.id))
            }
            if rows.isEmpty && !busy { Text("这张组织表还没有记录。").foregroundStyle(.secondary) }
            ForEach(rows) { row in
                VStack(alignment:.leading,spacing:6) {
                    HStack {
                        Text(row.id).font(.subheadline)
                        Spacer()
                        if canWrite(selected) && !row.frozen {
                            Button("编辑",systemImage:"pencil") { open(.edit(selected,row)) }
                                .disabled(busy || blocked(row.id))
                        }
                    }
                    messageSource(row.id)
                    artifactSubject(.row,row.id,row.crewId,row.frozen)
                    ForEach(selected.fields.indices,id:\.self) { index in
                        LabeledContent(selected.fields[index].name) {
                            Text(row.values[index].display).textSelection(.enabled)
                        }
                    }
                }.padding(.vertical,4)
                Divider()
            }
            if let nextRow {
                Button("更多记录") { loadMore(after: nextRow, table: selected.id) }.disabled(busy)
            }
        }
    }
    private var decisionLog: some View {
        VStack(alignment:.leading,spacing:10) {
            Button("记录决定",systemImage:"plus") { open(.recordDecision) }
                .disabled(busy || scope == nil || currentCrewFrozen || blocked("recordDecision"))
            if decisions.isEmpty && !busy { Text("还没有组织决定。").foregroundStyle(.secondary) }
            ForEach(decisions) { item in
                Button {
                    showDecision(item.id)
                } label: {
                    HStack {
                        VStack(alignment:.leading,spacing:3) {
                            Text(item.draft.question).foregroundStyle(.primary)
                            Text(item.id + " · " + item.status.label + " · " + owner(item.crewId) + " · " + author(item.author))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Image(systemName:"chevron.right").font(.caption)
                    }.contentShape(Rectangle())
                }.buttonStyle(.plain).disabled(busy)
                Divider()
            }
            if let nextDecision {
                Button("更早的决定") { loadMore(after:nextDecision,table:nil) }.disabled(busy)
            }
        }
    }

    private func decisionDetail(_ item: Wire.Decision) -> some View {
        VStack(alignment:.leading,spacing:12) {
            Button("返回决定日志",systemImage:"chevron.backward") { showDecision(nil) }.disabled(busy)
            Text(item.draft.question).font(.title3).textSelection(.enabled)
            let archived = item.frozen ? " · 已归档，只读" : ""
            let decisionSummary: String = "\(item.id) · \(item.status.label) · \(owner(item.crewId))\(archived)"
            Text(decisionSummary)
                .font(.caption).foregroundStyle(.secondary)
            messageSource(item.id)
            artifactSubject(.decision,item.id,item.crewId,item.frozen)
            LabeledContent("决定者",value:author(item.author))
            LabeledContent("记录时间",value:item.createdAt)
            if let decisionSource, let capture=decisionSource.capture {
                LabeledContent("拍板来源",value:decisionSource.ask + " · " + capture.source.label + " · 选项 " + capture.optionId)
                    .textSelection(.enabled)
                if let reason=capture.humanReason { decisionText("拍板时填写的理由",reason) }
            }
            decisionText("选择",item.draft.choice)
            decisionText("理由",item.draft.rationale)
            if let alternatives = item.draft.alternatives { decisionText("其他方案",alternatives) }
            if let reversibility = item.draft.reversibility { LabeledContent("可逆性",value:reversibility.label) }
            Text("关于").font(.subheadline)
            ForEach(item.draft.about.indices,id:\.self) { index in
                let link = item.draft.about[index]
                LabeledContent(link.kind.label,value:link.id).textSelection(.enabled)
            }
            if let predecessor = item.supersedes {
                Button("查看被取代的 " + predecessor,systemImage:"arrow.backward") { showDecision(predecessor) }
                    .disabled(busy)
            }
            if let successor = item.supersededBy {
                Button("查看后续决定 " + successor,systemImage:"arrow.forward") { showDecision(successor) }
                    .disabled(busy)
            }
            if let withdrawal = item.withdrawal {
                decisionText("撤回理由",withdrawal.reason)
                LabeledContent("撤回记录",value:author(withdrawal.author) + " · " + withdrawal.at)
            }
            if item.canChange(crew:route.crew) && !currentCrewFrozen {
                HStack {
                    Button("用新决定取代",systemImage:"arrow.triangle.swap") { open(.replaceDecision(item)) }
                    Button("撤回决定",systemImage:"xmark.circle",role:.destructive) { open(.withdrawDecision(item)) }
                }.disabled(busy || blocked(item.id))
            }
        }
    }
    @ViewBuilder private func artifactSubject(_ kind:CrewdArtifactWire.SubjectKind,_ id:String,_ crew:String,_ frozen:Bool)->some View {
        CrewdArtifactSubjectEntry(workspace:workspace,subject:.init(kind:kind,id:id),crew:crew,frozen:frozen,current:{objectCurrent(kind,id)})
        CrewdManagedLinksEntry(workspace:workspace,source:.init(kind:kind.rawValue,id:id),current:{objectCurrent(kind,id)})
    }
    private func objectCurrent(_ kind:CrewdArtifactWire.SubjectKind,_ id:String)->Bool {
        guard presentsCurrentScope else{return false}
        switch kind {
        case .table:return table?.id==id
        case .row:return rows.contains{$0.id==id}
        case .decision:return decision?.id==id
        case .risk,.lesson:return riskLessonDetail?.id==id
        default:return false
        }
    }
    @ViewBuilder private func messageSource(_ id: String) -> some View {
        if let scope, store.supportsMessageSources {
            CrewdManagedMessageSourceView(workspace:workspace,store:store,scope:scope,objectID:id,
                current:{ self.scope == scope && presentsCurrentScope })
        }
    }
    private func decisionText(_ title: String, _ value: String) -> some View {
        VStack(alignment:.leading,spacing:4) {
            Text(title).font(.subheadline)
            Text(value).textSelection(.enabled)
        }
    }
    private func author(_ value: Wire.DecisionAuthor) -> String {
        switch value.kind {
        case "human": "人"
        case "systemDefault": "系统执行默认方案"
        default: "Agent " + (value.sessionId ?? "")
        }
    }
    private func showDecision(_ id: String?) {
        let captured = route
        Task { await reload(captured,target:id,reset:false) }
    }

    private var riskLessonCatalog: some View {
        TimelineView(.periodic(from:.now,by:60)) { timeline in
            VStack(alignment:.leading,spacing:10) {
                let lessons = route.section == .lessons
                Button(lessons ? "记录教训":"登记风险",systemImage:"plus") {
                    open(.riskLesson(lessons ? .recordLesson:.registerRisk))
                }.disabled(busy || scope == nil || currentCrewFrozen || blocked(lessons ? "recordLesson":"recordRisk"))
                if riskLessonItems.isEmpty && !busy {
                    Text(lessons ? "还没有组织教训。":"还没有组织风险。").foregroundStyle(.secondary)
                }
                let due = riskLessonItems.filter { entry in
                    if case .risk(let risk) = entry { return risk.due(at:timeline.date) }
                    return false
                }
                if !due.isEmpty {
                    Text("已到复核日期（已加载）").font(.subheadline).foregroundStyle(.orange)
                    ForEach(due) { entry in riskLessonRow(entry,due:true,at:timeline.date) }
                    Text("其他风险").font(.subheadline)
                }
                let dueIDs = Set(due.map(\.id))
                ForEach(riskLessonItems.filter { !dueIDs.contains($0.id) }) { entry in
                    riskLessonRow(entry,due:false,at:timeline.date)
                }
                if let nextRiskLesson {
                    Button("更早的记录") { loadMore(after:nextRiskLesson,table:nil) }.disabled(busy)
                }
            }
        }
    }
    private func riskLessonRow(_ entry: Wire.RiskLessonEntry, due: Bool, at now: Date) -> some View {
        let title: String
        let detail: String
        let overdue: Bool
        switch entry {
        case .risk(let risk):
            title = risk.draft.summary
            overdue = due && (Wire.managedTime(risk.nextReviewAt).map { now.timeIntervalSince($0) > 7 * 86_400 } ?? false)
            let review = overdue ? " · 复核逾期超过七天" : (due ? " · 待复核" : "")
            detail = risk.status.label + " · 影响" + risk.draft.impact.label + review
        case .lesson(let lesson):
            title = lesson.draft.lesson
            detail = lesson.status.label + (lesson.draft.learnedFrom.isEmpty ? " · 未附来源":"")
            overdue = false
        }
        return Button {
            showRiskLesson(entry.id)
        } label: {
            HStack {
                VStack(alignment:.leading,spacing:3) {
                    Text(title).foregroundStyle(.primary)
                    Text(entry.id + " · " + detail + " · " + owner(entry.crewId))
                        .font(.caption).foregroundStyle(overdue ? Color.red : Color.secondary)
                }
                Spacer()
                Image(systemName:"chevron.right").font(.caption)
            }.contentShape(Rectangle())
        }.buttonStyle(.plain).disabled(busy)
    }
    @ViewBuilder private func riskLessonObject(_ entry: Wire.RiskLessonEntry) -> some View {
        Button(route.section == .lessons ? "返回教训库":"返回风险板",systemImage:"chevron.backward") {
            let captured = route
            Task { await reload(captured,target:nil,reset:false) }
        }.disabled(busy)
        messageSource(entry.id)
        switch entry {
        case .risk(let risk): riskDetail(risk)
        case .lesson(let lesson): lessonDetail(lesson)
        }
    }
    private func riskDetail(_ risk: Wire.Risk) -> some View {
        VStack(alignment:.leading,spacing:10) {
            artifactSubject(.risk,risk.id,risk.crewId,risk.frozen)
            Text(risk.draft.summary).font(.title3).textSelection(.enabled)
            let archived = risk.frozen ? " · 已归档，只读" : ""
            let riskSummary: String = "\(risk.id) · \(risk.status.label) · \(owner(risk.crewId))\(archived)"
            Text(riskSummary).font(.caption).foregroundStyle(.secondary)
            LabeledContent("负责人",value:author(risk.owner))
            LabeledContent("记录者",value:author(risk.author))
            LabeledContent("影响",value:risk.draft.impact.label)
            if let likelihood = risk.draft.likelihood { LabeledContent("可能性",value:likelihood.label) }
            if risk.openForReview {
                if let date = Wire.managedTime(risk.nextReviewAt) {
                    LabeledContent("下次复核") { Text(date,format:.dateTime) }
                } else { LabeledContent("下次复核",value:risk.nextReviewAt) }
            }
            if let mitigation = risk.draft.mitigation { decisionText("缓解办法",mitigation) }
            if let reason = risk.statusReason { decisionText("状态说明",reason) }
            referenceList("关于",risk.draft.about)
            referenceList("缓解它的计划或决定",risk.draft.mitigatedBy)
            if let lesson = risk.becameLessonId {
                Button("查看关联教训 " + lesson,systemImage:"arrow.right") { showRiskLesson(lesson) }
                    .disabled(busy)
            }
            if !risk.frozen && !currentCrewFrozen && Wire.exact(risk.crewId,route.crew) && risk.status != .closed {
                VStack(alignment:.leading,spacing:8) {
                    if risk.openForReview {
                        Button("编辑风险",systemImage:"pencil") { open(.riskLesson(.updateRisk(risk))) }
                        Button("转交负责人",systemImage:"person.2") { open(.riskLesson(.assignRisk(risk))) }
                        if risk.owner.kind == "human" {
                            Button("记录本次复核",systemImage:"checkmark.circle") { open(.riskLesson(.reviewRisk(risk))) }
                        }
                        if risk.status == .open {
                            Button("标为已缓解") { open(.riskLesson(.riskStatus(risk,.mitigated))) }
                        } else {
                            Button("重新打开风险") { open(.riskLesson(.riskStatus(risk,.open))) }
                        }
                        Button("登记已发生并记录教训") { open(.riskLesson(.occurrence(risk))) }
                    }
                    Button("关闭风险",role:.destructive) { open(.riskLesson(.riskStatus(risk,.closed))) }
                }.disabled(busy || blocked(risk.id))
            }
        }
    }
    private func lessonDetail(_ lesson: Wire.Lesson) -> some View {
        VStack(alignment:.leading,spacing:10) {
            artifactSubject(.lesson,lesson.id,lesson.crewId,lesson.frozen)
            Text(lesson.draft.lesson).font(.title3).textSelection(.enabled)
            let archived = lesson.frozen ? " · 已归档，只读" : ""
            let lessonSummary: String = "\(lesson.id) · \(lesson.status.label) · \(owner(lesson.crewId))\(archived)"
            Text(lessonSummary).font(.caption).foregroundStyle(.secondary)
            LabeledContent("记录者",value:author(lesson.author))
            if let date = Wire.managedTime(lesson.lastCheckedAt) {
                LabeledContent("最后核对") { Text(date,format:.dateTime) }
            } else { LabeledContent("最后核对",value:lesson.lastCheckedAt) }
            decisionText("发生了什么",lesson.draft.happened)
            if let detection = lesson.draft.detection { decisionText("如何识别",detection) }
            if let reason = lesson.staleReason { decisionText("过时理由",reason) }
            if lesson.draft.learnedFrom.isEmpty { Text("未附来源").foregroundStyle(.secondary) }
            referenceList("来源",lesson.draft.learnedFrom)
            referenceList("适用范围",lesson.draft.appliesTo)
            if !lesson.draft.evidence.isEmpty {
                Text("证据").font(.subheadline)
                ForEach(lesson.draft.evidence.indices,id:\.self) { Text(lesson.draft.evidence[$0]).textSelection(.enabled) }
            }
            if let risk = lesson.riskId {
                Button("查看来源风险 " + risk,systemImage:"arrow.left") { showRiskLesson(risk) }.disabled(busy)
            }
            if !lesson.frozen && lesson.status == .active {
                VStack(alignment:.leading,spacing:8) {
                    Button("确认仍然适用",systemImage:"checkmark.circle") { open(.riskLesson(.confirmLesson(lesson))) }
                    if !currentCrewFrozen && Wire.exact(lesson.crewId,route.crew) {
                        Button("标为过时",role:.destructive) { open(.riskLesson(.staleLesson(lesson))) }
                    }
                }.disabled(busy || blocked(lesson.id))
            }
        }
    }
    @ViewBuilder private func referenceList(_ title: String, _ links: [Wire.ObjectLink]) -> some View {
        if !links.isEmpty {
            Text(title).font(.subheadline)
            ForEach(links.indices,id:\.self) { index in
                LabeledContent(links[index].kind.label,value:links[index].id).textSelection(.enabled)
            }
        }
    }
    private func showRiskLesson(_ id: String) {
        let target: SectionMode = id.hasPrefix("L") ? .lessons:.risks
        if target == route.section {
            let captured = route
            Task { await reload(captured,target:id,reset:false) }
        } else {
            linkedRiskLesson = .init(id:id,origin:route,section:target)
            section = target
        }
    }
    private var pendingPanel: some View {
        VStack(alignment:.leading,spacing:5) {
            Text("原请求待核对").font(.subheadline)
            ForEach(pending) { item in
                HStack {
                    let label = item.witness.kind == "createTable" ? "新建组织表" :
                        item.witness.kind == "recordDecision" ? "记录决定" :
                        item.witness.kind == "recordRisk" ? "登记风险" :
                        item.witness.kind == "recordLesson" ? "记录教训" : item.witness.target
                    Text(label + " · " + String(item.witness.requestId.prefix(8))).font(.caption)
                    Spacer()
                    Button("查看原回执") { lookup(item) }.disabled(busy || (item.witness.isDecision && !decisionsAvailable) ||
                        (item.witness.riskLesson != nil && !riskLessonsAvailable))
                }
            }
        }.padding(10).background(.secondary.opacity(0.10),in:RoundedRectangle(cornerRadius:8))
    }

    private func owner(_ crew: String) -> String {
        Wire.exact(crew, route.crew) ? "本机组" : "其他机组 · " + crew
    }
    private var currentCrewFrozen: Bool {
        workspace.crews.first { $0["id"].string == route.crew }?["removedAt"].string != nil
    }
    private func canWrite(_ table: Wire.Table) -> Bool {
        !table.frozen && !currentCrewFrozen && Wire.exact(table.crewId,route.crew) && scope != nil
    }
    private func blocked(_ target: String) -> Bool { pending.contains { $0.witness.target == target } }
    private func open(_ mode: EditorTarget.Mode) {
        guard let scope, !busy else { return }
        if currentCrewFrozen {
            guard case .riskLesson(let operation) = mode, operation.confirmLesson else { return }
        }
        editor = .init(mode:mode,scope:scope,route:route)
    }
    private func refreshPending(_ scope: Store.BrowseScope) {
        do { pending = try store.pending(scope) }
        catch { problem = error.localizedDescription }
    }
    private func reload(_ captured: Route, target requested: String?, reset: Bool,
                        kind: ReloadKind = .ordinary) async {
        guard route == captured, !Task.isCancelled else { return }
        let token = UUID()
        ticket = token
        if reset { resetProjection(clearNotice:requested == nil) }
        guard captured.journalReady else { busy = false; return }
        busy = true
        if kind != .invalidation { problem = nil }
        @MainActor func current() -> Bool { ticket == token && route == captured && !Task.isCancelled }
        defer { if current() { busy = false } }
        do {
            let active = try await store.scope(stillCurrent:current)
            var target = requested
            let authorityChanged = scope.map { prior in
                prior.origin.scope != active.origin.scope || prior.authority != active.authority
            } ?? false
            if authorityChanged {
                resetProjection(clearNotice:true)
                target = nil
            }
            if captured.section.riskLesson {
                if let target {
                    let item = try await store.riskLesson(active,id:target,stillCurrent:current)
                    guard current() else { return }
                    riskLessonDetail = item
                    if target == initialDecision {initialDecision=nil}
                } else {
                    let page = try await store.riskLessons(active,lessons:captured.section == .lessons,after:nil,stillCurrent:current)
                    guard current() else { return }
                    riskLessonItems = page.items; nextRiskLesson = page.next; riskLessonDetail = nil
                }
            } else if captured.section == .decisions {
                if let target {
                    let item = try await store.decision(active,id:target,stillCurrent:current)
                    let source = try await store.decisionOrigin(active,decision:item,stillCurrent:current)
                    guard current() else { return }
                    if let linkedAsk, target == linkedAsk.capture?.decisionId {
                        guard let source, source.sameDraft(as:linkedAsk), source.capture == linkedAsk.capture
                        else { throw Wire.Problem.invalid }
                    }
                    decision=item; decisionSource=source
                    if target == initialDecision { initialDecision=nil }
                } else {
                    let page = try await store.decisions(active,after:nil,stillCurrent:current)
                    guard current() else { return }
                    decisions = page.items; nextDecision = page.nextAfter; decision = nil; decisionSource = nil
                }
            } else if let target {
                let page = try await store.rows(active,table:target,after:nil,stillCurrent:current)
                guard current() else { return }
                table = page.table; rows = page.items; nextRow = page.nextAfter
            } else {
                let page = try await store.tables(active,after:nil,stillCurrent:current)
                guard current() else { return }
                tables = page.items; nextTable = page.nextAfter; table = nil; rows = []; nextRow = nil
            }
            scope = active
            shownSection = captured.section
            if kind != .invalidation || authorityChanged { refreshPending(active) }
        } catch {
            guard current() else { return }
            if let requested, Self.provenMissing(error) {
                if requested == initialDecision { initialDecision=nil }
                // A committed purge can remove the selected object. Clear only
                // that projection and return to the active catalogue.
                table = nil; rows = []; nextRow = nil; decision = nil; decisionSource = nil; riskLessonDetail = nil
                tables.removeAll { $0.id == requested }
                decisions.removeAll { $0.id == requested }
                riskLessonItems.removeAll { $0.id == requested }
                await reload(captured,target:nil,reset:false,kind:kind)
            } else if kind != .invalidation {
                if kind == .ordinary { scope = nil }
                problem = error.localizedDescription
            }
        }
    }
    private func resetProjection(clearNotice: Bool) {
        scope = nil; shownSection = nil; tables = []; table = nil; rows = []; nextTable = nil; nextRow = nil
        decisions = []; decision = nil; decisionSource = nil; nextDecision = nil
        riskLessonItems = []; riskLessonDetail = nil; nextRiskLesson = nil
        pending = []; editor = nil; problem = nil
        if clearNotice { notice = nil }
    }
    private static func provenMissing(_ error: Error) -> Bool {
        guard let refusal = error as? CrewdClientError else { return false }
        return refusal.code == -32004 && refusal.refusal == "resource.not_found"
    }
    private func scheduleInvalidation() {
        guard let pending = queuedInvalidation, store.owns(pending),
              !busy, editor == nil, refreshWake == nil else { return }
        let wake = UUID()
        refreshWake = wake
        Task {
            try? await Task.sleep(for:.milliseconds(150))
            guard refreshWake == wake else { return }
            refreshWake = nil
            guard let pending = queuedInvalidation, store.owns(pending,scope:scope), route.journalReady,
                  scope == nil || presentsCurrentScope, !busy, editor == nil else { return }
            queuedInvalidation = nil
            let captured = route
            await reload(captured,target:selectedID,reset:false,kind:.invalidation)
            scheduleInvalidation()
        }
    }
    private func loadMore(after: String, table target: String?) {
        guard let scope, !busy else { return }
        let captured = route, token = UUID()
        ticket = token; busy = true; problem = nil
        Task {
            @MainActor func current() -> Bool { ticket == token && route == captured && self.scope == scope && !Task.isCancelled }
            defer { if current() { busy = false } }
            do {
                if captured.section.riskLesson {
                    let page = try await store.riskLessons(scope,lessons:captured.section == .lessons,after:after,stillCurrent:current)
                    guard current(), riskLessonDetail == nil else { return }
                    riskLessonItems += page.items; nextRiskLesson = page.next
                } else if captured.section == .decisions {
                    let page = try await store.decisions(scope,after:after,stillCurrent:current)
                    guard current(), decision == nil else { return }
                    decisions += page.items; nextDecision = page.nextAfter
                } else if let target {
                    let page = try await store.rows(scope,table:target,after:after,stillCurrent:current)
                    guard current(), table?.id == target else { return }
                    table = page.table; rows += page.items; nextRow = page.nextAfter
                } else {
                    let page = try await store.tables(scope,after:after,stillCurrent:current)
                    guard current(), table == nil else { return }
                    tables += page.items; nextTable = page.nextAfter
                }
            } catch { if current() { problem = error.localizedDescription } }
        }
    }
    private func lookup(_ item: Store.Pending) {
        guard let scope, !busy else { return }
        let captured = route, token = UUID()
        ticket = token; busy = true; problem = nil; notice = nil
        Task {
            @MainActor func current() -> Bool { ticket == token && route == captured && self.scope == scope && !Task.isCancelled }
            defer { if current() { busy = false } }
            do {
                let observed = try await store.lookup(item,scope:scope,stillCurrent:current)
                guard current() else { return }
                refreshPending(scope)
                if let observed, let id = observed.receipt.object.id {
                    notice = observed.historical ? "已核对历史回执 " + id + "。当前状态请另行刷新。" :
                        "已核对原回执 " + id + "。"
                } else {
                    notice = "还没有可验证的原回执。原请求保留，可以稍后再查询。"
                }
            } catch { if current() { problem = error.localizedDescription } }
        }
    }

    struct Editor<Output>: View {
        let mode: EditorTarget.Mode
        let scope: Store.BrowseScope
        let heading: String
        var originalDraft: Store.Mutation? = nil
        var readOnly = false
        let allowRiskReferences: Bool
        let riskOwners: [CrewdValue]
        let current: @MainActor () -> Bool
        let submit: @MainActor (Store.Mutation, @MainActor () -> Bool) async throws -> Output
        let complete: @MainActor (Output) -> Void
        let refreshPending: @MainActor () -> Void
        @Environment(\.dismiss) private var dismiss
        @State private var title = ""
        @State private var fields: [Wire.Field] = [.init(name:"",fieldType:.text,nullable:false)]
        @State private var inputs: [Input] = []
        @State private var decisionDraft = Wire.DecisionDraft(question:"",choice:"",rationale:"",about:[])
        @State private var alternatives = ""
        @State private var withdrawalReason = ""
        @State private var riskDraft = Wire.RiskDraft(summary:"",impact:.medium,about:[],mitigatedBy:[])
        @State private var lessonDraft = Wire.LessonDraft(happened:"",lesson:"",learnedFrom:[],evidence:[],appliesTo:[])
        @State private var mitigation = ""
        @State private var detection = ""
        @State private var selectedOwner = ""
        @State private var actionReason = ""
        @State private var busy = false
        @State private var stopped = false
        @State private var problem: String?
        private struct Input { var text = ""; var null = false }
        private var model: (Wire.Table, Wire.Row?)? {
            switch mode {
            case .create, .recordDecision, .replaceDecision, .withdrawDecision, .riskLesson: nil
            case .add(let table): (table,nil)
            case .edit(let table,let row): (table,row)
            }
        }
        private var riskLessonMode: RiskLessonMode? {
            if case .riskLesson(let operation) = mode { return operation }; return nil
        }
        private var isWithdrawal: Bool { if case .withdrawDecision = mode { return true }; return false }
        private var isDecision: Bool {
            switch mode { case .recordDecision,.replaceDecision,.withdrawDecision: true; default: false }
        }
        var body: some View {
            NavigationStack {
                Form {
                    if let operation = riskLessonMode {
                        riskLessonFields(operation)
                    } else if isDecision {
                        decisionFields
                    } else if let (table,row) = model {
                        Section(row.map { "记录 " + $0.id + " · 版本 " + $0.revision } ?? "填写记录") {
                            ForEach(inputs.indices,id:\.self) { index in
                                let field = table.fields[index]
                                if field.nullable { Toggle("空值 · " + field.name,isOn:$inputs[index].null) }
                                if !inputs[index].null {
                                    TextField(field.name + " · " + field.fieldType.title,text:$inputs[index].text,axis:.vertical)
                                }
                            }
                        }
                    } else {
                        TextField("组织表名称",text:$title)
                        ForEach(fields.indices,id:\.self) { index in
                            Section("字段 " + String(index + 1)) {
                                TextField("字段名称",text:$fields[index].name)
                                Picker("类型",selection:$fields[index].fieldType) {
                                    ForEach(Wire.FieldType.allCases) { type in Text(type.title).tag(type) }
                                }
                                Toggle("允许空值",isOn:$fields[index].nullable)
                                if fields.count > 1 {
                                    Button("移除此字段",role:.destructive) { fields.remove(at:index) }
                                }
                            }
                        }
                        Button("添加字段",systemImage:"plus") {
                            fields.append(.init(name:"",fieldType:.text,nullable:false))
                        }.disabled(fields.count >= Wire.fieldLimit)
                    }
                    if let problem { Text(problem).foregroundStyle(.red).textSelection(.enabled) }
                    if !current() {
                        Text("原连接或资料范围已改变。输入仍保留，请重新打开当前对象后再提交。")
                            .foregroundStyle(.secondary)
                    }
                }
                .disabled(busy || stopped || readOnly)
                .navigationTitle(heading)
                .toolbar {
                    ToolbarItem(placement:.cancellationAction) {
                        Button(stopped || !current() ? "关闭" : "取消") { dismiss() }.disabled(busy && current())
                    }
                    ToolbarItem(placement:.confirmationAction) {
                        Button(isWithdrawal ? "确认撤回" : "保存") { save() }
                            .disabled(readOnly || busy || stopped || !current() || (model != nil && inputs.isEmpty))
                    }
                }
            }
            .interactiveDismissDisabled(busy && current())
            .task {
                switch mode {
                case .riskLesson(let operation):
                    prepareRiskLesson(operation)
                case .recordDecision:
                    decisionDraft.about = [.init(kind:.crew,id:scope.origin.crew)]
                case .replaceDecision(let original):
                    decisionDraft = original.draft
                    alternatives = decisionDraft.alternatives ?? ""
                    decisionDraft.choice = ""
                    decisionDraft.rationale = ""
                default: break
                }
                if let (table,row) = model {
                    inputs = table.fields.indices.map { index in
                        let original = row?.values[index]
                        return .init(text:original == .null ? "" : original?.display ?? "",null:original == .null)
                    }
                }
                if let originalDraft {
                    switch originalDraft {
                    case .recordDecision(let draft):
                        decisionDraft = draft; alternatives = draft.alternatives ?? ""
                    case .riskLesson(.recordRisk(let draft,let owner)):
                        riskDraft = draft; mitigation = draft.mitigation ?? ""
                        selectedOwner = owner.sessionId ?? ""
                    case .riskLesson(.recordLesson(let draft)):
                        lessonDraft = draft; detection = draft.detection ?? ""
                    case .add(_,let values):
                        inputs = values.map { .init(text:$0 == .null ? "" : $0.display,null:$0 == .null) }
                    default: break
                    }
                }
            }
            #if os(macOS)
            .frame(minWidth:440,minHeight:350)
            #endif
        }
        @ViewBuilder private var decisionFields: some View {
            if case .withdrawDecision(let original) = mode {
                Section(original.id + " · " + original.draft.question) {
                    Text("撤回后，原文保留在历史中；先前被取代的决定不会自动恢复。")
                    TextField("撤回理由",text:$withdrawalReason,axis:.vertical)
                }
            } else {
                if case .replaceDecision(let original) = mode {
                    Section("现行决定 " + original.id) {
                        Text(original.draft.choice)
                        Text("保存会记录一个新决定，原文与原作者仍可查阅。").foregroundStyle(.secondary)
                    }
                }
                TextField("要决定的问题",text:$decisionDraft.question)
                TextField("决定",text:$decisionDraft.choice,axis:.vertical)
                TextField("理由",text:$decisionDraft.rationale,axis:.vertical)
                TextField("其他方案（可选）",text:$alternatives,axis:.vertical)
                Picker("可逆性",selection:$decisionDraft.reversibility) {
                    Text("未填写").tag(nil as Wire.DecisionReversibility?)
                    ForEach(Wire.DecisionReversibility.allCases) { value in Text(value.label).tag(Optional(value)) }
                }
                Section("关于（至少一项）") {
                    ForEach(decisionDraft.about.indices,id:\.self) { index in
                        Picker("引用类型",selection:$decisionDraft.about[index].kind) {
                            ForEach(Wire.DecisionAboutKind.allCases.filter { allowRiskReferences || $0 != .risk }) { kind in Text(kind.label).tag(kind) }
                        }
                        TextField("对象编号或机组 ID",text:$decisionDraft.about[index].id)
                        if decisionDraft.about.count > 1 {
                            Button("移除此引用",role:.destructive) { decisionDraft.about.remove(at:index) }
                        }
                    }
                    Button("添加引用",systemImage:"plus") {
                        decisionDraft.about.append(.init(kind:.work,id:""))
                    }.disabled(decisionDraft.about.count >= 8)
                }
            }
        }
        @ViewBuilder private func riskLessonFields(_ operation: RiskLessonMode) -> some View {
            switch operation {
            case .registerRisk, .updateRisk:
                TextField("风险是什么",text:$riskDraft.summary)
                Picker("影响",selection:$riskDraft.impact) {
                    ForEach(Wire.RiskLevel.allCases) { Text($0.label).tag($0) }
                }
                Picker("可能性",selection:$riskDraft.likelihood) {
                    Text("未填写").tag(nil as Wire.RiskLevel?)
                    ForEach(Wire.RiskLevel.allCases) { Text($0.label).tag(Optional($0)) }
                }
                TextField("缓解办法（可选）",text:$mitigation,axis:.vertical)
                if case .registerRisk = operation { ownerPicker }
                editLinks("关于（至少一项）",links:$riskDraft.about,kinds:[.crew,.work,.requirement],minimum:1)
                editLinks("缓解它的计划或决定",links:$riskDraft.mitigatedBy,kinds:[.work,.decision])
            case .assignRisk(let item):
                Text(item.draft.summary)
                ownerPicker
            case .reviewRisk(let item):
                Text(item.draft.summary)
                Text("确认本次已复核。下次复核安排在七天后。")
            case .riskStatus(let item,let status):
                Text(item.draft.summary)
                Text(status == .closed ? "关闭后仍可查阅历史。":"保存会记录状态变更。")
                TextField("状态变更理由",text:$actionReason,axis:.vertical)
            case .recordLesson, .occurrence:
                if case .occurrence(let item) = operation {
                    Text(item.id + " · " + item.draft.summary)
                    Text("保存会将风险标为已发生，并同时记录一条关联教训。").foregroundStyle(.secondary)
                }
                TextField("发生了什么",text:$lessonDraft.happened,axis:.vertical)
                TextField("学到的教训",text:$lessonDraft.lesson,axis:.vertical)
                TextField("如何识别（可选）",text:$detection,axis:.vertical)
                editLinks("来源",links:$lessonDraft.learnedFrom,kinds:[.work,.ask],maximum: {
                    if case .occurrence = operation { return 7 }
                    return 8
                }())
                editLinks("适用范围",links:$lessonDraft.appliesTo,kinds:[.crew,.work,.table])
                Section("证据（可选）") {
                    ForEach(lessonDraft.evidence.indices,id:\.self) { index in
                        TextField("凭据或说明",text:$lessonDraft.evidence[index],axis:.vertical)
                        Button("移除此证据",role:.destructive) { lessonDraft.evidence.remove(at:index) }
                    }
                    Button("添加证据",systemImage:"plus") { lessonDraft.evidence.append("") }
                        .disabled(lessonDraft.evidence.count >= 8)
                }
            case .confirmLesson(let item):
                Text(item.draft.lesson)
                Text("确认这条教训仍然适用，并更新最后核对时间。")
            case .staleLesson(let item):
                Text(item.draft.lesson)
                TextField("过时理由",text:$actionReason,axis:.vertical)
            }
        }
        private var ownerPicker: some View {
            Picker("负责人",selection:$selectedOwner) {
                Text("人").tag("")
                let sessions = riskOwners.compactMap { row -> (String,String)? in
                    guard let id = row["id"].string, !id.isEmpty, row["role"].string != "admin" else { return nil }
                    return (id,row["name"].string ?? row["title"].string ?? id)
                }
                if !selectedOwner.isEmpty && !sessions.contains(where:{ $0.0 == selectedOwner }) {
                    Text(selectedOwner).tag(selectedOwner)
                }
                ForEach(sessions,id:\.0) { id,name in Text(name).tag(id) }
            }
        }
        private func editLinks(_ title: String, links: Binding<[Wire.ObjectLink]>, kinds: [Wire.LinkKind],
                               minimum: Int = 0, maximum: Int = 8) -> some View {
            Section(title) {
                ForEach(links.wrappedValue.indices,id:\.self) { index in
                    Picker("引用类型",selection:links[index].kind) {
                        ForEach(kinds) { Text($0.label).tag($0) }
                    }
                    TextField("对象编号或机组 ID",text:links[index].id)
                    if links.wrappedValue.count > minimum {
                        Button("移除此引用",role:.destructive) { links.wrappedValue.remove(at:index) }
                    }
                }
                Button("添加引用",systemImage:"plus") {
                    if let kind = kinds.first { links.wrappedValue.append(.init(kind:kind,id:"")) }
                }.disabled(links.wrappedValue.count >= maximum)
            }
        }
        private func prepareRiskLesson(_ operation: RiskLessonMode) {
            switch operation {
            case .registerRisk:
                riskDraft.about = [.init(kind:.crew,id:scope.origin.crew)]
            case .updateRisk(let item):
                riskDraft = item.draft
                mitigation = item.draft.mitigation ?? ""
            case .assignRisk(let item):
                selectedOwner = item.owner.sessionId ?? ""
            default: break
            }
        }
        private func riskLessonMutation(_ operation: RiskLessonMode) throws -> Store.RiskLessonMutation {
            let owner = Wire.DecisionAuthor(kind:selectedOwner.isEmpty ? "human":"session",
                                           sessionId:selectedOwner.isEmpty ? nil:selectedOwner)
            switch operation {
            case .registerRisk, .updateRisk:
                var draft = riskDraft
                draft.mitigation = nonblank(mitigation)
                guard draft.valid else { throw Wire.Problem.invalid }
                if case .updateRisk(let item) = operation { return .updateRisk(item,draft) }
                return .recordRisk(draft,owner)
            case .assignRisk(let item):
                guard owner != item.owner else { throw Wire.Problem.invalid }
                return .assignRisk(item,owner)
            case .reviewRisk(let item): return .reviewRisk(item)
            case .riskStatus(let item,let status):
                guard Wire.decisionText(actionReason) else { throw Wire.Problem.invalid }
                return .setRiskStatus(item,status,actionReason)
            case .recordLesson, .occurrence:
                var draft = lessonDraft
                draft.detection = nonblank(detection)
                guard draft.valid else { throw Wire.Problem.invalid }
                if case .occurrence(let item) = operation { return .occurrence(item,draft) }
                return .recordLesson(draft)
            case .confirmLesson(let item): return .confirmLesson(item)
            case .staleLesson(let item):
                guard Wire.decisionText(actionReason) else { throw Wire.Problem.invalid }
                return .staleLesson(item,actionReason)
            }
        }
        private func nonblank(_ value: String) -> String? {
            value.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty ? nil:value
        }
        private func mutation() throws -> Store.Mutation {
            if let operation = riskLessonMode { return .riskLesson(try riskLessonMutation(operation)) }
            if isDecision {
                var draft = decisionDraft
                draft.alternatives = alternatives.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty ? nil : alternatives
                switch mode {
                case .recordDecision:
                    guard draft.valid else { throw Wire.Problem.invalid }
                    return .recordDecision(draft)
                case .replaceDecision(let old):
                    guard draft.valid else { throw Wire.Problem.invalid }
                    return .replaceDecision(old,draft)
                case .withdrawDecision(let old):
                    guard Wire.decisionText(withdrawalReason) else { throw Wire.Problem.invalid }
                    return .withdrawDecision(old,withdrawalReason)
                default: throw Wire.Problem.invalid
                }
            }
            if let (table,row) = model {
                let values = try zip(inputs,table.fields).map { try Wire.Cell.parse($0.0.text,null:$0.0.null,field:$0.1) }
                guard Wire.validValues(values, fields:table.fields) else { throw Wire.Problem.invalid }
                if let row { return .edit(table,row,values) }
                return .add(table,values)
            }
            guard Wire.validDefinition(title:title,fields:fields) else { throw Wire.Problem.invalid }
            return .create(title,fields)
        }
        private func save() {
            guard !busy, !stopped, current() else { return }
            let change: Store.Mutation
            do { change = try mutation() }
            catch { problem = error.localizedDescription; return }
            busy = true; problem = nil
            Task {
                defer { busy = false }
                do {
                    let result = try await submit(change,current)
                    guard current() else { return }
                    complete(result)
                    stopped = true
                } catch {
                    guard current() else { return }
                    refreshPending()
                    problem = error.localizedDescription
                    stopped = !(isDecision || riskLessonMode != nil) || !Store.decisionInputRefused(error)
                }
            }
        }
    }
}

/// Source text is always the current Core projection, never a local draft excerpt.
private struct CrewdManagedMessageSourceView: View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:CrewdManagedDirectoryStore
    let scope:CrewdManagedDirectoryStore.BrowseScope
    let objectID:String
    let current:@MainActor ()->Bool
    @State private var source:CrewdManagedMessageWire.Source?
    @State private var problem:String?
    @State private var loading=false
    @State private var visible=true
    private var allowed:Bool { visible && current() && store.supportsMessageSources }
    var body:some View {
        VStack(alignment:.leading,spacing:8) {
            Text("消息出处").font(.headline)
            if allowed {
                if let source {
                    switch source {
                    case .none: Text("未记录消息出处。")
                    case .restricted: Text("当前无权查看原消息。")
                    case .deleted(let excerpt): Text("原消息已删除").foregroundStyle(.secondary); Text(excerpt).textSelection(.enabled)
                    case .available(_,_,_,let excerpt):
                        Text(excerpt).textSelection(.enabled)
                        Button("打开来源会话",systemImage:"arrow.up.right.square") { open() }.disabled(loading)
                        Text("打开对应会话；较早的原消息可能不在当前显示范围。").font(.caption).foregroundStyle(.secondary)
                    }
                } else { Text("尚未核对出处。") }
                if let problem { Text(problem).foregroundStyle(.red) }
                Button("重新核对出处") { Task { await load() } }.disabled(loading)
            } else { Text("连接或权限已改变，请重新打开对象。") }
        }
        .task(id:objectID) { await load() }
        .onChange(of:workspace.client.connectionEpoch) { _,_ in source=nil;problem=nil }
        .onChange(of:workspace.client.authorityBinding) { _,_ in source=nil;problem=nil }
        .onChange(of:store.invalidation) { _,_ in source=nil; Task { await load() } }
        .onAppear { visible=true }
        .onDisappear { visible=false;source=nil }
    }
    private func load() async {
        guard allowed,!loading else { return }; loading=true;source=nil;problem=nil
        defer { loading=false }
        do {
            let result=try await store.messageSource(scope,id:objectID,stillCurrent:{allowed})
            guard allowed else { return }; source=result
        } catch { if allowed { problem=error.localizedDescription } }
    }
    private func open() {
        guard allowed,!loading else { return };loading=true
        Task {
            defer { loading=false }
            do { try await workspace.openManagedMessageSource(object:objectID,stillCurrent:{allowed}) }
            catch { if allowed { source=nil;problem=error.localizedDescription } }
        }
    }
}

/// All normal message surfaces reuse the same controlled D/R/L/row forms.
struct CrewdMessageRecordSheet:View {
    typealias Store=CrewdManagedDirectoryStore
    typealias Wire=CrewdManagedDirectoryWire
    typealias Directory=CrewdManagedDirectoryView
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var store:Store
    let target:CrewdMessageInspectionTarget
    let presentationCurrent:@MainActor ()->Bool
    @Environment(\.dismiss) private var dismiss
    @State private var visible=true
    @State private var scope:Store.BrowseScope?
    @State private var objects:[Wire.Object]=[]
    @State private var next:String?
    @State private var tables:[Wire.Table]=[]
    @State private var nextTable:String?
    @State private var pending:[Store.MessagePending]=[]
    @State private var editor:FormTarget?
    @State private var outcome:Store.MessageObservation?
    @State private var busy=false
    @State private var problem:String?
    @State private var checked=false
    private struct FormTarget:Identifiable {
        let id=UUID()
        let mode:Directory.EditorTarget.Mode
        let title:String
        var draft:Store.Mutation?=nil
    }
    init(workspace:CrewdWorkspace,target:CrewdMessageInspectionTarget,presentationCurrent:@escaping @MainActor ()->Bool) {
        self.workspace=workspace;self.store=workspace.managedDirectory
        self.target=target;self.presentationCurrent=presentationCurrent
    }
    private var current:Bool { visible && presentationCurrent() && target.isCurrent(workspace) && store.supportsMessageSources }
    private var frozen:Bool { workspace.crews.first(where:{$0["id"] == .string(target.source.crew)})?["removedAt"].string != nil }
    var body:some View {
        NavigationStack {
            ScrollView {
                VStack(alignment:.leading,spacing:14) {
                    Text("记为组织对象").font(.headline)
                    Text("保存后，对象内容由组织成员可读；原消息和出处摘录仍按原会话权限提供。请先确认填写内容适合组织公开。")
                        .foregroundStyle(.secondary)
                    if let problem { Text(problem).foregroundStyle(.red).textSelection(.enabled) }
                    if current,let scope {
                        if !pending.isEmpty { history(scope) }
                        if frozen { Text("当前机组已归档，只能查看已有记录。") }
                        HStack {
                            Button("记录决定 D") { start(.recordDecision,"记录决定") }
                                .disabled(!workspace.client.capabilities.contains(Wire.decisionsCapability))
                            Button("登记风险 R") { start(.riskLesson(.registerRisk),"登记风险") }
                                .disabled(!workspace.client.capabilities.contains(Wire.riskLessonsCapability))
                            Button("记录教训 L") { start(.riskLesson(.recordLesson),"记录教训") }
                                .disabled(!workspace.client.capabilities.contains(Wire.riskLessonsCapability))
                        }.buttonStyle(.bordered).disabled(busy || frozen)
                        DisclosureGroup("记为组织表记录 O") {
                            ForEach(tables) { table in
                                Button(table.id+" · "+table.title) { start(.add(table),"新增记录 · "+table.id) }
                                    .disabled(frozen || busy || table.frozen || !Wire.exact(table.crewId,target.source.crew))
                            }
                            if let nextTable { Button("更多组织表") { loadTables(scope,after:nextTable) }.disabled(busy) }
                            Text("仅可写入当前机组的未归档组织表。").font(.caption).foregroundStyle(.secondary)
                        }
                        if let observed=outcome,editor == nil {
                            Text(observed.historical ? "原请求历史回执（不代表对象当前状态）":"原请求的确切回执")
                                .font(.headline)
                            Text(observed.receipt.created ? "原请求曾创建此对象。":"原请求返回已有对象；本次草稿未写入。")
                            snapshot(observed.receipt.object,scope:scope)
                        }
                        Divider()
                        Text("这条消息已记录的对象").font(.headline)
                        if checked && objects.isEmpty { Text("本次核对未发现已记录对象。") }
                        ForEach(objects.indices,id:\.self) { index in snapshot(objects[index],scope:scope) }
                        if let next { Button("更多已记录对象") { loadObjects(scope,after:next) }.disabled(busy) }
                        Button("重新核对") { Task { await reload() } }.disabled(busy)
                    } else { Text(current ? "正在读取原消息的记录…":"原连接或会话已改变，请重新打开消息。") }
                }.padding()
            }
            .toolbar { ToolbarItem(placement:.cancellationAction) { Button("关闭") { visible=false;dismiss() } } }
        }
        #if os(macOS)
        .frame(minWidth:520,minHeight:480)
        #endif
        .task { await reload() }
        .onDisappear { visible=false;objects=[];pending=[];scope=nil }
        .onChange(of:workspace.client.authorityBinding) { _,_ in clear() }
        .onChange(of:workspace.client.connectionEpoch) { _,_ in clear() }
        .sheet(item:$editor) { form in
            if let scope {
                VStack(spacing:0) {
                    Directory.Editor<Store.MessageObservation>(mode:form.mode,scope:scope,heading:form.title,originalDraft:form.draft,
                        allowRiskReferences:workspace.client.capabilities.contains(Wire.riskLessonsCapability),riskOwners:workspace.sessions,
                        current:{ current && editor?.id == form.id && !frozen },
                        submit:{ mutation,valid in
                            try await store.recordFromMessage(mutation,message:target.source.message,scope:scope,stillCurrent:valid)
                        },complete:{ result in
                            guard current,editor?.id == form.id else { return }
                            outcome=result;refreshHistory(scope)
                            if result.receipt.created { editor=nil;Task { await reload() } }
                        },refreshPending:{ refreshHistory(scope) })
                    if let savedResult=outcome {
                        VStack(alignment:.leading,spacing:8) {
                            Text(savedResult.receipt.created ? "已记录" : "这条消息已有同类对象；本次填写的草稿已保留在原请求与草稿中。")
                            snapshot(savedResult.receipt.object,scope:scope)
                            Button("关闭表单") { editor=nil;outcome=nil }
                        }.padding()
                    }
                }
            }
        }
    }
    private func clear() { scope=nil;objects=[];pending=[];tables=[];checked=false;next=nil;nextTable=nil;outcome=nil }
    private func start(_ mode:Directory.EditorTarget.Mode,_ title:String,draft:Store.Mutation?=nil) {
        guard current,!busy,!frozen else { return };outcome=nil;editor = .init(mode:mode,title:title,draft:draft)
    }
    private func refreshHistory(_ scope:Store.BrowseScope) {
        guard current else { return }
        do { pending=try store.messagePending(scope,message:target.source.message) }
        catch { pending=[];problem=error.localizedDescription }
    }
    private func reload() async {
        guard current,!busy else { return };busy=true;problem=nil
        defer { busy=false }
        do {
            let active=try await store.scope(stillCurrent:{current})
            let page=try await store.messageObjects(active,message:target.source.message,after:nil,stillCurrent:{current})
            let tablePage=try await store.tables(active,after:nil,stillCurrent:{current})
            guard current else { return }
            scope=active;objects=page.items;next=page.nextAfter;checked=true
            tables=tablePage.items;nextTable=tablePage.nextAfter;refreshHistory(active)
        } catch { if current { clear();problem=error.localizedDescription } }
    }
    private func loadObjects(_ scope:Store.BrowseScope,after:String) {
        guard current,!busy else { return };busy=true
        Task {
            defer { busy=false }
            do {
                let page=try await store.messageObjects(scope,message:target.source.message,after:after,stillCurrent:{current})
                guard current else { return }
                guard Set(objects.compactMap(\.id)).isDisjoint(with:page.items.compactMap(\.id)) else { throw Wire.Problem.invalid }
                objects+=page.items;next=page.nextAfter
            } catch { if current { objects=[];next=nil;checked=false;problem=error.localizedDescription } }
        }
    }
    private func loadTables(_ scope:Store.BrowseScope,after:String) {
        guard current,!busy else { return };busy=true
        Task {
            defer { busy=false }
            do {
                let page=try await store.tables(scope,after:after,stillCurrent:{current})
                guard current else { return };tables+=page.items;nextTable=page.nextAfter
            } catch { if current { problem=error.localizedDescription } }
        }
    }
    private func history(_ scope:Store.BrowseScope)->some View {
        DisclosureGroup("原请求与草稿（\(pending.count)）") {
            Text("仅查询原请求结果；不会重发。已有对象与新草稿分别保留。")
            ForEach(pending) { item in
                VStack(alignment:.leading,spacing:6) {
                    Text(item.witness.original.kind+" · "+item.witness.original.requestId).font(.caption).textSelection(.enabled)
                    Button("核对原请求") { recover(item,scope:scope) }.disabled(busy)
                    DisclosureGroup("本次填写的草稿") { draftText(item) }
                }
            }
        }
    }
    @ViewBuilder private func draftText(_ item:Store.MessagePending)->some View {
        if let draft=try? store.messageDraft(item) {
            switch draft {
            case .recordDecision(let value):Text(value.question);Text(value.choice);Text(value.rationale)
            case .riskLesson(.recordRisk(let value,_)):Text(value.summary);if let mitigation=value.mitigation { Text(mitigation) }
            case .riskLesson(.recordLesson(let value)):Text(value.happened);Text(value.lesson)
            case .add(let table,let values):ForEach(values.indices,id:\.self) { index in Text(table.fields[index].name+": "+values[index].display) }
            default:Text("草稿暂不可显示。")
            }
        }
    }
    private func recover(_ item:Store.MessagePending,scope:Store.BrowseScope) {
        guard current,!busy else { return };busy=true;problem=nil
        Task {
            defer { busy=false }
            do {
                let result=try await store.lookupMessage(item,scope:scope,stillCurrent:{current})
                guard current else { return }
                if let result {
                    outcome=result
                    problem=result.receipt.created ? "已核对原请求的确切记录。" : "原请求返回已有对象；本次填写草稿仍保留。"
                } else { problem="尚未找到可确认的原请求结果；原请求保留，不会重发。" }
                refreshHistory(scope)
            } catch { if current { problem=error.localizedDescription } }
        }
    }
    @ViewBuilder private func snapshot(_ object:Wire.Object,scope:Store.BrowseScope)->some View {
        VStack(alignment:.leading,spacing:6) {
            if let item=object.decision { Text(item.id+" · "+item.draft.question).font(.headline);Text(item.status.label);Text(item.draft.choice);Text(item.draft.rationale) }
            if let item=object.risk { Text(item.id+" · "+item.draft.summary).font(.headline);Text(item.status.label);if let mitigation=item.draft.mitigation { Text(mitigation) } }
            if let item=object.lesson { Text(item.id+" · "+item.draft.lesson).font(.headline);Text(item.draft.happened);Text(item.status.label) }
            if let item=object.row { Text(item.id+" · "+item.tableId).font(.headline);ForEach(item.values.indices,id:\.self) { index in Text("字段 \(index+1)："+item.values[index].display) } }
            if let id=object.id { CrewdManagedMessageSourceView(workspace:workspace,store:store,scope:scope,objectID:id,current:{current}) }
        }.padding(10).frame(maxWidth:.infinity,alignment:.leading).background(.quaternary,in:RoundedRectangle(cornerRadius:8))
    }
}

/// Available from the ordinary library even when the source message has moved
/// out of the current timeline. Recovery is always the original read-only lookup.
private struct CrewdManagedMessageHistoryView:View {
    typealias Store=CrewdManagedDirectoryStore
    typealias Directory=CrewdManagedDirectoryView
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:Store
    let scope:Store.BrowseScope
    let current:@MainActor ()->Bool
    @State private var items:[Store.MessagePending]=[]
    @State private var shown=16
    @State private var busy=false
    @State private var problem:String?
    @State private var result:Store.MessageObservation?
    @State private var draft:DraftTarget?
    private struct DraftTarget:Identifiable {
        let id=UUID()
        let mode:Directory.EditorTarget.Mode
        let mutation:Store.Mutation
    }
    var body:some View {
        DisclosureGroup("消息记录的原请求与草稿（\(items.count)）") {
            Text("记录过的原请求与本次填写内容保留在此。核对只读原回执，不重发；返回已有对象不表示本次草稿已写入。")
                .font(.caption).foregroundStyle(.secondary)
            if let problem { Text(problem).foregroundStyle(.secondary).textSelection(.enabled) }
            ForEach(Array(items.prefix(shown))) { item in
                HStack {
                    Text(item.witness.original.kind+" · "+String(item.witness.original.requestId.prefix(8))).font(.caption)
                    Button("核对原回执") { lookup(item) }.disabled(busy || !current())
                    Button("查看填写内容") { showDraft(item) }.disabled(!current())
                }
            }
            if shown < items.count { Button("更多原请求") { shown+=16 } }
            Button("重新读取原请求") { reload() }.disabled(busy || !current())
            if let result,current(),let id=result.receipt.object.id {
                Text((result.receipt.created ? "原请求已记录：":"原请求返回已有对象：")+id+(result.historical ? "（历史回执）":""))
                CrewdManagedMessageSourceView(workspace:workspace,store:store,scope:scope,objectID:id,current:current)
            }
        }
        .task(id:scope.authority.restoreEpoch) { reload() }
        .onChange(of:store.invalidation) { _,_ in reload() }
        .onChange(of:workspace.client.authorityBinding) { _,_ in items=[];result=nil;draft=nil }
        .onChange(of:workspace.client.connectionEpoch) { _,_ in items=[];result=nil;draft=nil }
        .sheet(item:$draft) { selected in
            Directory.Editor<Void>(mode:selected.mode,scope:scope,heading:"原请求填写内容",originalDraft:selected.mutation,
                readOnly:true,allowRiskReferences:workspace.client.capabilities.contains(CrewdManagedDirectoryWire.riskLessonsCapability),
                riskOwners:workspace.sessions,current:{current() && draft?.id == selected.id},
                submit:{_,_ in throw CrewdManagedDirectoryWire.Problem.stale},complete:{_ in},refreshPending:{})
        }
    }
    private func reload() {
        guard current() else {items=[];result=nil;return}
        do {items=try store.messagePending(scope)} catch {items=[];problem=error.localizedDescription}
    }
    private func showDraft(_ item:Store.MessagePending) {
        guard current() else {return}
        do {
            let mutation=try store.messageDraft(item)
            let mode:Directory.EditorTarget.Mode
            switch mutation {
            case .recordDecision:mode = .recordDecision
            case .riskLesson(.recordRisk):mode = .riskLesson(.registerRisk)
            case .riskLesson(.recordLesson):mode = .riskLesson(.recordLesson)
            case .add(let table,_):mode = .add(table)
            default:throw CrewdManagedDirectoryWire.Problem.invalid
            }
            draft = .init(mode:mode,mutation:mutation)
        } catch {problem=error.localizedDescription}
    }
    private func lookup(_ item:Store.MessagePending) {
        guard current(),!busy else {return};busy=true;problem=nil;result=nil
        Task {
            defer {busy=false}
            do {
                let observed=try await store.lookupMessage(item,scope:scope,stillCurrent:current)
                guard current() else {return};result=observed
                if observed == nil {problem="尚无可确认的原请求结果。原请求保留，不会重发。"}
                reload()
            } catch {if current(){problem=error.localizedDescription}}
        }
    }
}

/// One normal route for both work evidence and managed-object associations.
struct CrewdArtifactSubjectEntry:View {
    @ObservedObject var workspace:CrewdWorkspace
    let subject:CrewdArtifactWire.Subject
    let crew:String
    var frozen=false
    var current:@MainActor ()->Bool = {true}
    @State private var showing=false
    var body:some View {
        if workspace.client.capabilities.contains(CrewdArtifactWire.capability) {
            Button("关联产物",systemImage:"link") {showing=true}.disabled(!current())
                .sheet(isPresented:$showing) {
                    CrewdArtifactPanel(workspace:workspace,subject:subject,subjectCrew:crew,frozen:frozen,
                        current:{showing && current()})
                }
        }
    }
}

private struct CrewdArtifactPanel:View {
    typealias Wire=CrewdArtifactWire
    typealias Store=CrewdArtifactStore
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var client:CrewdClient
    @ObservedObject private var store:Store
    var subject:Wire.Subject?=nil
    var subjectCrew:String?=nil
    var frozen=false
    let current:@MainActor ()->Bool
    @Environment(\.dismiss) private var dismiss
    @State private var showHistory=false
    @State private var visible=true
    @State private var scope:Store.Scope?
    @State private var artifacts:[Wire.Artifact]=[]
    @State private var nextArtifact:Wire.Reference?
    @State private var edges:[Wire.Edge]=[]
    @State private var nextEdge:String?
    @State private var selected:Wire.Reference?
    @State private var detail:Wire.Artifact?
    @State private var pending:[Store.Pending]=[]
    @State private var busy=false
    @State private var loaded=false
    @State private var problem:String?
    @State private var notice:String?
    @State private var ticket=UUID()
    @State private var refreshQueued=false
    @State private var form:FormTarget?
    @State private var unlink:Wire.Edge?
    @State private var inspected:Wire.Edge?
    private struct FormTarget:Identifiable {let id=UUID();var reference:Wire.Reference?=nil}
    private struct Route:Hashable {let connection:UUID;let observation:UUID;let crew:String;let authority:UUID;let supported:Bool;let ready:Bool}
    init(workspace:CrewdWorkspace,subject:Wire.Subject?=nil,subjectCrew:String?=nil,frozen:Bool=false,initialReference:Wire.Reference?=nil,current:@escaping @MainActor ()->Bool) {
        self.workspace=workspace;self.client=workspace.client;self.store=workspace.artifacts
        self.subject=subject;self.subjectCrew=subjectCrew;self.frozen=frozen;self.current=current;self._selected=State(initialValue:initialReference)
    }
    private var route:Route {.init(connection:client.connectionEpoch,observation:workspace.observationGeneration,crew:workspace.crewID,authority:workspace.authorityRecordID,supported:store.supported,ready:client.readyForWork && workspace.originalJournalReady)}
    private var allowed:Bool {visible && current() && client.readyForWork && store.supported}
    private var crewFrozen:Bool {workspace.crews.first(where:{$0["id"] == .string(workspace.crewID)})?["removedAt"].string != nil}
    private var writable:Bool {
        let ledger=subject?.kind == .work || subject?.kind == .requirement
        return allowed && !crewFrozen && (ledger || !frozen)
    }
    var body:some View {
        NavigationStack {
            ScrollView {content.padding()}
                .navigationTitle(selected==nil ? (subject.map{$0.id+" 的产物"} ?? "组织产物目录"):"产物详情")
                .toolbar {
                    ToolbarItem(placement:.cancellationAction) {Button("关闭") {visible=false;dismiss()}}
                    ToolbarItem(placement:.primaryAction) {
                        if store.historySupported {Button("索引历史凭据") {showHistory=true}.disabled(!allowed)}
                    }
                    ToolbarItem(placement:.primaryAction) {Button("刷新") {Task {await load(reset:true)}}.disabled(busy || !allowed)}
                }
        }
        #if os(macOS)
        .frame(minWidth:540,minHeight:480)
        #endif
        .task(id:route) {clear();await load(reset:true)}
        .onChange(of:client.authorityBinding) {_,_ in clear();Task {await load(reset:true)}}
        .onChange(of:store.invalidation) {_,_ in refreshQueued=true;refreshIfReady()}
        .onChange(of:busy) {_,_ in refreshIfReady()}
        .onChange(of:form?.id) {_,_ in refreshIfReady()}
        .onDisappear {visible=false;ticket=UUID();artifacts=[];edges=[];detail=nil}
        .onChange(of:workspace.managedBusinessNavigation?.id){_,id in if id != nil {visible=false;dismiss()}}
        .sheet(isPresented:$showHistory) {
            CrewdArtifactHistorySheet(workspace:workspace,store:store,current:{showHistory && allowed})
        }
        .sheet(item:$form) {target in
            if let scope {
                CrewdArtifactEditor(store:store,scope:scope,subject:subject,existing:target.reference,current:{allowed && store.owns(scope) && writable && form?.id==target.id},
                    onSaved:{result in
                        guard allowed,form?.id==target.id else {return}
                        notice=receiptText(result);form=nil;Task {await load(reset:true)}
                    },onAttempt:{refreshPending(scope)})
            }
        }
        .sheet(item:$inspected) {edge in
            if let scope {CrewdArtifactSubjectInspector(workspace:workspace,store:store,scope:scope,edge:edge,current:{allowed && store.owns(scope) && inspected?.id==edge.id})}
        }
        .confirmationDialog("移除此手工产物关联？",isPresented:Binding(get:{unlink != nil},set:{if !$0 {unlink=nil}})) {
            if let edge=unlink {Button("确认移除关联",role:.destructive) {unlink=nil;remove(edge)}}
        } message: {Text("只移除这条手工关联，原工作、凭据文字和其他机组关联保留。")}
    }
    @ViewBuilder private var content:some View {
        VStack(alignment:.leading,spacing:12) {
            Text("未核验：产物只是指针，系统未检查提交、文件、测试或链接。登记内容由组织成员可读。")
                .font(.caption).foregroundStyle(.secondary)
            if let notice {Text(notice).textSelection(.enabled)}
            if let problem {Text(problem).foregroundStyle(.orange).textSelection(.enabled)}
            if allowed,let scope,store.owns(scope) {
                recovery(scope)
                if selected != nil {
                    Button("返回关联列表",systemImage:"chevron.backward") {self.selected=nil;Task {await load(reset:true)}}.disabled(busy)
                    if let detail {pointer(detail.reference,openExternal:true);Text("首次记录："+detail.createdAt).font(.caption)}
                    if let selected {
                        Button(subject==nil ? "将此产物关联到对象":"关联到当前对象",systemImage:"link.badge.plus") {form = .init(reference:selected)}
                            .disabled(!writable || busy)
                        CrewdManagedLinksEntry(workspace:workspace,source:.init(kind:"artifact",reference:selected,metadataCrewId:workspace.crewID),current:{allowed && self.selected==selected})
                    }
                    Text("反向关联").font(.headline)
                    edgeRows(scope,backlinks:true)
                } else {
                    Button(subject==nil ? "登记产物":"关联新的产物",systemImage:"plus") {form = .init()}.disabled(!writable || busy)
                    if subject?.kind == .lesson {Text("可以关联证据，或将文件产物记为适用范围；原教训文字保持原样。").font(.caption)}
                    if !writable {Text("当前范围只读。手工关联需具备当前机组及原对象的对应权限。").font(.caption).foregroundStyle(.secondary)}
                    if subject != nil {edgeRows(scope,backlinks:false)}
                    else {
                        ForEach(artifacts) {item in
                            Button {selected=item.reference;Task {await load(reset:true)}} label:{pointer(item.reference)}.buttonStyle(.plain)
                        }
                        if let nextArtifact {Button("更多产物") {Task {await load(reset:false,afterArtifact:nextArtifact)}}.disabled(busy)}
                        if loaded && artifacts.isEmpty {Text("本次读取未发现已登记产物。")}
                    }
                }
                if busy {ProgressView().controlSize(.small)}
            } else {Text("原连接或资料范围已改变，请重新打开产物。")}
        }
    }
    private func pointer(_ reference:Wire.Reference,openExternal:Bool=false)->some View {
        VStack(alignment:.leading,spacing:4) {
            Text(reference.key).textSelection(.enabled)
            Text(reference.location+" · 未核验").font(.caption).foregroundStyle(.secondary)
            if openExternal,reference.key.hasPrefix("url:"),let url=URL(string:String(reference.key.dropFirst(4))),["https","http"].contains(url.scheme ?? "") {
                Link("打开外部链接",destination:url).font(.caption)
            }
        }.frame(maxWidth:.infinity,alignment:.leading).padding(8).background(.quaternary,in:RoundedRectangle(cornerRadius:8))
    }
    private func edgeRows(_ scope:Store.Scope,backlinks:Bool)->some View {
        VStack(alignment:.leading,spacing:10) {
            ForEach(edges) {edge in
                VStack(alignment:.leading,spacing:6) {
                    if backlinks {
                        if workspace.client.capabilities.contains(CrewdManagedTableV2Wire.linksCapability),let kind=CrewdManagedTableV2Wire.TargetKind(rawValue:edge.subject.kind.rawValue) {
                            CrewdManagedTargetButton(workspace:workspace,target:.init(kind:kind,idValue:edge.subject.id),summary:edge.subject.kind.title+" "+edge.subject.id,current:{allowed && store.owns(scope)})
                        }else{Button(edge.subject.kind.title+" "+edge.subject.id) {inspected=edge}.disabled(busy)}
                    }
                    else {Button {selected=edge.reference;Task {await load(reset:true)}} label:{pointer(edge.reference)}.buttonStyle(.plain)}
                    Text(edge.relation.title+" · "+edge.origin.title+" · 归属机组 "+edge.crewId).font(.caption).foregroundStyle(.secondary)
                    if !backlinks && edge.origin.manual && writable && edge.crewId.utf8.elementsEqual(scope.origin.crew.utf8) {
                        Button("移除此手工关联",role:.destructive) {unlink=edge}.disabled(busy)
                    }
                }
            }
            if let nextEdge {Button("更多关联") {Task {await load(reset:false,afterEdge:nextEdge)}}.disabled(busy)}
            if loaded && edges.isEmpty {Text("本次读取未发现关联。")}
        }
    }
    private func recovery(_ scope:Store.Scope)->some View {
        VStack(alignment:.leading,spacing:6) {
            ForEach(pending) {item in
                Text("原请求待核对 · "+item.witness.reference.key).font(.caption)
                Text(item.witness.requestId).font(.caption).textSelection(.enabled)
                DisclosureGroup("原始填写内容") {
                    let change=item.witness.change
                    if let input=change.input {
                        Text(input.kind.title+"："+input.value).textSelection(.enabled)
                        Text("填写的仓库标识："+(input.repository ?? "未填写")).textSelection(.enabled)
                    }
                    if let subject=change.subject {Text(subject.kind.title+" "+subject.id).textSelection(.enabled)}
                    if let relation=change.relation {Text(relation.title)}
                    if let edge=change.expectedEdgeId {Text("原手工关联 "+edge).textSelection(.enabled)}
                    Text("规范引用："+item.witness.reference.key).textSelection(.enabled)
                    Text(item.witness.reference.location).font(.caption)
                }
                Button("查询原回执") {recover(item,scope:scope)}.disabled(busy)
            }
        }
    }
    private func clear() {ticket=UUID();scope=nil;detail=nil;artifacts=[];edges=[];nextArtifact=nil;nextEdge=nil;pending=[];loaded=false;busy=false;notice=nil;problem=nil}
    private func refreshIfReady() {guard refreshQueued,!busy,form==nil,allowed else {return};refreshQueued=false;Task {await load(reset:true)}}
    private func refreshPending(_ scope:Store.Scope) {do {pending=try store.pending(scope)} catch {if allowed {problem=error.localizedDescription}}}
    private func load(reset:Bool,afterArtifact:Wire.Reference?=nil,afterEdge:String?=nil) async {
        guard allowed,!busy else {return};busy=true;let request=UUID();ticket=request
        let captured=route,reference=selected
        func valid()->Bool {allowed && route==captured && ticket==request && selected==reference}
        defer {if ticket==request {busy=false}}
        do {
            let active=try await store.scope(stillCurrent:valid)
            if reset {artifacts=[];edges=[];detail=nil;loaded=false;nextArtifact=nil;nextEdge=nil}
            if let reference {
                let item=try await store.get(reference,scope:active,stillCurrent:valid)
                let page=try await store.edges(active,reference:reference,after:afterEdge,stillCurrent:valid)
                guard valid(),Set(edges.map(\.id)).isDisjoint(with:page.items.map(\.id)) else {throw Store.Problem.invalid}
                detail=item;edges+=page.items;nextEdge=page.nextAfter
            } else if let subject {
                let page=try await store.edges(active,subject:subject,after:afterEdge,stillCurrent:valid)
                guard valid(),Set(edges.map(\.id)).isDisjoint(with:page.items.map(\.id)) else {throw Store.Problem.invalid}
                edges+=page.items;nextEdge=page.nextAfter
            } else {
                let page=try await store.list(active,after:afterArtifact,stillCurrent:valid)
                guard valid(),Set(artifacts.map(\.id)).isDisjoint(with:page.items.map(\.id)) else {throw Store.Problem.invalid}
                artifacts+=page.items;nextArtifact=page.nextAfter
            }
            guard valid() else {return};scope=active;loaded=true;problem=nil;refreshPending(active)
        } catch {if valid() {artifacts=[];edges=[];detail=nil;nextArtifact=nil;nextEdge=nil;loaded=false;problem=error.localizedDescription}}
    }
    private func receiptText(_ observed:Store.Observation)->String {
        let r=observed.receipt,e=r.effect
        let action:String
        switch e.kind {case "registered":action=e.claimAdded==true ? "已登记产物":"本机组已登记此产物";case "linked":action=e.added==true ? "已建立关联":"该关联已存在";default:action="原手工关联已移除"}
        return (observed.historical ? "原请求历史回执（不代表当前状态）：":"原请求已确认：")+action+" · "+r.artifact.reference.key+" · 原请求 "+r.requestId
    }
    private func recover(_ pending:Store.Pending,scope:Store.Scope) {
        guard allowed,!busy else {return};busy=true
        Task {defer {busy=false}
            do {
                let result=try await store.lookup(pending,scope:scope,stillCurrent:{allowed})
                guard allowed else {return};notice=result.map(receiptText) ?? "尚无可确认的原结果。原请求保留，不会重新提交。";refreshPending(scope)
            } catch {if allowed {problem=error.localizedDescription}}
        }
    }
    private func remove(_ edge:Wire.Edge) {
        guard writable,!busy,edge.origin.manual,let scope else {return};busy=true
        Task {defer {busy=false}
            do {
                let result=try await store.submit(.init(kind:"unlink",subject:edge.subject,relation:edge.relation,reference:edge.reference,expectedEdgeId:edge.id),scope:scope,stillCurrent:{allowed && writable})
                guard allowed else {return};notice=receiptText(result);refreshPending(scope);refreshQueued=true
            } catch {if allowed {problem=error.localizedDescription;refreshPending(scope)}}
        }
    }
}

private struct CrewdArtifactEditor:View {
    typealias Wire=CrewdArtifactWire
    @ObservedObject var store:CrewdArtifactStore
    let scope:CrewdArtifactStore.Scope
    let subject:Wire.Subject?
    var existing:Wire.Reference?=nil
    let current:@MainActor ()->Bool
    let onSaved:@MainActor (CrewdArtifactStore.Observation)->Void
    let onAttempt:@MainActor ()->Void
    @Environment(\.dismiss) private var dismiss
    @State private var kind=Wire.InputKind.commit
    @State private var value=""
    @State private var repository=""
    @State private var targetKind=Wire.SubjectKind.work
    @State private var targetID=""
    @State private var relation=Wire.Relation.evidence
    @State private var canonical:Wire.Reference?
    @State private var previewed:String?
    @State private var busy=false
    @State private var stopped=false
    @State private var problem:String?
    private var input:Wire.Input {.init(kind:kind,value:value,repository:repository.isEmpty ? nil:repository)}
    private var effectiveSubject:Wire.Subject? {subject ?? (existing==nil ? nil:.init(kind:targetKind,id:targetID))}
    private var relations:[Wire.Relation] {
        let allowed=effectiveSubject?.relations ?? []
        if let existing,!existing.key.hasPrefix("file:") {return allowed.filter{$0 != .appliesTo}}
        return allowed
    }
    private var draft:String {
        if let existing {return existing.id+":"+(effectiveSubject?.kind.rawValue ?? "")+":"+(effectiveSubject?.id ?? "")+":"+relation.rawValue}
        return (try? Wire.bytes(input).base64EncodedString()) ?? ""
    }
    var body:some View {
        NavigationStack {
            Form {
                Text("登记组织可读的未核验指针。不会扫描仓库、读取文件、运行测试或访问外链。")
                if let existing {
                    Text(existing.key).textSelection(.enabled)
                    Text(existing.location+" · 已有指针，不另建产物").foregroundStyle(.secondary)
                    if subject==nil {
                        Picker("关联对象类型",selection:$targetKind) {ForEach(Wire.SubjectKind.allCases) {Text($0.title).tag($0)}}
                        TextField("对象的全局编号",text:$targetID)
                        Text("关联归属当前机组；后台会核对对象的当前编辑权限。").font(.caption)
                    }
                }
                if let effectiveSubject {Text(effectiveSubject.kind.title+" "+effectiveSubject.id);Picker("关联方式",selection:$relation) {ForEach(relations) {Text($0.title).tag($0)}}}
                if existing==nil {
                Picker("产物类型",selection:$kind) {ForEach(Wire.InputKind.allCases) {Text($0.title).tag($0)}}.disabled(relation == .appliesTo && subject != nil)
                TextField("SHA、绝对链接、仓库相对路径或测试名",text:$value,axis:.vertical)
                TextField("明确的仓库标识（可选）",text:$repository)
                Text("仓库标识是你明确填写的身份，不是本机路径，也不表示已核验。缺少归属的短提交号、文件和测试只记在本机组未归属范围。").font(.caption).foregroundStyle(.secondary)
                }
                Button(existing==nil ? "核对规范引用":"核对已有产物") {preview()}.disabled(busy || !current() || stopped)
                if let canonical {Text(canonical.key).textSelection(.enabled);Text(canonical.location+" · 未核验").foregroundStyle(.secondary)}
                if let problem {Text(problem).foregroundStyle(.orange).textSelection(.enabled)}
                if !current() {Text("原连接或范围已改变。输入保留，请重新打开后核对。").foregroundStyle(.secondary)}
            }.disabled(busy || stopped)
                .navigationTitle(subject==nil ? "登记产物":"关联产物")
                .toolbar {
                    ToolbarItem(placement:.cancellationAction) {Button(stopped ? "关闭":"取消") {dismiss()}.disabled(busy && current())}
                    ToolbarItem(placement:.confirmationAction) {Button(subject==nil ? "确认登记":"确认关联") {save()}.disabled(busy || stopped || !current() || canonical==nil || previewed != draft || (existing != nil && effectiveSubject?.valid != true))}
                }
        }
        #if os(macOS)
        .frame(minWidth:460,minHeight:390)
        #endif
        .task {if let first=effectiveSubject?.relations.first {relation=first}}
        .onChange(of:targetKind) {_,new in relation=Wire.Subject(kind:new,id:targetID).relations.first ?? .relates}
        .onChange(of:draft) {_,_ in canonical=nil;previewed=nil}
        .onChange(of:relation) {_,new in canonical=nil;previewed=nil;if new == .appliesTo {kind = .file}}
        .interactiveDismissDisabled(busy && current())
    }
    private func preview() {
        guard current(),!busy else {return};busy=true;problem=nil;let submitted=input,key=draft
        Task {defer {busy=false}
            do {
                let ref:Wire.Reference
                if let existing {
                    guard effectiveSubject?.valid==true else {throw CrewdArtifactStore.Problem.invalid}
                    ref=try await store.get(existing,scope:scope,stillCurrent:{current() && draft==key}).reference
                } else {ref=try await store.preview(submitted,scope:scope,stillCurrent:{current() && draft==key})}
                guard current(),draft==key else {return}
                guard relation != .appliesTo || ref.key.hasPrefix("file:") else {throw CrewdArtifactStore.Problem.invalid}
                canonical=ref;previewed=key
            } catch {if current() {problem=error.localizedDescription}}
        }
    }
    private func save() {
        guard current(),!busy,!stopped,canonical != nil,previewed==draft else {return};busy=true;problem=nil
        let change:Wire.Change
        if let existing,let effectiveSubject {change = .init(kind:"linkExisting",subject:effectiveSubject,relation:relation,reference:existing)}
        else {change=subject.map{Wire.Change(kind:"link",input:input,subject:$0,relation:relation)} ?? .init(kind:"register",input:input)}
        Task {defer {busy=false;onAttempt()}
            do {let result=try await store.submit(change,scope:scope,stillCurrent:current);guard current() else {return};stopped=true;onSaved(result)}
            catch {if current() {problem=error.localizedDescription;stopped=true}}
        }
    }
}

private struct CrewdArtifactSubjectInspector:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:CrewdArtifactStore
    let scope:CrewdArtifactStore.Scope
    let edge:CrewdArtifactWire.Edge
    let current:@MainActor ()->Bool
    @Environment(\.dismiss) private var dismiss
    @State private var value:CrewdValue?
    @State private var problem:String?
    var body:some View {
        NavigationStack {
            ScrollView {
                VStack(alignment:.leading,spacing:12) {
                    if current() {
                        Text(edge.subject.kind.title+" "+edge.subject.id).font(.headline)
                        Text("关联归属机组："+edge.crewId).font(.caption)
                        if let value {facts(value)}
                        else if let problem {Text(problem).foregroundStyle(.orange)}
                        else {Text("正在核对当前对象…")}
                    } else {Text("原连接或范围已改变，请重新打开关联对象。")}
                }.padding()
            }.navigationTitle("关联对象").toolbar {ToolbarItem(placement:.cancellationAction) {Button("关闭") {dismiss()}}}
        }
        #if os(macOS)
        .frame(minWidth:460,minHeight:350)
        #endif
        .task(id:edge.id) {
            do {let result=try await store.readSubject(edge,scope:scope,stillCurrent:current);if current() {value=result}}
            catch {if current() {value=nil;problem=error.localizedDescription}}
        }
        .onChange(of:store.invalidation) {_,_ in value=nil;problem="对象已变化，请重新打开核对。"}
    }
    @ViewBuilder private func facts(_ raw:CrewdValue)->some View {
        let kind=edge.subject.kind
        let item=(kind == .work || kind == .requirement) ? raw:raw[kind.rawValue]
        text("对象当前归属",item["crewId"])
        if kind == .work {text("计划",item["title"]);text("状态",item["status"]);text("目标",item["goal"]);text("验收",item["acceptance"]);text("关闭凭据",item["closure"]["evidence"]["value"])}
        else if kind == .requirement {text("需求",item["text"]);text("状态",item["status"])}
        else if kind == .decision {text("问题",item["draft"]["question"]);text("选择",item["draft"]["choice"]);text("理由",item["draft"]["rationale"]);text("状态",item["status"])}
        else if kind == .risk {text("风险",item["draft"]["summary"]);text("缓解办法",item["draft"]["mitigation"]);text("状态",item["status"])}
        else if kind == .lesson {text("教训",item["draft"]["lesson"]);text("发生了什么",item["draft"]["happened"]);text("状态",item["status"])}
        else if kind == .table {text("组织表",item["title"]);ForEach(item["fields"].array.indices,id:\.self) {i in text("字段",item["fields"].array[i]["name"])}}
        else if let row=try? CrewdKnowledgeStore.decode(item,CrewdManagedDirectoryWire.Row.self) {Text("组织表 "+row.tableId);ForEach(row.values.indices,id:\.self) {i in Text("字段 \(i+1)："+row.values[i].display).textSelection(.enabled)}}
        if kind != .work && kind != .requirement,workspace.managedDirectory.supportsMessageSources {
            CrewdManagedMessageSourceView(workspace:workspace,store:workspace.managedDirectory,scope:scope,objectID:edge.subject.id,current:current)
        }
    }
    @ViewBuilder private func text(_ label:String,_ value:CrewdValue)->some View {if let text=value.string,!text.isEmpty {LabeledContent(label) {Text(text).textSelection(.enabled)}}}
}

/// A maintenance sheet: effects require an explicit Begin or one-page Step.
/// All lifecycle, reconnect and notification tasks below are reads only.
private struct CrewdArtifactHistorySheet:View {
    typealias Wire=CrewdArtifactHistoryWire
    typealias Store=CrewdArtifactStore
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var client:CrewdClient
    @ObservedObject var store:Store
    let current:@MainActor ()->Bool
    @Environment(\.dismiss) private var dismiss
    @State private var visible=true
    @State private var crews:[CrewdValue]=[]
    @State private var targetCrew:String
    @State private var scope:Store.Scope?
    @State private var status:Wire.Status?
    @State private var selectedJob:String?
    @State private var pending:[Store.HistoryPending]=[]
    @State private var receipt:Store.HistoryObservation?
    @State private var problem:String?
    @State private var reading=false
    @State private var writing=false
    @State private var queueRead=false
    @State private var limit=32
    @State private var readToken=UUID()
    @State private var confirmBegin=false
    private struct Route:Hashable {let connection:UUID;let observation:UUID;let authority:UUID;let crew:String;let ready:Bool;let supported:Bool}
    init(workspace:CrewdWorkspace,store:Store,current:@escaping @MainActor ()->Bool) {
        self.workspace=workspace;self.client=workspace.client;self.store=store;self.current=current
        self._targetCrew=State(initialValue:workspace.crewID)
    }
    private var route:Route {.init(connection:client.connectionEpoch,observation:workspace.observationGeneration,authority:workspace.authorityRecordID,
        crew:workspace.crewID,ready:client.readyForWork && workspace.originalJournalReady,supported:store.historySupported)}
    private var allowed:Bool {visible && current() && route.ready && store.historySupported}
    private var scoped:Bool {allowed && scope.map(store.owns)==true}
    private var targetExists:Bool {crews.contains{$0["id"].string?.utf8.elementsEqual(targetCrew.utf8)==true}}
    private var unresolvedCurrent:Bool {guard let scope else {return true};return pending.contains{CrewdArtifactWire.exact($0.witness.scope,scope.authority)}}
    private var canBegin:Bool {scoped && targetExists && status != nil && !reading && !writing && !unresolvedCurrent}
    private var canStep:Bool {guard let status,let job=status.job,let scope else {return false};return canBegin && status.canContinue &&
        CrewdArtifactWire.exact(job.scope,scope.authority) && job.cursor.phase != .complete}
    var body:some View {
        NavigationStack {
            ScrollView {content.padding()}
                .navigationTitle("历史凭据索引")
                .toolbar {ToolbarItem(placement:.cancellationAction) {Button("关闭") {visible=false;dismiss()}}}
        }
        #if os(macOS)
        .frame(minWidth:560,minHeight:520)
        #endif
        .task(id:route) {clear();await read(roster:true)}
        .onChange(of:targetCrew) {_,_ in clear(keepRoster:true);selectedJob=nil;Task {await read(roster:false)}}
        .onChange(of:client.authorityBinding) {_,_ in clear();Task {await read(roster:true)}}
        .onChange(of:store.historyInvalidation) {_,_ in queueRead=true;refreshIfReady()}
        .onChange(of:writing) {_,_ in refreshIfReady()}
        .onChange(of:reading) {_,_ in refreshIfReady()}
        .onDisappear {visible=false;readToken=UUID();status=nil;receipt=nil;pending=[];crews=[]}
        .confirmationDialog("固定此机组的一份历史范围？",isPresented:$confirmBegin) {
            Button("固定范围，不自动处理下一页") {begin()}
        } message: {Text("只处理此机组在固定切口内实际保留的完成凭据和教训证据。不扫描仓库，不改写旧工作、教训或事件。已有任务可能被复用。")}
    }
    @ViewBuilder private var content:some View {
        VStack(alignment:.leading,spacing:14) {
            Text("历史索引只增加未核验的产物指针与来源关联。原业务、原凭据和原事件保留。每次只处理明确的一页，不会在后台自动继续。")
                .font(.caption).foregroundStyle(.secondary)
            if let problem,allowed {Text(problem).foregroundStyle(.orange).textSelection(.enabled)}
            if allowed {
                Picker("目标机组",selection:$targetCrew) {
                    if !targetExists {Text("请选择现存机组").tag(targetCrew)}
                    ForEach(crews.indices,id:\.self) {index in
                        let row=crews[index],id=row["id"].string ?? ""
                        Text((row["title"].string ?? id)+(row["removedAt"].string==nil ? "":"（已归档）")).tag(id)
                    }
                }.disabled(writing)
                Text("人类可维护现存归档机组；实际权限每次由后台重新核对。一次只针对选定机组。").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("读取此机组最新任务") {selectedJob=nil;Task {await read(roster:true)}}.disabled(reading || writing || !targetExists)
                    if selectedJob != nil {Button("刷新这份固定范围") {Task {await read(roster:false)}}.disabled(reading || writing)}
                }
                if scoped,let scope {
                    recovery(scope)
                    if let status {
                        if let job=status.job {jobFacts(job,scope:scope)}
                        else {Text("未读到这份范围的索引任务。可明确固定一份新范围。")}
                        Button(status.job==nil ? "固定历史范围":"固定或读取当前范围",systemImage:"pin") {confirmBegin=true}.disabled(!canBegin)
                        Stepper("本页最多 \(limit) 条来源",value:$limit,in:1...64).disabled(writing)
                        Button(writing ? "正在提交原步骤…":"处理下一页",systemImage:"forward.frame") {step()}.disabled(!canStep)
                        if unresolvedCurrent {Text("同一恢复范围有原步骤待核对。当前进度不能代替原 UUID 的回执，暂不推进。").foregroundStyle(.orange)}
                    }
                    if let receipt {receiptFacts(receipt)}
                } else {Text("正在核对原连接与索引范围…").foregroundStyle(.secondary)}
                if reading {ProgressView("正在读取索引状态…").controlSize(.small)}
            } else {Text("此连接尚未提供历史索引，或原权限范围已改变。没有推进下一页。")}
        }
    }
    private func jobFacts(_ job:Wire.Job,scope:Store.Scope)->some View {
        VStack(alignment:.leading,spacing:8) {
            Text(job.cursor.phase.title).font(.headline)
            Text("任务 "+job.id+" · 修订 "+job.revision).font(.caption).textSelection(.enabled)
            if !CrewdArtifactWire.exact(job.scope,scope.authority) {
                Text("这份任务属于旧恢复范围，只能查看历史。不会继续旧游标；如需处理当前资料，请明确固定新范围。").foregroundStyle(.orange)
            }
            LabeledContent("当前游标",value:job.cursor.after).textSelection(.enabled)
            counts(job.counts,title:"此任务累计索引记录")
            DisclosureGroup("固定范围与来源") {
                LabeledContent("旧事件序号上界",value:job.cut.legacyEventSeq)
                LabeledContent("旧工作号上界",value:job.cut.legacyItem)
                LabeledContent("冻结快照序号",value:job.cut.legacySnapshotSeq)
                LabeledContent("实际保留的旧事件数",value:job.cut.legacyRetainedEvents)
                if let digest=job.cut.legacySnapshotDigest {Text("冻结快照摘要："+digest).font(.caption).textSelection(.enabled)}
                LabeledContent("组织事件上界",value:job.cut.eventSeq)
                LabeledContent("教训号上界",value:job.cut.lessonNumber)
                LabeledContent("索引创建时间",value:job.createdAt)
                Text("旧版冻结快照是补充来源，不代表保留了原完成事件。全局序号可能稀疏，不能据此计算完成百分比。").font(.caption).foregroundStyle(.secondary)
            }
            if let last=job.lastRequestId {Text("任务最近请求编号（不代表本客户端已确认）："+last).font(.caption).textSelection(.enabled)}
        }
    }
    private func counts(_ value:Wire.Counts,title:String)->some View {
        VStack(alignment:.leading,spacing:4) {
            Text(title).font(.subheadline)
            Text("扫描来源 "+value.scanned+" · 真实完成事件 "+value.doneEvents+" · 冻结快照 "+value.legacySnapshots+" · 教训 "+value.lessons)
            Text("缺失目标 "+value.missingTargets+" · 新增产物 "+value.artifactsAdded+" · 新增关联 "+value.edgesAdded)
            Text("以上为索引过程累计记录，不是当前产物或关联总数。").font(.caption).foregroundStyle(.secondary)
        }.font(.caption)
    }
    private func recovery(_ scope:Store.Scope)->some View {
        VStack(alignment:.leading,spacing:8) {
            ForEach(pending) {item in
                let historical = !CrewdArtifactWire.exact(item.witness.scope,scope.authority)
                Text(historical ? "旧恢复范围的原请求待核对":"原索引请求待核对").font(.subheadline)
                Text(item.witness.requestId).font(.caption).textSelection(.enabled)
                if let cursor=item.witness.operation.expectedCursor {Text("原步骤："+cursor.phase.title+" · 游标 "+cursor.after).font(.caption)}
                Button("只查询这笔原回执") {lookup(item,scope:scope)}.disabled(writing || reading)
            }
        }
    }
    private func receiptFacts(_ observed:Store.HistoryObservation)->some View {
        let r=observed.receipt
        return VStack(alignment:.leading,spacing:5) {
            Text(observed.historical ? "原请求历史回执（不代表当前范围状态）":"已核对原索引回执").font(.headline)
            Text(r.requestId).font(.caption).textSelection(.enabled)
            if r.effect.kind=="begun" {Text(r.effect.created==true ? "已固定范围；尚未自动处理下一页。":"本次复用了已有固定范围；没有自动推进。")}
            if let delta=r.effect.delta {counts(delta,title:"这笔原步骤的实际增量")}
            Text("回执中的任务修订 "+r.job.revision+"，只证明这笔原操作。最新状态需单独读取。").font(.caption).foregroundStyle(.secondary)
        }
    }
    private func clear(keepRoster:Bool=false) {readToken=UUID();scope=nil;status=nil;pending=[];receipt=nil;problem=nil;reading=false;queueRead=writing;if !keepRoster {crews=[]}}
    private func refreshIfReady() {guard queueRead,!reading,!writing,allowed else {return};queueRead=false;Task {await read(roster:false)}}
    private func refreshPending(_ scope:Store.Scope) {
        do {pending=try store.historyPending(scope,crew:targetCrew)} catch {if scoped {problem=error.localizedDescription}}
    }
    private func read(roster:Bool) async {
        guard allowed,!writing,!reading else {return};reading=true;let token=UUID();readToken=token
        let captured=route,target=targetCrew,job=selectedJob
        func valid()->Bool {allowed && route==captured && targetCrew==target && readToken==token && selectedJob==job}
        defer {if readToken==token {reading=false}}
        do {
            let active=try await store.scope(stillCurrent:valid)
            guard valid() else {return};scope=active;refreshPending(active)
            if roster || crews.isEmpty {
                let rows=try await store.historyCrews(active,stillCurrent:valid)
                guard valid() else {return};crews=rows
            }
            guard valid(),crews.contains(where:{$0["id"].string?.utf8.elementsEqual(target.utf8)==true}) else {throw Store.Problem.unavailable}
            let state=try await store.historyStatus(target,job:job,scope:active,stillCurrent:valid)
            guard valid() else {return};scope=active;status=state;problem=nil;refreshPending(active)
            if selectedJob==nil,let id=state.job?.id {selectedJob=id}
        } catch {if valid() {status=nil;if scope.map(store.owns) != true {scope=nil;pending=[]};problem=error.localizedDescription}}
    }
    private func begin() {
        guard canBegin,let scope else {return};writing=true;problem=nil
        let captured=route,target=targetCrew
        func valid()->Bool {allowed && route==captured && targetCrew==target && store.owns(scope)}
        Task {defer {writing=false}
            do {
                let result=try await store.beginHistory(target,scope:scope,stillCurrent:valid)
                guard valid() else {return};receipt=result;selectedJob=result.receipt.job.id;refreshPending(scope);queueRead=true
            } catch {if valid() {problem=error.localizedDescription;refreshPending(scope);queueRead=true}}
        }
    }
    private func step() {
        guard canStep,let scope,let job=status?.job else {return};writing=true;problem=nil
        let captured=route,target=targetCrew,budget=limit
        func valid()->Bool {allowed && route==captured && targetCrew==target && selectedJob==job.id && store.owns(scope)}
        Task {defer {writing=false}
            do {
                let result=try await store.stepHistory(job,limit:budget,scope:scope,stillCurrent:valid)
                guard valid() else {return};receipt=result;refreshPending(scope);queueRead=true
            } catch {if valid() {problem=error.localizedDescription;refreshPending(scope);queueRead=true}}
        }
    }
    private func lookup(_ original:Store.HistoryPending,scope:Store.Scope) {
        guard scoped,!writing,!reading else {return};writing=true;problem=nil
        let captured=route,target=targetCrew
        func valid()->Bool {allowed && route==captured && targetCrew==target && store.owns(scope)}
        Task {defer {writing=false}
            do {
                let result=try await store.lookupHistory(original,scope:scope,stillCurrent:valid)
                guard valid() else {return};receipt=result
                if result==nil {problem="尚无可确认的原回执。保留原记录，不重发，也不处理下一页。"}
                refreshPending(scope);queueRead=true
            } catch {if valid() {problem=error.localizedDescription}}
        }
    }
}
