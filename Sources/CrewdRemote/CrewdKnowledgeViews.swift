import SwiftUI

/// Product surface: Agent-authored pages inside the original application layout.
/// Reading/reopening this surface never sends a prompt or resumes an Agent.
struct CrewdKnowledgeDocumentsEntry: View {
    @ObservedObject var workspace: CrewdWorkspace
    var body: some View {
        CrewdKnowledgeWorkspaceView(store: workspace.knowledge,
                                    available: workspace.client.capabilities.contains("data"))
    }
}

struct CrewdKnowledgeWorkspaceView: View {
    @ObservedObject var store: CrewdKnowledgeStore
    let available: Bool
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
            }.padding(8)
            Divider()
            if !available {
                ContentUnavailableView("知识数据尚不可用", systemImage: "externaldrive.badge.questionmark")
            } else if let detail = store.detail {
                page(detail)
            } else {
                ContentUnavailableView("这里显示你的页面", systemImage: "rectangle.on.rectangle",
                    description: Text("在聊天中明确告诉 Agent 你需要的界面。生成的页面和数据绑定会保存；打开这里不会自动启动 Agent。"))
            }
            Text(store.status).font(.caption).foregroundStyle(.secondary).padding(6)
        }
        .task(id: available) { if available { await reload() } }
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
            ForEach(store.objects.filter { $0.objectKind == "view" }, id: \.identity) {
                Text($0.displayTitle).tag($0.identity)
            }
        }.frame(maxWidth: 300)
    }
    private var reloadButton: some View {
        Button("重新读取") { Task { await reload() } }.disabled(store.busy)
    }
    private var systemButton: some View {
        Button("系统控制…") { showSystem = true }
    }
    private func reload() async {
        await store.refresh()
        await store.list(objectType: "view", includeArchived: showArchived)
        // Reopen a persisted page without executing its author code or calling an Agent.
        if let context = store.workspace?.context {
            let preferred = selected.isEmpty ? UserDefaults.standard.string(forKey: pagePreference(context)) ?? "" : selected
            let id = store.objects.contains(where: { $0.identity == preferred }) ? preferred : store.objects.first?.identity
            if let id { selected = id; await store.open(.init(namespace: context.namespace, kind: "view", id: id)) }
        }
    }
    private func pagePreference(_ context: CrewdDataContext) -> String {
        "pendingcrew.page." + context.authorityId + "." + context.crewId + "." + context.namespace
    }
    @ViewBuilder private func page(_ detail: CrewdDataObjectDetail) -> some View {
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
