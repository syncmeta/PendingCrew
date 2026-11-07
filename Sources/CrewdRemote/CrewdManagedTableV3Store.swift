import Combine
import Foundation

/// V3 current reads and original request recovery; no second table ledger.
@MainActor final class CrewdManagedTableV3Store: ObservableObject {
  typealias Wire = CrewdManagedTableV3Wire
  typealias V2 = CrewdManagedTableV2Wire
  typealias Origin = CrewdKnowledgeStore.Origin
  typealias Scope = CrewdManagedDirectoryStore.BrowseScope
  typealias RPC = CrewdKnowledgeStore.RPC
  typealias Problem = CrewdManagedDirectoryWire.Problem
  private let origin: () -> Origin?
  private let rpc: RPC
  private let journal: () throws -> CrewdPresentationJournal
  private let available: () -> Bool
  @Published private(set) var invalidation = UUID()
  init(
    origin: @escaping () -> Origin?, rpc: @escaping RPC,
    journal: @escaping () throws -> CrewdPresentationJournal, available: @escaping () -> Bool
  ) {
    self.origin = origin
    self.rpc = rpc
    self.journal = journal
    self.available = available
  }
  var supported: Bool { available() }
  private static func identity(_ o: Origin) -> Data { CrewdOrganizationSearchStore.identity(o) }
  func owns(_ scope: Scope) -> Bool {
    available() && origin().map(Self.identity) == Self.identity(scope.origin)
  }
  private func valid(_ o: Origin, _ current: @MainActor () -> Bool) -> Bool {
    available() && !Task.isCancelled && current() && origin().map(Self.identity) == Self.identity(o)
  }
  func handleEvent(_ name: String, _ raw: CrewdValue, connection: UUID) {
    guard origin()?.connection == connection else { return }
    if name == V2.changed {
      guard case .object(let f) = raw, Set(f.keys) == ["crewId"],
        raw["crewId"].string.map({ V2.opaque($0) }) == true
      else { return }
    } else if ![
      "events.lagged", "crew.changed", "crew.purged", "channel.changed", "channel.purged",
      "onto.changed", "work.changed", "ask.changed", "data.changed", "message.removed",
    ].contains(name) {
      return
    }
    invalidation = UUID()
  }
  func scope(stillCurrent: @MainActor () -> Bool) async throws -> Scope {
    guard let o = origin(), valid(o, stillCurrent) else { throw Problem.unavailable }
    let raw = try await rpc(Wire.scope, .object([:]))
    guard valid(o, stillCurrent) else { throw Problem.stale }
    try Wire.closedScope(raw)
    let authority = try CrewdKnowledgeStore.decode(raw, Wire.Scope.self)
    guard Wire.bytes(authority.dbId, o.scope.databaseID ?? ""), V2.uuid(authority.restoreEpoch)
    else { throw Problem.invalid }
    return .init(origin: o, authority: authority)
  }
  private func check(_ scope: Scope, _ current: @MainActor () -> Bool) async throws {
    let actual = try await self.scope(stillCurrent: current)
    guard Self.identity(actual.origin) == Self.identity(scope.origin),
      Wire.exact(actual.authority, scope.authority)
    else { throw Problem.stale }
  }
  private func call<T: Decodable>(
    _ method: String, _ params: CrewdValue, _ scope: Scope, _ current: @MainActor () -> Bool
  ) async throws -> T {
    guard valid(scope.origin, current) else { throw Problem.stale }
    let raw = try await rpc(method, params)
    guard valid(scope.origin, current) else { throw Problem.stale }
    try Wire.closed(raw, method: method)
    return try CrewdKnowledgeStore.decode(raw, T.self)
  }
  private func page(_ ids: [String], next: String?, after: String?, prefix: Character) -> Bool {
    ids.count <= 16 && ids.allSatisfy { Wire.objectID($0, prefix) } && V2.unique(ids)
      && !ids.contains(where: { $0 == after })
      && (next == nil || (next == ids.last && next != after))
  }
  func tables(_ scope: Scope, after: String?, includeArchived: Bool, current: @MainActor () -> Bool)
    async throws -> Wire.TablePage
  {
    let p: Wire.TablePage = try await call(
      Wire.tableList,
      .object([
        "after": after.map(CrewdValue.string) ?? .null, "includeArchived": .bool(includeArchived),
      ]), scope, current)
    guard p.items.allSatisfy(\.valid),
      page(p.items.map(\.id), next: p.nextAfter, after: after, prefix: "T"),
      includeArchived || p.items.allSatisfy({ !$0.lifecycle.archived })
    else { throw Problem.invalid }
    try await check(scope, current)
    return p
  }
  func table(_ id: String, scope: Scope, current: @MainActor () -> Bool) async throws -> Wire.Table
  {
    guard Wire.objectID(id, "T") else { throw Problem.invalid }
    let t: Wire.Table = try await call(Wire.tableGet, .object(["id": .string(id)]), scope, current)
    guard t.valid, t.id == id else { throw Problem.invalid }
    try await check(scope, current)
    return t
  }
  func rows(
    _ id: String, scope: Scope, after: String?, includeArchived: Bool,
    current: @MainActor () -> Bool
  ) async throws -> Wire.RowPage {
    let p: Wire.RowPage = try await call(
      Wire.rows,
      .object([
        "tableId": .string(id), "after": after.map(CrewdValue.string) ?? .null,
        "includeArchived": .bool(includeArchived),
      ]), scope, current)
    guard p.table.id == id, p.table.valid, p.items.allSatisfy({ $0.matches(p.table) }),
      page(p.items.map(\.id), next: p.nextAfter, after: after, prefix: "O"),
      includeArchived || p.items.allSatisfy({ !$0.lifecycle.archived })
    else { throw Problem.invalid }
    try await check(scope, current)
    return p
  }
  func row(_ id: String, scope: Scope, current: @MainActor () -> Bool) async throws -> (
    Wire.Table, Wire.Row
  ) {
    guard Wire.objectID(id, "O") else { throw Problem.invalid }
    let r: Wire.Row = try await call(Wire.rowGet, .object(["id": .string(id)]), scope, current)
    let t = try await table(r.tableId, scope: scope, current: current)
    guard r.id == id, r.matches(t) else { throw Problem.stale }
    return (t, r)
  }
  func links(
    _ row: Wire.Row, field: Wire.Field, table: Wire.Table, scope: Scope,
    current: @MainActor () -> Bool
  ) async throws -> Wire.LinkCell {
    guard row.matches(table), field.kind.isLink, table.field(field.id) != nil else {
      throw Problem.stale
    }
    let p: Wire.RowLinkCell = try await call(
      Wire.rowLinks,
      .object([
        "rowId": .string(row.id), "fieldId": .string(field.id),
        "expectedSchemaRevision": .string(row.schemaRevision),
        "expectedRowRevision": .string(row.revision),
      ]), scope, current)
    guard Wire.exact(p.scope, scope.authority), p.rowId == row.id,
      p.schemaRevision == row.schemaRevision, p.rowRevision == row.revision,
      p.cell.fieldId == field.id, Self.linkValid(p.cell, field: field)
    else { throw Problem.invalid }
    try await check(scope, current)
    return p.cell
  }
  static func linkValid(_ cell: Wire.LinkCell, field: Wire.Field) -> Bool {
    guard cell.visible.count <= 64,
      cell.visible.allSatisfy({ $0.valid && $0.summary.unicodeScalars.count <= 280 }),
      V2.unique(cell.visible.map(\.edgeId)), V2.unique(cell.visible.map({ $0.target.id })),
      cell.hiddenSet.map(\.valid) ?? true, field.kind.multiple || cell.visible.count <= 1
    else { return false }
    if field.kind.kind == .person {
      guard
        cell.visible.allSatisfy({
          $0.target.kind == .session || (field.kind.allowHuman == true && $0.target.kind == .human)
        })
      else { return false }
    }
    return !cell.incompleteRequiredLink
      || (!field.nullable && cell.visible.isEmpty && cell.hiddenSet == nil)
  }
  func definition(
    _ scope: Scope, crew: String, excluding: String?, title: String, purpose: String?,
    cursor: String?, current: @MainActor () -> Bool
  ) async throws -> Wire.DefinitionCheck {
    guard V2.opaque(crew), Wire.label(title, limit: 256),
      purpose.map({ Wire.label($0, limit: 1500) }) ?? (excluding != nil)
    else { throw Wire.Failure.invalid }
    let p: Wire.DefinitionCheck = try await call(
      Wire.defineCheck,
      .object([
        "scope": try CrewdKnowledgeStore.encode(scope.authority), "crewId": .string(crew),
        "excludingTableId": excluding.map(CrewdValue.string) ?? .null, "title": .string(title),
        "purpose": purpose.map(CrewdValue.string) ?? .null,
        "cursor": cursor.map(CrewdValue.string) ?? .null,
      ]), scope, current)
    guard Wire.exact(p.scope, scope.authority), Wire.bytes(p.crewId, crew),
      p.nearCandidates.count <= 8, V2.unique(p.nearCandidates.map(\.id)),
      p.nearCandidates.allSatisfy({
        Wire.objectID($0.tableId, "T") && V2.positive($0.schemaRevision)
          && V2.opaque($0.title, max: 4096)
      }), p.nextCursor.map({ V2.opaque($0, max: 16_384) && !Wire.bytes($0, cursor ?? "") }) ?? true
    else { throw Problem.invalid }
    if p.state == "checking" {
      guard p.definitionCheckToken == nil, p.nextCursor != nil else { throw Problem.invalid }
    } else if p.state == "complete" {
      guard p.nextCursor == nil,
        p.exactNameTaken
          ? p.definitionCheckToken == nil
          : p.definitionCheckToken.map({ V2.opaque($0, max: 16_384) }) == true
      else { throw Problem.invalid }
    } else {
      throw Problem.invalid
    }
    try await check(scope, current)
    return p
  }
  func history(
    _ source: Wire.HistorySource, scope: Scope, cursor: String?, current: @MainActor () -> Bool
  ) async throws -> Wire.HistoryPage {
    guard source.valid else { throw Problem.invalid }
    let raw: CrewdValue = .object([
      "source": try CrewdKnowledgeStore.encode(source),
      "cursor": cursor.map(CrewdValue.string) ?? .null,
    ])
    let p: Wire.HistoryPage = try await call(Wire.history, raw, scope, current)
    guard Wire.exact(p.scope, scope.authority), Wire.exact(p.source, source), p.items.count <= 64,
      V2.unique(p.items.map(\.id)),
      p.items.allSatisfy({
        $0.key.valid
          && [
            "createTable", "alterTable", "addRow", "patchRow", "archiveTable", "restoreTable",
            "archiveRow", "restoreRow", "legacyRowWrite",
          ].contains($0.action) && $0.actor.valid
          && CrewdManagedDirectoryWire.managedTime($0.recordedAt) != nil
          && $0.changedFieldIds.allSatisfy { V2.fieldNumber($0) != nil }
          && V2.unique($0.changedFieldIds)
          && ($0.reasonPreview.map({ $0.unicodeScalars.count <= 280 }) ?? true)
          && ($0.originalRevision.map(V2.positive) ?? true)
          && ($0.resultRevision.map(V2.positive) ?? true)
      }), p.nextCursor.map({ V2.opaque($0, max: 16_384) && $0 != cursor }) ?? true
    else { throw Problem.invalid }
    try await check(scope, current)
    return p
  }
  func historyValue(
    _ source: Wire.HistorySource, key: Wire.HistoryKey, path: Wire.HistoryPath, scope: Scope,
    cursor: String?, current: @MainActor () -> Bool
  ) async throws -> Wire.ValueChunk {
    guard source.valid, key.valid, path.valid else { throw Problem.invalid }
    let raw: CrewdValue = .object([
      "source": try CrewdKnowledgeStore.encode(source), "key": try CrewdKnowledgeStore.encode(key),
      "path": try CrewdKnowledgeStore.encode(path),
      "cursor": cursor.map(CrewdValue.string) ?? .null,
    ])
    let p: Wire.ValueChunk = try await call(Wire.historyValue, raw, scope, current)
    if p.status == "restricted" || p.status == "unavailable" { return p }
    guard p.scope.map({ Wire.exact($0, scope.authority) }) == true,
      p.source.map({ Wire.exact($0, source) }) == true, p.key.map({ Wire.exact($0, key) }) == true,
      p.path.map({ Wire.exact($0, path) }) == true,
      p.nextCursor.map({ V2.opaque($0, max: 16_384) && $0 != cursor }) ?? true
    else { throw Problem.invalid }
    if p.status == "scalar" {
      guard path.kind != "links", let text = p.utf8, text.utf8.count <= Wire.chunkBytes,
        let type = p.valueType,
        [
          "null", "text", "integer", "real", "decimal", "singleSelect", "date", "boolean", "url",
          "reason", "schema",
        ].contains(type),
        path.kind == "reason"
          ? type == "reason"
          : path.kind == "schema" ? type == "schema" : !["reason", "schema"].contains(type),
        p.visible == nil, p.restrictedDetails == nil
      else { throw Problem.invalid }
    } else if p.status == "links" {
      guard path.kind == "links",
        p.visible.map({
          $0.count <= 64 && $0.allSatisfy { $0.valid && $0.summary.unicodeScalars.count <= 280 }
            && V2.unique($0.map(\.edgeId))
        }) == true, p.restrictedDetails != nil, p.utf8 == nil, p.valueType == nil
      else { throw Problem.invalid }
    } else {
      throw Problem.invalid
    }
    try await check(scope, current)
    return p
  }
  struct Witness: Codable {
    let contractVersion: Int
    let managedScope: Wire.Scope
    let crewId: String
    let target: String
    let submit: Wire.Submit
    let digest: String
    var tableId: String? = nil
    var nextFieldNumber: String? = nil
    var nextOptionNumber: String? = nil
    var valid: Bool {
      contractVersion == 3 && V2.uuid(submit.requestId) && Wire.exact(managedScope, submit.scope)
        && Wire.bytes(crewId, submit.crewId) && V2.opaque(crewId) && !target.isEmpty
        && (try? submit.digest()) == digest
    }
  }
  struct Pending: Identifiable {
    let record: CrewdPresentationJournal.Prepared
    let witness: Witness
    var id: UUID { record.id }
    /// Used only to attach the failing current submission to its original journal entry.
    /// Historical recovery intentionally keeps old restore epochs visible separately.
    func matchesSubmission(scope: Wire.Scope, crew: String, target: String) -> Bool {
      Wire.exact(witness.managedScope, scope) && Wire.bytes(witness.crewId, crew)
        && Wire.bytes(witness.submit.operation.target, target)
    }
  }
  struct Observation {
    let receipt: Wire.Receipt
    let historical: Bool
  }
  struct DefinitiveRefusal: LocalizedError {
    let underlying: Error
    var errorDescription: String? { underlying.localizedDescription }
  }
  func pending(_ scope: Scope) throws -> [Pending] {
    guard owns(scope) else { throw Problem.stale }
    let ledger = try journal()
    guard ledger.scope == scope.origin.scope else { throw Problem.stale }
    return try ledger.unknownRecords().filter {
      $0.method == Wire.observation && $0.scope == ledger.scope
    }.map({ record in
      let w = try CrewdKnowledgeStore.decode(record.params, Witness.self)
      guard w.valid, Wire.bytes(w.crewId, record.crew),
        Wire.bytes(w.managedScope.dbId, scope.authority.dbId)
      else { throw Problem.invalid }
      return .init(record: record, witness: w)
    })
  }
  private func refusal(_ error: Error) -> Bool {
    guard let e = error as? CrewdClientError else { return false }
    if e.kind == .notSubmitted { return true }
    let known = [
      "resource.invalid", "resource.forbidden", "resource.not_found", "resource.frozen",
      "managedTable.schema_changed", "managed_object.row_changed",
      "managedTable.definition_review_required",
    ]
    return [-32602, -32003, -32004, -32009].contains(e.code ?? 0) && known.contains(e.refusal ?? "")
  }
  func submit(
    _ operation: Wire.Operation, crew: String, scope: Scope, current: @MainActor () -> Bool
  ) async throws -> Observation {
    guard supported, operation.valid, V2.opaque(crew) else { throw Problem.unavailable }
    try await check(scope, current)
    var baseline: Wire.Table?
    var actualRow: Wire.Row?
    if let row = operation.rowId {
      let (t, r) = try await self.row(row, scope: scope, current: current)
      baseline = t
      actualRow = r
    } else if let id = operation.tableId {
      baseline = try await table(id, scope: scope, current: current)
    }
    if let baseline {
      guard Wire.bytes(baseline.crewId, crew),
        baseline.schemaRevision == operation.expectedSchemaRevision,
        actualRow.map({ $0.revision == operation.expectedRowRevision }) ?? true
      else { throw CrewdClientError(message: "表定义或记录已变化，请读取最新版本对照。", kind: .notSubmitted) }
    }
    let p = Wire.Submit(
      scope: scope.authority, crewId: crew, requestId: UUID().uuidString.lowercased(),
      operation: operation)
    let target =
      scope.authority.restoreEpoch + ":" + String(crew.utf8.count) + ":" + crew + ":"
      + operation.target
    let w = Witness(
      contractVersion: 3, managedScope: p.scope, crewId: crew, target: target, submit: p,
      digest: try p.digest(), tableId: baseline?.id, nextFieldNumber: baseline?.nextFieldNumber,
      nextOptionNumber: baseline?.nextOptionNumber)
    let ledger = try journal()
    guard w.valid, valid(scope.origin, current), ledger.scope == scope.origin.scope else {
      throw Problem.stale
    }
    let record = try ledger.prepareManagedV3(CrewdKnowledgeStore.encode(w), crew: crew)
    var replyReceived = false
    do {
      let raw = try await rpc(Wire.submit, CrewdKnowledgeStore.encode(p))
      replyReceived = true
      guard valid(scope.origin, current) else { throw Problem.stale }
      try Wire.closed(raw, method: Wire.submit)
      let r = try CrewdKnowledgeStore.decode(raw, Wire.Receipt.self)
      try Self.verify(r, witness: w)
      try await check(scope, current)
      try ledger.finish(record, state: "replyObserved")
      return .init(receipt: r, historical: false)
    } catch {
      let known = !replyReceived && refusal(error)
      try? ledger.finish(record, state: known ? "refused" : "unknown")
      if known { throw DefinitiveRefusal(underlying: error) }
      throw error
    }
  }
  func lookup(_ original: Pending, scope: Scope, current: @MainActor () -> Bool) async throws
    -> Observation?
  {
    guard supported,
      try pending(scope).contains(where: {
        $0.id == original.id && Wire.exact($0.witness, original.witness)
      })
    else { throw Problem.stale }
    let w = original.witness
    let p = w.submit
    let raw: CrewdValue = .object([
      "scope": try CrewdKnowledgeStore.encode(w.managedScope), "crewId": .string(w.crewId),
      "requestId": .string(p.requestId),
    ])
    let result: Wire.Lookup = try await call(Wire.lookup, raw, scope, current)
    try await check(scope, current)
    switch (result.status, result.receipt) {
    case ("known", .some(let r)), ("historical", .some(let r)):
      guard (result.status == "known") == Wire.exact(w.managedScope, scope.authority) else {
        throw Problem.invalid
      }
      try Self.verify(r, witness: w)
      try await check(scope, current)
      guard valid(scope.origin, current) else { throw Problem.stale }
      try journal().acknowledge(original.record, detail: "已核对原V3表格请求；没有重新提交。")
      return .init(receipt: r, historical: result.status == "historical")
    case ("historical", nil): return nil
    case ("notFound", nil) where Wire.exact(w.managedScope, scope.authority): return nil
    default: throw Problem.invalid
    }
  }
}

extension CrewdManagedTableV3Store {
  private static func version(_ before: String, _ after: String?, changed: Bool) -> Bool {
    guard let b = V2.number(before), let after, let a = V2.number(after) else { return false }
    if !changed { return a == b }
    let (n, overflow) = b.addingReportingOverflow(1)
    return !overflow && n <= UInt64(Int64.max) && a == n
  }
  private static func event(_ id: String?, changed: Bool) -> Bool {
    changed ? id.map(V2.uuid) == true : id == nil
  }
  private static func ids(_ values: [String]?, fields: Bool = false) -> Bool {
    guard let values, V2.unique(values) else { return false }
    return values.allSatisfy { fields ? V2.fieldNumber($0) != nil : V2.positive($0) }
  }
  private static func issued(_ actual: [Wire.IssuedOption]?, fields: [String], start: String)
    -> Bool
  {
    guard let actual, actual.count == fields.count, let initial = V2.number(start), initial > 0,
      V2.unique(actual.map(\.optionId))
    else { return false }
    for (index, o) in actual.enumerated() {
      let (next, over) = initial.addingReportingOverflow(UInt64(index))
      guard !over, next <= UInt64(Int64.max), o.fieldId == fields[index],
        o.optionId == "e" + String(next)
      else { return false }
    }
    return true
  }
  static func verify(_ r: Wire.Receipt, witness w: Witness) throws {
    let p = w.submit
    let o = p.operation
    let e = r.effect
    try Wire.closedReceipt(CrewdKnowledgeStore.encode(r))
    guard w.valid, Wire.exact(r.scope, p.scope), Wire.bytes(r.crewId, p.crewId),
      Wire.bytes(r.requestId, p.requestId), r.intentDigest == w.digest, r.operationKind == o.kind,
      CrewdManagedDirectoryWire.managedTime(r.recordedAt) != nil, Wire.objectID(e.tableId, "T")
    else { throw Problem.invalid }
    switch o.kind {
    case "createTable":
      let fields = o.fields!
      let expected = (1...fields.count).map({ "p" + String($0) })
      let optionFields = fields.enumerated().flatMap({ index, f in
        Array(repeating: expected[index], count: f.kind.options?.count ?? 0)
      })
      guard e.kind == "tableCreated", e.schemaRevision == "1", e.fieldIds == expected,
        e.titleFieldId == expected[o.titleFieldIndex!],
        issued(e.issuedOptions, fields: optionFields, start: "1"), event(r.eventId, changed: true)
      else { throw Problem.invalid }
    case "alterTable":
      guard e.kind == "schemaChanged", e.tableId == o.tableId,
        e.originalSchemaRevision == o.expectedSchemaRevision, let changed = e.changed,
        version(o.expectedSchemaRevision!, e.resultSchemaRevision, changed: changed),
        event(r.eventId, changed: changed), ids(e.changedFieldIds, fields: true),
        let issuedValues = e.issuedOptions
      else { throw Problem.invalid }
      let c = o.change!
      let changedFields = e.changedFieldIds!
      if !changed {
        guard changedFields.isEmpty, issuedValues.isEmpty else { throw Problem.invalid }
      } else if c.kind == "addField" {
        guard let next = w.nextFieldNumber, changedFields == ["p" + next],
          issued(
            e.issuedOptions,
            fields: Array(repeating: "p" + next, count: c.field?.kind.options?.count ?? 0),
            start: w.nextOptionNumber ?? "")
        else { throw Problem.invalid }
      } else if c.kind == "addOption" {
        guard changedFields == [c.fieldId!],
          issued(e.issuedOptions, fields: [c.fieldId!], start: w.nextOptionNumber ?? "")
        else { throw Problem.invalid }
      } else {
        guard issuedValues.isEmpty,
          ["renameTable", "setPurpose"].contains(c.kind)
            ? changedFields.isEmpty : changedFields == [c.fieldId!]
        else { throw Problem.invalid }
      }
    case "addRow":
      let count = o.patches!.reduce(0) { $0 + ($1.change.values?.count ?? 0) }
      guard e.kind == "rowAdded", e.tableId == o.tableId,
        e.rowId.map({ Wire.objectID($0, "O") }) == true,
        e.schemaRevision == o.expectedSchemaRevision, e.rowRevision == "1", ids(e.edgeIds),
        e.edgeIds!.count == count, event(r.eventId, changed: true)
      else { throw Problem.invalid }
    case "patchRow":
      guard e.kind == "rowPatched", e.tableId == w.tableId, e.rowId == o.rowId,
        e.schemaRevision == o.expectedSchemaRevision,
        e.originalRowRevision == o.expectedRowRevision, let changed = e.changed,
        version(o.expectedRowRevision!, e.resultRowRevision, changed: changed),
        event(r.eventId, changed: changed), ids(e.changedFieldIds, fields: true),
        ids(e.addedEdgeIds), ids(e.removedEdgeIds),
        Set(e.addedEdgeIds!).isDisjoint(with: e.removedEdgeIds!)
      else { throw Problem.invalid }
      let edited = Set(o.patches!.filter { $0.change.kind != "keep" }.map(\.fieldId))
      guard Set(e.changedFieldIds!).isSubset(of: edited) else { throw Problem.invalid }
      if changed {
        guard !e.changedFieldIds!.isEmpty else { throw Problem.invalid }
      } else {
        guard e.changedFieldIds!.isEmpty, e.addedEdgeIds!.isEmpty, e.removedEdgeIds!.isEmpty else {
          throw Problem.invalid
        }
      }
    case "archiveTable", "restoreTable":
      guard e.kind == "tableLifecycle", e.tableId == o.tableId,
        e.originalSchemaRevision == o.expectedSchemaRevision, let changed = e.changed,
        e.archived == (o.kind == "archiveTable"),
        version(o.expectedSchemaRevision!, e.resultSchemaRevision, changed: changed),
        event(r.eventId, changed: changed)
      else { throw Problem.invalid }
    case "archiveRow", "restoreRow":
      guard e.kind == "rowLifecycle", e.tableId == w.tableId, e.rowId == o.rowId,
        e.schemaRevision == o.expectedSchemaRevision,
        e.originalRowRevision == o.expectedRowRevision, let changed = e.changed,
        e.archived == (o.kind == "archiveRow"),
        version(o.expectedRowRevision!, e.resultRowRevision, changed: changed),
        event(r.eventId, changed: changed)
      else { throw Problem.invalid }
    default: throw Problem.invalid
    }
  }
}
