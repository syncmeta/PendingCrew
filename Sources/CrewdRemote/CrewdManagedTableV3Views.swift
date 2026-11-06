import SwiftUI

private enum CrewdTableV3Palette {
  static func color(_ tone: CrewdManagedTableV3Wire.Tone) -> Color {
    switch tone {
    case .neutral: .primary
    case .good: .green
    case .attention: .orange
    case .bad: .red
    case .info: .blue
    }
  }
  static func color(_ cell: CrewdManagedTableV3Wire.Cell, field: CrewdManagedTableV3Wire.Field)
    -> Color
  {
    if case .singleSelect(let ref) = cell,
      let option = field.kind.options?.first(where: { $0.id == ref.optionId })
    {
      return color(option.tone)
    }
    return .primary
  }
}

/// One table route and editor shared by Mac, iPad and iPhone.
struct CrewdManagedTableV3View: View {
  typealias Wire = CrewdManagedTableV3Wire
  typealias Store = CrewdManagedTableV3Store
  @ObservedObject var workspace: CrewdWorkspace
  @ObservedObject private var client: CrewdClient
  @ObservedObject var store: Store
  var initialID: String? = nil
  @State private var scope: Store.Scope?
  @State private var tables: [Wire.Table] = []
  @State private var selected: Wire.Table?
  @State private var rows: [Wire.Row] = []
  @State private var directRow: Wire.Row?
  @State private var includeArchived = false
  @State private var next: String?
  @State private var loaded = false
  @State private var busy = false
  @State private var visible = true
  @State private var initialOpened = false
  @State private var ticket = UUID()
  @State private var refreshQueued = false
  @State private var failure: String?
  @State private var notice: String?
  @State private var pending: [Store.Pending] = []
  @State private var legacyPending: [CrewdManagedTableV2Store.Pending] = []
  @State private var legacyV1Pending: [CrewdManagedDirectoryStore.Pending] = []
  @State private var linkCells: [String: Wire.LinkCell] = [:]
  @State private var editor: CrewdTableV3EditorTarget?
  @State private var historyTarget: CrewdTableV3HistoryTarget?
  init(workspace: CrewdWorkspace, store: Store, initialID: String? = nil) {
    self.workspace = workspace
    self.client = workspace.client
    self.store = store
    self.initialID = initialID
  }
  private var route: Data {
    let raw: CrewdValue = .object([
      "authority": .string(workspace.authorityRecordID.uuidString),
      "observation": .string(workspace.observationGeneration.uuidString),
      "connection": .string(client.connectionEpoch.uuidString), "crew": .string(workspace.crewID),
      "ready": .bool(client.readyForWork), "journalReady": .bool(workspace.originalJournalReady),
      "capable": .bool(store.supported),
      "scope": (try? workspace.presentationScope.map(CrewdKnowledgeStore.encode)) ?? .null,
    ])
    return (try? CrewdOrganizationSearchWire.encoder.encode(raw)) ?? Data()
  }

  private var allowed: Bool {
    visible && store.supported && workspace.client.readyForWork && workspace.originalJournalReady
      && !workspace.isRetired
  }
  private var current: Bool { allowed && scope.map(store.owns) == true }
  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack {
        Text("组织表").font(.headline)
        Spacer()
        Button("刷新", systemImage: "arrow.clockwise") { Task { await load(selected?.id) } }.disabled(
          busy)
        Button("新建组织表", systemImage: "plus") {
          if let scope { editor = .init(mode: .create, scope: scope, route: route) }
        }.disabled(!current || busy || workspace.crewID.isEmpty)
      }
      Toggle("显示归档表与记录", isOn: $includeArchived).font(.caption).disabled(busy)
      if workspace.crewID.isEmpty {
        Text("新表需要明确的归属机组，请先选择一个机组。").font(.caption).foregroundStyle(.secondary)
      }
      Text("表和记录使用稳定编号；关联列读取实际对象，不改写原业务状态。").font(.caption).foregroundStyle(.secondary)
      if let notice { Text(notice).font(.caption).textSelection(.enabled) }
      if let failure {
        Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
      }
      if let scope, current { recovery(scope) }
      if let table = selected, current {
        HStack {
          Button("返回组织表目录") { Task { await load(nil) } }
          Spacer()
          Button("变更历史") {
            historyTarget = .init(
              source: .init(kind: "table", id: table.id), table: table, scope: scope!, route: route)
          }
        }
        Text(table.title + " · " + table.id).font(.title3).textSelection(.enabled)
        Text("所属机组 " + table.crewId + " · 定义版本 " + table.schemaRevision).font(.caption)
          .foregroundStyle(.secondary)
        if let purpose = table.purpose {
          Text(purpose).textSelection(.enabled)
        } else {
          Text("原表未填写用途。").font(.caption).foregroundStyle(.secondary)
        }
        Text("写权限：" + table.rowWritePolicy.title + " · " + table.author.title).font(.caption)
        lifecycle(table.lifecycle, reasons: table.frozenReasons)
        HStack {
          if !table.frozen {
            Button("管理列与表定义") { edit(.schema(table)) }
            Button("新增记录", systemImage: "plus") { edit(.add(table)) }
            Button("归档表") { edit(.lifecycle(table, nil, false)) }
          } else if table.lifecycle.archived
            && !table.frozenReasons.contains(where: { $0.kind == "crewArchived" })
          {
            Button("恢复此表") { edit(.lifecycle(table, nil, true)) }
          }
        }.disabled(busy)
        CrewdManagedLinksEntry(
          workspace: workspace, source: .init(kind: "table", id: table.id),
          current: { current && selected?.id == table.id })
        CrewdArtifactSubjectEntry(
          workspace: workspace, subject: .init(kind: .table, id: table.id), crew: table.crewId,
          frozen: table.frozen, current: { current && selected?.id == table.id })
        ScrollView {
          LazyVStack(alignment: .leading, spacing: 12) {
            if let directRow, !rows.contains(where: { $0.id == directRow.id }) {
              Text("当前打开的实际记录；下方目录仍按页读取。").font(.caption)
              rowView(directRow, table)
            }
            ForEach(rows) { row in rowView(row, table) }
            if let next { Button("更多记录") { Task { await more(next, table) } }.disabled(busy) }
            if loaded && rows.isEmpty && !busy { Text("本次读取没有记录。").foregroundStyle(.secondary) }
          }
        }
      } else if current {
        ScrollView {
          LazyVStack(alignment: .leading, spacing: 10) {
            ForEach(tables) { t in
              Button {
                Task { await load(t.id) }
              } label: {
                VStack(alignment: .leading) {
                  Text(t.title)
                  Text(t.id + " · " + t.crewId + (t.lifecycle.archived ? " · 已归档" : "")).font(
                    .caption
                  ).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity, alignment: .leading)
              }.buttonStyle(.plain)
            }
            if let next { Button("更多组织表") { Task { await more(next, nil) } }.disabled(busy) }
            if loaded && tables.isEmpty && !busy { Text("本次读取没有组织表。").foregroundStyle(.secondary) }
          }
        }
      } else if !busy {
        Text("原连接或表格范围已变化，请重新读取。").foregroundStyle(.secondary)
      }
      if busy { ProgressView().controlSize(.small) }
    }.padding(16)
      .task(id: route) {
        let id = directRow?.id ?? selected?.id ?? (initialOpened ? nil : initialID)
        initialOpened = true
        ticket = UUID()
        busy = false
        scope = nil
        selected = nil
        tables = []
        rows = []
        directRow = nil
        loaded = false
        next = nil
        linkCells = [:]
        pending = []
        legacyPending = []
        legacyV1Pending = []
        notice = nil
        failure = nil
        await load(id)
      }
      .onChange(of: includeArchived) { _, _ in Task { await load(directRow?.id ?? selected?.id) } }
      .onChange(of: store.invalidation) { _, _ in
        refreshQueued = true
        refreshIfReady()
      }
      .onChange(of: editor?.id) { _, _ in refreshIfReady() }
      .onChange(of: historyTarget?.id) { _, _ in refreshIfReady() }
      .onChange(of: busy) { _, _ in refreshIfReady() }
      .onAppear { visible = true }.onDisappear {
        visible = false
        ticket = UUID()
        busy = false
        linkCells = [:]
      }
      .sheet(item: $editor) { target in
        CrewdTableV3Editor(
          workspace: workspace, store: store, target: target,
          current: {
            current && editor?.id == target.id && route == target.route && store.owns(target.scope)
          },
          saved: { observation in
            guard editor?.id == target.id, route == target.route else { return }
            notice =
              (observation.historical ? "历史原请求已核对 · " : "原请求已核对 · ") + observation.receipt.requestId
            let id = observation.receipt.effect.rowId ?? observation.receipt.effect.tableId
            editor = nil
            Task { await load(id) }
          }, attempted: { if let scope { refreshPending(scope) } })
      }
      .sheet(item: $historyTarget) { target in
        CrewdTableV3HistoryView(
          workspace: workspace, store: store, target: target,
          current: { current && historyTarget?.id == target.id && route == target.route })
      }
  }
  private func edit(_ mode: CrewdTableV3EditorTarget.Mode) {
    if let scope { editor = .init(mode: mode, scope: scope, route: route) }
  }
  private func lifecycle(_ value: Wire.Lifecycle, reasons: [Wire.FrozenReason]) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      if let at = value.archivedAt {
        Text(
          "归档于 " + at + " · "
            + (value.archivedBy?.kind == "human" ? "人类" : value.archivedBy?.sessionId ?? "成员")
        ).font(.caption)
        if let reason = value.reason {
          Text("归档原因：" + reason).font(.caption).textSelection(.enabled)
        }
      }
      ForEach(reasons.map(\.kind), id: \.self) { r in
        Text(
          r == "crewArchived" ? "所属机组已归档，只读。" : r == "tableArchived" ? "此表已归档，只读。" : "此记录已归档，只读。"
        ).font(.caption).foregroundStyle(.secondary)
      }
    }
  }
  private func cellKey(_ row: Wire.Row, _ field: String) -> String {
    row.id + ":" + row.schemaRevision + ":" + row.revision + ":" + field
  }
  private func rowView(_ row: Wire.Row, _ table: Wire.Table) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack {
        Text(
          table.field(table.titleFieldId).map {
            row.scalar($0.id) == .null ? row.id : row.scalar($0.id).display($0)
          } ?? row.id
        ).font(.headline)
        Text(row.id).font(.caption)
        Spacer()
        if !row.frozen { Button("编辑记录") { edit(.patch(table, row)) } }
      }
      HStack {
        Button("变更历史") {
          if let scope {
            historyTarget = .init(
              source: .init(kind: "row", id: row.id), table: table, scope: scope, route: route)
          }
        }
        if !row.frozen {
          Button("归档记录") { edit(.lifecycle(table, row, false)) }
        } else if row.lifecycle.archived && !table.frozen {
          Button("恢复此记录") { edit(.lifecycle(table, row, true)) }
        }
      }.disabled(busy)
      lifecycle(row.lifecycle, reasons: row.frozenReasons)
      CrewdManagedLinksEntry(
        workspace: workspace, source: .init(kind: "row", id: row.id),
        current: { current && selected?.id == table.id })
      CrewdArtifactSubjectEntry(
        workspace: workspace, subject: .init(kind: .row, id: row.id), crew: row.crewId,
        frozen: row.frozen, current: { current && selected?.id == table.id })
      ForEach(table.active) { f in
        VStack(alignment: .leading, spacing: 4) {
          Text(f.name).font(.subheadline)
          if f.kind.isLink {
            if let cell = linkCells[cellKey(row, f.id)] {
              ForEach(cell.visible) { link in
                CrewdManagedTargetButton(
                  workspace: workspace, target: link.target, summary: link.summary,
                  current: { current })
              }
              if cell.hiddenSet != nil {
                Text("部分原关联当前不可见，原关系仍保留。").font(.caption).foregroundStyle(.secondary)
              }
              if cell.incompleteRequiredLink {
                Text("原必填目标已删除，需要重新补充。").font(.caption).foregroundStyle(.orange)
              }
              if cell.visible.isEmpty && cell.hiddenSet == nil && !cell.incompleteRequiredLink {
                Text("空值").foregroundStyle(.secondary)
              }
            } else {
              Text("关联尚未读取。").font(.caption).foregroundStyle(.secondary)
              Button("读取此列关联") { readLinks(row, field: f, table: table) }.disabled(busy)
            }
          } else {
            Text(
              {
                if case .text(let value) = row.scalar(f.id), value.isEmpty { return "空文本" }
                return row.scalar(f.id).display(f)
              }()
            ).foregroundStyle(
              CrewdTableV3Palette.color(row.scalar(f.id), field: f)
            ).textSelection(.enabled)
            if case .url(let raw) = row.scalar(f.id), let url = URL(string: raw) {
              Link("打开网址", destination: url).font(.caption)
            }
          }
        }
      }
      Text("记录版本 " + row.revision + " · " + row.author.title).font(.caption).foregroundStyle(
        .secondary)
    }.padding(10).frame(maxWidth: .infinity, alignment: .leading).background(
      .quaternary, in: RoundedRectangle(cornerRadius: 8))
  }
  private func readLinks(_ row: Wire.Row, field: Wire.Field, table: Wire.Table) {
    guard let scope, current, !busy else { return }
    busy = true
    let token = ticket
    let captured = route
    Task {
      defer { if token == ticket { busy = false } }
      do {
        let cell = try await store.links(
          row, field: field, table: table, scope: scope,
          current: { current && ticket == token && route == captured })
        guard current, ticket == token, route == captured else { return }
        linkCells[cellKey(row, field.id)] = cell
      } catch { if current, ticket == token { failure = error.localizedDescription } }
    }
  }
  private func refreshPending(_ s: Store.Scope) {
    do {
      pending = try store.pending(s)
      legacyPending = try workspace.managedTablesV2.pending(s).filter { $0.witness.table != nil }
      legacyV1Pending = try workspace.managedDirectory.pending(s).filter {
        ["createTable", "addRow", "editRow"].contains($0.witness.kind)
      }
    } catch { failure = error.localizedDescription }
  }
  private func recovery(_ s: Store.Scope) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      ForEach(pending) { p in
        Text("原表格请求待核对 · " + p.witness.crewId + " · " + p.witness.submit.requestId).font(.caption)
          .textSelection(.enabled)
        Button("只读查询此原回执") { recover(p, s) }.disabled(busy)
      }
      ForEach(legacyPending) { p in
        Text("旧版原表格请求待核对 · " + p.witness.requestId).font(.caption)
        Button("按原方式只读查询回执") { recoverLegacy(p, s) }.disabled(busy)
      }
      ForEach(legacyV1Pending) { p in
        Text("更早的原表格请求待核对 · " + p.witness.requestId).font(.caption)
        Button("只读查询这份原回执") { recoverV1(p, s) }.disabled(busy)
      }
    }
  }
  private func recover(_ p: Store.Pending, _ s: Store.Scope) {
    guard current, !busy else { return }
    busy = true
    let t = ticket
    Task {
      defer { if ticket == t { busy = false } }
      do {
        let r = try await store.lookup(p, scope: s, current: { current && ticket == t })
        guard current, ticket == t else { return }
        notice =
          r.map { ($0.historical ? "历史原回执 · " : "原回执 · ") + $0.receipt.requestId }
          ?? "原结果仍未知；未重新提交。"
        refreshPending(s)
      } catch { if current, ticket == t { failure = error.localizedDescription } }
    }
  }
  private func recoverLegacy(_ p: CrewdManagedTableV2Store.Pending, _ s: Store.Scope) {
    guard current, !busy else { return }
    busy = true
    let t = ticket
    Task {
      defer { if ticket == t { busy = false } }
      do {
        let r = try await workspace.managedTablesV2.lookup(
          p, scope: s, current: { current && ticket == t })
        guard current, ticket == t else { return }
        notice = r.map { "旧版原回执 · " + $0.receipt.requestId } ?? "旧版原结果仍未知；未重新提交。"
        refreshPending(s)
      } catch { if current, ticket == t { failure = error.localizedDescription } }
    }
  }
  private func recoverV1(_ p: CrewdManagedDirectoryStore.Pending, _ s: Store.Scope) {
    guard current, !busy else { return }
    busy = true
    let t = ticket
    Task {
      defer { if ticket == t { busy = false } }
      do {
        let r = try await workspace.managedDirectory.lookup(
          p, scope: s, stillCurrent: { current && ticket == t })
        guard current, ticket == t else { return }
        notice = r.map { "原回执 · " + $0.receipt.requestId } ?? "原结果仍未知；未重新提交。"
        refreshPending(s)
      } catch { if current, ticket == t { failure = error.localizedDescription } }
    }
  }
  private func refreshIfReady() {
    guard refreshQueued, !busy, editor == nil, historyTarget == nil, allowed else { return }
    refreshQueued = false
    Task { await load(directRow?.id ?? selected?.id) }
  }
  private func load(_ id: String?) async {
    guard allowed, !busy else { return }
    busy = true
    let t = UUID()
    ticket = t
    let captured = route
    @MainActor func valid() -> Bool { allowed && route == captured && ticket == t }
    defer { if ticket == t { busy = false } }
    do {
      let s = try await store.scope(stillCurrent: valid)
      guard valid() else { return }
      scope = s
      linkCells = [:]
      refreshPending(s)
      if let id, id.hasPrefix("O") {
        let (table, row) = try await store.row(id, scope: s, current: valid)
        let page = try await store.rows(
          table.id, scope: s, after: nil, includeArchived: includeArchived, current: valid)
        guard page.table.schemaRevision == table.schemaRevision else { throw Store.Problem.stale }
        if valid() {
          selected = table
          directRow = row
          rows = page.items
          next = page.nextAfter
        }
      } else if let id {
        let page = try await store.rows(
          id, scope: s, after: nil, includeArchived: includeArchived, current: valid)
        if valid() {
          selected = page.table
          rows = page.items
          directRow = nil
          next = page.nextAfter
        }
      } else {
        let page = try await store.tables(
          s, after: nil, includeArchived: includeArchived, current: valid)
        if valid() {
          tables = page.items
          selected = nil
          rows = []
          directRow = nil
          next = page.nextAfter
        }
      }
      if valid() {
        loaded = true
        failure = nil
        refreshPending(s)
      }
    } catch {
      if valid() {
        failure = error.localizedDescription
        selected = nil
        tables = []
        rows = []
        directRow = nil
        next = nil
        linkCells = [:]
      }
    }
  }
  private func more(_ after: String, _ table: Wire.Table?) async {
    guard let scope, current, !busy else { return }
    busy = true
    let t = ticket
    let captured = route
    @MainActor func valid() -> Bool { current && route == captured && ticket == t }
    defer { if ticket == t { busy = false } }
    do {
      if let table {
        let p = try await store.rows(
          table.id, scope: scope, after: after, includeArchived: includeArchived, current: valid)
        guard p.table.schemaRevision == table.schemaRevision,
          Set(rows.map(\.id)).isDisjoint(with: p.items.map(\.id))
        else { throw Store.Problem.stale }
        if valid() {
          rows += p.items
          next = p.nextAfter
        }
      } else {
        let p = try await store.tables(
          scope, after: after, includeArchived: includeArchived, current: valid)
        guard Set(tables.map(\.id)).isDisjoint(with: p.items.map(\.id)) else {
          throw Store.Problem.invalid
        }
        if valid() {
          tables += p.items
          next = p.nextAfter
        }
      }
    } catch { if valid() { failure = "后续页尚未读完。 " + error.localizedDescription } }
  }
}

struct CrewdTableV3EditorTarget: Identifiable {
  enum Mode {
    case create
    case schema(CrewdManagedTableV3Wire.Table)
    case add(CrewdManagedTableV3Wire.Table)
    case patch(CrewdManagedTableV3Wire.Table, CrewdManagedTableV3Wire.Row)
    case lifecycle(CrewdManagedTableV3Wire.Table, CrewdManagedTableV3Wire.Row?, Bool)
  }
  let id = UUID()
  let mode: Mode
  let scope: CrewdManagedTableV3Store.Scope
  let route: Data
}
struct CrewdTableV3HistoryTarget: Identifiable {
  let id = UUID()
  let source: CrewdManagedTableV3Wire.HistorySource
  let table: CrewdManagedTableV3Wire.Table
  let scope: CrewdManagedTableV3Store.Scope
  let route: Data
}

private struct CrewdTableV3Editor: View {
  typealias Wire = CrewdManagedTableV3Wire
  typealias Store = CrewdManagedTableV3Store
  @ObservedObject var workspace: CrewdWorkspace
  @ObservedObject private var client: CrewdClient
  @ObservedObject var store: Store
  let target: CrewdTableV3EditorTarget
  let current: @MainActor () -> Bool
  let saved: @MainActor (Store.Observation) -> Void
  let attempted: @MainActor () -> Void
  init(
    workspace: CrewdWorkspace, store: Store, target: CrewdTableV3EditorTarget,
    current: @escaping @MainActor () -> Bool,
    saved: @escaping @MainActor (Store.Observation) -> Void,
    attempted: @escaping @MainActor () -> Void
  ) {
    self.workspace = workspace
    self.client = workspace.client
    self.store = store
    self.target = target
    self.current = current
    self.saved = saved
    self.attempted = attempted
  }
  @Environment(\.dismiss) private var dismiss
  struct LocalField: Identifiable {
    let id = UUID()
    var draft: Wire.FieldDraft
  }
  @State private var title = ""
  @State private var purpose = ""
  @State private var reason = ""
  @State private var label = ""
  @State private var policy = Wire.NewPolicy.authorCaptainHuman
  @State private var fields = [
    LocalField(draft: .init(name: "名称", kind: .init(kind: .text), nullable: false))
  ]
  @State private var titleFieldID: UUID?
  @State private var schemaAction = "addField"
  @State private var fieldId = ""
  @State private var optionId = ""
  @State private var optionLabel = ""
  @State private var optionTone = Wire.Tone.neutral
  @State private var newField = Wire.FieldDraft(name: "", kind: .init(kind: .text), nullable: true)
  @State private var text: [String: String] = [:]
  @State private var edits: [String: Wire.CellPatch] = [:]
  @State private var loaded: [String: Wire.LinkCell] = [:]
  @State private var linkValues: [String: [Wire.LinkWrite]] = [:]
  @State private var issues: [String: String] = [:]
  @State private var approval: Wire.DefinitionCheck?
  @State private var approvalKey: Data?
  @State private var checkCursor: String?
  @State private var checkCursors = Set<Data>()
  @State private var confirmNew = false
  @State private var requests = Wire.EditorRequests()
  private var busy: Bool {
    get { requests.busy }
    nonmutating set { requests.busy = newValue }
  }
  @State private var stopped = false
  @State private var initialized = false
  @State private var visible = true
  @State private var confirming = false
  @State private var problem: String?
  @State private var original: Store.Pending?
  @State private var adoptedTable: Wire.Table?
  @State private var adoptedRow: Wire.Row?
  @State private var comparedTable: Wire.Table?
  @State private var comparedRow: Wire.Row?
  @State private var hasRows: Bool?
  @State private var selectingField: Wire.Field?
  private var readTicket: UUID {
    get { requests.ticket }
    nonmutating set { requests.ticket = newValue }
  }
  private var table: Wire.Table? {
    if let adoptedTable { return adoptedTable }
    switch target.mode {
    case .create: return nil
    case .schema(let t), .add(let t), .patch(let t, _), .lifecycle(let t, _, _): return t
    }
  }
  private var row: Wire.Row? {
    if let adoptedRow { return adoptedRow }
    switch target.mode {
    case .patch(_, let r): return r
    case .lifecycle(_, let r, _): return r
    default: return nil
    }
  }
  private var creating: Bool {
    if case .create = target.mode { return true }
    return false
  }
  private var schema: Bool {
    if case .schema = target.mode { return true }
    return false
  }
  private var lifecycle: Bool {
    if case .lifecycle = target.mode { return true }
    return false
  }
  private var restoring: Bool {
    if case .lifecycle(_, _, let restore) = target.mode { return restore }
    return false
  }
  private var crew: String { table?.crewId ?? target.scope.origin.crew }
  private var live: Bool { visible && current() && store.owns(target.scope) }
  private var heading: String {
    if creating { return "新建组织表" }
    if lifecycle { return (restoring ? "恢复" : "归档") + (row?.id ?? table?.id ?? "") }
    return schema ? "管理表定义" : row.map { "编辑 " + $0.id } ?? "新增记录"
  }
  private var checkTitle: String {
    creating ? title : schemaAction == "renameTable" ? label : table?.title ?? ""
  }
  private var checkPurpose: String? {
    creating ? purpose : schemaAction == "setPurpose" ? label : table?.purpose
  }
  private var needsCheck: Bool {
    creating || (schema && ["renameTable", "setPurpose"].contains(schemaAction))
  }
  private var checkKey: Data {
    let raw: CrewdValue = .object([
      "scope": (try? CrewdKnowledgeStore.encode(target.scope.authority)) ?? .null,
      "crewId": .string(crew),
      "excludingTableId": creating ? .null : table.map { .string($0.id) } ?? .null,
      "title": .string(checkTitle), "purpose": checkPurpose.map(CrewdValue.string) ?? .null,
    ])
    return (try? CrewdOrganizationSearchWire.encoder.encode(raw)) ?? Data()
  }
  private var availableFields: [Wire.Field] {
    guard let table else { return [] }
    switch schemaAction {
    case "restoreField": return table.fields.filter(\.hidden)
    case "hideField": return table.active.filter { $0.nullable && $0.id != table.titleFieldId }
    case "setTitleField": return table.active.filter { !$0.kind.isLink }
    case "widenSingleLink":
      return table.active.filter { $0.kind.kind == .link && !$0.kind.multiple }
    case "widenText": return table.active.filter { $0.kind.kind == .text }
    case "widenNumberToText":
      return table.active.filter { [.integer, .real, .decimal].contains($0.kind.kind) }
    case "addOption", "renameOption", "deactivateOption", "restoreOption", "setOptionTone":
      return table.active.filter { $0.kind.kind == .singleSelect }
    default: return table.active
    }
  }
  private var optionChoices: [Wire.OptionDefinition] {
    guard let options = table?.field(fieldId)?.kind.options else { return [] }
    if schemaAction == "restoreOption" { return options.filter { !$0.active } }
    if schemaAction == "deactivateOption" { return options.filter(\.active) }
    return options
  }
  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 14) {
          if live {
            Group {
              if creating {
                creationForm
              } else if lifecycle {
                lifecycleForm
              } else if schema {
                schemaForm
              } else if let table {
                rowForm(table)
              }
            }.disabled(busy || original != nil || stopped)
            if needsCheck { definitionReview }
            if !creating {
              if schema || lifecycle || row != nil {
                TextField("本次变更原因（必填）", text: $reason, axis: .vertical).lineLimit(2...5)
              }
              Button("读取最新版本用于对照") { readLatest() }.disabled(busy || !live)
            }
            if let comparedTable { comparison(comparedTable) }
            if let issue = draftIssue { Text(issue).font(.caption).foregroundStyle(.orange) }
            if let problem { Text(problem).foregroundStyle(.orange).textSelection(.enabled) }
            if let original {
              Text("原请求 " + original.witness.submit.requestId + " 尚待核对；不会重新提交。").font(.caption)
                .textSelection(.enabled)
              Button("只读查询原回执") { lookup(original) }.disabled(busy || !live)
            }
            if busy { ProgressView().controlSize(.small) }
          } else {
            Text("原连接或权限已经变化，请重新打开表格。").foregroundStyle(.secondary)
          }
        }.padding(20)
      }.navigationTitle(heading).toolbar {
        ToolbarItem(placement: .cancellationAction) { Button("关闭") { dismiss() }.disabled(busy) }
        ToolbarItem(placement: .confirmationAction) {
          Button("核对并保存…") { confirming = true }.disabled(
            !live || busy || stopped || operation == nil || original != nil)
        }
      }
    }
    #if os(macOS)
      .frame(minWidth: 580, minHeight: 540)
    #endif
    .interactiveDismissDisabled(busy)
    .onAppear {
      visible = true
      initialize()
    }.onDisappear {
      visible = false
      requests.cancel()
    }
    .task { if schema { await readStructureFacts() } }
    .onChange(of: live) { _, active in
      if !active {
        requests.cancel()
        confirming = false
        selectingField = nil
      }
    }
    .onChange(of: checkKey) { _, _ in
      approval = nil
      approvalKey = nil
      checkCursor = nil
      checkCursors = []
      confirmNew = false
      requests.definitionChanged()
    }
    .onChange(of: schemaAction) { _, _ in
      fieldId = ""
      optionId = ""
      optionLabel = ""
      label = ""
      approval = nil
      approvalKey = nil
      confirmNew = false
    }
    .onChange(of: fieldId) { _, _ in
      optionId = ""
      optionLabel = ""
    }
    .confirmationDialog("保存这次明确的表格变更？", isPresented: $confirming) {
      Button("确认保存") { submit() }
      Button("取消", role: .cancel) {}
    } message: {
      Text(
        (table.map { $0.id + " · 定义版本 " + $0.schemaRevision } ?? "新组织表")
          + (row.map { " · 记录版本 " + $0.revision } ?? "") + "。使用本次核对的稳定字段与版本；原请求失败时只读查回，不自动重发。")
    }
    .sheet(item: $selectingField) { field in
      CrewdManagedV2TargetPicker(
        workspace: workspace, store: workspace.managedTablesV2, scope: target.scope, crew: crew,
        constraint: pickerConstraint(field), current: { live && selectingField?.id == field.id },
        choose: { value in acceptLink(value, field: field) })
    }
  }
  private var creationForm: some View {
    VStack(alignment: .leading, spacing: 12) {
      TextField("表名", text: $title)
      TextField("用途（必填）", text: $purpose, axis: .vertical).lineLimit(2...5)
      Picker("记录写权限", selection: $policy) {
        ForEach(Wire.NewPolicy.allCases) { Text($0.title).tag($0) }
      }
      Text("表归属于当前机组；组织成员可读，写权限按所选策略与当前授权核对。").font(.caption).foregroundStyle(.secondary)
      ForEach($fields) { $item in
        VStack(alignment: .leading, spacing: 8) {
          fieldForm($item.draft)
          if fields.count > 1 {
            Button("移除此草稿列", role: .destructive) {
              if let index = fields.firstIndex(where: { $0.id == item.id }) {
                let wasTitle = titleFieldID == item.id
                titleFieldID = Wire.titleAfterRemoval(
                  selected: titleFieldID, removed: item.id, order: fields.map(\.id))
                fields.remove(at: index)
                if wasTitle { problem = "标题列已移除，请重新选择。" }
              }
            }
          }
        }.padding(10).background(.quaternary, in: RoundedRectangle(cornerRadius: 8))
      }
      Button("增加列") {
        fields.append(.init(draft: .init(name: "", kind: .init(kind: .text), nullable: true)))
      }.disabled(fields.count >= 64)
      Picker("标题列", selection: $titleFieldID) {
        Text("请选择标题列").tag(Optional<UUID>.none)
        ForEach(fields.indices, id: \.self) { i in
          if !fields[i].draft.kind.kind.isLink {
            Text(fields[i].draft.name.isEmpty ? "第" + String(i + 1) + "列" : fields[i].draft.name)
              .tag(Optional(fields[i].id))
          }
        }
      }
    }
  }
  private func fieldForm(_ value: Binding<Wire.FieldDraft>) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      TextField("列名", text: value.name)
      Picker(
        "类型",
        selection: Binding(
          get: { value.wrappedValue.kind.kind }, set: { value.wrappedValue.kind = defaultKind($0) })
      ) { ForEach(Wire.TypeName.allCases) { Text($0.title).tag($0) } }
      Toggle("允许空值", isOn: value.nullable)
      switch value.wrappedValue.kind.kind {
      case .text: Text("新文本最多500个Unicode字符；原有列读取其实际旧范围。").font(.caption)
      case .longText: Text("新长文本最多1500个Unicode字符；不会截断旧文本。").font(.caption)
      case .decimal:
        TextField(
          "单位（可选）",
          text: Binding(
            get: { value.wrappedValue.kind.unit ?? "" },
            set: { value.wrappedValue.kind.unit = $0.isEmpty ? nil : $0 }))
        Picker(
          "最少显示小数位",
          selection: Binding(
            get: { value.wrappedValue.kind.displayScale ?? -1 },
            set: { value.wrappedValue.kind.displayScale = $0 < 0 ? nil : $0 })
        ) {
          Text("按原值").tag(-1)
          ForEach(0...18, id: \.self) { Text(String($0)).tag($0) }
        }
        Text("最多38位有效数字、小数最多18位；显示补零不改存值、不舍入。").font(.caption)
      case .singleSelect:
        ForEach(value.wrappedValue.kind.options ?? []) { option in
          HStack {
            TextField(
              "选项",
              text: Binding(
                get: { value.wrappedValue.kind.options?.first { $0.id == option.id }?.label ?? "" },
                set: { text in
                  if let i = value.wrappedValue.kind.options?.firstIndex(where: {
                    $0.id == option.id
                  }) {
                    value.wrappedValue.kind.options?[i].label = text
                  }
                }))
            Picker(
              "色调",
              selection: Binding(
                get: {
                  value.wrappedValue.kind.options?.first { $0.id == option.id }?.tone ?? .neutral
                },
                set: { tone in
                  if let i = value.wrappedValue.kind.options?.firstIndex(where: {
                    $0.id == option.id
                  }) {
                    value.wrappedValue.kind.options?[i].tone = tone
                  }
                })
            ) { ForEach(Wire.Tone.allCases) { Text($0.title).tag($0) } }
            Button("移除") { value.wrappedValue.kind.options?.removeAll { $0.id == option.id } }
          }
        }
        Button("增加选项") { value.wrappedValue.kind.options?.append(.init(label: "")) }.disabled(
          (value.wrappedValue.kind.options?.count ?? 0) >= 12)
        Text("选项有服务端发行的稳定编号；之后可停用并保留原值。").font(.caption)
      case .person:
        Toggle(
          "允许选择人类",
          isOn: Binding(
            get: { value.wrappedValue.kind.allowHuman ?? false },
            set: { value.wrappedValue.kind.allowHuman = $0 }))
        Text("人员保存为真实成员或人类的关联。").font(.caption)
      case .link:
        Picker(
          "关联对象类型",
          selection: Binding(
            get: { value.wrappedValue.kind.target?.kind ?? "work" },
            set: { value.wrappedValue.kind.target = .init(kind: $0) })
        ) { ForEach(constraintKinds, id: \.self) { Text(Wire.Constraint(kind: $0).title).tag($0) } }
        if value.wrappedValue.kind.target?.kind == "row" {
          TextField(
            "限定所属表T编号（可选）",
            text: Binding(
              get: { value.wrappedValue.kind.target?.tableId ?? "" },
              set: { value.wrappedValue.kind.target?.tableId = $0.isEmpty ? nil : $0 }))
        }
        Toggle(
          "允许多个目标",
          isOn: Binding(
            get: { value.wrappedValue.kind.many ?? false },
            set: { value.wrappedValue.kind.many = $0 }))
      case .real: Text("IEEE浮点数；需要精确十进制时选择精确小数。").font(.caption)
      case .date: Text("公历日期 YYYY-MM-DD；不含时间和时区。").font(.caption)
      case .boolean: Text("必须明确选择是或否；空值与否不同。").font(.caption)
      case .url: Text("绝对HTTP/HTTPS网址；此标量不会自动登记产物。").font(.caption)
      default: EmptyView()
      }
    }
  }
  private var constraintKinds: [String] {
    var kinds = [
      "work", "requirement", "ask", "table", "row", "decision", "risk", "lesson", "artifact",
      "crew", "session", "channel", "message", "human", "anyReadable",
    ]
    if workspace.client.capabilities.contains(CrewdManagedTableV2Wire.knowledgeCapability) {
      kinds += ["document", "knowledgeView"]
    }
    return kinds
  }
  private func defaultKind(_ type: Wire.TypeName) -> Wire.KindDraft {
    switch type {
    case .decimal: return .init(kind: type)
    case .singleSelect:
      return .init(kind: type, options: [.init(label: "选项1"), .init(label: "选项2")])
    case .person: return .init(kind: type, allowHuman: true)
    case .link: return .init(kind: type, target: .init(kind: "work"), many: false)
    default: return .init(kind: type)
    }
  }
  private var schemaForm: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text(table!.title + " · " + table!.id).font(.headline)
      Picker("变更", selection: $schemaAction) {
        Text("新增列").tag("addField")
        Text("修改列名").tag("renameField")
        Text("修改表名").tag("renameTable")
        Text("修改用途").tag("setPurpose")
        Text("隐藏列").tag("hideField")
        Text("恢复列").tag("restoreField")
        Text("选择标题列").tag("setTitleField")
        Text("单关联改多关联").tag("widenSingleLink")
        Text("文本改长文本").tag("widenText")
        Text("数值改文本").tag("widenNumberToText")
        Text("增加单选项").tag("addOption")
        Text("修改选项标签").tag("renameOption")
        Text("停用选项").tag("deactivateOption")
        Text("恢复选项").tag("restoreOption")
        Text("修改选项色调").tag("setOptionTone")
      }
      if schemaAction == "addField" {
        fieldForm($newField)
        Text("已有记录的表只能新增允许空值的列；既有数据不被补写。").font(.caption)
        if hasRows == nil {
          Button("读取是否空表的当前状态") { Task { await readStructureFacts() } }.disabled(busy || !live)
        }
      } else if ["renameTable", "setPurpose"].contains(schemaAction) {
        TextField(schemaAction == "renameTable" ? "新表名" : "新用途（必填）", text: $label, axis: .vertical)
      } else {
        Picker("稳定列", selection: $fieldId) {
          Text("请选择").tag("")
          ForEach(availableFields) { Text($0.name + " · " + $0.id).tag($0.id) }
        }
        if schemaAction == "renameField" { TextField("新列名", text: $label) }
        if schemaAction == "hideField" { Text("必填列暂不能隐藏；标题列需先选择其他标题列。").font(.caption) }
        if schemaAction == "widenText" { Text("保留此列的实际旧文本范围，不裁剪原值。").font(.caption) }
        if schemaAction == "widenNumberToText" {
          Text("原数值保持原证据，当前读取采用可往返的文本表示；不批量重写整表。").font(.caption)
        }
        if schemaAction == "addOption" {
          TextField("新选项标签", text: $optionLabel)
          tonePicker
        }
        if ["renameOption", "deactivateOption", "restoreOption", "setOptionTone"].contains(
          schemaAction)
        {
          Picker("选项编号", selection: $optionId) {
            Text("请选择").tag("")
            ForEach(optionChoices) {
              Text($0.label + " · " + $0.id + ($0.active ? "" : " · 已停用")).tag($0.id)
            }
          }
          if schemaAction == "renameOption" { TextField("新选项标签", text: $optionLabel) }
          if schemaAction == "setOptionTone" { tonePicker }
          if schemaAction == "deactivateOption" { Text("原记录继续显示并保留此选项；新的填写不能选择它。").font(.caption) }
        }
      }
    }
  }
  private var tonePicker: some View {
    Picker("色调", selection: $optionTone) { ForEach(Wire.Tone.allCases) { Text($0.title).tag($0) } }
  }
  private var lifecycleForm: some View {
    VStack(alignment: .leading, spacing: 10) {
      Text((row?.id ?? table!.id) + " · " + table!.title).font(.headline)
      Text(
        restoring
          ? "恢复保留原编号、列和值；所属机组或上级表仍归档时，不能通过此动作绕过。"
          : row == nil ? "归档此表后，其记录与表定义只读；不会删除内容、关联或原回执。" : "归档此记录后只读；这不是计划完成或业务关闭。")
      if let prior = (row?.lifecycle ?? table!.lifecycle).reason {
        Text("原归档原因：" + prior).textSelection(.enabled)
      }
    }
  }
  private var definitionReview: some View {
    VStack(alignment: .leading, spacing: 8) {
      Button(checkCursor == nil ? "检查名称与用途" : "继续检查名称与用途") { checkDefinition() }.disabled(
        busy || !live || !Wire.label(checkTitle, limit: 256)
          || !(checkPurpose.map { Wire.label($0, limit: 1500) } ?? (table != nil)))
      if let approval, approvalKey == checkKey {
        if approval.state == "checking" { Text("检查尚未完成，不能据此新建或修改定义。").font(.caption) }
        if approval.exactNameTaken { Text("同机组已有此名称；归档表仍占用名称，请修改表名。").foregroundStyle(.orange) }
        ForEach(approval.nearCandidates) { candidate in
          VStack(alignment: .leading) {
            Text(candidate.title + " · " + candidate.tableId + (candidate.archived ? " · 已归档" : ""))
            if let purpose = candidate.purpose { Text(purpose).font(.caption) }
          }
        }
        if approval.moreSimilar { Text("还有其他名称或用途相近的表。").font(.caption) }
        if approval.state == "complete", !approval.exactNameTaken,
          !approval.nearCandidates.isEmpty || approval.moreSimilar
        {
          Toggle("我已核对相近表，仍明确创建这份新定义", isOn: $confirmNew)
        }
      }
    }
  }
  private func rowForm(_ table: Wire.Table) -> some View {
    VStack(alignment: .leading, spacing: 14) {
      ForEach(table.active) { field in
        VStack(alignment: .leading, spacing: 7) {
          HStack {
            Text(field.name + " · " + field.id).font(.headline)
            Text(field.kind.title + (field.nullable ? " · 可空" : " · 必填")).font(.caption)
              .foregroundStyle(.secondary)
          }
          if field.kind.isLink {
            linkForm(field)
          } else {
            if let row {
              Text("原值：" + cellDescription(row.scalar(field.id), field)).font(.caption)
                .foregroundStyle(.secondary)
            } else if edits[field.id] == nil && field.nullable {
              Text("尚未填写，将保持空值。").font(.caption).foregroundStyle(.secondary)
            }
            scalarForm(field)
          }
          if row != nil { Button("保留此列原值") { discard(field) } }
          if let issue = issues[field.id] { Text(issue).font(.caption).foregroundStyle(.orange) }
        }
      }
    }
  }
  private func cellDescription(_ cell: Wire.Cell, _ field: Wire.Field) -> String {
    if case .text(let value) = cell, value.isEmpty { return "空文本" }
    return cell.display(field)
  }
  private func scalarValue(_ f: Wire.Field) -> Wire.Cell {
    edits[f.id]?.value ?? row?.scalar(f.id) ?? .null
  }
  @ViewBuilder private func scalarForm(_ field: Wire.Field) -> some View {
    if field.kind.kind == .boolean {
      Picker(
        "明确选择",
        selection: Binding(
          get: {
            if case .boolean(let b) = scalarValue(field) { return b ? "true" : "false" }
            return (row != nil || edits[field.id] != nil) && scalarValue(field) == .null
              ? "null" : ""
          },
          set: { value in
            if value == "true" {
              set(.boolean(true), field)
            } else if value == "false" {
              set(.boolean(false), field)
            } else if value == "null" {
              set(.null, field)
            } else {
              edits.removeValue(forKey: field.id)
            }
          })
      ) {
        Text("尚未选择").tag("")
        Text("是").tag("true")
        Text("否").tag("false")
        if field.nullable { Text("空值").tag("null") }
      }
    } else if field.kind.kind == .singleSelect {
      Picker(
        "选项",
        selection: Binding(
          get: {
            if case .singleSelect(let r) = scalarValue(field) { return r.optionId }
            return (row != nil || edits[field.id] != nil) && scalarValue(field) == .null
              ? "null" : ""
          },
          set: { value in
            if value == "null" {
              set(.null, field)
            } else if !value.isEmpty {
              set(
                .singleSelect(.init(tableId: table!.id, fieldId: field.id, optionId: value)), field)
            } else {
              edits.removeValue(forKey: field.id)
            }
          })
      ) {
        Text("尚未选择").tag("")
        ForEach(field.kind.options ?? []) { o in
          Text(o.label + (o.active ? "" : "（已停用，原值可保留）")).foregroundStyle(
            CrewdTableV3Palette.color(o.tone)
          ).tag(o.id).disabled(!o.active)
        }
        if field.nullable { Text("空值").tag("null") }
      }
      if field.kind.options?.contains(where: { $0.active }) != true {
        Text("此列尚无启用选项，请先在表定义中增加或恢复选项。").font(.caption).foregroundStyle(.secondary)
      }
    } else if field.kind.kind == .longText {
      TextEditor(text: Binding(get: { text[field.id] ?? "" }, set: { changeText($0, field) }))
        .frame(minHeight: 120)
    } else {
      TextField(
        field.kind.kind == .date
          ? "YYYY-MM-DD" : field.kind.kind == .url ? "https://…" : field.name,
        text: Binding(get: { text[field.id] ?? "" }, set: { changeText($0, field) }),
        axis: field.kind.kind == .text ? .vertical : .horizontal
      ).lineLimit(1...5)
    }
    if field.kind.kind == .text || field.kind.kind == .longText {
      Button("明确写入空文本") {
        text[field.id] = ""
        set(.text(""), field)
      }
    }
    if field.nullable, field.kind.kind != .boolean, field.kind.kind != .singleSelect {
      Button("明确设为空值") { set(.null, field) }
    }
    if edits[field.id]?.value == .null { Text("本次将写入空值。").font(.caption) }
  }
  private func linkForm(_ field: Wire.Field) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      if row != nil, loaded[field.id] == nil {
        Text("此列关联尚未读取；保持原值不会清空它。").font(.caption)
        Button("读取此列原关联") { loadLinks(field) }.disabled(busy || !live)
      } else {
        ForEach(Array((linkValues[field.id] ?? []).enumerated()), id: \.offset) { i, value in
          HStack {
            Text(value.target?.label ?? value.input?.value ?? "新产物").textSelection(.enabled)
            Button("移除") {
              linkValues[field.id]?.remove(at: i)
              markLinks(field)
            }
          }
        }
        if loaded[field.id]?.hiddenSet != nil {
          Text("部分原关联当前不可见，整组保留；可见项不能净增加。").font(.caption).foregroundStyle(.secondary)
        }
        if loaded[field.id]?.incompleteRequiredLink == true {
          Text("原必填目标已删除，可明确补充；保留此列仍可编辑其他列。").font(.caption).foregroundStyle(.orange)
        }
        Button(field.kind.kind == .person ? "选择成员" : "选择关联对象") { selectingField = field }.disabled(
          busy || !live || !canChoose(field))
        if !canChoose(field) {
          Text("当前连接不能选择此类目标；原关联仍可保留。").font(.caption).foregroundStyle(.secondary)
        }
        if field.kind.kind == .person, field.kind.allowHuman == true {
          Button("选择人类") { chooseHuman(field) }.disabled(busy || !live || !canChoose(field))
        }
        if (linkValues[field.id] ?? []).isEmpty { Text("当前可见选择为空。").font(.caption) }
      }
    }
  }
  private func initialize() {
    guard !initialized else { return }
    initialized = true
    if creating { titleFieldID = fields.first?.id }
    for f in table?.active ?? [] {
      if !f.kind.isLink {
        text[f.id] = (row?.scalar(f.id) == .null ? "" : row?.scalar(f.id).display) ?? ""
      }
    }
  }
  private func set(_ value: Wire.Cell, _ field: Wire.Field) {
    edits[field.id] = .init(kind: "setScalar", value: value)
    issues.removeValue(forKey: field.id)
    if !value.accepts(field, tableId: table!.id, allowInactive: false) {
      issues[field.id] = "此值不符合当前列的类型、范围或选项状态。"
    }
  }
  private func changeText(_ value: String, _ field: Wire.Field) {
    text[field.id] = value
    let cell: Wire.Cell?
    switch field.kind.kind {
    case .text, .longText: cell = .text(value)
    case .integer: cell = Int64(value).flatMap { String($0) == value ? .integer(value) : nil }
    case .real: cell = Double(value).flatMap { $0.isFinite ? .real($0) : nil }
    case .decimal: cell = Wire.decimal(value) ? .decimal(value) : nil
    case .date: cell = Wire.date(value) ? .date(value) : nil
    case .url: cell = Wire.url(value) ? .url(value) : nil
    default: cell = nil
    }
    if let cell { set(cell, field) } else { issues[field.id] = "请填写此类型的明确合法值；未使用上一份有效输入。" }
  }
  private func discard(_ f: Wire.Field) {
    edits.removeValue(forKey: f.id)
    issues.removeValue(forKey: f.id)
    text[f.id] = (row?.scalar(f.id) == .null ? "" : row?.scalar(f.id).display) ?? ""
    if let cell = loaded[f.id] {
      linkValues[f.id] = cell.visible.map { .init(kind: "reference", target: $0.target) }
    } else {
      linkValues.removeValue(forKey: f.id)
    }
  }
  private func canChoose(_ field: Wire.Field) -> Bool {
    guard workspace.managedTablesV2.supportsLinks else { return false }
    if ["document", "knowledgeView"].contains(field.kind.target?.kind ?? "") {
      return client.capabilities.contains(Wire.V2.knowledgeCapability)
    }
    return true
  }
  private func pickerConstraint(_ field: Wire.Field) -> Wire.Constraint? {
    if field.kind.kind == .person { return .init(kind: "session") }
    guard var constraint = field.kind.target else { return nil }
    if constraint.kind == "row" { constraint.tableId = nil }
    return constraint
  }
  private func loadLinks(_ field: Wire.Field) {
    guard let row, let table, live, !busy else { return }
    busy = true
    let t = UUID()
    readTicket = t
    Task {
      defer { if readTicket == t { busy = false } }
      do {
        let cell = try await store.links(
          row, field: field, table: table, scope: target.scope, current: { live && readTicket == t }
        )
        guard live, readTicket == t else { return }
        loaded[field.id] = cell
        linkValues[field.id] = cell.visible.map { .init(kind: "reference", target: $0.target) }
        issues.removeValue(forKey: field.id)
      } catch { if live, readTicket == t { problem = error.localizedDescription } }
    }
  }
  private func markLinks(_ field: Wire.Field) {
    edits[field.id] = .init(
      kind: "replaceLinks", values: linkValues[field.id] ?? [],
      keepHiddenSetToken: loaded[field.id]?.hiddenSet?.keepToken)
    issues.removeValue(forKey: field.id)
  }
  private func chooseHuman(_ field: Wire.Field) {
    guard live, !busy else { return }
    busy = true
    let t = UUID()
    readTicket = t
    Task {
      defer { if t == readTicket { busy = false } }
      do {
        let resolved = try await workspace.managedTablesV2.resolve(
          .init(kind: .human), scope: target.scope, current: { live && readTicket == t })
        guard live, readTicket == t, resolved.target.kind == .human else { return }
        applyLink(.init(kind: "reference", target: resolved.target), field)
      } catch { if live, readTicket == t { problem = error.localizedDescription } }
    }
  }
  private func acceptLink(_ value: Wire.LinkWrite, field: Wire.Field) {
    guard live, selectingField?.id == field.id else { return }
    if field.kind.kind == .person && value.target?.kind != .session {
      problem = "此人员列只能选择真实成员。"
      return
    }
    if field.kind.target?.kind == "row", let required = field.kind.target?.tableId,
      let id = value.target?.idValue
    {
      busy = true
      let t = UUID()
      readTicket = t
      Task {
        defer { if t == readTicket { busy = false } }
        do {
          let (actual, _) = try await store.row(
            id, scope: target.scope,
            current: { live && selectingField?.id == field.id && readTicket == t })
          guard live, selectingField?.id == field.id, readTicket == t, actual.id == required else {
            throw Store.Problem.invalid
          }
          applyLink(value, field)
          selectingField = nil
        } catch { if live, readTicket == t { problem = error.localizedDescription } }
      }
      return
    }
    applyLink(value, field)
    selectingField = nil
  }
  private func applyLink(_ value: Wire.LinkWrite, _ field: Wire.Field) {
    var values = linkValues[field.id] ?? []
    if !field.kind.multiple { values = [] }
    if !values.contains(where: { Wire.V2.exact($0, value) }) { values.append(value) }
    linkValues[field.id] = values
    markLinks(field)
  }
  private var schemaChange: Wire.SchemaChange? {
    switch schemaAction {
    case "addField": return .init(kind: schemaAction, field: newField)
    case "renameField": return .init(kind: schemaAction, fieldId: fieldId, name: label)
    case "renameTable":
      return .init(
        kind: schemaAction, title: label, definitionCheckToken: approval?.definitionCheckToken,
        confirmNew: confirmNew)
    case "setPurpose":
      return .init(
        kind: schemaAction, purpose: label, definitionCheckToken: approval?.definitionCheckToken,
        confirmNew: confirmNew)
    case "addOption":
      return .init(
        kind: schemaAction, fieldId: fieldId,
        optionDraft: .init(label: optionLabel, tone: optionTone))
    case "renameOption":
      return .init(
        kind: schemaAction, fieldId: fieldId, option: optionReference, optionLabel: optionLabel)
    case "deactivateOption", "restoreOption":
      return .init(kind: schemaAction, fieldId: fieldId, option: optionReference)
    case "setOptionTone":
      return .init(kind: schemaAction, fieldId: fieldId, option: optionReference, tone: optionTone)
    default: return .init(kind: schemaAction, fieldId: fieldId)
    }
  }
  private var optionReference: Wire.OptionRef? {
    guard let table, !fieldId.isEmpty, !optionId.isEmpty else { return nil }
    return .init(tableId: table.id, fieldId: fieldId, optionId: optionId)
  }
  private var checkComplete: Bool {
    guard let approval, approvalKey == checkKey, approval.state == "complete",
      !approval.exactNameTaken, approval.definitionCheckToken != nil
    else { return false }
    return (approval.nearCandidates.isEmpty && !approval.moreSimilar) || confirmNew
  }
  private var operation: Wire.Operation? {
    guard issues.isEmpty, original == nil else { return nil }
    if creating {
      guard checkComplete, let titleFieldID,
        let titleIndex = fields.firstIndex(where: { $0.id == titleFieldID }),
        !fields[titleIndex].draft.kind.kind.isLink
      else { return nil }
      let op = Wire.Operation(
        kind: "createTable", title: title, purpose: purpose, fields: fields.map(\.draft),
        titleFieldIndex: titleIndex, rowWritePolicy: policy,
        definitionCheckToken: approval?.definitionCheckToken, confirmNew: confirmNew)
      return op.valid ? op : nil
    }
    guard let table else { return nil }
    if lifecycle {
      guard !table.frozenReasons.contains(where: { $0.kind == "crewArchived" }),
        row == nil || !table.lifecycle.archived
      else { return nil }
      let op: Wire.Operation
      if let row {
        op = .init(
          kind: restoring ? "restoreRow" : "archiveRow", rowId: row.id,
          expectedSchemaRevision: table.schemaRevision, expectedRowRevision: row.revision,
          reason: reason)
      } else {
        op = .init(
          kind: restoring ? "restoreTable" : "archiveTable", tableId: table.id,
          expectedSchemaRevision: table.schemaRevision, reason: reason)
      }
      return op.valid ? op : nil
    }
    guard !table.frozen, row?.frozen != true else { return nil }
    if schema {
      guard !needsCheck || checkComplete, let change = schemaChange else { return nil }
      if schemaAction == "addField" {
        guard table.fields.count < 64, newField.nullable || hasRows == false else { return nil }
      }
      if !["addField", "renameTable", "setPurpose"].contains(schemaAction) {
        guard availableFields.contains(where: { $0.id == fieldId }) else { return nil }
      }
      if schemaAction == "addOption" {
        guard let options = table.field(fieldId)?.kind.options, options.count < 256,
          options.filter(\.active).count < 12,
          !options.contains(where: {
            Wire.asciiLower($0.label).utf8.elementsEqual(Wire.asciiLower(optionLabel).utf8)
          })
        else { return nil }
      }
      if schemaAction == "restoreOption" {
        guard (table.field(fieldId)?.kind.options?.filter(\.active).count ?? 12) < 12 else {
          return nil
        }
      }
      let op = Wire.Operation(
        kind: "alterTable", tableId: table.id, expectedSchemaRevision: table.schemaRevision,
        reason: reason, change: change)
      return op.valid ? op : nil
    }
    guard let patches = try? Wire.rowPatches(table: table, row: row, edits: edits, loaded: loaded)
    else { return nil }
    let op: Wire.Operation
    if let row {
      op = .init(
        kind: "patchRow", rowId: row.id, expectedSchemaRevision: table.schemaRevision,
        expectedRowRevision: row.revision, reason: reason, patches: patches)
    } else {
      op = .init(
        kind: "addRow", tableId: table.id, expectedSchemaRevision: table.schemaRevision,
        patches: patches)
    }
    return op.valid ? op : nil
  }
  private var draftIssue: String? {
    if creating {
      if !Wire.label(title, limit: 256) { return "请填写表名，最多256个字符，首尾不留空白。" }
      if !Wire.label(purpose, limit: 1500) { return "请填写用途，最多1500个字符，首尾不留空白。" }
      if !fields.allSatisfy({ $0.draft.valid }) { return "请核对每列的名称、类型设置与选项；选项标签不可重复。" }
      if titleFieldID.flatMap({ id in fields.first(where: { $0.id == id }) }).map({
        !$0.draft.kind.kind.isLink
      }) != true {
        return "请选择一个有效的标量标题列。"
      }
    }
    if let issue = issues.values.first { return issue }
    if let table {
      if lifecycle {
        if table.frozenReasons.contains(where: { $0.kind == "crewArchived" }) {
          return "所属机组已归档，不能在此改变表或记录的归档状态。"
        }
        if row != nil && table.lifecycle.archived { return "请先恢复所属表；记录自身的归档状态仍保留。" }
      } else if table.frozen || row?.frozen == true {
        return "当前表或记录只读，请读取最新状态核对。"
      }
      if schema && schemaAction == "addField" {
        if table.fields.count >= 64 { return "此表已达到64列上限（包含隐藏列）。" }
        if !newField.nullable && hasRows != false {
          return hasRows == nil ? "尚未核对表内是否已有记录；请先读取当前状态。" : "此表已有记录，只能新增允许空值的列。"
        }
      }
      if schema && ["addOption", "restoreOption"].contains(schemaAction) {
        let options = table.field(fieldId)?.kind.options ?? []
        if options.filter(\.active).count >= 12 { return "此列已有12个启用选项，请先停用一个选项。" }
        if schemaAction == "addOption" && options.count >= 256 {
          return "此列已达到256个保留选项上限；停用不会删除旧值。"
        }
      }
    }
    if !creating, schema || lifecycle || row != nil, !Wire.label(reason, limit: 1500) {
      return "请填写非空变更原因（不含换行，首尾不留空白），最多1500个字符。"
    }
    if needsCheck && !checkComplete { return "请完成名称与用途检查，并明确核对相近定义。" }
    if !schema && !creating && !lifecycle, let table {
      do { _ = try Wire.rowPatches(table: table, row: row, edits: edits, loaded: loaded) } catch {
        return error.localizedDescription
      }
    }
    return nil
  }
  private func checkDefinition() {
    guard live, !busy else { return }
    let captured = checkKey
    let after = checkCursor
    let t = requests.begin()
    let definitionVersion = requests.definitionVersion
    problem = nil
    Task {
      defer { requests.finish(t) }
      do {
        let result = try await store.definition(
          target.scope, crew: crew, excluding: creating ? nil : table?.id, title: checkTitle,
          purpose: checkPurpose, cursor: after,
          current: {
            live && checkKey == captured && requests.definitionVersion == definitionVersion
              && readTicket == t
          })
        guard live, checkKey == captured, requests.definitionVersion == definitionVersion,
          readTicket == t
        else { return }
        if let next = result.nextCursor {
          guard !checkCursors.contains(Data(next.utf8)) else { throw Store.Problem.invalid }
          checkCursors.insert(Data(next.utf8))
        }
        approval = result
        approvalKey = captured
        checkCursor = result.nextCursor
        confirmNew = false
      } catch {
        if live, checkKey == captured, requests.definitionVersion == definitionVersion,
          readTicket == t
        {
          approval = nil
          approvalKey = nil
          checkCursor = nil
          checkCursors = []
          problem = error.localizedDescription
        }
      }
    }
  }
  private func readStructureFacts(ticket: UUID? = nil) async {
    guard schema, let table, live else { return }
    let t: UUID
    if let ticket {
      guard requests.owns(ticket), busy else { return }
      t = ticket
    } else {
      guard !busy else { return }
      t = requests.begin()
    }
    defer { requests.finish(t) }
    do {
      let page = try await store.rows(
        table.id, scope: target.scope, after: nil, includeArchived: true,
        current: { live && readTicket == t })
      guard live, readTicket == t, page.table.schemaRevision == table.schemaRevision else {
        throw Store.Problem.stale
      }
      hasRows = !page.items.isEmpty || page.nextAfter != nil
    } catch {
      if live, readTicket == t {
        hasRows = nil
        problem = error.localizedDescription
      }
    }
  }
  private func readLatest() {
    guard let table, live, !busy else { return }
    busy = true
    let t = UUID()
    readTicket = t
    Task {
      defer { if readTicket == t { busy = false } }
      do {
        let actual: Wire.Table
        let actualRow: Wire.Row?
        if let row {
          let pair = try await store.row(
            row.id, scope: target.scope, current: { live && readTicket == t })
          actual = pair.0
          actualRow = pair.1
        } else {
          actual = try await store.table(
            table.id, scope: target.scope, current: { live && readTicket == t })
          actualRow = nil
        }
        guard live, readTicket == t, actual.id == table.id, Wire.bytes(actual.crewId, table.crewId)
        else { return }
        comparedTable = actual
        comparedRow = actualRow
      } catch { if live, readTicket == t { problem = error.localizedDescription } }
    }
  }
  private func comparison(_ actual: Wire.Table) -> some View {
    DisclosureGroup(
      "当前实际定义版本 " + actual.schemaRevision + (comparedRow.map { " · 记录版本 " + $0.revision } ?? "")
    ) {
      ForEach(actual.active) { f in
        Text(f.name + " · " + f.id + " · " + f.kind.title).font(.caption)
        if !f.kind.isLink, let comparedRow {
          Text(comparedRow.scalar(f.id).display(f)).textSelection(.enabled)
        }
      }
      ForEach(edits.keys.sorted(), id: \.self) { id in
        Button("放弃 " + id + " 的本地修改") {
          edits.removeValue(forKey: id)
          issues.removeValue(forKey: id)
        }
      }
      Button("采用此版本重新核对本地编辑") { adoptCompared() }.disabled(busy || original != nil || !live)
      Text("只保留你实际编辑且仍兼容的字段；未编辑列采用当前值。含隐藏关系的编辑需要重新读取并明确选择。").font(.caption)
    }
  }
  private func adoptCompared() {
    guard let actual = comparedTable, live, !busy, original == nil else { return }
    let t = requests.begin()
    Task {
      defer { requests.finish(t) }
      do {
        var newLoaded: [String: Wire.LinkCell] = [:]
        for (id, edit) in edits {
          guard let field = actual.active.first(where: { $0.id == id }) else {
            throw Wire.Failure.bounds(id)
          }
          if edit.kind == "setScalar" {
            guard edit.value?.accepts(field, tableId: actual.id, allowInactive: false) == true
            else { throw Wire.Failure.bounds(field.name) }
          }
          if edit.kind == "replaceLinks", let row = comparedRow {
            let cell = try await store.links(
              row, field: field, table: actual, scope: target.scope,
              current: { live && readTicket == t })
            guard loaded[id]?.hiddenSet == nil, cell.hiddenSet == nil else {
              throw Wire.Failure.unloaded(field.name)
            }
            newLoaded[id] = cell
          }
        }
        guard live, readTicket == t else { return }
        adoptedTable = actual
        adoptedRow = comparedRow
        hasRows = nil
        loaded = newLoaded
        for f in actual.active where edits[f.id] == nil {
          text[f.id] =
            (comparedRow?.scalar(f.id) == .null ? "" : comparedRow?.scalar(f.id).display) ?? ""
        }
        comparedTable = nil
        comparedRow = nil
        approval = nil
        approvalKey = nil
        checkCursor = nil
        checkCursors = []
        confirmNew = false
        stopped = false
        problem = nil
        if schema {
          guard let factsTicket = requests.handoff(from: t) else { return }
          await readStructureFacts(ticket: factsTicket)
        }
      } catch { if live, readTicket == t { problem = error.localizedDescription } }
    }
  }
  private func submit() {
    guard live, !busy, !stopped, original == nil, let operation else { return }
    busy = true
    problem = nil
    Task {
      defer { busy = false }
      guard live else { return }
      do {
        let observed = try await store.submit(
          operation, crew: crew, scope: target.scope, current: { live })
        guard live else { return }
        attempted()
        saved(observed)
      } catch {
        guard live else { return }
        attempted()
        problem = error.localizedDescription
        if !(error is Store.DefinitiveRefusal) { stopped = true }
        do {
          original = try store.pending(target.scope).first {
            $0.matchesSubmission(
              scope: target.scope.authority, crew: crew, target: operation.target)
          }
        } catch { problem = error.localizedDescription }
      }
    }
  }
  private func lookup(_ original: Store.Pending) {
    guard live, !busy else { return }
    busy = true
    Task {
      defer { busy = false }
      do {
        let result = try await store.lookup(original, scope: target.scope, current: { live })
        guard live else { return }
        attempted()
        if let result { saved(result) } else { problem = "原请求结果仍未知；保留原请求编号，未重新提交。" }
      } catch { if live { problem = error.localizedDescription } }
    }
  }
}

private struct CrewdTableV3HistoryView: View {
  typealias Wire = CrewdManagedTableV3Wire
  typealias Store = CrewdManagedTableV3Store
  @ObservedObject var workspace: CrewdWorkspace
  @ObservedObject private var client: CrewdClient
  @ObservedObject var store: Store
  let target: CrewdTableV3HistoryTarget
  let current: @MainActor () -> Bool
  init(
    workspace: CrewdWorkspace, store: Store, target: CrewdTableV3HistoryTarget,
    current: @escaping @MainActor () -> Bool
  ) {
    self.workspace = workspace
    self.client = workspace.client
    self.store = store
    self.target = target
    self.current = current
  }
  @Environment(\.dismiss) private var dismiss
  @State private var items: [Wire.HistorySummary] = []
  @State private var next: String?
  @State private var loaded = false
  @State private var busy = false
  @State private var visible = true
  @State private var ticket = UUID()
  @State private var pageCursors = Set<Data>()
  @State private var selected: Wire.HistorySummary?
  @State private var path: Wire.HistoryPath?
  @State private var text = ""
  @State private var valueType: String?
  @State private var links: [Wire.V2.VisibleLink] = []
  @State private var restricted = false
  @State private var unavailable = false
  @State private var valueNext: String?
  @State private var valueCursors = Set<Data>()
  @State private var problem: String?
  private var live: Bool {
    visible && current() && store.owns(target.scope) && workspace.client.readyForWork
  }
  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 12) {
          if live {
            Text("实际变更历史 · " + target.source.id).font(.headline)
            Text("历史只展示保留的原动作证据；受限或未保存的旧值不会补造。").font(.caption).foregroundStyle(.secondary)
            if let problem { Text(problem).foregroundStyle(.orange).textSelection(.enabled) }
            if let selected { valueView(selected) }
            ForEach(items) { entry in eventView(entry) }
            if let next { Button("更多历史记录") { readPage(next) }.disabled(busy) }
            if loaded && items.isEmpty && !busy {
              Text(next == nil ? "没有保留的历史记录。" : "本批没有可见记录，仍有后续页。").foregroundStyle(.secondary)
            }
            if busy { ProgressView().controlSize(.small) }
          } else {
            Text("历史来源或当前权限已变化，请关闭后重新打开。").foregroundStyle(.secondary)
          }
        }.padding(20)
      }.navigationTitle("表格变更历史").toolbar {
        ToolbarItem(placement: .cancellationAction) { Button("关闭") { dismiss() } }
      }
    }
    #if os(macOS)
      .frame(minWidth: 600, minHeight: 520)
    #endif
    .task { if live { readPage(nil) } }.onAppear { visible = true }.onDisappear {
      visible = false
      ticket = UUID()
      clearValue()
    }
    .onChange(of: live) { _, active in
      if !active {
        ticket = UUID()
        busy = false
        items = []
        next = nil
        clearValue()
      }
    }
    .onChange(of: store.invalidation) { _, _ in
      ticket = UUID()
      busy = false
      items = []
      next = nil
      loaded = false
      clearValue()
      problem = "数据或权限已变化，请显式重新读取历史。"
    }
    .safeAreaInset(edge: .bottom) {
      if live {
        Button("重新读取当前历史") {
          ticket = UUID()
          busy = false
          items = []
          next = nil
          pageCursors = []
          clearValue()
          readPage(nil)
        }.disabled(busy).padding(10)
      }
    }
  }
  private func action(_ a: String) -> String {
    [
      "createTable": "建立表", "alterTable": "修改表定义", "addRow": "新增记录", "patchRow": "修改记录",
      "archiveTable": "归档表", "restoreTable": "恢复表", "archiveRow": "归档记录", "restoreRow": "恢复记录",
      "legacyRowWrite": "旧版记录变更",
    ][a] ?? "表格变更"
  }
  private func eventView(_ entry: Wire.HistorySummary) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(action(entry.action)).font(.headline)
      Text(
        entry.recordedAt + " · "
          + (entry.actor.kind == "human" ? "人类" : entry.actor.sessionId ?? "成员")
      ).font(.caption)
      if let before = entry.originalRevision, let after = entry.resultRevision {
        Text("版本 " + before + " → " + after).font(.caption)
      }
      if let reason = entry.reasonPreview { Text(reason).textSelection(.enabled) }
      if entry.legacyDetailsUnavailable {
        Text("旧动作没有保留完整前后值；只展示实际存在的元记录。").font(.caption).foregroundStyle(.secondary)
      }
      if entry.restrictedDetails { Text("部分历史细节当前不可见。").font(.caption).foregroundStyle(.secondary) }
      if entry.reasonAvailable {
        Button("读取完整变更原因") { open(entry, .init(kind: "reason")) }.disabled(busy)
      }
      HStack {
        Button("读取变更前定义") { open(entry, .init(kind: "schema", side: "before")) }
        Button("读取变更后定义") { open(entry, .init(kind: "schema", side: "after")) }
      }.disabled(busy)
      ForEach(entry.changedFieldIds, id: \.self) { field in fieldActions(field, entry) }
    }.padding(10).frame(maxWidth: .infinity, alignment: .leading).background(
      .quaternary, in: RoundedRectangle(cornerRadius: 8))
  }
  private func fieldActions(_ field: String, _ entry: Wire.HistorySummary) -> some View {
    let actual = target.table.field(field)
    let name = (actual?.name ?? "列") + " · " + field
    let kind = actual?.kind.isLink == true ? "links" : "scalar"
    return VStack(alignment: .leading) {
      Text(name).font(.caption)
      HStack {
        Button("读取原值") { open(entry, .init(kind: kind, fieldId: field, side: "before")) }
        Button("读取新值") { open(entry, .init(kind: kind, fieldId: field, side: "after")) }
      }
    }.disabled(busy)
  }
  private func clearValue() {
    selected = nil
    path = nil
    text = ""
    links = []
    restricted = false
    unavailable = false
    valueNext = nil
    valueType = nil
    valueCursors = []
  }
  private func valueView(_ entry: Wire.HistorySummary) -> some View {
    VStack(alignment: .leading, spacing: 7) {
      HStack {
        Text("当前读取：" + action(entry.action)).font(.headline)
        Spacer()
        Button("收起详情") {
          ticket = UUID()
          busy = false
          clearValue()
        }
      }
      if let path {
        Text(
          path.kind == "reason"
            ? "变更原因"
            : path.kind == "schema"
              ? "原始定义快照 · " + (path.side == "before" ? "变更前" : "变更后")
              : "稳定列 " + (path.fieldId ?? "") + " · " + (path.side == "before" ? "原值" : "新值")
        ).font(.caption)
      }
      if unavailable { Text("此动作没有保留所选原值。").foregroundStyle(.secondary) }
      if restricted { Text("部分或全部所选历史细节当前不可见。").foregroundStyle(.secondary) }
      if !text.isEmpty {
        Text(text).font(valueType == "schema" ? .system(.body, design: .monospaced) : .body)
          .textSelection(.enabled)
      }
      if text.isEmpty, valueType == "text", !unavailable {
        Text("原值是空文本。").foregroundStyle(.secondary)
      }
      ForEach(links) { value in
        CrewdManagedTargetButton(
          workspace: workspace, target: value.target, summary: value.summary, current: { live })
      }
      if valueNext != nil {
        Text("尚未读取完整内容。").font(.caption)
        Button("继续读取此值") { readValue(valueNext) }.disabled(busy)
      }
    }.padding(12).frame(maxWidth: .infinity, alignment: .leading).background(
      .quaternary, in: RoundedRectangle(cornerRadius: 8))
  }
  private func readPage(_ cursor: String?) {
    guard live, !busy else { return }
    busy = true
    let t = UUID()
    ticket = t
    Task {
      defer { if ticket == t { busy = false } }
      do {
        let p = try await store.history(
          target.source, scope: target.scope, cursor: cursor, current: { live && ticket == t })
        guard live, ticket == t else { return }
        if let n = p.nextCursor {
          guard !pageCursors.contains(Data(n.utf8)) else { throw Store.Problem.invalid }
          pageCursors.insert(Data(n.utf8))
        }
        guard Set(items.map(\.id)).isDisjoint(with: p.items.map(\.id)) else {
          throw Store.Problem.invalid
        }
        items += p.items
        next = p.nextCursor
        loaded = true
        problem = nil
      } catch { if live, ticket == t { problem = error.localizedDescription } }
    }
  }
  private func open(_ entry: Wire.HistorySummary, _ valuePath: Wire.HistoryPath) {
    guard live, !busy else { return }
    clearValue()
    selected = entry
    path = valuePath
    readValue(nil)
  }
  private func readValue(_ cursor: String?) {
    guard live, !busy, let selected, let path else { return }
    busy = true
    let t = UUID()
    ticket = t
    Task {
      defer { if ticket == t { busy = false } }
      do {
        let p = try await store.historyValue(
          target.source, key: selected.key, path: path, scope: target.scope, cursor: cursor,
          current: { live && ticket == t })
        guard live, ticket == t else { return }
        if p.status == "restricted" {
          text = ""
          links = []
          valueType = nil
          valueNext = nil
          restricted = true
          return
        }
        if p.status == "unavailable" {
          text = ""
          links = []
          valueType = nil
          valueNext = nil
          unavailable = true
          return
        }
        if let n = p.nextCursor {
          guard !valueCursors.contains(Data(n.utf8)) else { throw Store.Problem.invalid }
          valueCursors.insert(Data(n.utf8))
        }
        if p.status == "scalar" {
          guard valueType == nil || valueType == p.valueType,
            text.utf8.count + (p.utf8?.utf8.count ?? 0) <= 262_144
          else { throw Store.Problem.invalid }
          valueType = p.valueType
          text += p.utf8 ?? ""
        } else {
          guard Set(links.map(\.edgeId)).isDisjoint(with: (p.visible ?? []).map(\.edgeId)) else {
            throw Store.Problem.invalid
          }
          links += p.visible ?? []
          restricted = restricted || p.restrictedDetails == true
        }
        valueNext = p.nextCursor
        problem = nil
      } catch {
        if live, ticket == t {
          text = ""
          links = []
          valueNext = nil
          valueType = nil
          problem = error.localizedDescription
        }
      }
    }
  }
}
