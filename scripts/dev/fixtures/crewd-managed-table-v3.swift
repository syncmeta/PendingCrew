import Foundation

@main struct ManagedTablesV3Proof {
  typealias Wire = CrewdManagedTableV3Wire
  typealias Store = CrewdManagedTableV3Store
  static func fail(_ label: String) -> Never { fatalError(label) }
  static func require(_ value: Bool, _ label: String = "assertion failed") {
    precondition(value, label)
  }
  @MainActor static func editorStateRegression() async -> Bool {
    final class Held {
      var requests = Wire.EditorRequests()
      var release: CheckedContinuation<Void, Never>?
      var entered: CheckedContinuation<Void, Never>?
      var accepted = false
      func facts(_ ticket: UUID) async {
        defer { requests.finish(ticket) }
        await withCheckedContinuation {
          release = $0
          entered?.resume()
          entered = nil
        }
        accepted = requests.owns(ticket)
      }
    }
    let held = Held()
    let adopting = held.requests.begin()
    let factsTicket = held.requests.handoff(from: adopting)!
    let task = Task { await held.facts(factsTicket) }
    await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
      if held.release != nil { c.resume() } else { held.entered = c }
    }
    held.requests.definitionChanged()
    held.release!.resume()
    await task.value
    held.requests.finish(adopting)
    let released = !held.requests.busy && held.accepted
    let a = UUID()
    let b = UUID()
    let c = UUID()
    let retained = Wire.titleAfterRemoval(selected: b, removed: a, order: [a, b, c]) == b
    let removed = Wire.titleAfterRemoval(selected: b, removed: b, order: [a, b, c]) == nil
    print(
      "heldFactsAcceptedAndBusyReleased=\(released) titleIdentityRetained=\(retained) removedTitleExplicit=\(removed)"
    )
    return released && retained && removed
  }
  @MainActor static func main() async throws {
    if CommandLine.arguments.count == 5, CommandLine.arguments[1] == "prepare-child" {
      let root = URL(fileURLWithPath: CommandLine.arguments[2])
      let scope = try JSONDecoder().decode(
        CrewdPresentationJournal.Scope.self,
        from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[3])))
      let params = try JSONDecoder().decode(
        CrewdValue.self, from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[4])))
      let ledger = try CrewdPresentationJournal(root: root, scope: scope)
      do {
        _ = try ledger.prepareManagedV3(params, crew: params["crewId"].string!)
        print("winner")
      } catch { print("blocked") }
      return
    }
    if CommandLine.arguments.count == 2, CommandLine.arguments[1] == "editor-state" {
      let result = await editorStateRegression()
      print("editor-state", result)
      if !result { Foundation.exit(1) }
      return
    }
    guard CommandLine.arguments.count == 3 else { fail("frozen intent and value vectors required") }
    require(await editorStateRegression(), "actual editor held response/title identity")
    require(
      !CrewdClient.eventTopics(capabilities: [], includePersonalDraft: false).contains("links"))
    require(
      CrewdClient.eventTopics(capabilities: [Wire.capability], includePersonalDraft: false)
        .contains("links"))
    require(
      CrewdClient.eventTopics(capabilities: [Wire.capability], includePersonalDraft: true).contains(
        "links"))
    let goldens = try JSONDecoder().decode(
      [CrewdValue].self, from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1])))
    var reports: [CrewdValue] = []
    for g in goldens {
      let p = try CrewdKnowledgeStore.decode(g["normalized"], Wire.Submit.self)
      require(p.operation.valid)
      let bytes = try Wire.intentBytes(p.intent)
      let hex = bytes.map { String(format: "%02x", $0) }.joined()
      let digest = try p.digest()
      require(
        hex == g["canonicalHex"].string && digest == g["sha256"].string,
        g["name"].string ?? "golden")
      let transported = try CrewdKnowledgeStore.decode(
        CrewdKnowledgeStore.encode(p), Wire.Submit.self)
      require(try transported.digest() == digest)
      reports.append(
        .object(["name": g["name"], "sha256": .string(digest), "canonicalHex": .string(hex)]))
    }
    let vectors = try JSONDecoder().decode(
      CrewdValue.self, from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2])))
    for item in vectors["decimal"].array {
      require(item["valid"] == .bool(Wire.decimal(item["input"].string!)))
    }
    for item in vectors["date"].array {
      require(item["valid"] == .bool(Wire.date(item["input"].string!)))
    }
    require(Wire.Cell.real(-0.0) != Wire.Cell.real(0.0))
    let plus: Wire.Cell = .real(0)
    let minus: Wire.Cell = .real(-0.0)
    for cell in [plus, minus] {
      let transport = try CrewdKnowledgeStore.decode(
        CrewdKnowledgeStore.encode(cell), Wire.Cell.self)
      require(transport == cell)
    }
    require(!Wire.date("1900-02-29") && Wire.date("2000-02-29") && !Wire.date("0000-01-01"))
    require(
      !Wire.url("https://user:pass@example.invalid/a") && Wire.url("https://example.invalid/原文"))
    require(Wire.Cell.boolean(false) != .null)
    let presentation = CrewdPresentationJournal.Scope(
      namespace: UUID(), localDirectory: nil, authorityID: "db", databaseID: "db", grantID: "grant",
      grantRevision: "1")
    var origin = Store.Origin(
      scope: presentation, observation: UUID(), connection: UUID(), crew: "crew")
    let authority = Wire.Scope(dbId: "db", restoreEpoch: UUID().uuidString.lowercased())
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
      "pcw-table-v3-proof-" + UUID().uuidString)
    let ledger = try CrewdPresentationJournal(root: root, scope: presentation)
    let body: CrewdValue = .object([
      "id": .string("T10"), "crewId": .string("crew"), "schemaRevision": .string("1"),
      "title": .string("真实表"), "purpose": .null, "titleFieldId": .string("p1"),
      "rowWritePolicy": .string("authorCaptainHuman"),
      "author": .object(["kind": .string("human")]),
      "fields": .array([
        .object([
          "id": .string("p1"), "name": .string("名称"),
          "kind": .object([
            "kind": .string("text"),
            "bounds": .object(["maxUtf8Bytes": .integer(65_536), "maxUnicodeScalars": .null]),
          ]), "nullable": .bool(false), "hidden": .bool(false),
        ]),
        .object([
          "id": .string("p2"), "name": .string("是否"), "kind": .object(["kind": .string("boolean")]),
          "nullable": .bool(false), "hidden": .bool(false),
        ]),
        .object([
          "id": .string("p3"), "name": .string("原选项"),
          "kind": .object([
            "kind": .string("singleSelect"),
            "options": .array([
              .object([
                "id": .string("e1"), "label": .string("旧值"), "tone": .string("neutral"),
                "active": .bool(false),
              ]),
              .object([
                "id": .string("e2"), "label": .string("新值"), "tone": .string("good"),
                "active": .bool(true),
              ]),
            ]),
          ]), "nullable": .bool(true), "hidden": .bool(false),
        ]),
        .object([
          "id": .string("p4"), "name": .string("成员"),
          "kind": .object(["kind": .string("person"), "allowHuman": .bool(true)]),
          "nullable": .bool(true), "hidden": .bool(false),
        ]),
      ]), "nextFieldNumber": .string("5"), "nextOptionNumber": .string("3"),
      "lifecycle": .object(["archivedAt": .null, "archivedBy": .null, "reason": .null]),
      "frozenReasons": .array([]), "v1Readable": .bool(false), "v1Writable": .bool(false),
      "v2Readable": .bool(false), "v2Writable": .bool(false),
    ])
    let table = try CrewdKnowledgeStore.decode(body, Wire.Table.self)
    require(table.valid)
    let rowBody: CrewdValue = .object([
      "id": .string("O20"), "crewId": .string("crew"), "tableId": .string("T10"),
      "schemaRevision": .string("1"), "revision": .string("1"),
      "author": .object(["kind": .string("human")]),
      "scalarCells": .array([
        .object([
          "fieldId": .string("p1"),
          "value": .object(["type": .string("text"), "value": .string("原值")]),
        ]),
        .object([
          "fieldId": .string("p2"),
          "value": .object(["type": .string("boolean"), "value": .bool(false)]),
        ]),
        .object([
          "fieldId": .string("p3"),
          "value": .object([
            "type": .string("singleSelect"),
            "value": .object([
              "tableId": .string("T10"), "fieldId": .string("p3"), "optionId": .string("e1"),
            ]),
          ]),
        ]),
      ]), "linkFieldIds": .array([.string("p4")]),
      "lifecycle": .object(["archivedAt": .null, "archivedBy": .null, "reason": .null]),
      "frozenReasons": .array([]),
    ])
    let row = try CrewdKnowledgeStore.decode(rowBody, Wire.Row.self)
    require(row.matches(table))
    // Old disabled options and arbitrary old text remain Keep; new Set is not allowed.
    let patches = try Wire.rowPatches(
      table: table, row: row, edits: ["p1": .init(kind: "setScalar", value: .text("修改"))],
      loaded: [:])
    require(patches.first(where: { $0.fieldId == "p3" })?.change.kind == "keep")
    do {
      _ = try Wire.rowPatches(
        table: table, row: row, edits: ["p3": .init(kind: "setScalar", value: row.scalar("p3"))],
        loaded: [:])
      fail("inactive option newly Set")
    } catch {}
    do {
      _ = try Wire.rowPatches(
        table: table, row: nil, edits: ["p1": .init(kind: "setScalar", value: .text("新行"))],
        loaded: [:])
      fail("required Boolean defaulted")
    } catch {}
    let falsePatches = try Wire.rowPatches(
      table: table, row: nil,
      edits: [
        "p1": .init(kind: "setScalar", value: .text("新行")),
        "p2": .init(kind: "setScalar", value: .boolean(false)),
      ], loaded: [:])
    require(falsePatches.contains { $0.fieldId == "p2" && $0.change.value == .boolean(false) })
    do {
      _ = try Wire.rowPatches(
        table: table, row: row, edits: ["p4": .init(kind: "replaceLinks", values: [])], loaded: [:])
      fail("unloaded relationships emptied")
    } catch {}
    let hidden = Wire.LinkCell(
      fieldId: "p4", visible: [], hiddenSet: .init(keepToken: "keep-token"),
      incompleteRequiredLink: false)
    do {
      _ = try Wire.rowPatches(
        table: table, row: row,
        edits: [
          "p4": .init(
            kind: "replaceLinks", values: [.init(kind: "reference", target: .init(kind: .human))],
            keepHiddenSetToken: "keep-token")
        ], loaded: ["p4": hidden])
      fail("hidden visible net growth")
    } catch {}
    let preserved = try Wire.rowPatches(
      table: table, row: row,
      edits: ["p4": .init(kind: "replaceLinks", values: [], keepHiddenSetToken: "keep-token")],
      loaded: ["p4": hidden])
    require(preserved.first { $0.fieldId == "p4" }?.change.keepHiddenSetToken == "keep-token")
    do {
      _ = try Wire.rowPatches(
        table: table, row: row,
        edits: [
          "p1": .init(kind: "setScalar", value: .text(String(repeating: "x", count: 65_536)))
        ], loaded: [:])
      fail("whole scalar row budget")
    } catch {}
    let realBudget: [String: Wire.Cell] = ["p1": .real(1e30), "p2": .real(-0.0)]
    let nativeBytes = try Wire.byteCount(realBudget)
    let lowerBytes = try Wire.scalarBudgetLowerBound(realBudget)
    require(lowerBytes <= nativeBytes)
    let oldLarge = String(repeating: "x", count: 65_000)
    _ = try Wire.rowPatches(
      table: table, row: row, edits: ["p1": .init(kind: "setScalar", value: .text(oldLarge))],
      loaded: [:])
    let widened = Wire.Field(
      id: "p1", name: "原文本",
      kind: .init(kind: .longText, bounds: .init(maxUtf8Bytes: 65_536, maxUnicodeScalars: nil)),
      nullable: false, hidden: false)
    require(Wire.Cell.text(oldLarge).accepts(widened, tableId: table.id, allowInactive: true))
    let optionField = table.field("p3")!
    require(
      !Wire.Cell.singleSelect(.init(tableId: "T11", fieldId: "p3", optionId: "e2")).accepts(
        optionField, tableId: table.id, allowInactive: false))
    require(
      !Wire.Cell.singleSelect(.init(tableId: "T10", fieldId: "p2", optionId: "e2")).accepts(
        optionField, tableId: table.id, allowInactive: false))
    var tokenBoundary = Wire.Operation(
      kind: "createTable", title: "边界表", purpose: "明确用途",
      fields: [.init(name: "名称", kind: .init(kind: .text), nullable: false)], titleFieldIndex: 0,
      rowWritePolicy: .authorCaptainHuman,
      definitionCheckToken: String(repeating: "a", count: 16_384), confirmNew: false)
    require(tokenBoundary.valid)
    tokenBoundary.definitionCheckToken = String(repeating: "a", count: 16_385)
    require(!tokenBoundary.valid)
    var verifiedEffects = 0
    func verifyEffect(_ operation: Wire.Operation, _ effect: Wire.Effect, changed: Bool = true)
      throws
    {
      let p = Wire.Submit(
        scope: authority, crewId: "crew", requestId: UUID().uuidString.lowercased(),
        operation: operation)
      let w = Store.Witness(
        contractVersion: 3, managedScope: authority, crewId: "crew", target: "original", submit: p,
        digest: try p.digest(), tableId: "T10", nextFieldNumber: "5", nextOptionNumber: "3")
      let receipt = Wire.Receipt(
        scope: authority, crewId: "crew", requestId: p.requestId,
        recordedAt: "2026-11-06T02:00:00Z", operationKind: operation.kind, intentDigest: w.digest,
        eventId: changed ? UUID().uuidString.lowercased() : nil, effect: effect)
      try Store.verify(receipt, witness: w)
      verifiedEffects += 1
    }
    let createFields: [Wire.FieldDraft] = [
      .init(name: "标题", kind: .init(kind: .text), nullable: false),
      .init(
        name: "选项",
        kind: .init(kind: .singleSelect, options: [.init(label: "甲"), .init(label: "乙")]),
        nullable: true),
    ]
    try verifyEffect(
      .init(
        kind: "createTable", title: "新表", purpose: "明确用途", fields: createFields, titleFieldIndex: 0,
        rowWritePolicy: .authorCaptainHuman, definitionCheckToken: "checked", confirmNew: true),
      .init(
        kind: "tableCreated", tableId: "T11", schemaRevision: "1", fieldIds: ["p1", "p2"],
        titleFieldId: "p1",
        issuedOptions: [.init(fieldId: "p2", optionId: "e1"), .init(fieldId: "p2", optionId: "e2")])
    )
    try verifyEffect(
      .init(
        kind: "alterTable", tableId: "T10", expectedSchemaRevision: "1", reason: "新增可空列",
        change: .init(
          kind: "addField",
          field: .init(name: "空选项列", kind: .init(kind: .singleSelect, options: []), nullable: true))
      ),
      .init(
        kind: "schemaChanged", tableId: "T10", issuedOptions: [], originalSchemaRevision: "1",
        resultSchemaRevision: "2", changedFieldIds: ["p5"], changed: true))
    try verifyEffect(
      .init(
        kind: "alterTable", tableId: "T10", expectedSchemaRevision: "1", reason: "增加选项",
        change: .init(kind: "addOption", fieldId: "p3", optionDraft: .init(label: "丙"))),
      .init(
        kind: "schemaChanged", tableId: "T10",
        issuedOptions: [.init(fieldId: "p3", optionId: "e3")], originalSchemaRevision: "1",
        resultSchemaRevision: "2", changedFieldIds: ["p3"], changed: true))
    try verifyEffect(
      .init(
        kind: "alterTable", tableId: "T10", expectedSchemaRevision: "1", reason: "保留名称",
        change: .init(kind: "renameField", fieldId: "p1", name: "名称")),
      .init(
        kind: "schemaChanged", tableId: "T10", issuedOptions: [], originalSchemaRevision: "1",
        resultSchemaRevision: "1", changedFieldIds: [], changed: false), changed: false)
    try verifyEffect(
      .init(
        kind: "addRow", tableId: "T10", expectedSchemaRevision: "1",
        patches: [
          .init(fieldId: "p1", change: .init(kind: "setScalar", value: .text("真实新行"))),
          .init(fieldId: "p2", change: .init(kind: "setScalar", value: .boolean(false))),
          .init(
            fieldId: "p4",
            change: .init(
              kind: "replaceLinks", values: [.init(kind: "reference", target: .init(kind: .human))])
          ),
        ]),
      .init(
        kind: "rowAdded", tableId: "T10", rowId: "O30", schemaRevision: "1", rowRevision: "1",
        edgeIds: ["4"]))
    for restoring in [false, true] {
      try verifyEffect(
        .init(
          kind: restoring ? "restoreTable" : "archiveTable", tableId: "T10",
          expectedSchemaRevision: "1", reason: "明确生命周期动作"),
        .init(
          kind: "tableLifecycle", tableId: "T10", originalSchemaRevision: "1",
          resultSchemaRevision: "2", changed: true, archived: !restoring))
      try verifyEffect(
        .init(
          kind: restoring ? "restoreRow" : "archiveRow", rowId: "O20", expectedSchemaRevision: "1",
          expectedRowRevision: "1", reason: "明确生命周期动作"),
        .init(
          kind: "rowLifecycle", tableId: "T10", rowId: "O20", schemaRevision: "1",
          originalRowRevision: "1", resultRowRevision: "2", changed: true, archived: !restoring))
    }
    try verifyEffect(
      .init(kind: "restoreTable", tableId: "T10", expectedSchemaRevision: "1", reason: "确认已恢复"),
      .init(
        kind: "tableLifecycle", tableId: "T10", originalSchemaRevision: "1",
        resultSchemaRevision: "1", changed: false, archived: false), changed: false)
    var available = true
    var mode = "normal"
    var calls: [String] = []
    var receipt: CrewdValue = .null
    var submits = 0
    func scopeValue() throws -> CrewdValue { try CrewdKnowledgeStore.encode(authority) }
    func rpc(_ method: String, _ p: CrewdValue) async throws -> CrewdValue {
      calls.append(method)
      switch method {
      case "managedObject.scope": return try scopeValue()
      case Wire.tableGet: return body
      case Wire.rowGet: return rowBody
      case Wire.rowLinks:
        return .object([
          "scope": try scopeValue(), "rowId": .string("O20"), "schemaRevision": .string("1"),
          "rowRevision": .string("1"), "cell": try CrewdKnowledgeStore.encode(hidden),
        ])
      case Wire.tableList: return .object(["items": .array([body]), "nextAfter": .string("T10")])
      case Wire.defineCheck:
        return .object([
          "scope": try scopeValue(), "crewId": .string("crew"), "state": .string("complete"),
          "exactNameTaken": .bool(false), "nearCandidates": .array([]), "moreSimilar": .bool(false),
          "definitionCheckToken": .string("checked"), "nextCursor": .null,
        ])
      case Wire.history:
        return .object([
          "scope": try scopeValue(), "source": p["source"],
          "items": .array([
            .object([
              "key": .object([
                "kind": .string("event"),
                "eventId": .string("00000000-0000-4000-8000-000000000003"),
              ]), "action": .string("patchRow"), "actor": .object(["kind": .string("human")]),
              "recordedAt": .string("2026-11-06T02:00:00Z"), "reasonPreview": .string("原原因"),
              "reasonAvailable": .bool(true), "changedFieldIds": .array([.string("p1")]),
              "originalRevision": .string("1"), "resultRevision": .string("2"),
              "legacyDetailsUnavailable": .bool(false), "restrictedDetails": .bool(false),
            ])
          ]), "nextCursor": .string("history-next"),
        ])
      case Wire.historyValue:
        if mode == "restricted" { return .object(["status": .string("restricted")]) }
        return .object([
          "status": .string("scalar"), "scope": try scopeValue(), "source": p["source"],
          "key": p["key"], "path": p["path"],
          "valueType": .string(p["path"]["kind"].string == "reason" ? "reason" : "text"),
          "utf8": .string(p["cursor"] == .null ? "原值前半" : "原值后半"),
          "nextCursor": p["cursor"] == .null ? .string("value-next") : .null,
        ])
      case Wire.submit:
        submits += 1
        if mode == "refused" {
          throw CrewdClientError(
            message: "fresh stale", code: -32009,
            data: .object(["refusal": .string("managedTable.schema_changed")]))
        }
        if mode == "generic" {
          throw CrewdClientError(message: "SQL outcome unknown", code: -32009)
        }
        let submitted = try CrewdKnowledgeStore.decode(p, Wire.Submit.self)
        receipt = .object([
          "scope": try scopeValue(), "crewId": p["crewId"], "requestId": p["requestId"],
          "recordedAt": .string("2026-11-06T02:00:00Z"), "operationKind": .string("patchRow"),
          "intentDigest": .string(try submitted.digest()),
          "eventId": .string(UUID().uuidString.lowercased()),
          "effect": .object([
            "kind": .string("rowPatched"), "tableId": .string("T10"), "rowId": .string("O20"),
            "schemaRevision": .string("1"), "originalRowRevision": .string("1"),
            "resultRowRevision": .string("2"), "changedFieldIds": .array([.string("p2")]),
            "addedEdgeIds": .array([]), "removedEdgeIds": .array([]), "changed": .bool(true),
          ]),
        ])
        if mode == "lost" { throw NSError(domain: "synthetic lost reply", code: 1) }
        if mode == "malformed" {
          guard case .object(var r) = receipt, case .object(var effect) = r["effect"] else {
            fail("shape")
          }
          effect["resultRowRevision"] = .string("9223372036854775808")
          r["effect"] = .object(effect)
          return .object(r)
        }
        return receipt
      case Wire.lookup:
        if mode == "notFound" { return .object(["status": .string("notFound"), "receipt": .null]) }
        if mode == "retired" {
          return .object(["status": .string("historical"), "receipt": .null])
        }
        return .object(["status": .string("known"), "receipt": receipt])
      default: fail("unexpected RPC " + method)
      }
    }
    func make(_ ledger: CrewdPresentationJournal) -> Store {
      Store(origin: { origin }, rpc: rpc, journal: { ledger }, available: { available })
    }
    let store = make(ledger)
    let scope = try await store.scope(stillCurrent: { true })
    let page = try await store.tables(scope, after: nil, includeArchived: true, current: { true })
    require(page.nextAfter == "T10" && page.items.count == 1)
    _ = try await store.links(
      row, field: table.field("p4")!, table: table, scope: scope, current: { true })
    let check = try await store.definition(
      scope, crew: "crew", excluding: "T10", title: "新名", purpose: nil, cursor: nil,
      current: { true })
    require(check.definitionCheckToken == "checked")
    let history = try await store.history(
      .init(kind: "row", id: "O20"), scope: scope, cursor: nil, current: { true })
    require(history.items.count == 1 && history.nextCursor == "history-next")
    let firstValue = try await store.historyValue(
      history.source, key: history.items[0].key, path: .init(kind: "reason"), scope: scope,
      cursor: nil, current: { true })
    let secondValue = try await store.historyValue(
      history.source, key: history.items[0].key, path: .init(kind: "reason"), scope: scope,
      cursor: firstValue.nextCursor, current: { true })
    require(
      firstValue.utf8 == "原值前半" && secondValue.utf8 == "原值后半" && secondValue.nextCursor == nil)
    mode = "restricted"
    let denied = try await store.historyValue(
      history.source, key: history.items[0].key,
      path: .init(kind: "links", fieldId: "p4", side: "before"), scope: scope, cursor: nil,
      current: { true })
    require(denied.status == "restricted")
    mode = "normal"
    let op = Wire.Operation(
      kind: "patchRow", rowId: "O20", expectedSchemaRevision: "1", expectedRowRevision: "1",
      reason: "明确变更",
      patches: [.init(fieldId: "p2", change: .init(kind: "setScalar", value: .boolean(true)))])
    let beforeInvalidOwner = calls.count
    do {
      _ = try await store.submit(op, crew: "", scope: scope, current: { true })
      fail("empty owner dispatched")
    } catch {}
    require(calls.count == beforeInvalidOwner)
    let direct = try await store.submit(op, crew: "crew", scope: scope, current: { true })
    let pendingAfterDirect = try store.pending(scope)
    require(direct.receipt.effect.resultRowRevision == "2" && pendingAfterDirect.isEmpty)
    mode = "refused"
    do {
      _ = try await store.submit(op, crew: "crew", scope: scope, current: { true })
      fail("refusal accepted")
    } catch { require(error is Store.DefinitiveRefusal) }
    require(try store.pending(scope).isEmpty)
    mode = "lost"
    do {
      _ = try await store.submit(op, crew: "crew", scope: scope, current: { true })
      fail("lost reply became success")
    } catch {}
    let pending = try store.pending(scope)
    require(pending.count == 1)
    let originalUUID = pending[0].witness.submit.requestId
    let count = submits
    // Recreate both Store and durable journal; lookup must retain exact original request.
    let reopened = try CrewdPresentationJournal(root: root, scope: presentation)
    let store2 = make(reopened)
    mode = "retired"
    require(try await store2.lookup(pending[0], scope: scope, current: { true }) == nil)
    require(try store2.pending(scope).count == 1)
    mode = "normal"
    let recovered = try await store2.lookup(pending[0], scope: scope, current: { true })
    let pendingAfterRecovery = try store2.pending(scope)
    require(
      recovered?.receipt.requestId == originalUUID && submits == count
        && pendingAfterRecovery.isEmpty)
    mode = "malformed"
    do {
      _ = try await store2.submit(op, crew: "crew", scope: scope, current: { true })
      fail("malformed reply acknowledged")
    } catch {}
    require(try store2.pending(scope).count == 1)
    let malformedOriginal = try store2.pending(scope)[0]
    mode = "normal"
    _ = try await store2.lookup(malformedOriginal, scope: scope, current: { true })
    available = false
    let before = calls.count
    do {
      _ = try await store2.table("T10", scope: scope, current: { true })
      fail("absent capability used V3")
    } catch {}
    require(calls.count == before)
    available = true
    let oldOrigin = origin
    origin = .init(
      scope: presentation, observation: UUID(), connection: origin.connection, crew: origin.crew)
    do {
      _ = try await store2.table("T10", scope: scope, current: { true })
      fail("stale source read")
    } catch {}
    origin = oldOrigin
    // Same owner-private lock covers V1/V2/V3 table-family prepare; unrelated namespaces stay unchanged.
    let stable = authority.restoreEpoch + ":4:crew:O20"
    let v2 = try reopened.prepareManagedV2(
      .object(["crewId": .string("crew"), "target": .string(stable)]), crew: "crew", manual: false)
    do {
      _ = try reopened.prepareManagedV3(
        .object([
          "contractVersion": .integer(3), "crewId": .string("crew"), "target": .string(stable),
        ]), crew: "crew")
      fail("V2 pending allowed V3")
    } catch {}
    try reopened.finish(v2, state: "refused")
    // Real two-process preparation: both execute the actual journal lock code, only one wins.
    let raceRoot = root.appendingPathComponent("race")
    let scopePath = root.appendingPathComponent("scope.json")
    let paramsPath = root.appendingPathComponent("params.json")
    _ = try CrewdPresentationJournal(root: raceRoot, scope: presentation)
    try JSONEncoder().encode(presentation).write(to: scopePath)
    let raceSubmit = Wire.Submit(
      scope: authority, crewId: "crew", requestId: UUID().uuidString.lowercased(), operation: op)
    let raceWitness = Store.Witness(
      contractVersion: 3, managedScope: authority, crewId: "crew", target: stable,
      submit: raceSubmit, digest: try raceSubmit.digest(), tableId: "T10", nextFieldNumber: "5",
      nextOptionNumber: "3")
    let params = try CrewdKnowledgeStore.encode(raceWitness)
    try JSONEncoder().encode(params).write(to: paramsPath)
    var processes: [(Process, Pipe)] = []
    for _ in 0..<2 {
      let process = Process()
      let pipe = Pipe()
      process.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
      process.arguments = ["prepare-child", raceRoot.path, scopePath.path, paramsPath.path]
      process.standardOutput = pipe
      process.standardError = FileHandle.standardError
      try process.run()
      processes.append((process, pipe))
    }
    var outputs: [String] = []
    for (process, pipe) in processes {
      process.waitUntilExit()
      require(process.terminationStatus == 0)
      outputs.append(
        String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
          .trimmingCharacters(in: .whitespacesAndNewlines))
    }
    require(outputs.sorted() == ["blocked", "winner"])
    mode = "generic"
    do {
      _ = try await store2.submit(op, crew: "crew", scope: scope, current: { true })
      fail("generic SQL became success")
    } catch { require(!(error is Store.DefinitiveRefusal)) }
    let uncertain = try store2.pending(scope)
    require(uncertain.count == 1)
    mode = "notFound"
    let absent = try await store2.lookup(uncertain[0], scope: scope, current: { true })
    require(absent == nil)
    require(try store2.pending(scope).count == 1)
    let beforeBlockedSubmit = submits
    do {
      _ = try await store2.submit(op, crew: "crew", scope: scope, current: { true })
      fail("unknown original allowed a fresh submit")
    } catch {}
    require(submits == beforeBlockedSubmit)
    // A protected restore may leave the old Unknown beside a current-epoch Unknown.
    // Both must stay visible to history, but a current failure can attach only to its own epoch.
    let restoredAuthority = Wire.Scope(
      dbId: authority.dbId, restoreEpoch: UUID().uuidString.lowercased())
    let restoredScope = Store.Scope(origin: origin, authority: restoredAuthority)
    let restoredSubmit = Wire.Submit(
      scope: restoredAuthority, crewId: "crew", requestId: UUID().uuidString.lowercased(),
      operation: op)
    let restoredWitness = Store.Witness(
      contractVersion: 3, managedScope: restoredAuthority, crewId: "crew",
      target: restoredAuthority.restoreEpoch + ":4:crew:O20", submit: restoredSubmit,
      digest: try restoredSubmit.digest())
    let restoredRecord = try reopened.prepareManagedV3(
      CrewdKnowledgeStore.encode(restoredWitness), crew: "crew")
    try reopened.finish(restoredRecord, state: "unknown")
    let bothEpochs = try store2.pending(restoredScope)
    require(bothEpochs.count == 2)
    let attached = bothEpochs.first {
      $0.matchesSubmission(scope: restoredAuthority, crew: "crew", target: op.target)
    }
    require(attached?.witness.submit.requestId == restoredSubmit.requestId)
    require(
      bothEpochs.first { $0.matchesSubmission(scope: authority, crew: "crew", target: op.target) }?
        .id == uncertain[0].id)
    require(
      !bothEpochs[0].matchesSubmission(
        scope: restoredAuthority, crew: "different", target: op.target))
    print(
      "PASS actual V3 Wire+Store/journal mock: effects=\(verifiedEffects) golden=\(goldens.count) RPC=\(calls.count) originalLookupOnly=true hiddenKeep=true falseNullKeep=true processRace=true"
    )
    FileHandle.standardOutput.write(
      try JSONEncoder().encode(CrewdValue.array(reports)) + Data([10]))
  }
}
