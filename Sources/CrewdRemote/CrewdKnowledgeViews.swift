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
            if collection == .managedDirectory {
                if workspace.client.capabilities.contains(CrewdManagedDirectoryWire.capability) {
                    CrewdManagedDirectoryView(workspace:workspace,store:workspace.managedDirectory)
                } else {
                    CrewdEmptySurface(title:"组织目录暂不可用",detail:"当前连接尚未提供组织目录。",symbol:"list.bullet.rectangle")
                }
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

private struct CrewdManagedDirectoryView: View {
    typealias Wire = CrewdManagedDirectoryWire
    typealias Store = CrewdManagedDirectoryStore
    @ObservedObject var workspace: CrewdWorkspace
    let store: Store
    @State private var scope: Store.BrowseScope?
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

    private struct Route: Equatable, Hashable {
        let crew: String
        let observation: UUID
        let connection: UUID
        let journalReady: Bool
    }
    private var route: Route {
        .init(crew:workspace.crewID,observation:workspace.observationGeneration,
              connection:workspace.client.connectionEpoch,
              journalReady:workspace.originalJournalReady && workspace.client.readyForWork)
    }
    private var presentsCurrentScope: Bool {
        guard let active = scope?.origin else { return false }
        return route.journalReady && active.observation == route.observation &&
            active.connection == route.connection && Wire.exact(active.crew,route.crew)
    }
    private struct EditorTarget: Identifiable {
        enum Mode { case create; case add(Wire.Table); case edit(Wire.Table, Wire.Row) }
        let id = UUID()
        let mode: Mode
        let scope: Store.BrowseScope
        let route: Route
        var title: String {
            switch mode { case .create: "新建组织表"; case .add: "新增记录"; case .edit: "编辑记录" }
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                header
                if let problem { Text(problem).foregroundStyle(.red).textSelection(.enabled) }
                if presentsCurrentScope {
                    if let notice { Text(notice).foregroundStyle(.secondary).textSelection(.enabled) }
                    if !pending.isEmpty { pendingPanel }
                    if let table { records(table) } else { catalog }
                }
            }
            .frame(maxWidth: 760, alignment: .leading)
            .padding(16)
            .frame(maxWidth: .infinity)
        }
        .task(id: route) { await reload(route, target: nil, reset: true) }
        .onDisappear { ticket = UUID(); editor = nil }
        .sheet(item: $editor) { target in
            Editor(target: target, current: { editor?.id == target.id && route == target.route && scope == target.scope && !currentCrewFrozen },
                   submit: { mutation, current in
                try await store.submit(mutation, scope: target.scope, stillCurrent: current)
            }, complete: { result in
                guard editor?.id == target.id && route == target.route else { return }
                editor = nil
                notice = "已保存 " + (result.receipt.object.id ?? "组织对象") + "。"
                let selected = table?.id
                Task { await reload(target.route, target: selected, reset: false) }
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
                if busy { ProgressView().controlSize(.small) }
                Spacer()
                Button("刷新",systemImage:"arrow.clockwise") {
                    let captured = route, selected = table?.id
                    Task { await reload(captured, target: selected, reset: false) }
                }.disabled(busy)
            }
            Text("其他机组的组织表可阅读。本机组维护自己的表和记录。")
                .font(.caption).foregroundStyle(.secondary)
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
    private var pendingPanel: some View {
        VStack(alignment:.leading,spacing:5) {
            Text("原请求待核对").font(.subheadline)
            ForEach(pending) { item in
                HStack {
                    let label = item.witness.kind == "createTable" ? "新建组织表" : item.witness.target
                    Text(label + " · " + String(item.witness.requestId.prefix(8))).font(.caption)
                    Spacer()
                    Button("查看原回执") { lookup(item) }.disabled(busy)
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
        guard let scope, !busy, !currentCrewFrozen else { return }
        editor = .init(mode:mode,scope:scope,route:route)
    }
    private func refreshPending(_ scope: Store.BrowseScope) {
        do { pending = try store.pending(scope) }
        catch { problem = error.localizedDescription }
    }
    private func reload(_ captured: Route, target: String?, reset: Bool) async {
        guard route == captured, !Task.isCancelled else { return }
        let token = UUID()
        ticket = token
        if reset {
            scope = nil; tables = []; table = nil; rows = []; nextTable = nil; nextRow = nil
            pending = []; editor = nil; notice = nil
        }
        guard captured.journalReady else { busy = false; return }
        busy = true; problem = nil
        @MainActor func current() -> Bool { ticket == token && route == captured && !Task.isCancelled }
        defer { if current() { busy = false } }
        do {
            let active = try await store.scope(stillCurrent:current)
            if let target {
                let page = try await store.rows(active,table:target,after:nil,stillCurrent:current)
                guard current() else { return }
                table = page.table; rows = page.items; nextRow = page.nextAfter
            } else {
                let page = try await store.tables(active,after:nil,stillCurrent:current)
                guard current() else { return }
                tables = page.items; nextTable = page.nextAfter; table = nil; rows = []; nextRow = nil
            }
            scope = active
            refreshPending(active)
        } catch { if current() { scope = nil; problem = error.localizedDescription } }
    }
    private func loadMore(after: String, table target: String?) {
        guard let scope, !busy else { return }
        let captured = route, token = UUID()
        ticket = token; busy = true; problem = nil
        Task {
            @MainActor func current() -> Bool { ticket == token && route == captured && self.scope == scope && !Task.isCancelled }
            defer { if current() { busy = false } }
            do {
                if let target {
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

    private struct Editor: View {
        let target: EditorTarget
        let current: @MainActor () -> Bool
        let submit: @MainActor (Store.Mutation, @MainActor () -> Bool) async throws -> Store.Observation
        let complete: @MainActor (Store.Observation) -> Void
        let refreshPending: @MainActor () -> Void
        @Environment(\.dismiss) private var dismiss
        @State private var title = ""
        @State private var fields: [Wire.Field] = [.init(name:"",fieldType:.text,nullable:false)]
        @State private var inputs: [Input] = []
        @State private var busy = false
        @State private var stopped = false
        @State private var problem: String?
        private struct Input { var text = ""; var null = false }
        private var model: (Wire.Table, Wire.Row?)? {
            switch target.mode {
            case .create: nil
            case .add(let table): (table,nil)
            case .edit(let table,let row): (table,row)
            }
        }
        var body: some View {
            NavigationStack {
                Form {
                    if let (table,row) = model {
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
                }
                .disabled(busy || stopped)
                .navigationTitle(target.title)
                .toolbar {
                    ToolbarItem(placement:.cancellationAction) {
                        Button(stopped ? "关闭" : "取消") { dismiss() }.disabled(busy)
                    }
                    ToolbarItem(placement:.confirmationAction) {
                        Button("保存") { save() }.disabled(busy || stopped || (model != nil && inputs.isEmpty))
                    }
                }
            }
            .interactiveDismissDisabled(busy)
            .task {
                if let (table,row) = model {
                    inputs = table.fields.indices.map { index in
                        let original = row?.values[index]
                        return .init(text:original == .null ? "" : original?.display ?? "",null:original == .null)
                    }
                }
            }
            #if os(macOS)
            .frame(minWidth:440,minHeight:350)
            #endif
        }
        private func mutation() throws -> Store.Mutation {
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
                defer { if current() { busy = false } }
                do {
                    let result = try await submit(change,current)
                    guard current() else { return }
                    complete(result)
                } catch {
                    guard current() else { return }
                    refreshPending()
                    problem = error.localizedDescription
                    stopped = true
                }
            }
        }
    }
}
